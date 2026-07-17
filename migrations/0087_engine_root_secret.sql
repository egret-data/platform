-- 0087_engine_root_secret.sql
-- Per-engine secret holding the generated StarRocks FE `root` password
-- (secure-by-default: every NEW StarRocks engine mints one at create). NULL for
-- legacy engines provisioned before this column existed — those stay passwordless
-- (Task 2 applies no initpwd when `root_password` resolves to None). The password
-- is stored in `secrets`; this column references it.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN root_secret_id uuid;
