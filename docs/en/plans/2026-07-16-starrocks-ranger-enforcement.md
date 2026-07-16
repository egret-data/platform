# StarRocks ↔ Ranger Enforcement — Full Native Parity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a platform admin author StarRocks resource-based **access / column-masking / row-level-filter** Ranger policies in egret and have a real StarRocks cluster enforce them for the querying user — everything StarRocks + Ranger natively support (tags excluded; StarRocks' plugin is resource-only, upstream [#67458](https://github.com/StarRocks/starrocks/issues/67458)).

**Architecture:** egret's `/service` Ranger-Admin REST already serves any service's def + policies verbatim, and the StarRocks connector already presents the per-user OIDC JWT. The gaps are (1) a StarRocks service-def + startup seed, (2) making the policy-body serializer + FE editor **service-type-driven** instead of Trino-hardcoded, (3) a create-profile endpoint so a `starrocks` policy profile can exist without provisioning, and (4) delivering the FE Ranger plugin config to the dev StarRocks + a live masked/filtered/denied smoke.

**Tech Stack:** Rust (`egret-policy`, `egret-bin`), React + TypeScript + react-query (`frontend`), Apache Ranger service-def JSON, StarRocks `allin1-ubuntu` FE (`access_control=ranger`), docker-compose.

**Spec:** `docs/en/specs/2026-07-16-starrocks-ranger-enforcement-design.md`.

---

## File Structure

- **Create** `crates/egret-policy/src/service-defs/starrocks.json` — the StarRocks Ranger service-def (id 205, `implClass:""`), served verbatim.
- **Modify** `crates/egret-policy/src/service_def.rs` — `STARROCKS_*` consts, `starrocks_service_def()`, `seed_starrocks()`, parse test.
- **Modify** `crates/egret-policy/src/lib.rs` — re-export `seed_starrocks` / `STARROCKS_DEF_NAME` (match how `seed_trino` is exported).
- **Modify** `crates/egret-bin/src/main.rs` (~975) — call `seed_starrocks` in the `ranger.enabled` block.
- **Modify** `crates/egret-policy/src/admin/ranger_json.rs` — thread `service_type` through `editor_to_ranger_json`; update its 3 in-file test call sites; add a StarRocks serialization test.
- **Modify** `crates/egret-policy/src/admin/mod.rs` — resolve service type in `create_policy`/`update_policy`; add `create_profile` handler + `POST /policies/services` route.
- **Modify** `frontend/src/api/policies.ts` — `createProfile()` helper; keep `MASK_TYPES` as the fallback.
- **Modify** `frontend/src/components/policy/MaskItemTable.tsx` — accept a `maskTypes` prop.
- **Modify** `frontend/src/routes/control-plane/PolicyEditorPage.tsx` — pass def mask types to `MaskItemTable`; def-derived initial resources on create.
- **Modify** `frontend/src/routes/control-plane/Policies.tsx` — a "New profile" affordance (name + type trino/starrocks).
- **Create** `deploy/starrocks/ranger/ranger-starrocks-security.xml`, `deploy/starrocks/ranger/ranger-starrocks-audit.xml` — FE plugin config.
- **Modify** `deploy/docker-compose.dev.yml` — mount the Ranger config + `access_control=ranger` into the StarRocks FE.
- **Modify** `deploy/starrocks/README.md` — Ranger setup + activation steps.

---

## Task 1: StarRocks service-def + startup seed

**Files:**
- Create: `crates/egret-policy/src/service-defs/starrocks.json`
- Modify: `crates/egret-policy/src/service_def.rs`
- Modify: `crates/egret-policy/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs:973-990`

- [ ] **Step 1: Add the StarRocks service-def JSON.** Create `crates/egret-policy/src/service-defs/starrocks.json` from the canonical upstream file `https://raw.githubusercontent.com/StarRocks/starrocks/main/conf/ranger/ranger-servicedef-starrocks.json`, copied **verbatim** (preserves the exact matchers, mask `transformer` expressions, and `itemId`s), then apply **exactly two edits**:
  - set top-level `"id": 205` (Trino uses 203, tag 204 — 205 is the next stable id),
  - set top-level `"implClass": ""` (egret does not implement Ranger's autocomplete/resource-lookup endpoint for StarRocks; empty disables the plugin's autocomplete calls).

  The committed file MUST satisfy the structural contract asserted in Step 2 (name `starrocks`; resources `catalog→database→table→column` + `view`/`materialized_view`/`function`/`global_function`/`resource`/`resource_group`/`storage_volume`/`user`/`system`; accessTypes incl. `select`; `dataMaskDef.maskTypes` incl. `MASK_HASH`; `rowFilterDef` present). If offline, hand-author to that contract using the StarRocks resource model (levels: catalog=10, database=20, table=30, column=40; siblings view/materialized_view/function at level 30 under database).

