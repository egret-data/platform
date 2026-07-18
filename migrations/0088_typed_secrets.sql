-- Typed secrets P0: move single-value secrets to a multi-field (key -> sealed value)
-- model. `secrets` gains a k8s-style `type`; values live in `secret_data`, one
-- sealed row per key. Legacy single-value secrets become Opaque with a `value` key.
ALTER TABLE secrets ADD COLUMN type text NOT NULL DEFAULT 'Opaque';

CREATE TABLE secret_data (
    secret_id  uuid  NOT NULL REFERENCES secrets(id) ON DELETE CASCADE,
    key        text  NOT NULL,
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text  NOT NULL DEFAULT 'cfg',
    PRIMARY KEY (secret_id, key)
);

-- Copy every existing sealed value into secret_data under the `value` key.
INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id)
    SELECT id, 'value', nonce, ciphertext, key_id FROM secrets;

-- secret_data is now the single source of truth for values.
ALTER TABLE secrets DROP COLUMN nonce, DROP COLUMN ciphertext;
