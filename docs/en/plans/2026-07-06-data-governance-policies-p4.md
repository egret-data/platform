# Data Governance — Policies P4 (Ranger-parity core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add Ranger-parity core to egret's Policies: resource selector v2 (multi-value + include/exclude + recursive + catalog-metadata autocomplete), Allow/Deny/Exceptions + deny-all-else, description + labels, an inline enable toggle in the list, and a summary that unions all principal buckets.

**Architecture:** Extend the P3 `editor_to_ranger_json` DTOs/serializers; fix `summarize_policy`; add `resource-lookup` + `enabled`-toggle endpoints; add a reusable `ResourceLevelEditor` and restructure the Access editor. Editor payloads stay camelCase; the list summary stays snake_case.

**Tech Stack:** Rust (axum 0.8, sqlx/Postgres), React 18 + TanStack Query + shadcn/ui, Vitest.

**Spec:** `docs/en/specs/2026-07-06-data-governance-policies-p4-design.md`.

**Depends on P1–P3** (branch `feat/data-governance-policies`): `ranger_json.rs` (`AccessPolicyInput`, `ResourceLevel` with values/is_excludes/is_recursive, `access_to_ranger_json`, `summarize_policy`, `editor_to_ranger_json`), the CRUD + kind-dispatch handlers, `Policies.tsx`, the three dialogs, `PrincipalPicker`, `api/policies.ts`.

**Conventions (memory):** DB tests self-skip without `DATABASE_URL`; `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings`; frontend `npm run lint && typecheck && test && build`. Never `cargo build -p egret-controlplane` alone (k8s-openapi quirk) — use `cargo build -p egret-bin` / `cargo test -p egret-controlplane --no-run`.

---

### Task 1: Model + serializer — exceptions, deny-all-else, description, labels

**Files:** `crates/egret-controlplane/src/policies/ranger_json.rs`

- [ ] **Step 1: Failing tests** (append to `#[cfg(test)] mod tests`):

```rust
    #[test]
    fn access_serializes_exceptions_and_deny_all_else() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind":"access","name":"p","service":"trino","isEnabled":true,
            "description":"analysts read","labels":["pii","sales"],
            "resources":{"catalog":{"values":["iceberg"],"isExcludes":false,"isRecursive":false},
                         "schema":{"values":["sales"]},"table":{"values":["a","b"]},"column":{"values":["*"]}},
            "items":[{"users":["alice"],"accesses":["select"],"effect":"allow"}],
            "allowExceptions":[{"groups":["contractors"],"accesses":["select"]}],
            "isDenyAllElse":true,
            "denyExceptions":[{"users":["x"],"accesses":["select"]}]
        })).unwrap();
        let b = access_to_ranger_json(&inp);
        assert_eq!(b["policyType"], 0);
        assert_eq!(b["description"], "analysts read");
        assert_eq!(b["policyLabels"][0], "pii");
        assert_eq!(b["resources"]["table"]["values"].as_array().unwrap().len(), 2); // multi-value
        assert_eq!(b["allowExceptions"][0]["groups"][0], "contractors");
        assert_eq!(b["isDenyAllElse"], true);
        // deny-all-else forces deny buckets empty
        assert!(b["denyPolicyItems"].as_array().unwrap().is_empty());
        assert!(b["denyExceptions"].as_array().unwrap().is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**: `cargo test -p egret-controlplane ranger_json --no-run` → FAIL (unknown fields / no allowExceptions on the struct).

- [ ] **Step 3: Extend the DTO + serializer**

In `AccessPolicyInput` add (after `items`):
```rust
    #[serde(default, rename = "allowExceptions")]
    pub allow_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "denyExceptions")]
    pub deny_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "isDenyAllElse")]
    pub is_deny_all_else: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