- [ ] **Step 2: Write the failing parse test.** Append to the `#[cfg(test)] mod tests` block in `crates/egret-policy/src/service_def.rs` (mirrors `embedded_trino_def_parses_with_id_and_resources`):

```rust
    #[test]
    fn embedded_starrocks_def_parses_with_id_and_surfaces() {
        let def = starrocks_service_def().unwrap();
        assert_eq!(def["name"], "starrocks");
        assert_eq!(def["id"].as_i64(), Some(205));
        // Autocomplete disabled — egret has no StarRocks resource-lookup endpoint.
        assert_eq!(def["implClass"].as_str(), Some(""));
        let res: Vec<&str> = def["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["name"].as_str())
            .collect();
        for want in ["catalog", "database", "table", "column"] {
            assert!(res.contains(&want), "missing resource {want}");
        }
        let access: Vec<&str> = def["accessTypes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect();
        assert!(access.contains(&"select"));
        let masks: Vec<&str> = def["dataMaskDef"]["maskTypes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["name"].as_str())
            .collect();
        assert!(masks.contains(&"MASK_HASH"));
        assert!(def["rowFilterDef"].is_object());
    }
```

- [ ] **Step 3: Run it to confirm it fails.**

Run: `cargo test -p egret-policy embedded_starrocks_def -- --nocapture`
Expected: FAIL — `cannot find function starrocks_service_def`.

- [ ] **Step 4: Add the consts + functions.** In `crates/egret-policy/src/service_def.rs`, after the tag block (near line 20), add:

```rust
/// The StarRocks Ranger service-def, embedded at compile time. Resource-based
/// only — StarRocks' Ranger plugin does not support tag policies (upstream
/// starrocks#67458), so there is no StarRocks tag service.
pub const STARROCKS_SERVICE_DEF_JSON: &str = include_str!("service-defs/starrocks.json");
/// Service-def name as declared in the embedded JSON.
pub const STARROCKS_DEF_NAME: &str = "starrocks";

/// Parse the embedded StarRocks service-def.
pub fn starrocks_service_def() -> Result<Value, PolicyStoreError> {
    Ok(serde_json::from_str(STARROCKS_SERVICE_DEF_JSON)?)
}

/// Idempotently seed the StarRocks service-def into the store. Call at startup
/// when ranger is enabled (alongside `seed_trino`/`seed_tag`).
pub async fn seed_starrocks(store: &PolicyStore) -> Result<(), PolicyStoreError> {
    let def = starrocks_service_def()?;
    let id = def["id"]
        .as_i64()
        .ok_or_else(|| PolicyStoreError::BadRequest("starrocks def missing numeric id".into()))?;
    let name = def["name"].as_str().unwrap_or(STARROCKS_DEF_NAME);
    store.upsert_service_def(id, name, &def).await
}
```

- [ ] **Step 5: Re-export from the crate root.** In `crates/egret-policy/src/lib.rs`, find the line re-exporting `seed_trino` (e.g. `pub use service_def::{... seed_tag, seed_trino ...};`) and add `seed_starrocks, starrocks_service_def, STARROCKS_DEF_NAME` to the same `pub use`.

- [ ] **Step 6: Run the test to confirm it passes.**

Run: `cargo test -p egret-policy embedded_starrocks_def`
Expected: PASS.

- [ ] **Step 7: Wire the startup seed.** In `crates/egret-bin/src/main.rs`, inside the `if config.ranger.enabled { … }` block (after the `seed_tag` call at ~983), add:

```rust
        // Seed the StarRocks service-def too, so StarRocks policy profiles +
        // /service downloads have a def. Resource-based only (no tag service —
        // StarRocks' Ranger plugin does not evaluate tags).
        egret_policy::seed_starrocks(&store)
            .await
            .map_err(anyhow::Error::from)?;
```

- [ ] **Step 8: Build the binary to confirm wiring compiles.**

Run: `cargo build -p egret-bin`
Expected: PASS.

- [ ] **Step 9: Commit.**

