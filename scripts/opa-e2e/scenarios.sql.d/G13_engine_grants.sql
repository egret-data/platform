-- G13: engine grants on trino-opa: ann may set query_max_memory and impersonate bob; an
-- impersonation of the reserved root is skipped by the compile.
-- users: ann,bob
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s1"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/engines/@ENGINE@/grants {"capability":"trino.set_system_session_property","values":["query_max_memory"],"principals":[{"kind":"user","ref":"ann"}]}
-- setup: POST /api/control-plane/perm/engines/@ENGINE@/grants {"capability":"trino.impersonate_user","values":["bob"],"principals":[{"kind":"user","ref":"ann"}]}
SET SESSION query_max_memory = '1GB'
SET SESSION query_max_total_memory = '1GB'
@session-user bob: SELECT current_user
@session-user carol: SELECT current_user
@session-user root: SELECT current_user
SELECT count(*) FROM lake.s1.t1
