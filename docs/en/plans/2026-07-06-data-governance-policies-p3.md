# Data Governance — Policies P3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the Policies editor to full Ranger parity — add **Column-masking** and **Row-filter** policy editors (policyType 1 & 2) alongside the P2 Access editor — and deliver the **Layer-B "Download Trino config"** bundle that lets a Trino coordinator enforce a profile's policies via the native `trino-ranger` plugin.

**Architecture:** `ranger_json.rs` gains `masking_to_ranger_json` + `rowfilter_to_ranger_json` serializers and a single `editor_to_ranger_json` dispatcher keyed on a `kind` field (default `access`, so the P2 access path stays wire-compatible). The create/update handlers accept `Json<Value>` and dispatch. A new `GET /policies/services/{service}/trino-config` endpoint renders the two plugin config files from `CpState.engine_egret_url` + the service name. Two new React dialogs (Masking, Row-filter) reuse `PrincipalPicker`; the list gains New/Edit/Delete for all three tabs plus a "Download Trino config" action.

**Tech Stack:** Rust (axum 0.8, sqlx/Postgres), React 18 + TanStack Query + shadcn/ui, Vitest, and (verification only) the Java `tests/ranger` Maven harness.

**Spec:** `docs/en/specs/2026-07-06-data-governance-policies-design.md` (§3 masking/row-filter editors, §4 Layer-B config artifact).

**Depends on P1+P2** (branch `feat/data-governance-policies`): `ranger_json.rs` (`AccessPolicyInput`, `access_to_ranger_json`), the CRUD endpoints, `Policies.tsx` list with the Access editor, `PrincipalPicker`, `api/policies.ts`.

**Ranger facts (from `crates/egret-authz/src/service-defs/trino.json`, verified):**
- **Masking** = policyType 1; resources `catalog/schema/table/column` (column is the masked field); items in `dataMaskPolicyItems: [{users,groups,accesses:[{type:"select",isAllowed:true}], dataMaskInfo:{dataMaskType,valueExpr?}}]`. Mask types: `MASK, MASK_SHOW_LAST_4, MASK_SHOW_FIRST_4, MASK_HASH, MASK_NULL, MASK_NONE, MASK_DATE_SHOW_YEAR, CUSTOM` (CUSTOM carries a `valueExpr`).
- **Row-filter** = policyType 2; resources `catalog/schema/table` (mandatory, NO column); items in `rowFilterPolicyItems: [{users,groups,accesses:[{type:"select",isAllowed:true}], rowFilterInfo:{filterExpr}}]`.
- The native `trino-ranger` plugin (ships in stock Trino) is enabled by `etc/access-control.properties` (`access-control.name=ranger`, `ranger.service.name`, `ranger.plugin.config.resource=ranger-trino-security.xml`) + `ranger-trino-security.xml` whose `ranger.plugin.trino.policy.rest.url` points at egret's base URL (which serves `/service/...`). egret's default `ranger.auth_token` is empty → **anonymous `/service`**, which the stock plugin uses (no client-auth wiring needed).

**Conventions (project memory):** DB tests self-skip without `DATABASE_URL`; `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings`; frontend `npm run lint && typecheck && test && build`. Do NOT `cargo build -p egret-controlplane` alone — use `cargo build -p egret-bin` or `cargo test -p egret-controlplane --no-run`.

---

### Task 1: Masking + Row-filter serializers + dispatcher

**Files:**
- Modify: `crates/egret-controlplane/src/policies/ranger_json.rs`

- [ ] **Step 1: Write the failing tests** (append to the `#[cfg(test)] mod tests` in `ranger_json.rs`):