```bash
git add crates/egret-policy/src/service-defs/starrocks.json \
        crates/egret-policy/src/service_def.rs \
        crates/egret-policy/src/lib.rs \
        crates/egret-bin/src/main.rs
git commit -m "feat(policy): embed + seed the StarRocks Ranger service-def (id 205)"
```

---

## Task 2: Service-type-aware policy serializer

The three `*_to_ranger_json` builders hardcode `serviceType: "trino"`. Make `editor_to_ranger_json` stamp the **actual** service type onto the built body (mirroring how `tagify` overrides it for tag policies), and resolve that type from the target service in the handlers.

**Files:**
- Modify: `crates/egret-policy/src/admin/ranger_json.rs`
- Modify: `crates/egret-policy/src/admin/mod.rs:792-869`

- [ ] **Step 1: Write the failing test.** Append to the `#[cfg(test)] mod tests` block in `crates/egret-policy/src/admin/ranger_json.rs`:

```rust
    #[test]
    fn editor_stamps_starrocks_service_type() {
        let pol = editor_to_ranger_json(
            &json!({
                "kind": "masking", "name": "m", "service": "sr1", "isEnabled": true,
                "resources": { "catalog": { "values": ["ice"] },
                               "database": { "values": ["sales"] },
                               "table": { "values": ["orders"] },
                               "column": { "values": ["phone"] } },
                "items": [{ "users": ["u"], "groups": [], "maskType": "MASK_HASH" }]
            }),
            "starrocks",
        )
        .unwrap();
        assert_eq!(pol.body["serviceType"], "starrocks");
        // StarRocks uses `database`, not Trino's `schema`.
        assert!(pol.body["resources"]["database"].is_object());
        assert_eq!(
            pol.body["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"],
            "MASK_HASH"
        );
    }
```

- [ ] **Step 2: Run it to confirm it fails.**

Run: `cargo test -p egret-policy editor_stamps_starrocks -- --nocapture`
Expected: FAIL — `editor_to_ranger_json` takes 1 argument, not 2.

- [ ] **Step 3: Add the `service_type` parameter + override.** In `crates/egret-policy/src/admin/ranger_json.rs`, change `editor_to_ranger_json` (currently starting line 274) to:

```rust
/// Dispatch an editor form (`{kind, ...}`) to the right serializer and stamp the
/// target service's type onto the body. Missing `kind` defaults to "access" so
/// P2 clients stay wire-compatible. `service_type` is the profile's Ranger
/// service type (`"trino"` | `"starrocks"`) — the builders default to `"trino"`,
/// this overrides it for the actual target service.
pub fn editor_to_ranger_json(v: &Value, service_type: &str) -> Result<RangerPolicy, String> {
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("access");
    let mut pol: RangerPolicy = match kind {
        "access" => {
            let inp: AccessPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: access_to_ranger_json(&inp),
            }
        }
        "masking" => {
            let inp: MaskingPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: masking_to_ranger_json(&inp),
            }
        }
        "rowfilter" => {
            let inp: RowFilterPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: rowfilter_to_ranger_json(&inp),
            }
        }
        other => return Err(format!("unknown policy kind: {other}")),
    };
    if let Value::Object(m) = &mut pol.body {
        m.insert("serviceType".into(), Value::from(service_type));
    }
    Ok(pol)
}
```

- [ ] **Step 4: Fix the 3 in-file test call sites.** In the same file's test module, `editor_to_ranger_json(...)` is called at (previously) lines ~668, ~673, ~676. Add `, "trino"` as the second argument to each:

```rust
        let acc = editor_to_ranger_json(&json!({"kind":"access","name":"a","service":"trino",
            /* …existing body… */}), "trino").unwrap();
        // …
        let m = editor_to_ranger_json(&serde_json::to_value(masking_sample()).unwrap(), "trino").unwrap();
        // …
        assert!(editor_to_ranger_json(&json!({"name":"x","service":"trino","resources":{},"items":[]}), "trino")
            /* …existing …*/);
```

(Match each existing call exactly, only inserting the `, "trino"` argument before the closing paren.)

- [ ] **Step 5: Thread the type in the handlers.** In `crates/egret-policy/src/admin/mod.rs`:

In `create_policy` (the generic path after the `is_tag_kind` block, ~819-826), replace:

```rust
    let pol = editor_to_ranger_json(&v).map_err(PolicyHttpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
    let svc = st.store.get_service(&pol.service).await?;
```

