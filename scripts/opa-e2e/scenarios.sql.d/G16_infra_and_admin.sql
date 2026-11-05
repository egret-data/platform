-- G16 and G22 (the infrastructure rows): no rule; ann reads s1. The platform admin (egret-admin,
-- converged from the IR) runs every compared statement too, so the delegator, per-user baseline
-- and egret-admin rows are measured on both sides.
-- users: ann,@ADMIN@
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s1"},"principals":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
SELECT count(*) FROM lake.s1.t1
SELECT count(*) FROM lake.s2.t3
SET SESSION query_max_memory = '1GB'
@session-user bob: SELECT current_user
SELECT count(*) FROM system.runtime.nodes
