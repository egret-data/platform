-- 0223_withhold_check.sql
-- A withhold rule carries no mask/filter payload and is tag-selected.
ALTER TABLE perm.rules
  ADD CONSTRAINT rules_withhold_has_no_payload CHECK (
    kind <> 'withhold'
    OR (mask_type IS NULL AND value_expr IS NULL AND filter_expr IS NULL
        AND selector_kind = 'tag')
  );
