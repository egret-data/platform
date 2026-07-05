# Catalog View / Edit / Secrets — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Let operators view + edit an existing engine catalog's config (and back a credential with an egret secret), and redesign the Catalogs tab as a Config-style master-detail.

**Architecture:** New control-plane catalog detail (`GET`) + update (`PUT`) endpoints over the already-stored `config`/`secret_ref`; a storage `update_engine_catalog`; a generic-connector secret mapping. Frontend: extract `CatalogForm` (add+edit) and replace the table+modal Catalogs tab with a left-nav master-detail. All re-provision through the existing `build_provision_req` + `prov.update` path.

**Tech Stack:** Rust (`egret-controlplane`, storage), React 18 + TS + shadcn, Vitest.

**Branch:** `feat/engine-config-ui-remap`. **Spec:** `docs/en/specs/2026-07-05-catalog-view-edit-secrets-design.md`.

---

## Conventions
- Backend from repo root; frontend from `frontend/`. Gate: backend `cargo fmt --all && cargo clippy -p egret-controlplane --all-targets -- -D warnings && cargo test -p egret-controlplane`; frontend `npm run typecheck && npm run lint && npm run test -- --run`.
- Conventional Commits, repo identity, one commit per task.
- **Security guardrail:** egret-warehouse `config` (`credential`, `clientRowId`) must never leave the backend. **Parity guardrail:** add + generic-without-secret render byte-identically to today.

---

## Task 1: Storage — `update_engine_catalog`

**Files:** the storage trait + Postgres impl that already define `create_engine_catalog`/`get_engine_catalog`/`list_engine_catalogs`/`delete_engine_catalog` (grep `fn create_engine_catalog` to locate the trait + impl; likely `egret-controlplane/src/storage*.rs` or the `Storage` trait module).