```
Rewrite `access_to_ranger_json`'s tail (keep the resources map + allow/deny partition of `items`):
```rust
    let deny_all = input.is_deny_all_else;
    let deny_items: Vec<Value> = if deny_all { vec![] } else { deny.iter().map(|it| item_obj(it)).collect() };
    let deny_exc: Vec<Value> = if deny_all { vec![] } else { input.deny_exceptions.iter().map(item_obj).collect() };
    let allow_exc: Vec<Value> = input.allow_exceptions.iter().map(item_obj).collect();
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 0, "isEnabled": input.is_enabled,
        "description": input.description, "policyLabels": input.labels,
        "resources": Value::Object(resources),
        "policyItems": policy_items,
        "denyPolicyItems": deny_items,
        "allowExceptions": allow_exc,
        "denyExceptions": deny_exc,
        "isDenyAllElse": deny_all,
    })
```
(`item_obj` already takes `&AccessItemInput`; `input.deny_exceptions.iter().map(item_obj)` works since it's `fn(&AccessItemInput)->Value`.) Add the same `description: String` + `labels: Vec<String>` fields to `MaskingPolicyInput` and `RowFilterPolicyInput`, and emit `"description"` + `"policyLabels"` in `masking_to_ranger_json`/`rowfilter_to_ranger_json`.

- [ ] **Step 4: Run to verify pass**: `cargo test -p egret-controlplane ranger_json` → all pass (existing P2/P3 tests still green — new fields default, so old payloads unaffected).

- [ ] **Step 5: fmt + commit**
```bash
cargo fmt --all
git add crates/egret-controlplane/src/policies/ranger_json.rs
git commit -m "feat(controlplane): policy exceptions, deny-all-else, description, labels"
```
End with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

### Task 2: Summary fix — union all buckets + description/labels

**Files:** `crates/egret-controlplane/src/policies/ranger_json.rs` (`summarize_policy` + `PolicySummary`)

- [ ] **Step 1: Failing test**:
```rust
    #[test]
    fn summary_unions_deny_and_exception_principals() {
        let body = json!({
            "name":"deny-only","policyType":0,
            "resources":{"catalog":{"values":["iceberg"]}},
            "policyItems":[],
            "denyPolicyItems":[{"groups":["contractors"],"accesses":[{"type":"select"}]}],
            "allowExceptions":[{"users":["alice"],"accesses":[{"type":"insert"}]}],
            "description":"d","policyLabels":["l1"]
        });
        let s = summarize_policy(1, "trino", true, &body);
        assert!(s.principals.contains(&"contractors".to_string())); // from denyPolicyItems
        assert!(s.principals.contains(&"alice".to_string()));       // from allowExceptions
        assert!(s.permissions.contains(&"select".to_string()));
        assert!(s.permissions.contains(&"insert".to_string()));
        assert_eq!(s.description, "d");
        assert_eq!(s.labels, vec!["l1".to_string()]);
    }
```

- [ ] **Step 2: verify fail** (`--no-run`: `description`/`labels` not on `PolicySummary`; and the union assertion would fail once it compiles).

- [ ] **Step 3: Implement**

Add to `PolicySummary`: `pub description: String,` and `pub labels: Vec<String>,`.
In `summarize_policy`, replace the single-array principal/permission collection with a union across buckets for access policies:
```rust
    let buckets: &[&str] = if policy_type == 0 {
        &["policyItems", "denyPolicyItems", "allowExceptions", "denyExceptions"]
    } else {
        std::slice::from_ref(&item_key(policy_type)) // masking/rowfilter: single array
    };
    let mut principals: Vec<String> = Vec::new();
    let mut permissions: Vec<String> = Vec::new();
    for bk in buckets {
        if let Some(items) = body.get(*bk).and_then(Value::as_array) {
            for it in items {
                for key in ["users", "groups", "roles"] {
                    if let Some(arr) = it.get(key).and_then(Value::as_array) {
                        for v in arr { if let Some(s) = v.as_str() {
                            if !principals.contains(&s.to_string()) { principals.push(s.to_string()); }
                        }}
                    }
                }
                if let Some(acc) = it.get("accesses").and_then(Value::as_array) {
                    for a in acc { if let Some(t) = a.get("type").and_then(Value::as_str) {
                        if !permissions.contains(&t.to_string()) { permissions.push(t.to_string()); }
                    }}
                }
            }
        }
    }
