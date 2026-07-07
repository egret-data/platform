# Data Governance — Policies P2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full CRUD for **Access** policies to the Control-Plane Policies page — a service-def-driven editor (resource hierarchy + allow/deny policy items), a principal picker sourced from egret users/groups, and the backend create/update/delete endpoints that serialize the editor form into valid Ranger policy JSON.

**Architecture:** A new `policies/ranger_json.rs` submodule owns the editor-DTO↔Ranger-JSON serialization (unit-tested by round-tripping through P1's `summarize_policy`). Three new mutating endpoints in `policies.rs` call the existing `AuthzStore::{create,update,delete}_policy`. A `/control-plane/principals` endpoint lists egret users + groups for the picker. The React editor is a Dialog (mirrors `ControlPlaneSecrets`'s add-dialog), opened from **New** and per-row **Edit** on the P1 list.

**Tech Stack:** Rust (axum 0.8, sqlx/Postgres), React 18 + TanStack Query + shadcn/ui, Vitest.

**Spec:** `docs/en/specs/2026-07-06-data-governance-policies-design.md` (§1 create/update, §3 editor + principal picker — Access subset). Masking + Row-filter editors and the Layer-B config bundle stay in **P3**.

**Depends on P1** (branch `feat/data-governance-policies`): `policies.rs` (parser + read endpoints), `CpState.authz`, `From<AuthzError> for CpError`, the frontend `api/policies.ts` + `Policies.tsx` list.

**Conventions (from project memory):** DB route tests self-skip without `DATABASE_URL`; backend passes `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings`; frontend passes `npm run lint && typecheck && test && build`. Do NOT `cargo build -p egret-controlplane` alone (k8s-openapi feature quirk — use `cargo build -p egret-bin` or `cargo test -p egret-controlplane --no-run`). Ranger payloads are **snake_case** on egret's own DTOs but **camelCase** inside the stored Ranger policy body (`policyType`, `isEnabled`, `policyItems`, `dataMaskPolicyItems`, `isExcludes`).

---

### Task 1: `ranger_json.rs` — Access editor DTO → Ranger JSON (pure, unit-tested)

**Files:**
- Create: `crates/egret-controlplane/src/policies/ranger_json.rs`
- Modify: `crates/egret-controlplane/src/policies.rs` (convert to a module dir OR add `mod ranger_json;` — see Step 4)

- [ ] **Step 1: Decide module layout**

`policies.rs` is currently a single file. Keep it a file and add a sibling by making `ranger_json` a submodule path. Rust allows `#[path = "policies/ranger_json.rs"] mod ranger_json;` from within `policies.rs`, but the clean idiomatic move is to convert `src/policies.rs` → `src/policies/mod.rs` and put `ranger_json.rs` beside it. **Do the conversion:** `git mv crates/egret-controlplane/src/policies.rs crates/egret-controlplane/src/policies/mod.rs` then create `crates/egret-controlplane/src/policies/ranger_json.rs`. In `mod.rs` add `mod ranger_json;` and `pub use ranger_json::{AccessPolicyInput, access_to_ranger_json};`.

- [ ] **Step 2: Write the failing round-trip test**

Create `crates/egret-controlplane/src/policies/ranger_json.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize_policy;
    use serde_json::json;

    fn sample() -> AccessPolicyInput {
        serde_json::from_value(json!({
            "name": "allow-analysts-select",
            "service": "trino",
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]}
            },
            "items": [
                {"users": ["alice"], "groups": ["analysts"], "accesses": ["select"], "effect": "allow"},
                {"groups": ["contractors"], "accesses": ["select"], "effect": "deny"}
            ]
        })).unwrap()
    }

    #[test]
    fn serializes_access_policy_to_ranger_json() {
        let body = access_to_ranger_json(&sample());
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["name"], "allow-analysts-select");
        assert_eq!(body["isEnabled"], true);
        assert_eq!(body["resources"]["catalog"]["values"][0], "iceberg");
        assert_eq!(body["resources"]["column"]["values"][0], "*");
        assert_eq!(body["resources"]["catalog"]["isExcludes"], false);
        // allow item -> policyItems, deny item -> denyPolicyItems
        assert_eq!(body["policyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["denyPolicyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["policyItems"][0]["users"][0], "alice");
        assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "select");
        assert_eq!(body["policyItems"][0]["accesses"][0]["isAllowed"], true);
    }

    #[test]
    fn round_trips_through_summary() {
        // serialize, then parse back with P1's summarizer — the flat view must match.
        let body = access_to_ranger_json(&sample());
        let s = summarize_policy(7, "trino", true, &body);
        assert_eq!(s.policy_type, "access");
        assert_eq!(s.resource_path, "iceberg.sales.orders.*");
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.principals.contains(&"analysts".to_string()));
        assert_eq!(s.permissions, vec!["select".to_string()]);
    }

    #[test]
    fn omits_empty_resource_levels() {
        let mut inp = sample();
        inp.resources.column.values.clear();
        let body = access_to_ranger_json(&inp);
        assert!(body["resources"].get("column").is_none());
        assert!(body["resources"].get("catalog").is_some());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p egret-controlplane ranger_json --no-run`