with:

```rust
    let service_name = v.get("service").and_then(Value::as_str).unwrap_or("");
    let svc = st.store.get_service(service_name).await?;
    let pol = editor_to_ranger_json(&v, &svc.r#type).map_err(PolicyHttpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest(
            "policy name is required".into(),
        ));
    }
```

In `update_policy` (~859), replace:

```rust
    let pol = editor_to_ranger_json(&v).map_err(PolicyHttpError::BadRequest)?;
```

with:

```rust
    let service_name = v.get("service").and_then(Value::as_str).unwrap_or("");
    let svc = st.store.get_service(service_name).await?;
    let pol = editor_to_ranger_json(&v, &svc.r#type).map_err(PolicyHttpError::BadRequest)?;
```

- [ ] **Step 6: Run the policy tests to confirm green.**

Run: `cargo test -p egret-policy`
Expected: PASS (new StarRocks test + all existing Trino/tag serializer tests unchanged).

- [ ] **Step 7: Commit.**

```bash
git add crates/egret-policy/src/admin/ranger_json.rs crates/egret-policy/src/admin/mod.rs
git commit -m "feat(policy): stamp the target service's type on serialized policy bodies"
```

---

## Task 3: Create-profile endpoint (`POST /policies/services`)

Profiles are created today only by Trino provisioning auto-bind. StarRocks has no provisioner, so add a thin admin endpoint to create a `starrocks` (or `trino`) profile directly.

**Files:**
- Modify: `crates/egret-policy/src/admin/mod.rs`

- [ ] **Step 1: Write the failing test.** Add a DB test in the crate's admin tests (mirror the fixture used by the existing `admin_routes.rs` tests — reuse its `Fixture`/router setup). Append to `crates/egret-policy/tests/admin_routes.rs`:

```rust
#[sqlx::test]
async fn create_profile_makes_a_starrocks_service(pool: sqlx::PgPool) {
    let f = Fixture::new(pool).await; // same helper the file's other tests use
    egret_policy::seed_starrocks(&f.store).await.expect("seed starrocks def");
    let resp = f
        .post_json(
            "/control-plane/policies/services",
            serde_json::json!({ "name": "sr1", "type": "starrocks" }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let svc = f.store.get_service("sr1").await.expect("service exists");
    assert_eq!(svc.r#type, "starrocks");
}
```

(If `Fixture` lacks `post_json`, use the same request-builder pattern the neighbouring tests in this file already use for POSTs; keep the assertion on `get_service("sr1").r#type`.)

- [ ] **Step 2: Run it to confirm it fails.**

Run: `cargo test -p egret-policy create_profile_makes_a_starrocks_service`
Expected: FAIL — route `POST /policies/services` returns 405/404 (only `get(list_profiles)` is registered).

- [ ] **Step 3: Add the handler.** In `crates/egret-policy/src/admin/mod.rs`, near `list_profiles` (~474), add:

```rust
#[derive(serde::Deserialize, utoipa::ToSchema)]
struct CreateProfileBody {
    name: String,
    /// Ranger service type: `"trino"` or `"starrocks"`.
    r#type: String,
    /// Optional service config (e.g. StarRocks JDBC coordinates). Stored verbatim.
    #[serde(default)]
    config: serde_json::Value,
}

/// POST /policies/services — create a policy profile (a Ranger service instance)
/// of the given type. The manual analogue of Trino provisioning's auto-bind, so
/// a StarRocks profile can exist without an egret-provisioned engine.
#[utoipa::path(
    post,
    path = "/api/control-plane/policies/services",
    tag = "policy-admin",
    request_body = CreateProfileBody,
    responses(
        (status = 200, description = "Profile created", body = RangerServiceRef),
        (status = 400, description = "Invalid type or missing name", body = ErrorBody),
        (status = 409, description = "A profile with this name already exists", body = ErrorBody),
    ),
)]
async fn create_profile(
    State(st): State<PolicyAdminState>,
    Auth(p): Auth,
    Json(b): Json<CreateProfileBody>,
) -> Result<Json<Value>, PolicyHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:create").await?;
    if b.name.trim().is_empty() {
        return Err(PolicyHttpError::BadRequest("profile name is required".into()));
    }
    if b.r#type != "trino" && b.r#type != "starrocks" {
        return Err(PolicyHttpError::BadRequest(
            "type must be \"trino\" or \"starrocks\"".into(),
        ));
    }
    let cfg = if b.config.is_null() { json!({}) } else { b.config.clone() };
    let svc = st.store.create_service(&b.name, &b.r#type, &cfg).await?;
    Ok(Json(json!({ "id": svc.id, "name": svc.name, "type": svc.r#type })))
}
```