```
(`item_key(policy_type)` returns `&'static str`; `from_ref` needs a binding — do `let ik = item_key(policy_type); let buckets = if ... else std::slice::from_ref(&ik);`.) Populate the two new `PolicySummary` fields:
```rust
        description: body.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
        labels: body.get("policyLabels").and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
```

- [ ] **Step 4: pass** (`cargo test -p egret-controlplane ranger_json`), **clippy** (`cargo clippy --workspace --all-targets -- -D warnings`), **fmt**, commit:
```bash
git add crates/egret-controlplane/src/policies/ranger_json.rs
git commit -m "feat(controlplane): summary unions all principal buckets + description/labels"
```
+ trailer.

---

### Task 3: `set_policy_enabled` + `PUT /policies/{id}/enabled`

**Files:** `crates/egret-authz/src/store.rs`, `crates/egret-controlplane/src/policies/mod.rs`, `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1:** Add to `AuthzStore` (mirror `update_policy`'s version+policy_version bump pattern — read it):
```rust
    /// Flip a policy's enabled flag without rewriting its body; bumps versions.
    pub async fn set_policy_enabled(&self, id: i64, enabled: bool) -> Result<(), AuthzError> {
        let svc: Option<i64> = sqlx::query_scalar("SELECT service_id FROM authz_policies WHERE id = $1")
            .bind(id).fetch_optional(&self.db.pool).await?;
        let Some(service_id) = svc else { return Err(AuthzError::NotFound(format!("policy id {id}"))); };
        sqlx::query("UPDATE authz_policies SET enabled=$2, version=version+1, updated_at=now() WHERE id=$1")
            .bind(id).bind(enabled).execute(&self.db.pool).await?;
        sqlx::query("UPDATE authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1")
            .bind(service_id).execute(&self.db.pool).await?;
        Ok(())
    }
```
(Confirm the exact `authz_services` bump SQL matches the one in `update_policy`.)

- [ ] **Step 2:** In `mod.rs` add a handler + route:
```rust
#[derive(Debug, Deserialize)]
pub struct EnabledBody { pub enabled: bool }

async fn set_policy_enabled(
    State(st): State<CpState>, Auth(p): Auth, Path(id): Path<i64>, Json(b): Json<EnabledBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    st.authz.set_policy_enabled(id, b.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}
```
Route: `.route("/policies/{id}/enabled", axum::routing::put(set_policy_enabled))`.

- [ ] **Step 3:** DB-gated route test (reuse helpers): create an access policy → `PUT /policies/{id}/enabled {enabled:false}` → 204 → `GET /policies/{id}` shows `isEnabled:false`. (Model on the P2 CRUD test.)

- [ ] **Step 4:** `cargo test -p egret-controlplane --no-run` (0), clippy clean, fmt, commit:
```bash
git add crates/egret-authz/src/store.rs crates/egret-controlplane/src/policies/mod.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): PUT /policies/{id}/enabled toggle"
```
+ trailer.

---

### Task 4: `resource-lookup` endpoint

**Files:** `crates/egret-controlplane/src/policies/mod.rs`, `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1: Add the handler + route**
```rust
#[derive(Debug, Deserialize)]
pub struct ResourceLookupQuery {
    pub service: String,
    pub level: String, // catalog | schema | table | column
    #[serde(default)] pub catalog: Option<String>,
    #[serde(default)] pub schema: Option<String>,
    #[serde(default)] pub table: Option<String>,
    #[serde(default)] pub q: Option<String>,
}

/// GET /policies/resource-lookup — suggestions for one resource level.
/// catalog: engine-catalog names of engines bound to the profile + values used
/// in the service's existing policies. schema/table/column: distinct values used
/// in the service's policies, filtered so parent levels match. Always allows the
/// caller to free-type in the UI; this only feeds autocomplete.
async fn resource_lookup(
    State(st): State<CpState>, Auth(p): Auth, Query(qp): Query<ResourceLookupQuery>,
) -> Result<Json<Vec<String>>, CpError> {
    require_platform_admin(&p)?;
    let svc = st.authz.get_service(&qp.service).await?;
    let mut out: Vec<String> = Vec::new();
    let mut push = |v: &str| { let v = v.to_string(); if !out.contains(&v) { out.push(v); } };

    // catalog: engine-catalog config of bound engines
    if qp.level == "catalog" {
        if let Ok(engines) = st.storage.list_engines().await {
            for e in engines.iter().filter(|e| e.authz_service_id == Some(svc.id)) {
                if let Ok(cats) = st.storage.list_engine_catalogs(e.id).await {
                    for c in cats { push(&c.name); }
                }
            }
        }
    }
    // all levels: distinct values already used in this service's policies, parent-filtered
    for row in st.authz.list_policies(svc.id).await? {
        let res = row.policy_jsonb.get("resources").and_then(Value::as_object);
        let vals = |k: &str| -> Vec<String> {
            res.and_then(|r| r.get(k)).and_then(|v| v.get("values")).and_then(Value::as_array)
               .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let parent_ok = |k: &str, want: &Option<String>| -> bool {
            match want { None => true, Some(w) => vals(k).iter().any(|v| v == w || v == "*") }
        };
        if qp.level != "catalog" && !parent_ok("catalog", &qp.catalog) { continue; }
        if (qp.level == "table" || qp.level == "column") && !parent_ok("schema", &qp.schema) { continue; }
        if qp.level == "column" && !parent_ok("table", &qp.table) { continue; }
        for v in vals(&qp.level) { if v != "*" { push(&v); } }
    }

    if let Some(q) = qp.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty()) {
        out.retain(|v| v.to_lowercase().contains(&q));
    }
    out.sort();
    out.truncate(50);
    Ok(Json(out))
}
```
Route: `.route("/policies/resource-lookup", get(resource_lookup))`. (Confirm `st.storage.list_engine_catalogs(engine_id)` + `Engine.authz_service_id` exist — they do from P1/P3.)

