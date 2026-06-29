-- Token scope v2: a single jsonb list of resource grants replaces the flat
-- permissions / namespace_scope / table_scope columns.
ALTER TABLE catalog_tokens ADD COLUMN scope jsonb NOT NULL DEFAULT '[]'::jsonb;

-- Backfill existing rows into grants.
--
-- AND -> OR semantics shift: the old model ANDed namespace_scope and table_scope
-- (a row with namespace_scope=['sales'] AND table_scope=['sales.orders'] could
-- reach ONLY `sales.orders`). The new model ORs grants, so emitting BOTH a
-- whole-namespace grant (from namespace_scope) AND the table grants (from
-- table_scope) for such a row would silently WIDEN it to every table in `sales`
-- -- a privilege escalation. To stay safe we drop the namespace-level grant for
-- any row that ALSO has a table_scope: such a row is backfilled from table_scope
-- ALONE (branch c). This UNDER-grants the namespace-level list/create the old AND
-- row technically allowed, which is the safe direction -- the new model has no
-- "namespace-ops-but-not-all-tables" grant, and `decide_ns_read`'s navigation
-- rule still lets a {sales,'orders'} grant list `sales`/root to reach the table,
-- so read access to the granted table is preserved; only broad namespace
-- write/list beyond navigation is dropped. (Tokens with BOTH lists set are
-- nonexistent in practice -- this feature ships fresh -- so this is documentation
-- of a corner, not a real data change.)
--
-- (a) no namespace_scope AND no table_scope -> one all-namespaces grant.
UPDATE catalog_tokens
   SET scope = jsonb_build_array(jsonb_build_object(
       'namespace', '*', 'table', NULL, 'permissions', to_jsonb(permissions)))
 WHERE cardinality(namespace_scope) = 0 AND cardinality(table_scope) = 0;

-- (b) each namespace_scope entry -> a whole-namespace grant, ONLY for rows with
-- NO table_scope (else the AND-scoped row would be over-granted; see above).
UPDATE catalog_tokens t
   SET scope = t.scope || ns.g
  FROM (
    SELECT id, jsonb_agg(jsonb_build_object(
        'namespace', n, 'table', NULL, 'permissions', to_jsonb(permissions))) AS g
      FROM catalog_tokens, unnest(namespace_scope) AS n
     WHERE cardinality(table_scope) = 0
     GROUP BY id, permissions
  ) ns
 WHERE t.id = ns.id;

-- (c) each table_scope entry "a.b.tbl" -> {namespace:"a.b", table:"tbl"}.
UPDATE catalog_tokens t
   SET scope = t.scope || ts.g
  FROM (
    SELECT id, jsonb_agg(jsonb_build_object(
        'namespace', regexp_replace(fqn, '\.[^.]+$', ''),
        'table', regexp_replace(fqn, '^.*\.', ''),
        'permissions', to_jsonb(permissions))) AS g
      FROM catalog_tokens, unnest(table_scope) AS fqn
     GROUP BY id, permissions
  ) ts
 WHERE t.id = ts.id;

ALTER TABLE catalog_tokens
    DROP COLUMN permissions,
    DROP COLUMN namespace_scope,
    DROP COLUMN table_scope;
