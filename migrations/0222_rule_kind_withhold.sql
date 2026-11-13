-- 0222_rule_kind_withhold.sql
-- Clearance becomes a withhold rule: a tag-selected perm.rules row with no
-- mask/filter payload, whose rule_exemptions name the cleared principals.
-- ADD VALUE only: Postgres rejects any use of the new value (even naming it in
-- a CHECK) inside the same transaction, so the CHECK lives in 0223.
ALTER TYPE perm.rule_kind ADD VALUE IF NOT EXISTS 'withhold';