Expected: FAIL — `AccessPolicyInput` / `access_to_ranger_json` not found.

- [ ] **Step 4: Implement the DTO + serializer** (above the tests in `ranger_json.rs`):

```rust
//! Editor form ↔ Ranger access-policy JSON. The React editor posts an
//! `AccessPolicyInput`; `access_to_ranger_json` renders it to the camelCase
//! Ranger policy body stored verbatim in `authz_policies.policy_jsonb`.
//! P2 covers policyType 0 (access) only; masking/row-filter come in P3.

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// One resource level (catalog / schema / table / column).
#[derive(Debug, Default, Deserialize)]
pub struct ResourceLevel {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, rename = "isRecursive")]
    pub is_recursive: bool,
}

/// The four Trino table-access resource levels.
#[derive(Debug, Default, Deserialize)]
pub struct EditorResources {
    #[serde(default)]
    pub catalog: ResourceLevel,
    #[serde(default)]
    pub schema: ResourceLevel,
    #[serde(default)]
    pub table: ResourceLevel,
    #[serde(default)]
    pub column: ResourceLevel,
}

/// One allow/deny row: principals + access types + effect.
#[derive(Debug, Deserialize)]
pub struct AccessItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub accesses: Vec<String>,
    /// "allow" (default) or "deny".
    #[serde(default = "default_effect")]
    pub effect: String,
}

fn default_effect() -> String {
    "allow".to_string()
}
fn default_true() -> bool {
    true
}

/// The whole Access-policy editor form.
#[derive(Debug, Deserialize)]
pub struct AccessPolicyInput {
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: EditorResources,
    #[serde(default)]
    pub items: Vec<AccessItemInput>,
}

fn resource_obj(l: &ResourceLevel) -> Value {
    json!({
        "values": l.values,
        "isExcludes": l.is_excludes,
        "isRecursive": l.is_recursive,
    })
}

fn item_obj(it: &AccessItemInput) -> Value {
    let accesses: Vec<Value> = it
        .accesses
        .iter()
        .map(|a| json!({ "type": a, "isAllowed": true }))
        .collect();
    json!({
        "users": it.users,
        "groups": it.groups,
        "roles": [],
        "accesses": accesses,
        "delegateAdmin": false,
    })
}

/// Render an `AccessPolicyInput` into a Ranger policyType-0 JSON body.
pub fn access_to_ranger_json(input: &AccessPolicyInput) -> Value {
    // Only include resource levels that have at least one value.
    let mut resources = Map::new();
    for (key, level) in [
        ("catalog", &input.resources.catalog),
        ("schema", &input.resources.schema),
        ("table", &input.resources.table),
        ("column", &input.resources.column),
    ] {
        if !level.values.is_empty() {
            resources.insert(key.to_string(), resource_obj(level));
        }
    }

    let (allow, deny): (Vec<_>, Vec<_>) =
        input.items.iter().partition(|it| it.effect != "deny");
    let policy_items: Vec<Value> = allow.iter().map(|it| item_obj(it)).collect();
    let deny_items: Vec<Value> = deny.iter().map(|it| item_obj(it)).collect();

    json!({
        "name": input.name,
        "service": input.service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": input.is_enabled,
        "resources": Value::Object(resources),
        "policyItems": policy_items,
        "denyPolicyItems": deny_items,
        "allowExceptions": [],
        "denyExceptions": [],
    })
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p egret-controlplane ranger_json`
Expected: all three tests PASS.

- [ ] **Step 6: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-controlplane/src/policies/
git commit -m "feat(controlplane): Access editor DTO -> Ranger JSON serializer"
```
End the commit body with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

### Task 2: Create / update / delete endpoints

**Files:**
- Modify: `crates/egret-controlplane/src/policies/mod.rs` (add 3 handlers + routes)
- Test: `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1: Add handlers + routes in `mod.rs`**

Add to the imports (merge with existing): `use axum::routing::post;`. Add the routes to `router()`:

```rust
pub fn router() -> Router<CpState> {
    Router::new()
        .route("/policies", get(list_policies).post(create_policy))
        .route(
            "/policies/{id}",
            get(get_policy).put(update_policy).delete(delete_policy),
        )
        .route("/policies/services", get(list_profiles))
        .route("/policies/service-def/{name}", get(get_service_def))
}
```