- [ ] **Step 2:** DB-gated route test: seed a service + one policy with `resources.catalog=["iceberg"], schema=["sales"]`; `GET /policies/resource-lookup?service=..&level=schema&catalog=iceberg` → contains `"sales"`; admin-gate check.

- [ ] **Step 3:** compile (0), clippy clean, fmt, commit:
```bash
git add crates/egret-controlplane/src/policies/mod.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): resource-lookup endpoint (catalog metadata + policy values)"
```
+ trailer.

---

### Task 5: Frontend api — types + resourceLookup + setPolicyEnabled

**Files:** `frontend/src/api/policies.ts`

- [ ] **Step 1:** Extend types + functions:
```ts
// ResourceLevel already exists (values, isExcludes?, isRecursive?). Add to AccessPolicyInput:
//   allowExceptions: AccessItem[]; denyExceptions: AccessItem[]; isDenyAllElse: boolean;
//   description: string; labels: string[];
// Add description?: string; labels?: string[] to MaskingPolicyInput + RowFilterPolicyInput.
// PolicySummary gains: description: string; labels: string[];

export function resourceLookup(
  service: string, level: "catalog"|"schema"|"table"|"column",
  parents: { catalog?: string; schema?: string; table?: string } = {}, q = "",
): Promise<string[]> {
  const p = new URLSearchParams({ service, level });
  if (parents.catalog) p.set("catalog", parents.catalog);
  if (parents.schema) p.set("schema", parents.schema);
  if (parents.table) p.set("table", parents.table);
  if (q) p.set("q", q);
  return apiJson<string[]>(`/control-plane/policies/resource-lookup?${p.toString()}`);
}

export function setPolicyEnabled(id: number, enabled: boolean): Promise<void> {
  return apiFetch(`/control-plane/policies/${id}/enabled`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ enabled }),
  }).then(() => undefined);
}
```
Update `AccessPolicyInput`, `MaskingPolicyInput`, `RowFilterPolicyInput`, `PolicySummary` interfaces accordingly.