```rust
    fn masking_sample() -> MaskingPolicyInput {
        serde_json::from_value(json!({
            "kind": "masking", "name": "mask-email", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},
                          "table":{"values":["users"]},"column":{"values":["email"]}},
            "items": [{"users":["alice"],"groups":["pii"],"maskType":"MASK_HASH"}]
        })).unwrap()
    }

    #[test]
    fn serializes_masking_policy() {
        let body = masking_to_ranger_json(&masking_sample());
        assert_eq!(body["policyType"], 1);
        assert_eq!(body["resources"]["column"]["values"][0], "email");
        let items = body["dataMaskPolicyItems"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["dataMaskInfo"]["dataMaskType"], "MASK_HASH");
        assert_eq!(items[0]["accesses"][0]["type"], "select");
        assert_eq!(items[0]["users"][0], "alice");
        // round-trip through the P1 summarizer
        let s = crate::summarize_policy(1, "trino", true, &body);
        assert_eq!(s.policy_type, "masking");
        assert_eq!(s.resource_path, "iceberg.sales.users.email");
    }

    #[test]
    fn serializes_masking_custom_value_expr() {
        let mut inp = masking_sample();
        inp.items[0].mask_type = "CUSTOM".into();
        inp.items[0].value_expr = Some("substr({col},1,2)".into());
        let body = masking_to_ranger_json(&inp);
        assert_eq!(body["dataMaskPolicyItems"][0]["dataMaskInfo"]["valueExpr"], "substr({col},1,2)");
    }

    fn rowfilter_sample() -> RowFilterPolicyInput {
        serde_json::from_value(json!({
            "kind": "rowfilter", "name": "us-only", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},"table":{"values":["orders"]}},
            "items": [{"groups":["analysts"],"filterExpr":"country = 'US'"}]
        })).unwrap()
    }

    #[test]
    fn serializes_rowfilter_policy() {
        let body = rowfilter_to_ranger_json(&rowfilter_sample());
        assert_eq!(body["policyType"], 2);
        assert!(body["resources"].get("column").is_none());
        let items = body["rowFilterPolicyItems"].as_array().unwrap();
        assert_eq!(items[0]["rowFilterInfo"]["filterExpr"], "country = 'US'");
        assert_eq!(items[0]["groups"][0], "analysts");
        let s = crate::summarize_policy(2, "trino", true, &body);
        assert_eq!(s.policy_type, "rowfilter");
        assert_eq!(s.resource_path, "iceberg.sales.orders");
    }

    #[test]
    fn dispatcher_routes_by_kind() {
        let acc = editor_to_ranger_json(&json!({"kind":"access","name":"a","service":"trino",
            "resources":{"catalog":{"values":["c"]}},"items":[]})).unwrap();
        assert_eq!(acc.body["policyType"], 0);
        assert_eq!(acc.name, "a");
        let m = editor_to_ranger_json(&serde_json::to_value(masking_sample()).unwrap()).unwrap();
        assert_eq!(m.body["policyType"], 1);
        // missing kind defaults to access (back-compat with P2 clients)
        let d = editor_to_ranger_json(&json!({"name":"x","service":"trino","resources":{},"items":[]})).unwrap();
        assert_eq!(d.body["policyType"], 0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-controlplane ranger_json --no-run`
Expected: FAIL — `MaskingPolicyInput` / `RowFilterPolicyInput` / `masking_to_ranger_json` / `rowfilter_to_ranger_json` / `editor_to_ranger_json` not found.

- [ ] **Step 3: Implement the masking + row-filter DTOs + serializers** (add to `ranger_json.rs`, reusing the existing `EditorResources` / `resource_obj` / `default_true`):

```rust
/// One masking row: principals + the mask applied to the resource column.
#[derive(Debug, Deserialize)]
pub struct MaskItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    /// One of the service-def maskTypes (MASK, MASK_HASH, MASK_NULL, CUSTOM, …).
    #[serde(default, rename = "maskType")]
    pub mask_type: String,
    /// Custom transform expression, only when maskType == "CUSTOM".
    #[serde(default, rename = "valueExpr")]
    pub value_expr: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MaskingPolicyInput {
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: EditorResources,
    #[serde(default)]
    pub items: Vec<MaskItemInput>,
}

fn mask_item_obj(it: &MaskItemInput) -> Value {
    let mut info = serde_json::Map::new();
    info.insert("dataMaskType".into(), Value::from(it.mask_type.clone()));
    if it.mask_type == "CUSTOM" {
        if let Some(expr) = &it.value_expr {
            info.insert("valueExpr".into(), Value::from(expr.clone()));
        }
    }
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "dataMaskInfo": Value::Object(info),
    })
}

/// Render a `MaskingPolicyInput` into a Ranger policyType-1 (masking) body.
pub fn masking_to_ranger_json(input: &MaskingPolicyInput) -> Value {
    let mut resources = serde_json::Map::new();
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
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 1, "isEnabled": input.is_enabled,
        "resources": Value::Object(resources),
        "dataMaskPolicyItems": input.items.iter().map(mask_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
    })
}

/// One row-filter row: principals + the SQL predicate applied to the resource table.
#[derive(Debug, Deserialize)]
pub struct RowFilterItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, rename = "filterExpr")]
    pub filter_expr: String,
}

/// Row-filter resources are catalog/schema/table only (no column).
#[derive(Debug, Default, Deserialize)]
pub struct RowFilterResources {
    #[serde(default)]
    pub catalog: ResourceLevel,
    #[serde(default)]
    pub schema: ResourceLevel,
    #[serde(default)]
    pub table: ResourceLevel,
}

#[derive(Debug, Deserialize)]
pub struct RowFilterPolicyInput {
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: RowFilterResources,
    #[serde(default)]
    pub items: Vec<RowFilterItemInput>,
}

fn rowfilter_item_obj(it: &RowFilterItemInput) -> Value {
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "rowFilterInfo": { "filterExpr": it.filter_expr },
    })
}

/// Render a `RowFilterPolicyInput` into a Ranger policyType-2 (row-filter) body.
pub fn rowfilter_to_ranger_json(input: &RowFilterPolicyInput) -> Value {
    let mut resources = serde_json::Map::new();
    for (key, level) in [
        ("catalog", &input.resources.catalog),
        ("schema", &input.resources.schema),
        ("table", &input.resources.table),
    ] {
        if !level.values.is_empty() {
            resources.insert(key.to_string(), resource_obj(level));
        }
    }
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 2, "isEnabled": input.is_enabled,
        "resources": Value::Object(resources),
        "rowFilterPolicyItems": input.items.iter().map(rowfilter_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
    })
}
```

