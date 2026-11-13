-- 0220_managed_group_source.sql
-- Managed groups (role:<name> holders, ws:<slug> members) are maintained by egret,
-- not by local admin or OIDC. Widen the source check to admit them.
ALTER TABLE public.groups DROP CONSTRAINT IF EXISTS groups_source_check;
ALTER TABLE public.groups
  ADD CONSTRAINT groups_source_check CHECK (source IN ('local','oidc','managed'));
