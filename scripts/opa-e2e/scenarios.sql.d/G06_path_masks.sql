-- G06: path masks on columns: CUSTOM with {col} on email exempting ann and group l2_admins (bob);
-- MASK_NULL on ssn with no exemption.
-- users: ann,bob,carol
-- setup: G_ADMINS = POST /api/control-plane/groups {"name":"l2_admins"}
-- setup: POST /api/control-plane/groups/@G_ADMINS@/members {"userId":"@UID_bob@"}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g06-custom","kind":{"type":"mask","mask_type":"CUSTOM","value_expr":"concat('x-', {col})"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1","column":"email"},"exempt":{"include":[{"kind":"user","ref":"ann"},{"kind":"group","ref":"l2_admins"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g06-null","kind":{"type":"mask","mask_type":"MASK_NULL"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1","column":"ssn"},"exempt":{"include":[],"exclude":[]}}
-- expect: carol | SELECT email, ssn FROM lake.s1.t1 WHERE id = 1 | [["x-user1@example.com", null]]
-- expect: bob | SELECT email, ssn FROM lake.s1.t1 WHERE id = 1 | [["user1@example.com", null]]
-- expect: ann | SELECT email FROM lake.s1.t1 WHERE id = 1 | [["user1@example.com"]]
SELECT id, email, ssn, name FROM lake.s1.t1 ORDER BY id LIMIT 5
SELECT count(DISTINCT email), count(ssn) FROM lake.s1.t1
SELECT id, email FROM lake.s1.t1 WHERE email LIKE 'user1%' ORDER BY id LIMIT 3