- [ ] **Step 4: Implement the dispatcher** (in `ranger_json.rs`):

```rust
/// A serialized policy plus the metadata the handlers need to store it.
pub struct RangerPolicy {
    pub name: String,
    pub service: String,
    pub is_enabled: bool,
    pub body: Value,
}

/// Dispatch an editor form (`{kind, ...}`) to the right serializer. Missing
/// `kind` defaults to "access" so P2 clients stay wire-compatible.
pub fn editor_to_ranger_json(v: &Value) -> Result<RangerPolicy, String> {
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("access");
    match kind {
        "access" => {
            let inp: AccessPolicyInput = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy { name: inp.name.clone(), service: inp.service.clone(), is_enabled: inp.is_enabled, body: access_to_ranger_json(&inp) })
        }
        "masking" => {
            let inp: MaskingPolicyInput = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy { name: inp.name.clone(), service: inp.service.clone(), is_enabled: inp.is_enabled, body: masking_to_ranger_json(&inp) })
        }
        "rowfilter" => {
            let inp: RowFilterPolicyInput = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy { name: inp.name.clone(), service: inp.service.clone(), is_enabled: inp.is_enabled, body: rowfilter_to_ranger_json(&inp) })
        }
        other => Err(format!("unknown policy kind: {other}")),
    }
}
```
Update the `pub use ranger_json::{...}` in `mod.rs` to also export `editor_to_ranger_json` (and `RangerPolicy` if referenced by handlers). `AccessPolicyInput` must `#[derive(Serialize)]` too now (the dispatcher test serializes `masking_sample()` — actually it serializes MaskingPolicyInput; add `Serialize` to whichever inputs the tests `to_value`, i.e. `MaskingPolicyInput` + its `MaskItemInput` + `EditorResources`/`ResourceLevel`. Add `#[derive(Serialize)]` alongside `Deserialize` on those, and `#[derive(Default)]` already exists where needed).

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p egret-controlplane ranger_json`
Expected: all masking/rowfilter/dispatcher tests PASS (plus the P2 access tests still green).

- [ ] **Step 6: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-controlplane/src/policies/ranger_json.rs crates/egret-controlplane/src/policies/mod.rs
git commit -m "feat(controlplane): masking + row-filter serializers + kind dispatcher"
```
End with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

### Task 2: Dispatch create/update by kind

**Files:**
- Modify: `crates/egret-controlplane/src/policies/mod.rs` (rewrite `create_policy` + `update_policy` to take `Json<Value>` and dispatch)
- Test: `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1: Rewrite the two handlers**

Replace the P2 `create_policy` / `update_policy` bodies (which took `Json<AccessPolicyInput>`) with the dispatcher form:

```rust
/// POST /policies — create an Access/Masking/Row-filter policy (by `kind`).
async fn create_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(v): Json<Value>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    let pol = editor_to_ranger_json(&v).map_err(CpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    let svc = st.authz.get_service(&pol.service).await?;
    let id = st.authz.create_policy(svc.id, &pol.name, &pol.body).await?;
    if !pol.is_enabled {
        st.authz.update_policy(id, &pol.name, &pol.body, false).await?;
    }
    Ok(Json(json!({ "id": id })))
}

