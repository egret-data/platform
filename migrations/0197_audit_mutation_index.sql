-- DG #3 (Governance Overview), spec §3.3/§3.4: the governance feed reads
-- `kind = 'mutation'` rows newest first. PDP decision rows share the table and
-- dominate it (one per authorisation), so without a partial index the feed's
-- `ORDER BY ts DESC, id DESC LIMIT n` walks `audit_log_ts_idx` through every
-- decision row between two governance events.
CREATE INDEX audit_log_mutation_ts_idx ON audit.audit_log (ts DESC, id DESC) WHERE kind = 'mutation';