Add the handlers (P2 handles Access only; the `AccessPolicyInput` carries `service` + `is_enabled`):

```rust
/// POST /policies — create an Access policy from the editor form.
async fn create_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(input): Json<AccessPolicyInput>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    if input.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    let svc = st.authz.get_service(&input.service).await?;
    let body = access_to_ranger_json(&input);
    let id = st.authz.create_policy(svc.id, &input.name, &body).await?;
    Ok(Json(json!({ "id": id })))
}

/// PUT /policies/{id} — replace an Access policy's body.
async fn update_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(input): Json<AccessPolicyInput>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    if input.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    let body = access_to_ranger_json(&input);
    st.authz
        .update_policy(id, &input.name, &body, input.is_enabled)
        .await?;
    Ok(Json(json!({ "id": id })))
}

/// DELETE /policies/{id}
async fn delete_policy_handler(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.authz.delete_policy(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

NOTE: the P1 GET handler is already named `get_policy`. Name the DELETE handler `delete_policy_handler` to avoid colliding with `AuthzStore::delete_policy`; reference it in the route as `.delete(delete_policy_handler)`. Add `use axum::http::StatusCode;` if not already imported.

- [ ] **Step 2: Write failing route tests**

In `routes_api.rs`, reuse the P1 helpers (`setup`/`setup_full`, `session_for(&f, tag, true)`, `req(method, uri, token, body)`, `into_json`, `Fixture.db`). Add a DB-gated test:

```rust
#[tokio::test]
async fn admin_can_create_and_delete_access_policy() {
    let Some(f) = setup().await else { return };
    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz).await.unwrap();
    let svc = authz.create_service("trino_crud_test", "trino", &serde_json::json!({})).await.unwrap();

    let token = session_for(&f, "crud", true).await;
    let create = serde_json::json!({
        "name": "p-create", "service": "trino_crud_test", "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let (status, body) = into_json(
        f.app.clone().oneshot(req("POST", "/policies", &token, Some(create))).await.unwrap()
    ).await;
    assert_eq!(status, StatusCode::OK);
    let id = body["id"].as_i64().unwrap();

    // list shows it
    let (_, list) = into_json(
        f.app.clone().oneshot(req("GET", "/policies?service=trino_crud_test&type=access", &token, None)).await.unwrap()
    ).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // delete
    let (del_status, _) = into_json(
        f.app.clone().oneshot(req("DELETE", &format!("/policies/{id}"), &token, None)).await.unwrap()
    ).await;
    assert_eq!(del_status, StatusCode::NO_CONTENT);

    authz.delete_service(svc.id).await.unwrap();
}
```

Match the EXACT signatures of `req`/`session_for`/`into_json` in the file (read them — the P1 tests use them). If `req`/`oneshot` differ, copy the exact pattern an existing mutating test uses.

- [ ] **Step 3: Verify**

- `cargo test -p egret-controlplane --no-run` → EXIT 0.
- `cargo clippy -p egret-controlplane --all-targets -- -D warnings` → clean.
- Best-effort DB run: `set -a; source .env 2>/dev/null; set +a; cargo test -p egret-controlplane admin_can_create_and_delete_access_policy`. It will PoolTimedOut with no Postgres — acceptable; the goal is compile + correctness for the live run.

- [ ] **Step 4: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-controlplane/src/policies/mod.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): create/update/delete Access policies"
```
End with the Co-Authored-By trailer.

---

### Task 3: `/control-plane/principals` endpoint + `GroupRepo::list`

**Files:**
- Modify: `crates/egret-auth/src/groups.rs` (add `list`)
- Modify: `crates/egret-controlplane/src/lib.rs` (`CpState` gains `users` + `groups`)
- Modify: `crates/egret-bin/src/main.rs` + `crates/egret-controlplane/tests/routes_api.rs` (construct them)
- Modify: `crates/egret-controlplane/src/policies/mod.rs` (add handler + route)

- [ ] **Step 1: Add `GroupRepo::list`**

Read `crates/egret-auth/src/groups.rs` for the `Group` row shape (the `groups` table is `id uuid, name text, source text`). If there's no `Group` struct, add one; then add:

```rust
    /// List groups (name-ordered) for the principal picker.
    pub async fn list(&self, limit: i64) -> Result<Vec<Group>> {
        Ok(sqlx::query_as::<_, Group>(
            "SELECT id, name, source FROM groups ORDER BY name LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await?)
    }
```
Define `Group` if missing:
```rust
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct Group {
    pub id: uuid::Uuid,
    pub name: String,
    pub source: String,
}
```
Confirm the `db.pool` access + `Result` alias match the file's other methods (`upsert_oidc_group` uses `&self.db...`). Export `Group` from `egret-auth`'s `lib.rs` if the crate re-exports its public types.