- [ ] **Step 2:** `npm run typecheck` (surfaces call sites needing the new fields — fix defaults in the dialogs in Tasks 7-8; for this task, just ensure the interfaces compile — the dialogs still build blank inputs without the new fields until Task 7). To keep this task green, add the new fields as **optional** on the input interfaces? NO — they must be required to match the backend serde defaults semantics on the TS side; instead, in this task ALSO add the new fields to the dialogs' `blankInput`/`emptyItem` (a small edit each) so typecheck passes. Recommended: do the minimal `blankInput` additions here.

- [ ] **Step 3:** commit:
```bash
git add frontend/src/api/policies.ts frontend/src/components/policy/*.tsx
git commit -m "feat(frontend): policies API — exceptions/deny-all-else/description/labels, resource-lookup, enable toggle"
```
+ trailer.

---

### Task 6: `ResourceLevelEditor` component

**Files:** `frontend/src/components/policy/ResourceLevelEditor.tsx` (new), `__tests__/ResourceLevelEditor.test.tsx`

- [ ] **Step 1: Failing test** — renders selected value badges, adds a typed value, toggles exclude:
```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { ResourceLevelEditor } from "../ResourceLevelEditor";
vi.mock("@/api/policies", () => ({ resourceLookup: () => Promise.resolve(["sales","eng"]) }));
function wrap(u: React.ReactNode){const qc=new QueryClient({defaultOptions:{queries:{retry:false}}});return render(<QueryClientProvider client={qc}>{u}</QueryClientProvider>);}
describe("ResourceLevelEditor", () => {
  it("shows values and toggles exclude", () => {
    const onChange = vi.fn();
    wrap(<ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
      value={{ values:["sales"], isExcludes:false, isRecursive:false }} onChange={onChange} />);
    expect(screen.getByText("sales")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Exclude Schema"));
    expect(onChange).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Implement** (mirror `PrincipalPicker`'s Popover+Input+badge pattern; add two `Switch`es — confirm shadcn `Switch` exists at `@/components/ui/switch`; if not, use a `Checkbox` or a plain styled button toggle):
```tsx
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { type ResourceLevel, resourceLookup } from "@/api/policies";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

interface Props {
  label: string;
  level: "catalog" | "schema" | "table" | "column";
  service: string;
  parents: { catalog?: string; schema?: string; table?: string };
  value: ResourceLevel;
  onChange: (v: ResourceLevel) => void;
}

