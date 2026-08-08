-- Constrain analytics.dataset_columns.semantic_type to its actual vocabulary.
--
-- THE GAP. 0098 created the column as `text NOT NULL DEFAULT ''` with no CHECK, Rust typed it
-- `String`, utoipa emitted a bare `type: string`, and the only enumeration of the permitted values
-- anywhere was nine `<option>` elements in ColumnsTab.tsx. `PUT semantic_type: "banana"` therefore
-- persisted. That was harmless for as long as nothing read the field; PR #438 gave it a reader —
-- the pin map defaults its latitude/longitude pickers from it — so a typo now produces a picker
-- that silently does not fill, with no error at any layer.
--
-- The API-level guard (`validate_semantic_type`, called from `ds_update_column`) is the check that
-- produces a good error message. THIS constraint is the one that makes the guarantee true of the
-- DATA rather than of one code path: a future handler, a repair script, or a psql session cannot
-- reintroduce a value the readers do not understand. The same defence-in-depth 24 other CHECK
-- allow-lists in this directory already apply (0018_dashboard_width.sql is the closest precedent:
-- `text NOT NULL DEFAULT <x> CHECK (col IN (...))`).
--
-- '' IS IN THE LIST AND HAS TO BE. It is the column DEFAULT and it means "no semantic role"; every
-- one of the three INSERT paths in dataset.rs relies on it. Omitting it would make every existing
-- row and every newly-synced column violate the constraint immediately.
--
-- WHY ALL NINE VALUES AND NOT THE SIX THAT ARE READ. `category` and `country` are write-only — the
-- editor offers them, nothing reads them. It is tempting to treat "no reader" as "not a real value"
-- and drop them. THE LIVE DEPLOYMENT WAS QUERIED BEFORE THIS FILE WAS WRITTEN, and that is what
-- settled it: analytics.dataset_columns holds 72 rows, 71 at '' and exactly one at 'category'. A
-- six-value list would have failed this migration on deploy against real data. Write-only means
-- "no consumer yet", not "illegitimate".
--
-- THE NORMALISING UPDATE, AND WHY IT IS NOT PARANOIA ABOUT NOTHING. On the deployment measured
-- above it affects ZERO rows (verified: `SELECT count(*) ... WHERE semantic_type NOT IN (...)`
-- returned 0), so it is a no-op there. It exists for every deployment that was NOT measured. Until
-- today the API accepted arbitrary strings, so any instance may hold anything at all, and a bare
-- `ADD CONSTRAINT` on such an instance aborts the migration — which does not merely skip a feature,
-- it stops the binary from starting. Clearing an unrecognised value to '' is not a behaviour change
-- either: every reader compares with `=` against a known word, so an unrecognised value ALREADY
-- behaves as "no role" everywhere it is read, and the editor's `<select>` already renders it blank.
-- The row is being made to say what the application has always done with it.
--
-- REJECTED: `ADD CONSTRAINT ... NOT VALID`, which would also never fail on deploy. It leaves the
-- offending rows in place, which sounds gentler but means the table permanently holds values that
-- violate its own declared constraint, and someone must later remember to run VALIDATE CONSTRAINT —
-- a follow-up that in practice never happens and leaves the guarantee half-true. Normalising makes
-- the constraint true the moment it is added.
--
-- REJECTED: normalising to 'category' or any other real value. '' is the only safe target because
-- it is the one value that asserts nothing about the column.

UPDATE analytics.dataset_columns
   SET semantic_type = ''
 WHERE semantic_type NOT IN
       ('', 'category', 'country', 'currency', 'email', 'url', 'image', 'latitude', 'longitude');

ALTER TABLE analytics.dataset_columns
  ADD CONSTRAINT dataset_columns_semantic_type_check
  CHECK (semantic_type IN
         ('', 'category', 'country', 'currency', 'email', 'url', 'image', 'latitude', 'longitude'));