- [ ] **Step 2: Add `users` + `groups` to `CpState`**

In `crates/egret-controlplane/src/lib.rs`, add fields after `authz`:
```rust
    /// Identity lookups for the policy principal picker.
    pub users: egret_auth::UserRepo,
    pub groups: egret_auth::GroupRepo,
```
Construct in `egret-bin/src/main.rs` (the `CpState { ... }` literal, after `authz:`):
```rust
        users: egret_auth::UserRepo::new(db.clone()),
        groups: egret_auth::GroupRepo::new(db.clone()),
```
And in `routes_api.rs` `setup_full`'s `CpState { ... }` (there is a `users` fixture already built via `UserRepo::new(db.clone())` — reuse it; add a `groups: egret_auth::GroupRepo::new(db.clone())`). Grep for OTHER `CpState {` literals (the P1 work found one in `engines.rs` tests) and add both fields there too.

- [ ] **Step 3: Write the failing principals test**

In `routes_api.rs`:
```rust
#[tokio::test]
async fn admin_can_list_principals() {
    let Some(f) = setup().await else { return };
    let token = session_for(&f, "principals", true).await;
    let (status, body) = into_json(
        f.app.clone().oneshot(req("GET", "/principals?q=", &token, None)).await.unwrap()
    ).await;
    assert_eq!(status, StatusCode::OK);
    // at least the admin user we just created is returned as a user principal
    let arr = body.as_array().unwrap();
    assert!(arr.iter().any(|p| p["kind"] == "user"));
}
```

- [ ] **Step 4: Implement the handler + route in `policies/mod.rs`**

```rust
#[derive(Debug, Deserialize)]
pub struct PrincipalQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>, // "user" | "group"
}

#[derive(Debug, Serialize)]
pub struct Principal {
    pub kind: &'static str,
    /// The value written into the policy (user email / group name).
    pub name: String,
    /// Display label.
    pub label: String,
}

/// GET /principals?q=&kind= — egret users + groups for the policy picker.
async fn list_principals(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(qp): Query<PrincipalQuery>,
) -> Result<Json<Vec<Principal>>, CpError> {
    require_platform_admin(&p)?;
    let q = qp.q.unwrap_or_default().to_lowercase();
    let want_users = qp.kind.as_deref() != Some("group");
    let want_groups = qp.kind.as_deref() != Some("user");
    let mut out: Vec<Principal> = Vec::new();

    if want_users {
        for u in st.users.list(200, 0).await.map_err(|_| CpError::Internal)? {
            let name = u.email.clone();
            let label = if u.name.is_empty() { u.email.clone() } else { u.name.clone() };
            if q.is_empty() || name.to_lowercase().contains(&q) || label.to_lowercase().contains(&q) {
                out.push(Principal { kind: "user", name, label });
            }
        }
    }
    if want_groups {
        for g in st.groups.list(200).await.map_err(|_| CpError::Internal)? {
            if q.is_empty() || g.name.to_lowercase().contains(&q) {
                out.push(Principal { kind: "group", name: g.name.clone(), label: g.name });
            }
        }
    }
    out.truncate(50);
    Ok(Json(out))
}
```
`u.email` / `u.name` come from `egret_auth::User` (fields `id, provider, email, name, password_hash, status`). `st.users.list(limit, offset)` and `st.groups.list(limit)` per Steps 1-2. Map identity errors to `CpError::Internal` (the `?` needs a `From<egret_auth::AuthError>`; if that impl doesn't exist, use `.map_err(|_| CpError::Internal)?` as shown to avoid adding a new From).

Add the route to `router()`: `.route("/principals", get(list_principals))`.

> Correctness note for the implementer: the user `name` value written into a policy is the string Trino evaluates against. egret returns `email` as the principal value here; if the deployment's Trino group-mapping keys on a different identity (username/sub), the admin can still free-type the correct value in the picker. This is the documented "principals must match Trino group-mapping" risk — do not try to resolve it here.

- [ ] **Step 5: Verify**

- `cargo test -p egret-controlplane --no-run` → EXIT 0; `cargo build -p egret-bin` → success.
- `cargo clippy -p egret-controlplane --all-targets -- -D warnings` + `cargo clippy -p egret-auth -- -D warnings` → clean.
- Best-effort DB run (will time out w/o Postgres — acceptable).

- [ ] **Step 6: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-auth/src/groups.rs crates/egret-auth/src/lib.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/src/policies/mod.rs crates/egret-bin/src/main.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): principals endpoint (users + groups) for the policy picker"
```
End with the Co-Authored-By trailer.

---

### Task 4: Frontend API — mutations, principals, service-def

**Files:**
- Modify: `frontend/src/api/policies.ts`

- [ ] **Step 1: Extend `api/policies.ts`**

Add types + functions (keep the P1 `PolicySummary`/`Profile` exports):

```ts
import { apiFetch, apiJson } from "./client";

