# Catalog List Filter by Grant — Implementation Plan

> **For agentic workers:** small, single-file authz change. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A grant-scoped token only sees, in a namespace's table/view listing, the entities it can access — by denying `ListEverything` unless a whole-namespace read grant covers the namespace (so lakekeeper falls through to its per-entity `IncludeInList` filter, which egret already enforces).

**Spec:** `docs/en/specs/2026-06-29-catalog-list-filter-by-grant-design.md`.

**Tech Stack:** Rust (embedded lakekeeper authz).

---

## Task 1: Route `ListEverything` to the whole-namespace decision

**Files:**
- Modify: `crates/egret-catalog-engine/src/authorizer.rs` (`are_allowed_namespace_actions_impl` + a unit test)

### Step 1: Write the failing unit test
In the `#[cfg(test)]` module (alongside the existing grant tests), add:
```rust
#[test]
fn list_everything_needs_whole_namespace_grant() {
    // Table-only grant: can read the namespace + the granted table, but must NOT
    // be able to "list everything" (so lakekeeper per-table-filters the listing).
    let table_only = vec![g("sale", Some("orders"), &["catalog:read"])];
    assert!(decide_ns_read(&table_only, &["sale".into()]));          // endpoint reachable
    assert!(decide_table(&table_only, &["sale".into()], "orders", false)); // its table
    assert!(!decide_object_ns(&table_only, &["sale".into()], false)); // NOT list-everything

    // Whole-namespace grant: can list everything.
    let whole = vec![g("sale", None, &["catalog:read"])];
    assert!(decide_object_ns(&whole, &["sale".into()], false));

    // '*' grant: can list everything anywhere.
    let star = vec![g("*", None, &["catalog:read"])];
    assert!(decide_object_ns(&star, &["anything".into()], false));
}
```
(`g(...)` and `decide_*` helpers already exist in the test module from the grants work.)

### Step 2: Run it — expect PASS already for the decide-level invariants
`cargo test -p egret-catalog-engine authorizer` — this test asserts the existing pure helpers behave as the fix relies on; it should pass immediately (it documents the invariant). The behavioural change is the wiring in Step 3.

### Step 3: Wire `ListEverything` to `gate_object`
In `are_allowed_namespace_actions_impl` (currently `self.gate_namespace(metadata, levels, namespace_action_is_write(action))`), branch on the action:
```rust
.map(|(info, action)| {
    let levels: &[String] = parent_namespaces.get(&info.namespace_id()).map_or_else(
        || info.namespace().namespace_ident.as_ref().as_slice(),
        |nwp| nwp.namespace_ident().as_ref().as_slice(),
    );
    match action {
        // `ListEverything` controls lakekeeper's "skip per-entity authz" short-circuit
        // in `list_entities!`. Only a whole-namespace read grant should enable it; a
        // table-specific grant must fall through to per-entity `IncludeInList`
        // filtering (handled by `are_allowed_table_actions_impl`/`gate_table`), so the
        // listing hides tables/views the token can't access.
        CatalogNamespaceAction::ListEverything => self.gate_object(metadata, levels, false),
        _ => self.gate_namespace(metadata, levels, namespace_action_is_write(action)),
    }
})
```
(`CatalogNamespaceAction` is already imported — `namespace_action_is_write` matches its variants. `gate_object(metadata, ns, false)` = a `table==None` read grant covering `ns`.)

### Step 4: Run tests + clippy + fmt
- `cargo test -p egret-catalog-engine authorizer` → PASS (incl. the new test).
- `cargo fmt --all` + `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings` → clean.

### Step 5: Commit
```bash
git add crates/egret-catalog-engine/src/authorizer.rs
git commit -m "fix(catalog): scope REST table/view listings to token grants

Deny ListEverything unless a whole-namespace read grant covers the namespace,
so lakekeeper falls through to per-entity IncludeInList filtering (already
enforced by gate_table) — a table-scoped token no longer sees sibling table
names in list_tables. Whole-namespace and '*' grants list everything as before."
```

---

## Final verification (before merge)
1. **Full local CI**: `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · per-binary `cargo test -p egret-catalog-engine` (DB+MinIO up; the live `front_auth` suite exercises the real router). FE unchanged.
2. **Rebuild + restart backend**, **live REST e2e**: mint `[{sale,null,[read]}, {mkt,"campaigns",[read,write]}]`; `GET …/namespaces/mkt/tables` → only `campaigns` (no `leads`); `GET …/namespaces/sale/tables` → all sale tables; `load mkt.leads` → 404 (deny unchanged); list endpoints return 200. Clean up seeded artifacts.
3. **Admin squash-merge** (identity `hoangquocvietbk2.13@gmail.com`).
4. **Memory**: in `[[egret-three-layer-management-model]]`, mark the listing-visibility limitation CLOSED.

---

## Self-review
- **Spec coverage:** the `ListEverything`→`gate_object` wiring (§Decision) → Task 1 Step 3; unit invariant → Step 1; live filtering proof → final verification §2. ✅
- **Placeholders:** none — exact code given.
- **Type consistency:** `gate_object(metadata, &[String], bool)` already exists and is called for views/generic; reused here for namespaces' `ListEverything`. `decide_object_ns`/`decide_ns_read`/`decide_table` are the existing pure helpers.
