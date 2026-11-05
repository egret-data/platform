-- G05: group, role and public principals. Group l2_analysts = {bob, dan}; role l2_auditor held
-- by carol. Read+write on s1 to the group, the role and bob (named twice); deny write on s1.t2
-- to public except the role.
-- users: ann,bob,carol,dan
-- setup: G_ANALYSTS = POST /api/control-plane/groups {"name":"l2_analysts"}
-- setup: POST /api/control-plane/groups/@G_ANALYSTS@/members {"userId":"@UID_bob@"}
-- setup: POST /api/control-plane/groups/@G_ANALYSTS@/members {"userId":"@UID_dan@"}
-- setup: R_AUDITOR = POST /api/control-plane/access/roles {"scope":"platform","name":"l2_auditor"}
-- setup: POST /api/control-plane/users/@UID_carol@/roles {"roleId":"@R_AUDITOR@"}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read","write"],"scope":{"catalog_id":"@CAT@","schema":"s1"},"principals":{"include":[{"kind":"group","ref":"l2_analysts"},{"kind":"role","ref":"l2_auditor"},{"kind":"user","ref":"bob"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"deny","actions":["write"],"scope":{"catalog_id":"@CAT@","schema":"s1","table":"t2"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[{"kind":"role","ref":"l2_auditor"}]}}
SELECT count(*) FROM lake.s1.t1
SELECT id, v FROM lake.s1.t2 ORDER BY id LIMIT 3
DELETE FROM lake.s1.t2 WHERE id < 0
DELETE FROM lake.s1.t1 WHERE id < 0