Ensure `PolicyStoreError::AlreadyExists` maps to HTTP 409 in `PolicyHttpError`'s `From` impl (it already handles the store errors the other handlers surface — confirm `AlreadyExists` is covered; if not, add a `409` arm).

- [ ] **Step 4: Register the route.** In `policy_admin_router()` (~263), change:

```rust
        .route("/policies/services", get(list_profiles))
```

to:

```rust
        .route("/policies/services", get(list_profiles).post(create_profile))
```

Add `create_profile` and `CreateProfileBody` to the `#[openapi(paths(...), components(schemas(...)))]` doc lists near the bottom of the file if that pattern is used there.

- [ ] **Step 5: Run the test to confirm it passes.**

Run: `cargo test -p egret-policy create_profile_makes_a_starrocks_service`
Expected: PASS.

- [ ] **Step 6: Regenerate the frontend API client** (orval) so `policyAdminCreateProfile` exists for Task 4. Run the repo's client-gen script:

Run: `cd frontend && npm run gen` (or the script named in `package.json` that runs orval; check `package.json` `scripts` for `orval`/`gen`/`api`).
Expected: `frontend/src/api/generated/policy-admin/policy-admin.ts` now exports a create-profile fetcher.

- [ ] **Step 7: Commit.**

```bash
git add crates/egret-policy/src/admin/mod.rs crates/egret-policy/tests/admin_routes.rs frontend/src/api/generated
git commit -m "feat(policy): add POST /policies/services to create a profile (trino|starrocks)"
```

---

## Task 4: Def-driven FE authoring (mask types, initial resources, New-profile)

`ResourceSelector` and access types are already def-driven — StarRocks' resource hierarchy renders once the def is served. Two hardcodes remain: `MaskItemTable` uses the `MASK_TYPES` constant, and `PolicyEditorPage`'s blank form seeds Trino resource keys (`schema`), which leak into a saved StarRocks body.

**Files:**
- Modify: `frontend/src/api/policies.ts`
- Modify: `frontend/src/components/policy/MaskItemTable.tsx`
- Modify: `frontend/src/routes/control-plane/PolicyEditorPage.tsx`
- Modify: `frontend/src/routes/control-plane/Policies.tsx`

- [ ] **Step 1: Write a failing component test** for def-driven mask types. Create `frontend/src/components/policy/__tests__/MaskItemTable.starrocks.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { MaskItemTable } from "@/components/policy/MaskItemTable";

test("renders the passed mask types, not the Trino defaults", () => {
  render(
    <MaskItemTable
      items={[{ users: [], groups: [], maskType: "MASK_HASH", valueExpr: undefined }]}
      onChange={() => {}}
      maskTypes={[
        { name: "MASK_HASH", label: "Hash" },
        { name: "MASK_DATE_SHOW_YEAR", label: "Date: year only" },
      ]}
    />,
  );
  // Trino-only option must be absent when StarRocks mask types are supplied.
  expect(screen.queryByText("Show last 4")).toBeNull();
});
```

- [ ] **Step 2: Run it to confirm it fails.**

