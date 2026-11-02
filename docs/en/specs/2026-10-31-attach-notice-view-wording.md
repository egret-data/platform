# Attach notice for a view (KI-307 follow-up)

## Premise

#818 answers `PUT /control-plane/warehouses/{id}/resource-tags` with notice
`clearance_masks_pending_table` when a whole-table classification with a
role-bearing clearance is attached to a name a StarRocks profile serves and
`table_columns::current_columns_via` finds no active table. That lookup is
`None` for an unknown name, a staged create AND a view, so a view such as
`sales.orders_us` got "egret found no table … The masks take effect once the
table exists in the catalog". A view never becomes a table, so the second
sentence is false for it. Reproduced on main (10b0badf) by the handler test
below, red before the fix.

## What the compiler emits for a view

`egret_perm_compile::clearance` (whole-table branch, D2/U4), for a name with no
column list in the catalog:

- one StarRocks access policy, `select` deny on `catalog/database/table/column`
  = `<catalog>.<schema>.<view>.*`, excepting the cleared users;
- no `MASK_NULL` clearance masks (they are built from a table's columns), and a
  `skipped` entry in the recompile report.

StarRocks' service-def has a separate `view` resource, and the measured view
reads (SP4c live case 4, a native StarRocks view) go through
`checkViewPrivilege`, not the table/column check. Whether that deny applies to
an Iceberg view read through a StarRocks external catalog is NOT measured, so
the notice says so instead of promising a refused direct read. What does hold
through views, measured (view governance A2), is a base table's column mask.

## Decisions

- Two cases, two codes: a name that is an active view of the catalog
  (`typ = 'view'`, not deleted, with a metadata location, single-level
  namespace) answers new code `clearance_on_view`; anything else with no active
  table keeps `clearance_masks_pending_table` and its sentence unchanged.
- Source: one query on the embedded catalog's `lakekeeper.tabular`, in
  `egret_perm_compile::table_columns` (where the Lakekeeper coupling is
  confined), on the write's own transaction, only when the table lookup
  already said "no table".
- The frontend shows `notice.message` as a warning toast for any code, so no
  frontend logic changes; the OpenAPI description and the generated model doc
  list the new code.
- The recompile's skip text in `clearance.rs` (the `skipped` list of
  `POST /perm/compile`) told the same wrong promise. It now tells a view apart
  the same way (`table_columns::active_views_batch_via`, one batch query over
  the names the column read did not find), with the same wording; an unknown
  name keeps "until the table exists".
- Two doc comments #818 separated from their functions are moved back
  (`refuse_while_held`, `live_tabular_id_via`).