export function ResourceLevelEditor({ label, level, service, parents, value, onChange }: Props) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const sug = useQuery<string[]>({
    queryKey: ["resource-lookup", service, level, parents, q],
    queryFn: () => resourceLookup(service, level, parents, q),
    enabled: open, retry: false,
  });
  const add = (v: string) => { const t = v.trim(); if (t && !value.values.includes(t)) onChange({ ...value, values: [...value.values, t] }); setQ(""); };
  const remove = (v: string) => onChange({ ...value, values: value.values.filter((x) => x !== v) });
  const suggestions = (sug.data ?? []).filter((s) => !value.values.includes(s));

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between">
        <label className="text-sm font-medium">{label}</label>
        <div className="flex gap-3 text-xs">
          <label className="flex items-center gap-1"><input type="checkbox" aria-label={`Exclude ${label}`}
            checked={!!value.isExcludes} onChange={(e) => onChange({ ...value, isExcludes: e.target.checked })} />Exclude</label>
          <label className="flex items-center gap-1"><input type="checkbox" aria-label={`Recursive ${label}`}
            checked={!!value.isRecursive} onChange={(e) => onChange({ ...value, isRecursive: e.target.checked })} />Recursive</label>
        </div>
      </div>
      <div className="flex flex-wrap gap-1">
        {value.values.map((v) => (
          <Badge key={v} variant="secondary" className="gap-1">{v}
            <button type="button" aria-label={`remove ${v}`} onClick={() => remove(v)}><X className="h-3 w-3" /></button></Badge>
        ))}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Input placeholder="value or * (Enter to add)" value={q} onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); add(q); } }} />
        </PopoverTrigger>
        <PopoverContent align="start" className="max-h-56 w-64 overflow-y-auto p-1" onOpenAutoFocus={(e) => e.preventDefault()}>
          {suggestions.map((s) => (
            <button key={s} type="button" onClick={() => add(s)} className="block w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent">{s}</button>
          ))}
          {suggestions.length === 0 && <div className="px-2 py-1.5 text-sm text-muted-foreground">Type a value or * and press Enter.</div>}
        </PopoverContent>
      </Popover>
    </div>
  );
}
```
(Using native checkboxes for Exclude/Recursive to avoid a hard dependency on a shadcn `Switch` that may not exist — check `@/components/ui/switch`; if present, prefer it.)

- [ ] **Step 3:** test PASS; `npm run lint && typecheck`; commit:
```bash
git add frontend/src/components/policy/ResourceLevelEditor.tsx frontend/src/components/policy/__tests__/ResourceLevelEditor.test.tsx
git commit -m "feat(frontend): ResourceLevelEditor (multi-value + exclude/recursive + autocomplete)"
```
+ trailer.

---

### Task 7: Access editor restructure

**Files:** `frontend/src/components/policy/AccessPolicyDialog.tsx`, `frontend/src/routes/control-plane/Policies.tsx` (reverse-map), test updates.

- [ ] **Step 1:** Restructure `AccessPolicyDialog` (keep the mutation/`initial`/`useEffect` machinery). Changes:
  - `blankInput` returns the new fields: `allowExceptions: []`, `denyExceptions: []`, `isDenyAllElse: false`, `description: ""`, `labels: []`, resources with `values: []`.
  - **Header:** Name `Input` + Enabled checkbox (existing) + a `Popover` "…" button containing a `Textarea` for description (add `@/components/ui/textarea` — confirm it exists; else use a plain `<textarea>`) and a simple labels editor (comma-typed → array, or reuse the badge-input pattern).
  - **Resources:** replace the four resource `Input`s with four `ResourceLevelEditor`s (`level` catalog/schema/table/column, `parents` threaded from the higher levels' first value, `value`/`onChange` bound to `form.resources[level]`).
  - **Deny-all-else:** a checkbox/switch bound to `form.isDenyAllElse`.
  - **Allow | Deny `Tabs`** (`@/components/ui/tabs` — confirm exists; else two buttons toggling a local `side` state). When `isDenyAllElse`, render only the Allow tab (hide/disable Deny). Each tab: the existing repeatable item rows for that side (`items` filtered/kept by `effect`, OR keep `items` as the Allow list and a separate `denyItems` — SIMPLEST: keep the P2 `items` array with per-item `effect`, and render allow-effect items under Allow, deny-effect under Deny; "Add item" in a tab appends an item with that tab's effect). Below each tab's items, a collapsed `Accordion` "Exceptions" with its own repeatable rows bound to `form.allowExceptions` (Allow tab) / `form.denyExceptions` (Deny tab).
  - Reuse the existing access-type checkbox + `PrincipalPicker` per row for all four buckets.

- [ ] **Step 2:** Extend `toInput` in `Policies.tsx` to read `allowExceptions`, `denyExceptions`, `isDenyAllElse`, `description`, `policyLabels`, and multi-value/exclude/recursive resources (map `resources.<lvl>.values` as-is, `isExcludes`/`isRecursive`).

- [ ] **Step 3:** Update `AccessPolicyDialog.test.tsx` — the create test should still pass (fill name + a catalog value via the new `ResourceLevelEditor`'s input + Enter, click Save, assert `createPolicy` called with `kind:"access"` and the catalog value). Add a test: toggle deny-all-else and assert the Deny tab hides. Add a test: add an allow exception and assert it's in the posted `allowExceptions`.

- [ ] **Step 4:** `npm run lint && typecheck && test && build` — all pass. Commit:
```bash
git add frontend/src/components/policy/AccessPolicyDialog.tsx frontend/src/routes/control-plane/Policies.tsx frontend/src/components/policy/__tests__/AccessPolicyDialog.test.tsx
git commit -m "feat(frontend): Access editor — resources v2, allow/deny/exceptions, deny-all-else, description/labels"
```
+ trailer.

---

### Task 8: Masking/Row-filter + list (ResourceLevelEditor, description/labels, inline enable toggle)

**Files:** `MaskingPolicyDialog.tsx`, `RowFilterPolicyDialog.tsx`, `Policies.tsx`, test updates.

- [ ] **Step 1:** In `MaskingPolicyDialog` (4 levels) and `RowFilterPolicyDialog` (3 levels), replace resource `Input`s with `ResourceLevelEditor`s; add the description/labels "…" popover (same as Access); include `description`/`labels` in their `blankInput`. Extend `maskingToInput`/`rowfilterToInput` (in `Policies.tsx`) to read `description`/`policyLabels` + multi-value resources.

- [ ] **Step 2:** In `Policies.tsx` list table:
  - Add an inline **enable toggle** in the Enabled cell: a checkbox/switch that calls `setPolicyEnabled(r.id, next)` via a `useMutation`, invalidating `["control-plane","policies"]`. Replace the read-only badge with the toggle (keep a disabled look while pending).
  - Add a **Labels** column rendering `r.labels` as `Badge`s (empty → "—").
  - Optionally show `r.description` as a muted subtext under the name.

- [ ] **Step 3:** Update `Policies.test.tsx`: the enable toggle calls `setPolicyEnabled` (mock it); labels render. Update the masking/rowfilter mocks/tests if their resource input changed (fill via `ResourceLevelEditor`).

- [ ] **Step 4:** `npm run lint && typecheck && test && build` — all pass. Commit:
```bash
git add frontend/src/components/policy/MaskingPolicyDialog.tsx frontend/src/components/policy/RowFilterPolicyDialog.tsx frontend/src/routes/control-plane/Policies.tsx frontend/src/routes/control-plane/__tests__/Policies.test.tsx
git commit -m "feat(frontend): masking/rowfilter resources v2 + description/labels + inline enable toggle in list"
```
+ trailer.

---

### Task 9: Full CI + manual smoke

- [ ] **Step 1:** `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --no-run`, then against live Postgres `cargo test --workspace` (new DB tests: exceptions/deny-all-else create + summary union, resource-lookup, enabled toggle).
- [ ] **Step 2:** `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] **Step 3:** Manual smoke (per `egret-frontend-viz-verify`): create an Access policy with a multi-value table resource + an Exclude on a column + an allow exception; toggle deny-all-else (Deny tab hides); Save → list shows it with unioned principals; toggle its enable switch in the list; confirm a deny-only policy now shows its principals. State exactly what was exercised.
- [ ] **Step 4:** Commit any fixups, then stop for review.