/// PUT /policies/{id} — replace a policy's body (any kind).
async fn update_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>, CpError> {
    require_platform_admin(&p)?;
    let pol = editor_to_ranger_json(&v).map_err(CpError::BadRequest)?;
    if pol.name.trim().is_empty() {
        return Err(CpError::BadRequest("policy name is required".into()));
    }
    st.authz.update_policy(id, &pol.name, &pol.body, pol.is_enabled).await?;
    Ok(Json(json!({ "id": id })))
}
```
Remove the now-unused direct `AccessPolicyInput`/`access_to_ranger_json` imports IF they're no longer referenced in `mod.rs` (the dispatcher is now the only caller) — but keep the `pub use` re-exports. Confirm `editor_to_ranger_json` is imported/in-scope.

- [ ] **Step 2: Add masking + row-filter route tests** (in `routes_api.rs`, reuse `setup`/`session_for`/`req`/`into_json`):

```rust
#[tokio::test]
async fn admin_can_create_masking_and_rowfilter_policies() {
    let Some(f) = setup().await else { return };
    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz).await.unwrap();
    let svc = authz.create_service("trino_mrf", "trino", &serde_json::json!({})).await.unwrap();
    let token = session_for(&f, "mrf", true).await;

    let mask = serde_json::json!({
        "kind":"masking","name":"m1","service":"trino_mrf","isEnabled":true,
        "resources":{"catalog":{"values":["c"]},"schema":{"values":["s"]},"table":{"values":["t"]},"column":{"values":["email"]}},
        "items":[{"groups":["pii"],"maskType":"MASK_HASH"}]
    });
    let (ms, _) = into_json(f.app.clone().oneshot(req("POST","/policies",&token,Some(mask))).await.unwrap()).await;
    assert_eq!(ms, StatusCode::OK);

    let rf = serde_json::json!({
        "kind":"rowfilter","name":"r1","service":"trino_mrf","isEnabled":true,
        "resources":{"catalog":{"values":["c"]},"schema":{"values":["s"]},"table":{"values":["t"]}},
        "items":[{"users":["bob"],"filterExpr":"country = 'US'"}]
    });
    let (rs, _) = into_json(f.app.clone().oneshot(req("POST","/policies",&token,Some(rf))).await.unwrap()).await;
    assert_eq!(rs, StatusCode::OK);

    let (_, masking_list) = into_json(f.app.clone().oneshot(req("GET","/policies?service=trino_mrf&type=masking",&token,None)).await.unwrap()).await;
    assert_eq!(masking_list.as_array().unwrap().len(), 1);
    let (_, rf_list) = into_json(f.app.clone().oneshot(req("GET","/policies?service=trino_mrf&type=rowfilter",&token,None)).await.unwrap()).await;
    assert_eq!(rf_list.as_array().unwrap().len(), 1);

    authz.delete_service(svc.id).await.unwrap();
}
```

- [ ] **Step 3: Verify**

- `cargo test -p egret-controlplane --no-run` → EXIT 0 (the P2 access route test still compiles — it posts a body without `kind`, which the dispatcher defaults to access, so it still passes).
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- Best-effort DB run (PoolTimedOut acceptable).

- [ ] **Step 4: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-controlplane/src/policies/mod.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): dispatch policy create/update by kind (access/masking/rowfilter)"
```
End with the Co-Authored-By trailer.

---

### Task 3: Layer-B — `trino-config` endpoint

**Files:**
- Modify: `crates/egret-controlplane/src/policies/mod.rs`
- Test: `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1: Add the handler + route**

```rust
#[derive(Debug, Serialize)]
pub struct ConfigFile {
    pub name: String,
    pub content: String,
}

