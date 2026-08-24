-- M2: an endpoint's definition is a LIST of named statements, not one string.
--
-- `sql text` becomes `statements jsonb`, one element per statement:
--   [{"name":"rows","sql":"SELECT …"}, {"name":"total","sql":"SELECT …"}]
--
-- jsonb rather than a child table, and that is the same call every other
-- structured column on this table already made (`params`, `output`, `cache`,
-- `limits`, `rate_limit`): the list is read and written WHOLE, always by the one
-- endpoint that owns it, is never joined and never queried across rows, and its
-- ORDER is part of the definition because statements execute in order. A child
-- table would buy per-statement constraints at the price of an ordering column,
-- a join on every draft read, and a second place for the order to disagree.
--
-- Every existing draft converts to a one-element list named `main`. The name is
-- not cosmetic — once an endpoint has more than one statement, it is the key a
-- caller reads its results under — but a one-statement endpoint never shows it,
-- because `data` stays an ARRAY rather than becoming a keyed object. That is
-- precisely why an existing row can be handed a name here without changing
-- anything any caller can see.
--
-- Published versions are deliberately NOT touched. `dataapi.endpoint_versions.
-- snapshot` is immutable and the serving path reads only it; an already-published
-- contract must keep working because the READER accommodates the old shape, not
-- because a migration rewrote frozen data underneath it. Same precedent as M1's
-- `#[serde(default)]` on `ColumnSpec::required`.
ALTER TABLE dataapi.endpoints ADD COLUMN statements jsonb;

UPDATE dataapi.endpoints
   SET statements = jsonb_build_array(jsonb_build_object('name', 'main', 'sql', sql));

ALTER TABLE dataapi.endpoints ALTER COLUMN statements SET NOT NULL;

-- An endpoint with no statements has nothing to run and no columns to freeze, so
-- an empty list is not a state worth representing. A CHECK rather than a rule
-- each writer remembers: the route answers `[]` with a 400 naming the reason,
-- and this is what makes the route the only way to get it wrong rather than the
-- only thing standing in the way. `jsonb_typeof` is here too because a jsonb
-- column will happily hold `{}` or `"SELECT 1"`, and neither is a statement list.
ALTER TABLE dataapi.endpoints
  ADD CONSTRAINT dataapi_endpoints_statements_nonempty
  CHECK (jsonb_typeof(statements) = 'array' AND jsonb_array_length(statements) > 0);

ALTER TABLE dataapi.endpoints DROP COLUMN sql;
