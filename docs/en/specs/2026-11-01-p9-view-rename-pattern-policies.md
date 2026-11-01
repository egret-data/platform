# P9: view renames carry grants (KI-319); pattern hand-written policies are checked at rename (KI-320)

Status: implemented, 2026-11-01. Base: `main` `9417afe1`. Branch `fix/view-rename-and-pattern-policies`.
Parent: [`2026-10-27-grants-follow-table-rename-design.md`](2026-10-27-grants-follow-table-rename-design.md)
("the #764 spec"), §11 items 1 and 3, which this package closes.

## Premise, re-verified on main

- **KI-319 reproduces.** The catalog guard authorizes `POST …/views/rename` like a table rename (F-A, both
  sides checked). Behind it, the view hold (`view_hold.rs`) handles `views/rename` but moves only lineage and
  classification rows; the table hook (`table_hook.rs`), which carries #764's grant/rule/catalog-grant
  follow, classifies only `tables/rename`. So deny grants, path masks, path row filters, allow grants and
  `table` catalog grants on a view stay on the old name. Red tests: `tests/view_rename_db.rs`.
- **KI-320 reproduces.** `follow::handwritten_names` (#764 D5) returns `None` unless the table level names the
  source literally, so a hand-written deny on `ord*` lets `orders → archive` through. Red tests:
  `tests/table_hook_db.rs`, "KI-320" section.
- **View column renames:** the Iceberg REST view API has no column-rename operation. A view's columns change
  only through a new view version (`CREATE OR REPLACE VIEW`, a commit with `add-schema` / `add-view-version`),
  which the view hold already judges as a new definition. Grants on a view column name are therefore not
  carried; there is no rename to carry them through.

## What changes

### KI-319: views follow #764's rules, through #764's code

The view hold stays the one place a view rename is held (it is the outer layer and already takes the name
locks; a second hold in the table hook would contend for the same locks and add a second wait). It now
reuses #764's permission part instead of duplicating it:

- `table_hook::PermPart` (plan, re-read under the locks, refusal text, copy, move allows),
  `record_perm_events`, `delete_copies_in_tx`, the policy-version readers and `CatalogFollow` are made
  `pub(crate)` and called by the view hold. `CatalogFollow` takes a `MovedObject` (table or view) and reads a
  live view id through a new `store::embedded::live_view_id_via`.
- Order under the view hold's lock: tags/lineage write (as before) → one transaction that re-reads the perm
  plan, copies the restrictions, recompiles the bound profiles (`recompile_services_in_tx`) and refuses a
  created overlap → **one** wait (`wait_for_versions`) over the moved tags and the copied policies → forward
  → finish (tags/lineage, then allows + events) or undo (copies, then tags).
- A non-2xx is checked against the catalog (#764 review finding 4): when the destination is now the source's
  view, the move is finished, not undone. This also applies to the tag part, which used to undo on any
  non-2xx.
- Catalog grants follow after a committed view rename, held or not, as for tables.
- Refusals keep the view hold's prefix (`egret refused the view: …`) and #764's body text.

### KI-320: hand-written policies reached through a pattern

`follow::handwritten_via` keeps D5's literal test and adds, for a policy that does not name the source
literally, Ranger's own matching for the profile's service-def: `egret-ranger-eval`'s
`PreparedPolicyResources` through `handwritten::object_request` / `object_match` (the code route A and explain
use). No new matcher. A policy *reaches* an object when a resource map applies to it or names part of it
(a column of the table); every resource map counts, `additionalResources` included.

| reaches old | reaches new | policy | result |
|---|---|---|---|
| yes | no | mask, row filter, deny items, deny exceptions, or any exclusion list | 409 naming `profile/name (id)` |
| yes | no | allow only | `ranger_policy:left` in Recent changes |
| no | yes | has an exclusion list | 409 (the rename moves the object out of the exclusion) |
| otherwise | | | nothing |

## Decisions (conservative where open)

1. **Exclusion lists refuse in both directions** (row 3 above). The task wording covers "old reached, new
   not"; an exclusion-list allow that would newly reach a renamed table is the same escape seen from the
   other side, so it is refused too.
2. **An unreadable policy body refuses** (egret cannot tell what it reaches).
3. **A multi-level destination is never "reached"** by a hand-written policy (no Ranger resource names it,
   #764 D14), so a restrictive pattern reaching the source refuses a rename into one.
4. **StarRocks `view` level:** for a view rename on a StarRocks profile, a request on the `view` level is
   asked as well as the `table` one, since a hand-written StarRocks policy may name views there.
5. **No new locks.** The view's copy recompiles through `recompile_services_in_tx` (which takes the profile
   locks itself); the table hook's per-profile schema-change locks are not taken (a view rename has no
   column overlay or dry run to serialize with).
6. Literal hand-written restrictions keep #764's behaviour (refused even when they also name the new name).

## Not covered

- Hand-written TAG-service policies: classifications follow renames already (#757).
- Expressions that name a renamed column (KI-321).
