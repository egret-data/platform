-- G03 (StarRocks in sub-project 1, adapted to Trino): allow write on a table to ann and bob,
-- bob excluded (allowExceptions), under a public catalog read. The writes change no row.
-- users: ann,bob
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["write"],"scope":{"catalog_id":"@CAT@","schema":"s2","table":"w"},"principals":{"include":[{"kind":"user","ref":"ann"},{"kind":"user","ref":"bob"}],"exclude":[{"kind":"user","ref":"bob"}]}}
-- expect: ann | INSERT INTO lake.s2.w SELECT id FROM lake.s2.w WHERE id < 0 | [[0]]
-- expect: bob | INSERT INTO lake.s2.w SELECT id FROM lake.s2.w WHERE id < 0 | denied
INSERT INTO lake.s2.w SELECT id FROM lake.s2.w WHERE id < 0
DELETE FROM lake.s2.w WHERE id < 0
SELECT count(*) FROM lake.s2.w
INSERT INTO lake.s2.t3 SELECT * FROM lake.s2.t3 WHERE id < 0
