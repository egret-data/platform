-- G20: an overlap refusal: a second path row filter on the same table with another expression is
-- refused at write; the first stays in force.
-- users: ann
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g20-us","kind":{"type":"row_filter","filter_expr":"region = 'us'"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1"},"exempt":{"include":[],"exclude":[]}}
-- refused: POST /api/control-plane/perm/rules {"name":"l2-g20-eu","kind":{"type":"row_filter","filter_expr":"region = 'eu'"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1"},"exempt":{"include":[],"exclude":[]}}
SELECT region, count(*) FROM lake.s1.t1 GROUP BY region ORDER BY region