// ... existing PolicySummary, Profile, ProfileEngine, listProfiles, listPolicies ...

export interface ResourceLevel { values: string[]; isExcludes?: boolean; isRecursive?: boolean; }
export interface AccessItem { users: string[]; groups: string[]; accesses: string[]; effect: "allow" | "deny"; }
export interface AccessPolicyInput {
  name: string;
  service: string;
  isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel; column: ResourceLevel };
  items: AccessItem[];
}

export interface Principal { kind: "user" | "group"; name: string; label: string; }

/** Slices of the Ranger service-def the editor renders from. */
export interface ServiceDef {
  accessTypes?: { name: string; label?: string }[];
  resources?: { name: string }[];
  [k: string]: unknown;
}

export function getServiceDef(name: string): Promise<ServiceDef> {
  return apiJson<ServiceDef>(`/control-plane/policies/service-def/${encodeURIComponent(name)}`);
}

export function getPolicy(id: number): Promise<Record<string, unknown>> {
  return apiJson<Record<string, unknown>>(`/control-plane/policies/${id}`);
}

export function listPrincipals(q: string, kind?: "user" | "group"): Promise<Principal[]> {
  const p = new URLSearchParams({ q });
  if (kind) p.set("kind", kind);
  return apiJson<Principal[]>(`/control-plane/principals?${p.toString()}`);
}