Run: `cd frontend && npx vitest run src/components/policy/__tests__/MaskItemTable.starrocks.test.tsx`
Expected: FAIL — `MaskItemTable` ignores `maskTypes` (prop doesn't exist), still renders `MASK_TYPES`.

- [ ] **Step 3: Add the `maskTypes` prop to `MaskItemTable`.** In `frontend/src/components/policy/MaskItemTable.tsx`:

Change the import and props:

```tsx
import { type MaskItem, MASK_TYPES, listPrincipals } from "@/api/policies";
// …
export interface MaskItemTableProps {
  items: MaskItem[];
  onChange: (items: MaskItem[]) => void;
  /** Mask types from the profile's service-def; falls back to the Trino set. */
  maskTypes?: readonly { name: string; label?: string }[];
}

export function MaskItemTable({ items, onChange, maskTypes }: MaskItemTableProps) {
  const types = maskTypes && maskTypes.length ? maskTypes : MASK_TYPES;
```

Then in the `<SelectContent>` replace `MASK_TYPES.map(...)` with `types.map((m) => (…))` (same JSX, iterate `types`).

- [ ] **Step 4: Run the component test to confirm it passes.**

Run: `cd frontend && npx vitest run src/components/policy/__tests__/MaskItemTable.starrocks.test.tsx`
Expected: PASS.

- [ ] **Step 5: Wire the def's mask types + def-derived initial resources in `PolicyEditorPage`.**

In `frontend/src/routes/control-plane/PolicyEditorPage.tsx`, pass the def mask types to `MaskItemTable` (the masking branch, ~778):

```tsx
                      <MaskItemTable
                        items={form.items}
                        maskTypes={def.data?.dataMaskDef?.maskTypes}
                        onChange={(items) => setForm((f: any) => ({ ...f, items }))}
                      />
```

Add a helper above the component (after `emptyColumnResources`, ~84) that builds the initial resource chain from the def instead of hardcoded Trino keys:

```tsx
// Build a default resource chain (root empty, deeper levels wildcard) from the
// service-def hierarchy so a fresh StarRocks policy uses catalog/database/…
// and a Trino one uses catalog/schema/…. Masking chains extend to `column`;
// access/row-filter stop at `table` (matching the previous Trino defaults).
function defaultResourcesFromDef(
  resDefs: ServiceDefResource[],
  kind: PolicyKind,
): Record<string, ResourceLevel> {
  const byName = new Map(resDefs.map((r) => [r.name, r]));
  const childrenOf = (n: string) => resDefs.filter((r) => r.parent === n);
  const stopAt = kind === "masking" ? "column" : "table";
  let node = resDefs.find((r) => r.name === "catalog" && !r.parent)
    ?? resDefs.find((r) => !r.parent && childrenOf(r.name).length > 0);
  const out: Record<string, ResourceLevel> = {};
  let first = true;
  while (node) {
    out[node.name] = first ? emptyLevel() : wildcardLevel();
    first = false;
    if (node.name === stopAt) break;
    const kids = childrenOf(node.name);
    // Follow the linear data hierarchy: prefer database/schema, then table, then column.
    const next =
      kids.find((k) => k.name === "database" || k.name === "schema") ??
      kids.find((k) => k.name === "table") ??
      kids.find((k) => k.name === "column");
    if (!next) break;
    node = byName.get(next.name);
  }
  return out;
}
```

Then reset the resources once the def loads on **create** (no `id`). Add this effect next to the existing `def` query (after the `def` `useQuery`, ~268):

```tsx
  // On create, once the def is known, seed the resource chain from the def so
  // the blank form matches the profile's engine (StarRocks: catalog/database/…;
  // Trino: catalog/schema/…) instead of the hardcoded Trino skeleton.
  useEffect(() => {
    if (!id && def.data) {
      const seeded = defaultResourcesFromDef(resDefs, kind);
      if (Object.keys(seeded).length) {
        setForm((f: any) => ({ ...f, resources: seeded }));
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, def.data, kind]);
```

- [ ] **Step 6: Add a "New profile" affordance in `Policies.tsx`.** Add `createProfile` to `frontend/src/api/policies.ts`:

```ts
import { policyAdminCreateProfile } from "./generated/policy-admin/policy-admin";
// …
export async function createProfile(name: string, type: "trino" | "starrocks"): Promise<Profile> {
  return (await policyAdminCreateProfile({ name, type })).data as Profile;
}
```

(Use the exact generated fetcher name emitted in Task 3 Step 6 if it differs from `policyAdminCreateProfile`.)

In `frontend/src/routes/control-plane/Policies.tsx`, add a "New profile" button that collects a name + a type `Select` (`trino` | `starrocks`), calls `createProfile`, and invalidates the profiles query (`queryClient.invalidateQueries({ queryKey: ["control-plane", "profiles"] })` — match the key `listProfiles` uses in this file). Mirror the page's existing dialog/button pattern; no new design system pieces.

- [ ] **Step 7: Typecheck + run the frontend policy tests.**

Run: `cd frontend && npm run typecheck && npx vitest run src/components/policy src/routes/control-plane/__tests__/PolicyEditorPage.test.tsx src/routes/control-plane/__tests__/Policies.test.tsx`
Expected: PASS (existing Trino editor tests still green — the def-derived seed produces `catalog/schema/table[/column]` for the Trino def, matching the previous skeleton).

- [ ] **Step 8: Commit.**

```bash
git add frontend/src/api/policies.ts frontend/src/components/policy/MaskItemTable.tsx \
        frontend/src/routes/control-plane/PolicyEditorPage.tsx \
        frontend/src/routes/control-plane/Policies.tsx \
        frontend/src/components/policy/__tests__/MaskItemTable.starrocks.test.tsx
git commit -m "feat(fe): def-driven mask types + initial resources + create StarRocks profile"
```

---

## Task 5: Deliver the Ranger plugin config to the dev StarRocks

No provisioner exists, so the StarRocks FE gets its Ranger config from the dev compose. StarRocks' Ranger support is built into the FE (no jar install) — enabled by `access_control=ranger` + the security XML.

**Files:**
- Create: `deploy/starrocks/ranger/ranger-starrocks-security.xml`
- Create: `deploy/starrocks/ranger/ranger-starrocks-audit.xml`
- Modify: `deploy/docker-compose.dev.yml`
- Modify: `deploy/starrocks/README.md`

- [ ] **Step 1: Create `deploy/starrocks/ranger/ranger-starrocks-security.xml`.** `<PROFILE>` is the StarRocks profile name created in Task 4; `<EGRET_BASE_URL>` is the egret base URL reachable from the StarRocks container (compose service DNS `http://egret:8080`, or `http://host.docker.internal:8080` if egret runs on the host). StarRocks' `RangerAdminRESTClient` appends `/service/plugins/policies/download/<PROFILE>`.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<configuration>
  <property><name>ranger.plugin.starrocks.service.name</name><value>sr1</value></property>
  <property><name>ranger.plugin.starrocks.policy.rest.url</name><value>http://egret:8080</value></property>
  <property><name>ranger.plugin.starrocks.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>
  <property><name>ranger.plugin.starrocks.policy.cache.dir</name><value>/tmp/ranger/sr1/policycache</value></property>
  <property><name>ranger.plugin.starrocks.policy.pollIntervalMs</name><value>30000</value></property>
  <property><name>ranger.plugin.starrocks.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>
  <property><name>ranger.plugin.starrocks.policy.rest.client.read.timeoutMs</name><value>30000</value></property>
</configuration>
```

- [ ] **Step 2: Create `deploy/starrocks/ranger/ranger-starrocks-audit.xml`** (audit-to-Ranger off for dev):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<configuration>
  <property><name>xasecure.audit.is.enabled</name><value>false</value></property>
</configuration>
```

- [ ] **Step 3: Mount the config + enable Ranger on the StarRocks FE.** In `deploy/docker-compose.dev.yml`, under the `starrocks` service (~49): mount both XML files into the FE conf dir and set `access_control=ranger`. For the `allin1-ubuntu` image the FE conf dir is `/data/deploy/starrocks/fe/conf` (verify with `docker exec <c> sh -lc 'ls $(dirname $(find / -name fe.conf 2>/dev/null | head -1))'`). Add:

```yaml
    volumes:
      - ./starrocks/ranger/ranger-starrocks-security.xml:/data/deploy/starrocks/fe/conf/ranger-starrocks-security.xml:ro
      - ./starrocks/ranger/ranger-starrocks-audit.xml:/data/deploy/starrocks/fe/conf/ranger-starrocks-audit.xml:ro
```

Append `access_control = ranger` to the FE config. If the image doesn't support a conf drop-in, set it at runtime after the FE is healthy: `mysql -h127.0.0.1 -P9030 -uroot -e "ADMIN SET FRONTEND CONFIG (\"access_control\"=\"ranger\");"` — document whichever the image accepts in the README (Step 4). Do NOT commit a mount path that doesn't exist on the running image; verify the path first.

- [ ] **Step 4: Document setup + activation** in `deploy/starrocks/README.md` — a new "Ranger authorization (resource-based)" section: seed prereqs (egret `ranger.enabled`, `seed_starrocks` at startup, a `starrocks` profile created via the Policies UI named to match `ranger.plugin.starrocks.service.name`), the two XML files, the exact `access_control=ranger` activation command that worked on the image, and how to confirm the FE is polling (egret logs show `GET /service/plugins/policies/download/sr1`). Note the tag-policy limitation (upstream #67458) and that port 9030 has no TLS in dev.

- [ ] **Step 5: Commit.**

```bash
git add deploy/starrocks/ranger deploy/docker-compose.dev.yml deploy/starrocks/README.md
git commit -m "feat(deploy): wire StarRocks FE to egret Ranger policy download"
```

---

## Task 6: Full-workspace gate + LIVE SMOKE

**Files:** none (verification only). This is controller-driven, not a subagent.

- [ ] **Step 1: Format + lint + full backend + frontend gates** (project rule: run full CI locally — GitHub CI is out of quota).

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd frontend && npm run lint && npm run typecheck && npx vitest run && npm run build
```
Expected: all green. (Do not run two `cargo test --workspace` concurrently.)

- [ ] **Step 2: Bring up the stack with Ranger enabled.** Start MinIO + dev compose (StarRocks + Keycloak) and `cargo run -p egret-bin -- serve` with `ranger.enabled=true` and `engine_egret_url`/the security-XML URL reachable from the StarRocks container. Confirm startup logs show the StarRocks def seeded.

- [ ] **Step 3: Create the profile + a real table.** In the Policies UI, create a `starrocks` profile named `sr1`. Via the StarRocks MySQL client, create a table with a sensitive column and a filterable dimension, e.g.:
```sql
CREATE DATABASE IF NOT EXISTS sales;
CREATE TABLE sales.customer (id INT, name VARCHAR(64), phone VARCHAR(32), region VARCHAR(8))
  PROPERTIES ("replication_num" = "1");
INSERT INTO sales.customer VALUES (1,'A','555-0001','US'),(2,'B','555-0002','EU');
```

- [ ] **Step 4: Author the three policies against `sr1`** (Policies UI, By resource): a **masking** policy on `default_catalog.sales.customer.phone` → `MASK_HASH`, exempting role/user `pii_reader`; a **row-filter** on `sales.customer` → `region = 'US'` for a limited user; an **access** policy denying `select` on the table to an uncleared user. Confirm egret logs show `GET /service/plugins/policies/download/sr1` from the FE after `access_control=ranger` is active.

- [ ] **Step 5: Enforce check — query as two JWT principals** through egret's StarRocks connector (per-user OIDC JWT). Record both outputs for each surface:
  - `SELECT name, phone FROM sales.customer` → cleared user sees `phone` clear, uncleared sees it hashed (masking).
  - `SELECT * FROM sales.customer` → limited user sees only `region='US'` rows (row-filter).
  - `SELECT * FROM sales.customer` as the denied user → authorization error (access-deny).
  Divergence on all three between cleared and uncleared principals = **StarRocks Ranger enforcement VERIFIED LIVE.**

- [ ] **Step 6: Report exactly what ran** (per verification-wording-precision): the live StarRocks query outputs, not automated tests. Note any XML-key/activation adjustments made vs. the plan (fold them back into `deploy/starrocks/README.md`).

**Fallback if the FE never polls:** check the FE log for `RangerAdminRESTClient` errors (URL unreachable from the container / wrong conf path / `access_control` not active). Confirm `curl http://egret:8080/service/plugins/policies/download/sr1` from inside the StarRocks container returns the ServicePolicies JSON. A key/URL mismatch is a one-line XML or activation fix — not a re-architecture.

---

## Self-Review

- **Spec coverage:** service-def+seed → Task 1; resource-based access/mask/row-filter authoring made service-type-aware → Task 2 (serializer) + Task 4 (FE mask types/resources); profile creation without provisioning → Task 3 + Task 4 Step 6; config delivery → Task 5; masked/filtered/denied live proof → Task 6. Tags/provisioning/autocomplete explicitly excluded per spec. Covered.
- **Type consistency:** `editor_to_ranger_json(v, service_type)` — new 2-arg signature used identically in `create_policy`/`update_policy` and all test call sites; `ServiceRow.r#type` is the resolved type; `CreateProfileBody { name, type, config }` ↔ FE `createProfile(name, type)`; `MaskItemTable` `maskTypes?` ↔ `def.data.dataMaskDef.maskTypes` (both `{name; label?}[]`).
- **No placeholders:** service-def id (205) + two concrete edits + a structural parse-test contract; exact serializer code; exact handler edits with surrounding context; exact XML property names; exact smoke SQL. The only intentionally deferred-to-runtime values are the FE conf path and the `access_control` activation form, which Task 5 requires be verified on the live image before committing a mount.
- **Ambiguity:** "parity" is pinned to native resource-based enforcement; the stale-Trino-`schema`-key leak (found in `ResourceSelector`) is fixed by the def-derived initial resources in Task 4 Step 5.
