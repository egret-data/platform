-- G16 and G22 (the infrastructure rows): no rule; ann reads s1. The platform admin (egret-admin,
-- converged from the IR) runs every compared statement too, so the delegator, per-user baseline
-- and egret-admin rows are measured on both sides.
-- kill_query: Trino asks the plugin KillQueryOwnedBy only when the caller is not the query's owner
-- (AccessControlUtil.checkCanKillQueryOwnedBy returns early for its own query: read in
-- the repo's trino/ tree, 483-SNAPSHOT). Here the admin kills ann's running query, which reaches KillQueryOwnedBy(admin ->
-- ann) on both plugins and must succeed. No non-admin can reach that check: kill_query is a
-- procedure of the `system` catalog, every non-admin is refused "Cannot access catalog system"
-- first, and egret refuses an execute_procedure grant on `system` (400, "engine trino-opa does
-- not reach catalog system", measured), so its refusal half is not measurable here.
-- users: ann,@ADMIN@
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s1"},"principals":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- expect: @ADMIN@ | KILL | ok
-- expect: ann | KILL | denied
-- expect: ann | SELECT count(*) FROM lake.s1.t1 | [[1000]]
SELECT count(*) FROM lake.s1.t1
SELECT count(*) FROM lake.s2.t3
SET SESSION query_max_memory = '1GB'
@session-user bob: SELECT current_user
SELECT count(*) FROM system.runtime.nodes