/// GET /policies/services/{service}/trino-config — the two files a Trino
/// coordinator needs to enforce this profile via the native ranger plugin.
async fn trino_config(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(service): Path<String>,
) -> Result<Json<Vec<ConfigFile>>, CpError> {
    require_platform_admin(&p)?;
    // validate the service exists
    let _ = st.authz.get_service(&service).await?;
    let base = st.engine_egret_url.trim_end_matches('/');

    let props = format!(
        "access-control.name=ranger\n\
         ranger.service.name={service}\n\
         ranger.plugin.config.resource=ranger-trino-security.xml\n"
    );
    let xml = format!(
        "<?xml version=\"1.0\"?>\n\
         <?xml-stylesheet type=\"text/xsl\" href=\"configuration.xsl\"?>\n\
         <configuration>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.policy.rest.url</name>\n\
         \x20   <value>{base}</value>\n\
         \x20 </property>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.service.name</name>\n\
         \x20   <value>{service}</value>\n\
         \x20 </property>\n\
         \x20 <property>\n\
         \x20   <name>ranger.plugin.trino.policy.pollIntervalMs</name>\n\
         \x20   <value>30000</value>\n\
         \x20 </property>\n\
         </configuration>\n"
    );
    Ok(Json(vec![
        ConfigFile { name: "access-control.properties".into(), content: props },
        ConfigFile { name: "ranger-trino-security.xml".into(), content: xml },
    ]))
}
```
Add the route: `.route("/policies/services/{service}/trino-config", get(trino_config))`.

> Note: egret's default `ranger.auth_token` is empty → anonymous `/service`, which the stock plugin reads without client-auth config. If a deployment sets an auth token, the coordinator needs an authenticating reverse proxy or a plugin build that sends the bearer — out of scope for P3; document in the download dialog.

- [ ] **Step 2: Route test**

```rust
#[tokio::test]
async fn admin_gets_trino_config_for_profile() {
    let Some(f) = setup().await else { return };
    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz).await.unwrap();
    let svc = authz.create_service("trino_cfg", "trino", &serde_json::json!({})).await.unwrap();
    let token = session_for(&f, "cfg", true).await;
    let (status, body) = into_json(
        f.app.clone().oneshot(req("GET","/policies/services/trino_cfg/trino-config",&token,None)).await.unwrap()
    ).await;
    assert_eq!(status, StatusCode::OK);
    let files = body.as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f["name"] == "access-control.properties"
        && f["content"].as_str().unwrap().contains("ranger.service.name=trino_cfg")));
    assert!(files.iter().any(|f| f["name"] == "ranger-trino-security.xml"
        && f["content"].as_str().unwrap().contains("ranger.plugin.trino.policy.rest.url")));
    authz.delete_service(svc.id).await.unwrap();
}
```
Note: the test fixture's `engine_egret_url` is `"http://egret:8080"` (set in `setup_full`), so the XML `<value>` will be that.

- [ ] **Step 3: Verify + commit**

`cargo test -p egret-controlplane --no-run` (EXIT 0), `cargo clippy --workspace --all-targets -- -D warnings` (clean), `cargo fmt --all`.
```bash
git add crates/egret-controlplane/src/policies/mod.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): Trino ranger-plugin config bundle endpoint (Layer B)"
```
End with the Co-Authored-By trailer.

---

### Task 4: Frontend API — masking/rowfilter inputs, kind, trino-config

**Files:**
- Modify: `frontend/src/api/policies.ts`

- [ ] **Step 1: Extend the types + functions**

```ts
export interface MaskItem { users: string[]; groups: string[]; maskType: string; valueExpr?: string; }
export interface MaskingPolicyInput {
  kind: "masking";
  name: string; service: string; isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel; column: ResourceLevel };
  items: MaskItem[];
}
export interface RowFilterItem { users: string[]; groups: string[]; filterExpr: string; }
export interface RowFilterPolicyInput {
  kind: "rowfilter";
  name: string; service: string; isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel };
  items: RowFilterItem[];
}
/** Access input now also carries an explicit kind. */
export type PolicyInput = (AccessPolicyInput & { kind: "access" }) | MaskingPolicyInput | RowFilterPolicyInput;

export interface ConfigFile { name: string; content: string; }
export function getTrinoConfig(service: string): Promise<ConfigFile[]> {
  return apiJson<ConfigFile[]>(`/control-plane/policies/services/${encodeURIComponent(service)}/trino-config`);
}

// mask-type options (labels mirror the trino service-def maskTypes)
export const MASK_TYPES = [
  { name: "MASK", label: "Redact" },
  { name: "MASK_SHOW_LAST_4", label: "Show last 4" },
  { name: "MASK_SHOW_FIRST_4", label: "Show first 4" },
  { name: "MASK_HASH", label: "Hash" },
  { name: "MASK_NULL", label: "Nullify" },
  { name: "MASK_NONE", label: "Unmasked" },
  { name: "MASK_DATE_SHOW_YEAR", label: "Date: year only" },
  { name: "CUSTOM", label: "Custom expression" },
] as const;
```
Update `createPolicy`/`updatePolicy` signatures to accept `PolicyInput` instead of `AccessPolicyInput` (a wider type; the Access editor must now pass `{...input, kind: "access"}`). Keep them otherwise unchanged.

- [ ] **Step 2: typecheck** (`cd frontend && npm run typecheck`) — this will surface the Access editor call sites that now need `kind: "access"`; you'll fix those in Task 7. For now, temporarily update `createPolicy`/`updatePolicy` to accept `PolicyInput` and let typecheck flag AccessPolicyDialog — note the errors, they're expected until Task 5-7. If you want this task to stay green in isolation, ALSO do the one-line AccessPolicyDialog fix here (add `kind: "access"` where it builds the mutation payload) — recommended.

- [ ] **Step 3: commit**

```bash
git add frontend/src/api/policies.ts frontend/src/components/policy/AccessPolicyDialog.tsx
git commit -m "feat(frontend): policies API — masking/rowfilter inputs, kind discriminator, trino-config"
```
End with the Co-Authored-By trailer.

---

### Task 5: `MaskingPolicyDialog`

Mirrors `AccessPolicyDialog` but: 4 resource levels (column required), and per-item a **mask-type `Select`** (+ a `valueExpr` `Input` shown only when `CUSTOM`) instead of allow/deny + access checkboxes.

**Files:**
- Create: `frontend/src/components/policy/MaskingPolicyDialog.tsx`
- Test: `frontend/src/components/policy/__tests__/MaskingPolicyDialog.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { MaskingPolicyDialog } from "../MaskingPolicyDialog";

