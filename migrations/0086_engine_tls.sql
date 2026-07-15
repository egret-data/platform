-- 0086_engine_tls.sql
-- jwt-mode delegation TLS: the coordinator serves HTTPS with an egret-generated
-- self-signed cert. `tls_secret_id` -> the egret secret holding the keystore PEM
-- (cert+key, mounted to the coordinator); `coordinator_cert` is the PUBLIC cert PEM
-- the query connector pins. Both NULL until an engine is provisioned in jwt mode.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN tls_secret_id uuid,
    ADD COLUMN coordinator_cert text;