export function createPolicy(input: AccessPolicyInput): Promise<{ id: number }> {
  return apiJson<{ id: number }>("/control-plane/policies", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function updatePolicy(id: number, input: AccessPolicyInput): Promise<{ id: number }> {
  return apiJson<{ id: number }>(`/control-plane/policies/${id}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function deletePolicy(id: number): Promise<void> {
  return apiFetch(`/control-plane/policies/${id}`, { method: "DELETE" }).then(() => undefined);
}
```

- [ ] **Step 2: typecheck**

Run: `cd frontend && npm run typecheck`
Expected: clean.

- [ ] **Step 3: commit**

```bash
git add frontend/src/api/policies.ts
git commit -m "feat(frontend): policies API — CRUD, principals, service-def"
```
End with the Co-Authored-By trailer.

---

### Task 5: `PrincipalPicker` component

A multi-value picker: type to search egret users/groups (debounced fetch), click a suggestion or press Enter to add it as a removable chip. No shadcn `Command` component exists, so build it with `Popover` + `Input` + a filtered list.

**Files:**
- Create: `frontend/src/components/policy/PrincipalPicker.tsx`
- Test: `frontend/src/components/policy/__tests__/PrincipalPicker.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { PrincipalPicker } from "../PrincipalPicker";

vi.mock("@/api/policies", () => ({
  listPrincipals: () => Promise.resolve([
    { kind: "user", name: "alice@egret.local", label: "Alice" },
    { kind: "group", name: "analysts", label: "analysts" },
  ]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("PrincipalPicker", () => {
  it("renders selected chips and removes on click", () => {
    const onChange = vi.fn();
    wrap(<PrincipalPicker users={["alice@egret.local"]} groups={["analysts"]} onChange={onChange} />);
    expect(screen.getByText("alice@egret.local")).toBeInTheDocument();
    expect(screen.getByText("analysts")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("remove alice@egret.local"));
    expect(onChange).toHaveBeenCalled();
  });
});
```

Run: `cd frontend && npx vitest run src/components/policy/__tests__/PrincipalPicker.test.tsx` → FAIL (no module).

- [ ] **Step 2: Implement `PrincipalPicker.tsx`**

```tsx
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { type Principal, listPrincipals } from "@/api/policies";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

interface Props {
  users: string[];
  groups: string[];
  onChange: (next: { users: string[]; groups: string[] }) => void;
}

export function PrincipalPicker({ users, groups, onChange }: Props) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);

  const suggestions = useQuery<Principal[]>({
    queryKey: ["principals", q],
    queryFn: () => listPrincipals(q),
    enabled: open,
    retry: false,
  });

  const add = (p: Principal) => {
    if (p.kind === "user") {
      if (!users.includes(p.name)) onChange({ users: [...users, p.name], groups });
    } else if (!groups.includes(p.name)) {
      onChange({ users, groups: [...groups, p.name] });
    }
    setQ("");
  };

  const removeUser = (n: string) => onChange({ users: users.filter((u) => u !== n), groups });
  const removeGroup = (n: string) => onChange({ users, groups: groups.filter((g) => g !== n) });

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1">
        {users.map((u) => (
          <Badge key={`u:${u}`} variant="secondary" className="gap-1">
            {u}
            <button type="button" aria-label={`remove ${u}`} onClick={() => removeUser(u)}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
        {groups.map((g) => (
          <Badge key={`g:${g}`} variant="outline" className="gap-1">
            {g}
            <button type="button" aria-label={`remove ${g}`} onClick={() => removeGroup(g)}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Input
            placeholder="Add user or group…"
            value={q}
            onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
          />
        </PopoverTrigger>
        <PopoverContent align="start" className="max-h-64 w-64 overflow-y-auto p-1">
          {(suggestions.data ?? []).map((p) => (
            <button
              key={`${p.kind}:${p.name}`}
              type="button"
              onClick={() => add(p)}
              className="flex w-full items-center justify-between rounded px-2 py-1.5 text-sm hover:bg-accent"
            >
              <span>{p.label}</span>
              <span className="text-xs text-muted-foreground">{p.kind}</span>
            </button>
          ))}
          {suggestions.data?.length === 0 && (
            <div className="px-2 py-1.5 text-sm text-muted-foreground">No matches.</div>
          )}
        </PopoverContent>
      </Popover>
    </div>
  );
}
```

- [ ] **Step 3: Run test → PASS**

Run: `cd frontend && npx vitest run src/components/policy/__tests__/PrincipalPicker.test.tsx`

- [ ] **Step 4: commit**

```bash
git add frontend/src/components/policy/PrincipalPicker.tsx frontend/src/components/policy/__tests__/PrincipalPicker.test.tsx
git commit -m "feat(frontend): PrincipalPicker (users/groups multi-select)"
```
End with the Co-Authored-By trailer.

---

### Task 6: `AccessPolicyDialog` editor

The create/edit form: name, enabled, four resource-level inputs, and a repeatable list of allow/deny items (each an access-type checkbox set from the service-def + a `PrincipalPicker`).

**Files:**
- Create: `frontend/src/components/policy/AccessPolicyDialog.tsx`
- Test: `frontend/src/components/policy/__tests__/AccessPolicyDialog.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { AccessPolicyDialog } from "../AccessPolicyDialog";

const createPolicy = vi.fn(() => Promise.resolve({ id: 1 }));
vi.mock("@/api/policies", () => ({
  createPolicy: (...a: unknown[]) => createPolicy(...a),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 1 })),
  getServiceDef: () => Promise.resolve({ accessTypes: [{ name: "select" }, { name: "insert" }] }),
  listPrincipals: () => Promise.resolve([]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("AccessPolicyDialog", () => {
  it("creates a policy from the form", async () => {
    wrap(<AccessPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "p1" } });
    fireEvent.change(screen.getByLabelText("Catalog"), { target: { value: "iceberg" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as { name: string; resources: { catalog: { values: string[] } } };
    expect(arg.name).toBe("p1");
    expect(arg.resources.catalog.values).toEqual(["iceberg"]);
  });
});
```

Run: FAIL (no module).

- [ ] **Step 2: Implement `AccessPolicyDialog.tsx`**

```tsx
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import {
  type AccessItem, type AccessPolicyInput, type ServiceDef,
  createPolicy, updatePolicy, getServiceDef,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PrincipalPicker } from "./PrincipalPicker";

interface Props {
  open: boolean;
  service: string;
  /** When set, edit this policy id (body preloaded by the caller). */
  initial?: { id: number; input: AccessPolicyInput };
  onOpenChange: (o: boolean) => void;
  onSaved: () => void;
}

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const emptyItem = (): AccessItem => ({ users: [], groups: [], accesses: [], effect: "allow" });

function blankInput(service: string): AccessPolicyInput {
  return {
    name: "", service, isEnabled: true,
    resources: { catalog: emptyLevel(), schema: emptyLevel(), table: emptyLevel(), column: emptyLevel() },
    items: [emptyItem()],
  };
}

export function AccessPolicyDialog({ open, service, initial, onOpenChange, onSaved }: Props) {
  const [form, setForm] = useState<AccessPolicyInput>(() => initial?.input ?? blankInput(service));
  useEffect(() => {
    setForm(initial?.input ?? blankInput(service));
  }, [initial, service, open]);

  const def = useQuery<ServiceDef>({
    queryKey: ["service-def", service],
    queryFn: () => getServiceDef(service),
    enabled: open,
    retry: false,
  });
  const accessTypes = def.data?.accessTypes?.map((a) => a.name) ?? ["select", "insert", "delete", "update", "create", "drop"];

  const qc = useQueryClient();
  const save = useMutation<{ id: number }, ApiError>({
    mutationFn: () => (initial ? updatePolicy(initial.id, form) : createPolicy(form)),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["control-plane", "policies"] });
      onSaved();
      onOpenChange(false);
    },
  });

  // single-value helper for a resource level (P2 edits one value per level; wildcard `*` allowed)
  const setLevel = (key: keyof AccessPolicyInput["resources"], value: string) =>
    setForm((f) => ({
      ...f,
      resources: { ...f.resources, [key]: { ...f.resources[key], values: value ? [value] : [] } },
    }));

  const setItem = (i: number, next: Partial<AccessItem>) =>
    setForm((f) => ({ ...f, items: f.items.map((it, j) => (j === i ? { ...it, ...next } : it)) }));

  const levelValue = (key: keyof AccessPolicyInput["resources"]) => form.resources[key].values[0] ?? "";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{initial ? "Edit" : "New"} access policy</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1">
            <Label htmlFor="pname">Policy name</Label>
            <Input id="pname" aria-label="Policy name" value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
          </div>

          <div className="grid grid-cols-2 gap-3">
            {(["catalog", "schema", "table", "column"] as const).map((k) => (
              <div key={k} className="space-y-1">
                <Label htmlFor={`res-${k}`} className="capitalize">{k}</Label>
                <Input id={`res-${k}`} aria-label={k[0].toUpperCase() + k.slice(1)}
                  placeholder="* or a name" value={levelValue(k)}
                  onChange={(e) => setLevel(k, e.target.value)} />
              </div>
            ))}
          </div>

          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label>Policy items</Label>
              <Button type="button" variant="outline" size="sm"
                onClick={() => setForm((f) => ({ ...f, items: [...f.items, emptyItem()] }))}>
                <Plus className="mr-1 h-4 w-4" /> Add item
              </Button>
            </div>
            {form.items.map((it, i) => (
              <div key={i} className="space-y-2 rounded-md border p-3">
                <div className="flex items-center justify-between">
                  <select className="rounded border bg-background px-2 py-1 text-sm" value={it.effect}
                    aria-label={`effect ${i}`}
                    onChange={(e) => setItem(i, { effect: e.target.value as "allow" | "deny" })}>
                    <option value="allow">Allow</option>
                    <option value="deny">Deny</option>
                  </select>
                  {form.items.length > 1 && (
                    <button type="button" aria-label={`remove item ${i}`}
                      onClick={() => setForm((f) => ({ ...f, items: f.items.filter((_, j) => j !== i) }))}>
                      <Trash2 className="h-4 w-4 text-muted-foreground" />
                    </button>
                  )}
                </div>
                <PrincipalPicker users={it.users} groups={it.groups}
                  onChange={(p) => setItem(i, p)} />
                <div className="flex flex-wrap gap-2">
                  {accessTypes.map((a) => (
                    <label key={a} className="flex items-center gap-1 text-sm">
                      <input type="checkbox" checked={it.accesses.includes(a)}
                        onChange={(e) => setItem(i, {
                          accesses: e.target.checked ? [...it.accesses, a] : it.accesses.filter((x) => x !== a),
                        })} />
                      {a}
                    </label>
                  ))}
                </div>
              </div>
            ))}
          </div>

          {save.isError && <Alert variant="destructive"><AlertDescription>{save.error.message}</AlertDescription></Alert>}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={() => save.mutate()} disabled={save.isPending || !form.name.trim()}>
            {save.isPending ? "Saving…" : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```

- [ ] **Step 3: Run test → PASS**, then commit:

```bash
git add frontend/src/components/policy/AccessPolicyDialog.tsx frontend/src/components/policy/__tests__/AccessPolicyDialog.test.tsx
git commit -m "feat(frontend): AccessPolicyDialog editor (resources + allow/deny items)"
```
End with the Co-Authored-By trailer.

---

### Task 7: Wire the editor into the Policies list

**Files:**
- Modify: `frontend/src/routes/control-plane/Policies.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/Policies.test.tsx` (extend)

- [ ] **Step 1: Add New + Edit + Delete to the list**

In `Policies.tsx`:
- Import `AccessPolicyDialog`, `ConfirmDeleteDialog` (`@/components/ui/confirm-delete-dialog`), `Button`, `deletePolicy`, `getPolicy`, and `useMutation`/`useQueryClient`.
- Add a **New policy** `Button` in the header (enabled only when `activeService` set and `policyType === "access"` — P2 only edits Access; for masking/rowfilter tabs show the read-only list + a disabled/"coming soon" hint).
- Add an **Actions** column to the table with **Edit** and **Delete** buttons per row (only for the `access` tab).
- **Edit** flow: fetch the raw body via `getPolicy(id)`, map it into an `AccessPolicyInput` (reverse of `access_to_ranger_json`: read `resources.<level>.values`, split `policyItems`→effect "allow" and `denyPolicyItems`→effect "deny", `accesses[].type`→`accesses[]`, `isEnabled`), then open `AccessPolicyDialog` with `initial={{id, input}}`.
- **Delete** flow: `ConfirmDeleteDialog` → `deletePolicy(id)` mutation → invalidate `["control-plane","policies"]`.

Write the reverse-map helper inline in `Policies.tsx`:
```tsx
function toInput(service: string, body: Record<string, any>): AccessPolicyInput {
  const lvl = (k: string) => ({
    values: body?.resources?.[k]?.values ?? [],
    isExcludes: !!body?.resources?.[k]?.isExcludes,
    isRecursive: !!body?.resources?.[k]?.isRecursive,
  });
  const items = [
    ...(body.policyItems ?? []).map((it: any) => ({ ...it, accesses: (it.accesses ?? []).map((a: any) => a.type), effect: "allow" as const })),
    ...(body.denyPolicyItems ?? []).map((it: any) => ({ ...it, accesses: (it.accesses ?? []).map((a: any) => a.type), effect: "deny" as const })),
  ].map((it: any) => ({ users: it.users ?? [], groups: it.groups ?? [], accesses: it.accesses, effect: it.effect }));
  return {
    name: String(body.name ?? ""), service, isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table"), column: lvl("column") },
    items: items.length ? items : [{ users: [], groups: [], accesses: [], effect: "allow" }],
  };
}
```

- [ ] **Step 2: Extend the list test**

Add to `Policies.test.tsx`: mock `deletePolicy`/`getPolicy`/`createPolicy`, click **New policy**, assert the dialog opens (`findByText("New access policy")`); click a row **Delete**, confirm, assert `deletePolicy` called. Keep the existing render assertions.

- [ ] **Step 3: Full frontend gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`
Expected: all pass. Fix any ESLint `any` complaints (the reverse-map uses `any`; if the repo's ESLint bans `any`, type the body as `Record<string, unknown>` and narrow with small guards, or add a scoped `// eslint-disable-next-line @typescript-eslint/no-explicit-any` with a comment — prefer narrowing).

- [ ] **Step 4: commit**

```bash
git add frontend/src/routes/control-plane/Policies.tsx frontend/src/routes/control-plane/__tests__/Policies.test.tsx
git commit -m "feat(frontend): wire Access policy create/edit/delete into the Policies list"
```
End with the Co-Authored-By trailer.

---

### Task 8: Full local CI + manual smoke

- [ ] **Step 1: Backend gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --no-run`
Then, against a live `egret_test` Postgres: `set -a; source .env; set +a; cargo test --workspace` (runs the new DB tests: create/delete policy, principals).

- [ ] **Step 2: Frontend gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`

- [ ] **Step 3: Manual smoke** (per `egret-frontend-viz-verify`)

Start the stack, log in as `admin@egret.local` / `Admin12345!`, open **Control Plane → Data Governance → Policies**. On the **Access** tab: click **New policy**, set name + `catalog=iceberg`, add an item (pick a user via the principal picker, check `select`, effect Allow), Save → the row appears. Edit it (change the table), Save → reflects. Delete it → gone. If a Trino cluster is wired with the Ranger config (P3), confirm enforcement via the `tests/ranger` harness. State exactly what was exercised.

- [ ] **Step 4: Commit any fixups, then stop for review.**

---

## Self-Review notes
- **Spec coverage (P2 subset):** Access full CRUD (Tasks 1, 2, 6, 7) ✓; editor DTO↔Ranger JSON with round-trip tests (Task 1) ✓; principal picker sourced from egret users/groups (Tasks 3, 5) ✓; resource hierarchy catalog→schema→table→column + wildcard (Task 6) ✓; def-driven access-type checkboxes via the P1 `service-def` endpoint (Task 6) ✓. **Deferred to P3:** Masking + Row-filter editors, Layer-B "Download Trino config" bundle, Java-harness enforcement tests, multi-value-per-resource-level and isExcludes/isRecursive UI toggles (P2 edits a single value per level; the DTO already carries the flags for a P3 UI).
- **Type consistency:** `AccessPolicyInput` shape is identical in Rust (`ranger_json.rs`, camelCase via serde rename: `isEnabled`) and TS (`api/policies.ts`); the editor posts exactly that shape; `access_to_ranger_json` and the frontend `toInput` reverse-map are mirror images (policyItems↔allow, denyPolicyItems↔deny, `accesses[].type`↔`accesses[]`). `Principal { kind, name, label }` matches between the Rust handler and TS.
- **Open confirmations for the implementer:** exact `req`/`session_for`/`into_json` helper names in `routes_api.rs` (Task 2/3 — read them, the P1 tests use them); whether `egret-auth` exposes a `Group` struct already (Task 3 Step 1 — reuse if present); the repo's ESLint stance on `any` (Task 7 Step 3).
