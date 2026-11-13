-- G05: group, auditor-group and public principals. Group l2_analysts = {bob, dan}; group l2_auditors
-- = {carol}. Read+write on s1 to both groups and bob (named twice); deny write on s1.t2 to public
-- except l2_auditors. (Role principals can no longer be authored since SP3c-1: roles are
-- represented by managed groups, so the auditor stand-in is a plain group.)
-- users: ann,bob,carol,dan
-- setup: G_ANALYSTS = POST /api/control-plane/groups {"name":"l2_analysts"}
-- setup: POST /api/control-plane/groups/@G_ANALYSTS@/members {"userId":"@UID_bob@"}
-- setup: POST /api/control-plane/groups/@G_ANALYSTS@/members {"userId":"@UID_dan@"}
-- setup: G_AUDITORS = POST /api/control-plane/groups {"name":"l2_auditors"}
-- setup: POST /api/control-plane/groups/@G_AUDITORS@/members {"userId":"@UID_carol@"}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read","write"],"scope":{"catalog_id":"@CAT@","schema":"s1"},"principals":{"include":[{"kind":"group","ref":"l2_analysts"},{"kind":"group","ref":"l2_auditors"},{"kind":"user","ref":"bob"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"deny","actions":["write"],"scope":{"catalog_id":"@CAT@","schema":"s1","table":"t2"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[{"kind":"group","ref":"l2_auditors"}]}}
-- expect: bob | SELECT count(*) FROM lake.s1.t2 | [[1000]]
-- expect: carol | DELETE FROM lake.s1.t2 WHERE id < 0 | [[0]]
-- expect: bob | DELETE FROM lake.s1.t2 WHERE id < 0 | denied
-- expect: ann | SELECT count(*) FROM lake.s1.t1 | denied
SELECT count(*) FROM lake.s1.t1
SELECT id, v FROM lake.s1.t2 ORDER BY id LIMIT 3
DELETE FROM lake.s1.t2 WHERE id < 0
DELETE FROM lake.s1.t1 WHERE id < 0
