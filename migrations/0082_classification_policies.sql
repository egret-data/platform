-- Classification access policies (Access P3.3) — tag-conditioned BI object access.
-- A classification policy binds ONE tag to a set of clearance role NAMES: a
-- principal must hold >=1 of `required_roles` to access objects carrying that tag.
--
-- SEMANTICS: an empty `required_roles` array is NO gate (an informational tag
-- only). Enforcement applies ONLY when `required_roles` is non-empty. This task
-- ships the model + CRUD; the Cedar PDP wiring that reads this table is a later
-- task. On any mutation the CRUD publishes the SAME `RolesChanged` reload event
-- the PDP already subscribes to, so a future `load_inner` reading this table is
-- kept fresh with zero extra plumbing.

CREATE TABLE classification_policies (
    tag_id         uuid PRIMARY KEY REFERENCES tags(id) ON DELETE CASCADE,
    required_roles text[] NOT NULL DEFAULT '{}',   -- role NAMES; principal needs >=1 to access objects with this tag
    created_by     uuid REFERENCES users(id),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
