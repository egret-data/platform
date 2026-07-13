-- P2a: time-bound grants. A grant may carry an expiry; once past, it is excluded
-- from every access decision (enforced by the read-path predicate, not Cedar).
ALTER TABLE access_grants ADD COLUMN expires_at timestamptz;
-- NULL = never expires. Partial index keeps the "active" predicate cheap.
CREATE INDEX access_grants_expires_idx ON access_grants (expires_at) WHERE expires_at IS NOT NULL;
