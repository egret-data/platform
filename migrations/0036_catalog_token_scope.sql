-- Token scope v2: a single jsonb list of resource grants replaces the flat
-- permissions / namespace_scope / table_scope columns.
ALTER TABLE catalog_tokens ADD COLUMN scope jsonb NOT NULL DEFAULT '[]'::jsonb;

-- Backfill existing rows into grants.
-- (a) no namespace_scope AND no table_scope -> one all-namespaces grant.
UPDATE catalog_tokens
   SET scope = jsonb_build_array(jsonb_build_object(
       'namespace', '*', 'table', NULL, 'permissions', to_jsonb(permissions)))
 WHERE cardinality(namespace_scope) = 0 AND cardinality(table_scope) = 0;

-- (b) each namespace_scope entry -> a whole-namespace grant.
UPDATE catalog_tokens t
   SET scope = t.scope || ns.g
  FROM (
    SELECT id, jsonb_agg(jsonb_build_object(
        'namespace', n, 'table', NULL, 'permissions', to_jsonb(permissions))) AS g
      FROM catalog_tokens, unnest(namespace_scope) AS n
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