- [ ] **Step 1:** Read the existing `create_engine_catalog` + `get_engine_catalog` signatures and the `EngineCatalog` struct (fields: id, engine_id, name, kind, config: serde_json::Value, secret_ref: Option<Uuid>, is_primary).
- [ ] **Step 2:** Add to the trait:
```rust
async fn update_engine_catalog(
    &self,
    id: Uuid,
    config: &serde_json::Value,
    secret_ref: Option<Uuid>,
) -> Result<EngineCatalog, StorageError>;
```
- [ ] **Step 3:** Implement it as `UPDATE engine_catalogs SET config = $2, secret_ref = $3 WHERE id = $1 RETURNING <same columns get_engine_catalog selects>`, mapping the row to `EngineCatalog` exactly like `get_engine_catalog`. Add a matching impl to any in-memory/test storage double if one exists (grep the trait's impls).
- [ ] **Step 4:** `cargo build -p egret-controlplane` clean; `cargo fmt --all`.
- [ ] **Step 5:** Commit `feat(catalog-storage): add update_engine_catalog`.

---

## Task 2: Detail endpoint (`GET`) + DTO redaction + `secretRef` on list

**Files:** `crates/egret-controlplane/src/engines.rs` (DTO ~544, list_catalogs ~1788, routes ~469).

- [ ] **Step 1:** Add `secret_ref` to the list DTO and a detail DTO:
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineCatalogDto {
    id: Uuid, name: String, kind: String, is_primary: bool,
    secret_ref: Option<Uuid>,
}
// From<EngineCatalog>: set secret_ref: c.secret_ref

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineCatalogDetailDto {
    id: Uuid, name: String, kind: String, is_primary: bool,
    secret_ref: Option<Uuid>,
    config: serde_json::Value, // redacted for egret-warehouse
}
```
- [ ] **Step 2:** Add a redaction helper:
```rust
/// Strip system-minted secret material from a catalog config before returning
/// it to clients. egret-warehouse configs carry a client credential; nothing
/// else should ever be exposed.
fn redact_catalog_config(kind: &str, mut config: serde_json::Value) -> serde_json::Value {
    if kind == "egret-warehouse" {
        if let Some(obj) = config.as_object_mut() {
            obj.remove("credential");
            obj.remove("clientRowId");
        }
    }
    config
}
```
- [ ] **Step 3:** Add the handler + route (`GET /engines/{id}/catalogs/{cid}`):
```rust
async fn get_catalog(
    State(st): State<CpState>, Auth(p): Auth, Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<EngineCatalogDetailDto>, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let cat = st.storage.get_engine_catalog(cid).await?.ok_or(CpError::NotFound)?;
    if cat.engine_id != engine.id { return Err(CpError::NotFound); }
    Ok(Json(EngineCatalogDetailDto {
        id: cat.id, name: cat.name.clone(), kind: cat.kind.clone(),
        is_primary: cat.is_primary, secret_ref: cat.secret_ref,
        config: redact_catalog_config(&cat.kind, cat.config.clone()),
    }))
}
```
Wire the route: change `"/engines/{id}/catalogs/{cid}"` to `.route(..., get(get_catalog).delete(remove_catalog))`.
- [ ] **Step 4:** Test `redact_catalog_config` strips `credential`/`clientRowId` for egret-warehouse and leaves postgres/generic config intact.
- [ ] **Step 5:** `cargo test -p egret-controlplane`; fmt. Commit `feat(catalog-api): catalog detail endpoint with redaction`.

---

## Task 3: Update endpoint (`PUT`) with rollback + re-provision

**Files:** `engines.rs` (near add_catalog ~1805, routes ~469).

- [ ] **Step 1:** Add the body + handler:
```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCatalogBody {
    #[serde(default)] config: serde_json::Value,
    #[serde(default)] secret_ref: Option<Uuid>,
}

async fn update_catalog(
    State(st): State<CpState>, Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>, Json(b): Json<UpdateCatalogBody>,
) -> Result<Json<EngineCatalogDto>, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let prev = st.storage.get_engine_catalog(cid).await?.ok_or(CpError::NotFound)?;
    if prev.engine_id != engine.id { return Err(CpError::NotFound); }
    if prev.is_primary { return Err(CpError::BadRequest("cannot edit the primary catalog".into())); }
    if prev.kind == "egret-warehouse" {
        return Err(CpError::BadRequest("egret-warehouse catalogs are managed and not editable".into()));
    }
    // Persist, re-provision, roll back the row on failure so it never leads the cluster.
    let updated = st.storage.update_engine_catalog(cid, &b.config, b.secret_ref).await?;
    let restore = |st: CpState, prev: EngineCatalog| async move {
        let _ = st.storage.update_engine_catalog(prev.id, &prev.config, prev.secret_ref).await;
    };
    let req = match build_provision_req(&st, &engine).await {
        Ok(r) => r, Err(e) => { restore(st.clone(), prev).await; return Err(e); }
    };
    let Some(prov) = st.engine_provisioner.as_ref() else {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
    Ok(Json(updated.into()))
}
```
- [ ] **Step 2:** Wire the route: `.route("/engines/{id}/catalogs/{cid}", get(get_catalog).put(update_catalog).delete(remove_catalog))`.
- [ ] **Step 3:** `cargo test -p egret-controlplane`; clippy; fmt. Commit `feat(catalog-api): update catalog endpoint with rollback`.

---

## Task 4: Generic connector secret in `build_catalog_req`

**Files:** `engines.rs` `build_catalog_req` generic arm (~686-699).

- [ ] **Step 1:** Replace the generic arm so a `secret_ref` fills a designated property (parity with postgres), reading the target from `config.secretProperty` (default `connection-password`), and NOT double-emitting it from the verbatim `properties` pass:
```rust
"generic" => {
    props.insert(
        "connector.name".into(),
        cat.config["connectorName"].as_str().unwrap_or_default().into(),
    );
    let secret_prop = cat.config["secretProperty"]
        .as_str().filter(|s| !s.is_empty())
        .unwrap_or("connection-password")
        .to_string();
    if let Some(obj) = cat.config["properties"].as_object() {
        for (k, v) in obj {
            if cat.secret_ref.is_some() && *k == secret_prop { continue; }
            props.insert(k.clone(), v.as_str().unwrap_or_default().into());
        }
    }
    if let Some(ref_id) = cat.secret_ref {
        let var = format!("GEN_{env_prefix}");
        props.insert(secret_prop, format!("${{ENV:{var}}}"));
        let val = st.secrets.resolve_plaintext(None, ref_id).await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        env.insert(var, val);
    }
}
```
- [ ] **Step 2:** Test: a generic catalog with `secret_ref` + `secretProperty` renders `props["connection-password"] == "${ENV:GEN_…}"` and `env` carries the resolved value; without `secret_ref` it renders exactly as before (verbatim properties, no env). Confirm `connector.name` still set.
- [ ] **Step 3:** `cargo test -p egret-controlplane`; clippy; fmt. Commit `feat(catalog): generic connector can source a property from an egret secret`.

---

## Task 5: Frontend API client

**Files:** `frontend/src/api/controlPlane.ts`.

- [ ] **Step 1:** Extend types + add calls (mirror `listEngineCatalogs`/`addEngineCatalog`):
```ts
export interface EngineCatalog {
  id: string; name: string; kind: string; isPrimary: boolean;
  secretRef?: string | null;
}
export interface EngineCatalogDetail extends EngineCatalog {
  config: Record<string, unknown>;
}
export function getEngineCatalog(id: string, cid: string): Promise<EngineCatalogDetail> {
  return apiJson<EngineCatalogDetail>(`/control-plane/engines/${id}/catalogs/${cid}`);
}
export function updateEngineCatalog(
  id: string, cid: string, input: { config: Record<string, unknown>; secretRef?: string | null },
): Promise<EngineCatalog> {
  return apiJson<EngineCatalog>(`/control-plane/engines/${id}/catalogs/${cid}`, {
    method: "PUT", body: JSON.stringify(input),
  });
}
```
- [ ] **Step 2:** `npm run typecheck`. Commit `feat(cp-api): catalog detail + update client calls`.

---

## Task 6: `CatalogForm` — extract add/edit form + secret picker

**Files:** Create `frontend/src/routes/control-plane/CatalogForm.tsx`; source logic from the current `AddCatalogDialog` body in `EngineDetail.tsx`.

- [ ] **Step 1:** Extract the per-kind form (Name, Kind, egret-warehouse warehouse `Select`, postgres host/port/db/user, generic connector combobox + `KeyedPropertyList`) into `CatalogForm` with a `mode: "add" | "edit"` prop. In `edit` mode: Name + Kind are read-only (shown as text), fields prefill from a passed `initial` (kind + config + secretRef), and egret-warehouse renders a read-only summary.
- [ ] **Step 2:** Add a **Secret** block: a `Select` over `listSecrets()` (already queried in the current dialog) → `secretRef`, plus (generic only) a `secretProperty` text input defaulting to `connection-password` (postgres fixes it to `connection-password`). Persist `secretProperty` into the generic `config`.
- [ ] **Step 3:** `CatalogForm` exposes an `onSubmit(payload)` where payload is the add body `{name, kind, config, secretRef}` (add) or update body `{config, secretRef}` (edit). Build `config` per kind exactly as the current dialog's `handleSubmit` does (reuse that mapping verbatim so the payload is unchanged for add).
- [ ] **Step 4:** `npm run typecheck && npm run lint`. Commit `feat(engine-catalog): extract CatalogForm (add/edit + secret)`.

---

## Task 7: Master-detail Catalogs tab

**Files:** `EngineDetail.tsx` (replace `CatalogsCard`; retire `AddCatalogDialog` modal).

- [ ] **Step 1:** Rewrite `CatalogsCard` as a master-detail `Card` (mirror `ConfigurationTab`'s two-column layout):
  - Left `<nav>`: `catalogs` list buttons (name + kind sub-label + `primary`/secret badges) + a separator + an active-highlightable "+ Add catalog" button; local `selected: string | "add"` state (default first catalog or "add").
  - Right pane: `selected === "add"` → `<CatalogForm mode="add" onSubmit={add}/>`; else fetch `getEngineCatalog(id, selected)` (react-query) → `<CatalogForm mode="edit" initial={detail} onSubmit={(body) => updateEngineCatalog(...)} />` with a **Remove** action (`ConfirmDeleteDialog`, hidden for primary). egret-warehouse/primary → read-only summary (no Save/Remove). tpch/tpcds → "no configurable properties".
- [ ] **Step 2:** Wire mutations: add (POST, existing `addEngineCatalog`), update (PUT, `updateEngineCatalog`), remove (existing `deleteEngineCatalog`); each `invalidateQueries(["cp","engine-catalogs",id])` and (for edit) the detail query key. After add, select the new catalog.
- [ ] **Step 3:** Delete the old `AddCatalogDialog` component + its now-unused imports. Keep `ConfirmDeleteDialog`.
- [ ] **Step 4:** `npm run typecheck && npm run lint && npm run test -- --run && npm run build`. Commit `feat(engine): master-detail Catalogs tab with view/edit`.

---

## Task 8: Verify + live smoke

- [ ] **Step 1:** Backend `cargo fmt --all --check && cargo clippy -p egret-controlplane --all-targets -- -D warnings && cargo test -p egret-controlplane`; rebuild `egret-bin`; restart `serve`.
- [ ] **Step 2:** Frontend full gate.
- [ ] **Step 3:** Live smoke on `demo2`: Catalogs tab is master-detail; open the `tpch` catalog (read-only note) and `egret` (redacted read-only, no credential in network response); add a generic MySQL catalog with a `connection-password` from an egret secret → Review/confirm it renders `${ENV:…}`; edit a catalog's property + Save → pods roll; primary/egret-warehouse show no Save/Remove. 0 console errors; the `GET …/{cid}` response contains no `credential`.

---

## Self-review
- **Coverage:** view (T2), edit (T3), generic secret (T4), storage (T1), API (T5), UX (T6-7). Redaction (T2) + rollback (T3) address the two spec risks.
- **Type consistency:** `EngineCatalogDetail.config` (T5) ← `EngineCatalogDetailDto.config` (T2); `CatalogForm` add payload == current dialog payload (T6) so POST is unchanged; `secretProperty` written in T6 is read in T4.
- **Guardrail:** add + generic-without-secret unchanged; egret-warehouse never exposes credential.