const createPolicy = vi.fn((_i: unknown) => Promise.resolve({ id: 1 }));
vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  createPolicy: (i: unknown) => createPolicy(i),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 1 })),
  listPrincipals: () => Promise.resolve([]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("MaskingPolicyDialog", () => {
  it("creates a masking policy", async () => {
    wrap(<MaskingPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "m1" } });
    fireEvent.change(screen.getByLabelText("Column"), { target: { value: "email" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as { kind: string; resources: { column: { values: string[] } } };
    expect(arg.kind).toBe("masking");
    expect(arg.resources.column.values).toEqual(["email"]);
  });
});
```

- [ ] **Step 2: Implement `MaskingPolicyDialog.tsx`**

Copy the structure of `AccessPolicyDialog.tsx` (name Input + Enabled checkbox + 4 resource Inputs `catalog/schema/table/column` via the same `setLevel`/`levelValue` helpers + `Add item` + per-item `PrincipalPicker`). Replace the per-item allow/deny select + access checkboxes with:
```tsx
import { MASK_TYPES, type MaskItem, type MaskingPolicyInput, createPolicy, updatePolicy } from "@/api/policies";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
// item type:
const emptyItem = (): MaskItem => ({ users: [], groups: [], maskType: "MASK_HASH", valueExpr: undefined });
// blankInput builds { kind: "masking", name:"", service, isEnabled:true, resources:{catalog,schema,table,column}, items:[emptyItem()] }
// per item, instead of accesses:
<Select value={it.maskType} onValueChange={(v) => setItem(i, { maskType: v })}>
  <SelectTrigger aria-label={`mask type ${i}`}><SelectValue /></SelectTrigger>
  <SelectContent>
    {MASK_TYPES.map((m) => <SelectItem key={m.name} value={m.name}>{m.label}</SelectItem>)}
  </SelectContent>
</Select>
{it.maskType === "CUSTOM" && (
  <Input aria-label={`value expr ${i}`} placeholder="cast(... {col} ...) as {type}"
    value={it.valueExpr ?? ""} onChange={(e) => setItem(i, { valueExpr: e.target.value })} />
)}
```
The mutation posts `form` (which has `kind: "masking"`). `save.mutationFn = () => (initial ? updatePolicy(initial.id, form) : createPolicy(form))`. Props: same `{ open, service, initial?: {id, input: MaskingPolicyInput}, onOpenChange, onSaved }`. Invalidate `["control-plane","policies"]` on success.

- [ ] **Step 3: Run test → PASS**, full frontend gate, commit:

```bash
git add frontend/src/components/policy/MaskingPolicyDialog.tsx frontend/src/components/policy/__tests__/MaskingPolicyDialog.test.tsx
git commit -m "feat(frontend): MaskingPolicyDialog editor (mask type per column)"
```
End with the Co-Authored-By trailer.

---

### Task 6: `RowFilterPolicyDialog`

Mirrors the masking dialog but: 3 resource levels (catalog/schema/table, no column), and per-item a **filter-expression `Input`** instead of a mask-type select.

**Files:**
- Create: `frontend/src/components/policy/RowFilterPolicyDialog.tsx`
- Test: `frontend/src/components/policy/__tests__/RowFilterPolicyDialog.test.tsx`

- [ ] **Step 1: Failing test** (mirror the masking test; assert `arg.kind === "rowfilter"` and `arg.items[0].filterExpr === "country = 'US'"` after filling name + a filter input + save; fill the filter via `screen.getByLabelText("filter 0")`).

- [ ] **Step 2: Implement `RowFilterPolicyDialog.tsx`** — copy the masking dialog; resources are `catalog/schema/table` only (a 3-col grid); `RowFilterResources` has no `column`; `emptyItem = (): RowFilterItem => ({ users: [], groups: [], filterExpr: "" })`; `blankInput` sets `kind:"rowfilter"`. Per item, replace the mask Select with:
```tsx
<Input aria-label={`filter ${i}`} placeholder="e.g. country = 'US'"
  value={it.filterExpr} onChange={(e) => setItem(i, { filterExpr: e.target.value })} />
```

- [ ] **Step 3: test PASS + gate + commit**

```bash
git add frontend/src/components/policy/RowFilterPolicyDialog.tsx frontend/src/components/policy/__tests__/RowFilterPolicyDialog.test.tsx
git commit -m "feat(frontend): RowFilterPolicyDialog editor (SQL predicate per table)"
```
End with the Co-Authored-By trailer.

---

### Task 7: Wire masking/rowfilter + Download config into the list

**Files:**
- Modify: `frontend/src/routes/control-plane/Policies.tsx`
- Create: `frontend/src/components/policy/TrinoConfigDialog.tsx`
- Test: extend `frontend/src/routes/control-plane/__tests__/Policies.test.tsx`

- [ ] **Step 1: Per-tab editor selection**

In `Policies.tsx`, the **New policy** button + **Edit** action now open the dialog matching the active tab:
- `policyType === "access"` → `AccessPolicyDialog` (already wired; ensure its payload now includes `kind: "access"` per Task 4).
- `policyType === "masking"` → `MaskingPolicyDialog`.
- `policyType === "rowfilter"` → `RowFilterPolicyDialog`.
Render whichever matches `policyType` when `activeService`. Enable **New policy** on all three tabs now (remove the P2 access-only gate).

Extend `toInput` into three reverse-maps (or one that branches on `policyType`):
```tsx
function maskingToInput(service: string, body: Record<string, any>): MaskingPolicyInput {
  const lvl = (k: string) => ({ values: body?.resources?.[k]?.values ?? [], isExcludes: !!body?.resources?.[k]?.isExcludes, isRecursive: !!body?.resources?.[k]?.isRecursive });
  const items = (body.dataMaskPolicyItems ?? []).map((it: any) => ({
    users: it.users ?? [], groups: it.groups ?? [],
    maskType: it.dataMaskInfo?.dataMaskType ?? "MASK_HASH",
    valueExpr: it.dataMaskInfo?.valueExpr,
  }));
  return { kind: "masking", name: String(body.name ?? ""), service, isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table"), column: lvl("column") },
    items: items.length ? items : [{ users: [], groups: [], maskType: "MASK_HASH" }] };
}
function rowfilterToInput(service: string, body: Record<string, any>): RowFilterPolicyInput {
  const lvl = (k: string) => ({ values: body?.resources?.[k]?.values ?? [], isExcludes: !!body?.resources?.[k]?.isExcludes, isRecursive: !!body?.resources?.[k]?.isRecursive });
  const items = (body.rowFilterPolicyItems ?? []).map((it: any) => ({
    users: it.users ?? [], groups: it.groups ?? [], filterExpr: it.rowFilterInfo?.filterExpr ?? "",
  }));
  return { kind: "rowfilter", name: String(body.name ?? ""), service, isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table") },
    items: items.length ? items : [{ users: [], groups: [], filterExpr: "" }] };
}
```
`openEdit` picks the reverse-map by `policyType` and opens the matching dialog. Keep the P2 `toInput` (access) unchanged.

- [ ] **Step 2: "Download Trino config" button + `TrinoConfigDialog`**

Add a **Download Trino config** button in the Profiles card (next to / under the active profile). Create `frontend/src/components/policy/TrinoConfigDialog.tsx`:
```tsx
import { useQuery } from "@tanstack/react-query";
import { type ConfigFile, getTrinoConfig } from "@/api/policies";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";

function download(name: string, content: string) {
  const url = URL.createObjectURL(new Blob([content], { type: "text/plain" }));
  const a = document.createElement("a");
  a.href = url; a.download = name; a.click();
  URL.revokeObjectURL(url);
}

export function TrinoConfigDialog({ open, service, onOpenChange }: { open: boolean; service: string; onOpenChange: (o: boolean) => void }) {
  const cfg = useQuery<ConfigFile[]>({ queryKey: ["trino-config", service], queryFn: () => getTrinoConfig(service), enabled: open, retry: false });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader><DialogTitle>Trino config — {service}</DialogTitle></DialogHeader>
        <p className="text-sm text-muted-foreground">
          Drop these into the Trino coordinator's <code>etc/</code>. The native ranger plugin then enforces this profile.
          (Assumes anonymous <code>/service</code>; a Ranger auth token needs extra proxy setup.)
        </p>
        {(cfg.data ?? []).map((file) => (
          <div key={file.name} className="space-y-1">
            <div className="flex items-center justify-between">
              <span className="font-mono text-sm">{file.name}</span>
              <Button size="sm" variant="outline" onClick={() => download(file.name, file.content)}>Download</Button>
            </div>
            <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">{file.content}</pre>
          </div>
        ))}
      </DialogContent>
    </Dialog>
  );
}
```
Wire it: a `configOpen` state + button in `Policies.tsx` (shown when `activeService`).

- [ ] **Step 3: Extend the list test**

In `Policies.test.tsx`, extend the `@/api/policies` mock to include `getTrinoConfig` (returns two files), `createPolicy`/`updatePolicy`/`getPolicy`, and add a test: switch to the Masking tab, click **New policy**, assert `New masking policy` (or the MaskingPolicyDialog title) appears. Add a test clicking **Download Trino config** opens the config dialog (`findByText(/Trino config/)`).

- [ ] **Step 4: Full frontend gate + commit**

`cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` — all pass.
```bash
git add frontend/src/routes/control-plane/Policies.tsx frontend/src/components/policy/TrinoConfigDialog.tsx frontend/src/routes/control-plane/__tests__/Policies.test.tsx
git commit -m "feat(frontend): masking/rowfilter editors + Download Trino config in the list"
```
End with the Co-Authored-By trailer.

---

### Task 8: Full CI + Java enforcement harness + manual smoke

- [ ] **Step 1: Backend + frontend gates**

`cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --no-run`, then against live Postgres `cargo test --workspace` (runs the new masking/rowfilter/config DB tests). `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.

- [ ] **Step 2: Java enforcement harness** (`tests/ranger/`, run by the user — needs Maven + a running egret with the seeded policies)

The harness `tests/ranger/src/test/java/EgretRangerEnforcementTest.java` ALREADY asserts enforcement for all three kinds (`aliceSelectAllowed`, `bobSelectDenied`, `emailMasked` → `MASK_HASH`, `rowFilterApplied` → filter expr). P3's job is to confirm those still pass when the policies are **authored through the new editors/serializers**. Two options — pick per how the harness loads policies (read its `@Before`/setup):
- If it loads a fixture policy JSON: regenerate the fixtures from `masking_to_ranger_json` / `rowfilter_to_ranger_json` output (a masking policy with `dataMaskType: "MASK_HASH"` on `email`, a row-filter with `country = 'US'`) so the harness proves the serializer output enforces.
- If it pulls from a live egret `/service`: create the masking + row-filter policies through the UI/API against a seeded `trino` service, point the harness's policy download at egret, and run `mvn -f tests/ranger/pom.xml test`.
Document exactly what was run and the pass/fail (per `egret-manual-trino-integration-test`).

- [ ] **Step 3: Manual smoke** (per `egret-frontend-viz-verify`)

Log in as `admin@egret.local`, **Control Plane → Data Governance → Policies**. On **Masking**: New → resource `iceberg.sales.users.email`, principal group `pii`, mask type **Hash**, Save → row appears; Edit → change to **Nullify**, Save. On **Row-filter**: New → `iceberg.sales.orders`, user `bob`, filter `country = 'US'`, Save. Click **Download Trino config** → the two files render with the profile's service name + egret URL; download works. State exactly what was exercised.

- [ ] **Step 4: Commit any fixups, then stop for review.**

---

## Self-Review notes
- **Spec coverage (P3):** masking editor (Tasks 1, 5) ✓; row-filter editor (Tasks 1, 6) ✓; both serializers with round-trip tests through `summarize_policy` (Task 1) ✓; kind-dispatched CRUD keeping the P2 access path wire-compatible (Task 2) ✓; Layer-B config bundle endpoint + download UI (Tasks 3, 7) ✓; Java enforcement verification (Task 8) ✓. Full 3-type Ranger parity reached.
- **Type consistency:** `MaskItem.maskType` / `MaskingPolicyInput` are identical in Rust (`ranger_json.rs`, serde `maskType`/`valueExpr`/`filterExpr` renames) and TS (`api/policies.ts`); the masking/rowfilter dialogs post exactly those shapes with `kind`; the `maskingToInput`/`rowfilterToInput` reverse-maps mirror `masking_to_ranger_json`/`rowfilter_to_ranger_json` (`dataMaskInfo.dataMaskType`↔`maskType`, `rowFilterInfo.filterExpr`↔`filterExpr`). `ConfigFile {name, content}` matches between the Rust handler and TS.
- **Open confirmations for the implementer:** the exact `req`/`session_for`/`into_json` helpers (Tasks 2/3 — read them); how `EgretRangerEnforcementTest` loads its policies (Task 8 — fixture vs live); whether `AccessPolicyInput` needs `#[derive(Serialize)]` for the dispatcher test (Task 1 — add it if `serde_json::to_value` is used on it).
- **Known-minor carried from P2 (not addressed here):** the serializer remains lossy for foreign/non-egret Ranger policies (documented in `ranger_json.rs`); principals endpoint keeps its combined 50-cap.
