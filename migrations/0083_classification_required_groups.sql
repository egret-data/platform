-- Classification clearance by GROUP (Access P3.4) — attribute-based access.
-- P3.3 gated a tagged object on clearance ROLE names. This extends clearance to
-- GROUPS: in egret, OIDC groups model departments, so a principal is CLEARED for
-- a gated tag if they hold >=1 required ROLE **OR** belong to >=1 required GROUP.
--
-- SEMANTICS: cleared = (>=1 of required_roles) OR (>=1 of required_groups). Both
-- arrays EMPTY ⇒ NO gate (the tag is informational only). Groups are stored as
-- uuid[] (group ids), NOT names, so no charset validation is needed at the DB
-- boundary; the CRUD layer parse-validates the string→uuid on write, and the PDP
-- splices them as quoted Cedar string literals (inherently safe: a uuid[] column
-- can only ever hold valid uuids).
ALTER TABLE classification_policies
    ADD COLUMN required_groups uuid[] NOT NULL DEFAULT '{}';
