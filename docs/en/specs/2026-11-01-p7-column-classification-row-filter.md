# P7: row filter by a column classification on Trino, notice at attach (KI-287, KI-307)

DG debt, last code package. Owner decisions are settled; this page records what
the fix does and the details the decisions left open.

## KI-287: compile a classification row filter table-level on Trino

**Premise on main (19e41645):** reproduces. On Trino a tag row-filter rule compiles to
ONE tag policy in the profile's tag service. The Trino plugin applies a tag row
filter only when the classification is on the table itself; a classification on
columns only reaches the table request as a descendant and filters nothing
(E14). Explain reports this through `row_filter_not_applied` ("… does not filter
on Trino here, because … is on a column, not on the table"), and the rule
preview's `trinoColumnOnly` asks for **Create anyway**. StarRocks already filters
these tables: since D2 its compile expands a tag row filter per TABLE, whether
the classification is on the table or on a column (`rules::compile_rule`,
`group_by_table`).

**Decision (owner):** a table carries the classification for row-filter purposes
when it is on the table or on any of its columns, and egret emits the Trino row
filter on every such table.

**What changes:**

- `rules::compile_rule` (Trino, tag selector, row filter): besides the tag
  policy, it emits one resource row-filter policy in the profile for each table
  where the classification is on columns only, named and shaped exactly as the
  StarRocks expansion names and shapes its row filters
  (`{prefix}{rule}-{catalog}.{schema}.{table}`, `catalog/schema/table`). Tables
  carrying it on the table keep being filtered by the tag policy, so no table is
  filtered by two policies of the same rule. The expansion ceiling counts these
  tables, as it counts StarRocks tables.
- `trigger::compile_one_rule` reads the classification's assignments for Trino
  row filters too (StarRocks read them already). The writes that change the
  assignments already recompile every profile bound to the warehouse
  (attach/detach, rename follow, lineage), Trino ones included.
- The boot pass (`recompile_table_classified_profiles`) also recompiles Trino
  profiles that reach a column attachment of a classification a row-filter rule
  selects, so an upgrade does not wait for an unrelated write.
- Removed, because the gap no longer exists: explain's `row_filter_not_applied`
  (`ExplainRowFilterGap`) and its rows-line clause, the rule effect's
  `trinoColumnOnly` (`TrinoColumnOnly`), and in the rule dialog the
  `TrinoColumnOnlyNotice`, its **Create anyway** reason and the Kind hint that
  said Trino filters only whole-table classifications. The second-mask
  **Create anyway** stays.
- Explain agrees with the compile because it evaluates the served artifact:
  the resource row filter is what decides the table, attributed to the rule.

**Both engines now:** a row filter by classification filters every table that
carries the classification on the table or on any of its columns, on Trino and
on StarRocks.

**Left open, conservative choice:** a table with a column-only attachment of
classification A and a whole-table attachment of classification B, each with a
row-filter rule, is decided by B's tag policy on Trino (a tag verdict shuts out
resource policies of equal priority, `egret_ranger_eval::eval::evaluate_sub`
arm 3). This is Ranger's existing tag-versus-resource precedence, which the
evaluator behind explain models, so explain still answers what Trino does. The
package does not change it.

## KI-307: notice when attaching a clearance to a table not in the catalog

**Premise on main:** reproduces. `PUT …/resource-tags` answers `{"ok": true}`;
the missing NULL masks are reported only by **Who can read?**.

**Decision (owner):** warn at attach. The attach succeeds, the masks are still
written when the table is created.

**What changes:** the assignment answers `{"ok": true, "notices": [...]}`. A
notice `{code: "clearance_masks_pending_table", message}` is present when the
attachment is to a whole table, the classification has a clearance with roles,
a StarRocks profile serves the warehouse, and the catalog has no active table of
that name (unknown, a view, a staged create). No existing response carried
notices, so this is a new, small shape: `code` for programs, `message` for
people. The two attach UIs (Governance **Data** → Add classification, and the
catalog browse tag editor) show the message as a warning toast.
