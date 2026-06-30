ALTER TABLE kernels ADD COLUMN token_hash text;
ALTER TABLE kernels ADD COLUMN token_expires_at timestamptz;
CREATE INDEX kernels_token_hash_idx ON kernels (token_hash);
