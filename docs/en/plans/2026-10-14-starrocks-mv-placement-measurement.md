# MV placement measurement (StarRocks 4.1.6)

Version: `SELECT current_version()` -> `4.1.6-6862092` (local image `starrocks/allin1-ubuntu`, the `<none>`-tagged one, id 5cbf09c4c788).
Setup: throwaway containers mvq-sr (StarRocks), mvq-s3 (rustfs/rustfs:1.0.1), mvq-rest (apache/iceberg-rest-fixture:1.9.2), network mvq-net. Real Iceberg REST catalog + S3, real data. All Measured unless labelled Inferred.

Setup SQL:

```sql
CREATE EXTERNAL CATALOG ice PROPERTIES ("type"="iceberg","iceberg.catalog.type"="rest","iceberg.catalog.uri"="http://mvq-rest:8181","aws.s3.endpoint"="http://mvq-s3:9000","aws.s3.access_key"="mvqadmin","aws.s3.secret_key"="mvqadmin123","aws.s3.enable_path_style_access"="true","aws.s3.region"="us-east-1");
CREATE TABLE ice.db1.t (id int, v string);
INSERT INTO ice.db1.t VALUES (1,'a'),(2,'b');
```

## Answers

- Q1 (Measured): NO. An MV cannot be created in an external catalog. Error:
  `ERROR 1064 (HY000): Getting analyzing error from line 1, column 25 to line 1, column 33. Detail message: Materialized view can only be created in default_catalog. You could either create it with default_catalog.<database>.<mv>, or switch to default_catalog through `set catalog <default_catalog>` statement.`
  Statement: `CREATE MATERIALIZED VIEW ice.db1.mv_ext REFRESH ASYNC AS SELECT id, v FROM ice.db1.t;`
  (Plain `REFRESH ASYNC` over an external table without interval also fails: `Materialized view which type is ASYNC need to specify refresh interval for external table`.)
  Consequence: a Ranger deny on create-materialized-view with resource {catalog: ice, database: db1} can never be hit by a real CREATE MV (Inferred from Q1 + Q5).
- Q2 (Measured): YES, it lives in default_catalog, in a database of the same name as chosen (db1 must exist in default_catalog; I created it with `CREATE DATABASE db1`).
  `CREATE MATERIALIZED VIEW default_catalog.db1.mv_def REFRESH ASYNC EVERY (INTERVAL 1 MINUTE) AS SELECT id, v FROM ice.db1.t;` -> OK; refreshed SUCCESS; `SELECT * FROM default_catalog.db1.mv_def` returned (1,a),(2,b).
- Q3 (Measured): `SHOW MATERIALIZED VIEWS FROM `ice`.`db1`;` -> `Empty set (0.01 sec)` (no error, no columns printed; the MV exists but is not listed). `FROM `ice`.`nonexistent`` -> `ERROR 5501 (3F000): Unknown database 'nonexistent'`, so the catalog.db form is parsed and the db is resolved in ice, but it lists only MVs of that catalog (none).
  Surprise: after `SET CATALOG ice;`, `SHOW MATERIALIZED VIEWS FROM db1;` returned mv_def (it resolves db1 in default_catalog regardless of the session catalog). So the catalog-qualified form is the one that gives empty.
  SHOW MATERIALIZED VIEWS column names: id, database_name, name, refresh_type, is_active, inactive_reason, partition_type, task_id, task_name, last_refresh_start_time, last_refresh_finished_time, last_refresh_duration, last_refresh_state, last_refresh_force_refresh, last_refresh_start_partition, last_refresh_end_partition, last_refresh_base_refresh_partitions, last_refresh_mv_refresh_partitions, last_refresh_error_code, last_refresh_error_message, rows, text, extra_message, query_rewrite_status, creator, last_refresh_process_time, last_refresh_job_id, last_refresh_time, warehouse, refresh_mode, refresh_trigger, refresh_policy, resource_group, query_rewrite_status_reason, base_table_refresh_version_times, last_freshness_confirmed_at.
