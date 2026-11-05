-- G04: the implicit workspace grant: the catalog is assigned to workspace l2-ws (ann a member)
-- and to l2-empty (no member: the empty-audience skip), through the assignment route, which owns
-- the `workspace_assignment` grants (a hand-written workspace_members grant is refused, measured).
-- users: ann,bob
-- setup: WS2 = ENSURE-WORKSPACE l2-ws
-- setup: WS3 = ENSURE-WORKSPACE l2-empty
-- setup: POST /api/ws/l2-ws/members {"principal_type":"user","principal_id":"@UID_ann@","role":"ws_viewer"}
-- setup: PUT /api/control-plane/catalogs/@CAT@/workspaces {"workspaceIds":["@WS@","@WS2@","@WS3@"]}
SELECT count(*) FROM lake.s1.t1
SELECT id, salary FROM lake.s2.t3 ORDER BY id LIMIT 3
