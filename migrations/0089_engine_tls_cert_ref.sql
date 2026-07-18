-- Optional: back jwt-mode coordinator TLS with a user-provided kubernetes.io/tls
-- secret (a shared/company cert) instead of the auto self-signed cert.
-- NULL keeps the current per-engine self-signed behavior.
ALTER TABLE engine_mgmt.engines ADD COLUMN tls_cert_secret_ref uuid;