- Q4 (Measured): query `information_schema.materialized_views` (columns TABLE_SCHEMA, TABLE_NAME, MATERIALIZED_VIEW_DEFINITION, BASE_TABLE_REFRESH_VERSION_TIMES, CREATOR, IS_ACTIVE, ... full list below). MATERIALIZED_VIEW_DEFINITION holds the full `CREATE MATERIALIZED VIEW ... AS SELECT id, v FROM ice.db1.t;` text exactly as the user wrote it (not normalised, not necessarily catalog-qualified: an unqualified or aliased reference would not contain `ice.db1`). `SHOW MATERIALIZED VIEWS` column `text` is the same; `SHOW CREATE MATERIALIZED VIEW` also. BASE_TABLE_REFRESH_VERSION_TIMES (`{"ice.db1.t":"2026-10-13 18:37:36"}`) is a structured, normalised `catalog.db.table` key map of the base tables, better than LIKE on the definition (Measured present for this refresh-tracked MV; Inferred that it is populated for every MV with external bases).
  `SELECT TABLE_SCHEMA, TABLE_NAME FROM information_schema.materialized_views WHERE MATERIALIZED_VIEW_DEFINITION LIKE '%ice%db1%';` -> `db1 | mv_def`.
  information_schema.materialized_views columns: MATERIALIZED_VIEW_ID, TABLE_SCHEMA, TABLE_NAME, REFRESH_TYPE, IS_ACTIVE, INACTIVE_REASON, PARTITION_TYPE, TASK_ID, TASK_NAME, LAST_REFRESH_START_TIME, LAST_REFRESH_FINISHED_TIME, LAST_REFRESH_DURATION, LAST_REFRESH_STATE, LAST_REFRESH_FORCE_REFRESH, LAST_REFRESH_START_PARTITION, LAST_REFRESH_END_PARTITION, LAST_REFRESH_BASE_REFRESH_PARTITIONS, LAST_REFRESH_MV_REFRESH_PARTITIONS, LAST_REFRESH_ERROR_CODE, LAST_REFRESH_ERROR_MESSAGE, TABLE_ROWS, MATERIALIZED_VIEW_DEFINITION, EXTRA_MESSAGE, QUERY_REWRITE_STATUS, CREATOR, LAST_REFRESH_PROCESS_TIME, LAST_REFRESH_JOB_ID, LAST_REFRESH_TIME, WAREHOUSE, REFRESH_MODE, REFRESH_TRIGGER, REFRESH_POLICY, RESOURCE_GROUP, QUERY_REWRITE_STATUS_REASON, BASE_TABLE_REFRESH_VERSION_TIMES, LAST_FRESHNESS_CONFIRMED_AT.
- Q5 (Measured, native privilege system): CREATE MATERIALIZED VIEW is checked on the TARGET database in default_catalog, not on the base table's catalog/db. User u1 with `GRANT CREATE MATERIALIZED VIEW ON DATABASE ice.db1` (plus USAGE on both catalogs, SELECT on ice.db1.t) was denied creating `default_catalog.db1.mv_u1`:
  `ERROR 5203 (42000): Access denied; you need (at least one of) the CREATE MATERIALIZED VIEW privilege(s) on DATABASE db1 for this operation.`
  After additionally granting `GRANT CREATE MATERIALIZED VIEW ON DATABASE default_catalog.db1` it succeeded; after revoking the ice.db1 grant it still succeeded (only the default_catalog.db1 grant is needed). The SELECT check on the base table (ice.db1.t) is separate and was granted throughout. Caveat: this is the native privilege system, not Ranger; the Ranger resource mapping for the same check was not measured. A deny belongs on {catalog: default_catalog, database: <db>}; since MV db names are chosen freely in default_catalog, it cannot be tied to the external db (Inferred).

## Raw output files
Scratch: /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/03142572-227f-431e-8a3a-48e48f5e3721/scratchpad/mvq/ (out.txt, out2.txt, out5.txt, *.sql).
