-- G11: clearance. l2-secret on column s1.t1.ssn and on the whole table s2.t3 (column_name = ''),
-- cleared by role l2_cleared (ann); l2-grouponly on s1.t2.v cleared by group l2_stewards
-- (carol); l2-unheld on s1.t1.name cleared by role l2_unheld that nobody holds. A MASK_SHOW_LAST_4
-- on ssn too, so a cleared user still sees it masked.
-- users: ann,bob,carol
-- setup: R_CLEARED = POST /api/control-plane/access/roles {"scope":"platform","name":"l2_cleared"}
-- setup: R_UNHELD = POST /api/control-plane/access/roles {"scope":"platform","name":"l2_unheld"}
-- setup: POST /api/control-plane/users/@UID_ann@/roles {"roleId":"@R_CLEARED@"}
-- setup: G_STEWARDS = POST /api/control-plane/groups {"name":"l2_stewards"}
-- setup: POST /api/control-plane/groups/@G_STEWARDS@/members {"userId":"@UID_carol@"}
-- setup: T_SECRET = POST /api/control-plane/tags {"key":"l2-secret","name":"L2 secret"}
-- setup: T_GROUPONLY = POST /api/control-plane/tags {"key":"l2-grouponly","name":"L2 group only"}
-- setup: T_UNHELD = POST /api/control-plane/tags {"key":"l2-unheld","name":"L2 unheld"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t1","column":"ssn","tag_id":"@T_SECRET@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","tag_id":"@T_SECRET@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t2","column":"v","tag_id":"@T_GROUPONLY@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t1","column":"name","tag_id":"@T_UNHELD@"}
-- setup: PUT /api/control-plane/tags/@T_SECRET@/access-policy {"required_roles":["l2_cleared"],"required_groups":[]}
-- setup: PUT /api/control-plane/tags/@T_GROUPONLY@/access-policy {"required_roles":[],"required_groups":["@G_STEWARDS@"]}
-- setup: PUT /api/control-plane/tags/@T_UNHELD@/access-policy {"required_roles":["l2_unheld"],"required_groups":[]}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g11-last4","kind":{"type":"mask","mask_type":"MASK_SHOW_LAST_4"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1","column":"ssn"},"exempt":{"include":[],"exclude":[]}}
SELECT id, ssn FROM lake.s1.t1 ORDER BY id LIMIT 3
SELECT id, email FROM lake.s1.t1 ORDER BY id LIMIT 3
SELECT id, name FROM lake.s1.t1 ORDER BY id LIMIT 3
SELECT * FROM lake.s2.t3 ORDER BY id LIMIT 3
SELECT count(*) FROM lake.s2.t3
SELECT id, v FROM lake.s1.t2 ORDER BY id LIMIT 3
SELECT count(*) FROM lake.s1.t2