---

## Self-Review notes
- **Spec coverage (P4 core):** resource selector v2 (Tasks 4,6,7,8) ✓; allowExceptions/denyExceptions + deny-all-else (Tasks 1,7) ✓; description + labels (Tasks 1,2,7,8) ✓; inline enable toggle (Tasks 3,8) ✓; summary union all buckets — fixes the deny-only bug (Task 2) ✓; autocomplete from catalog metadata + policy values (Task 4) ✓. **Deferred (P5):** roles, per-access isAllowed, validity, delegate-admin, conditions, zones, live warehouse-browser enrichment.
- **Type consistency:** editor inputs camelCase (`allowExceptions`/`denyExceptions`/`isDenyAllElse`/`policyLabels`→`labels` on the wire/`isExcludes`/`isRecursive`); list summary snake_case (`policy_type`/`resource_path`/`is_enabled` + new `description`/`labels`). `access_to_ranger_json` ↔ `toInput` mirror the four buckets + deny-all-else.
- **Open confirmations for the implementer:** the exact `req`/`session_for`/`into_json` helpers; the `authz_services` version-bump SQL to match `update_policy`. NOTE: `@/components/ui/{switch,tabs,textarea,accordion,checkbox,tooltip}` ALL EXIST — prefer the shadcn `Switch` for the Exclude/Recursive/deny-all-else/enable toggles and `Tabs`/`TabsList`/`TabsTrigger`/`TabsContent` for Allow|Deny, `Accordion` for the collapsed Exceptions, `Textarea` for description (the plan's "native fallback" notes are moot — use the real components).
