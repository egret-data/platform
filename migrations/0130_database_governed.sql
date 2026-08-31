-- Package 1 of the Data API production-readiness programme: mark a connection
-- whose data is governed at the ENGINE (Ranger row filters, column masks), so
-- that reading it through the Data API — publishing an endpoint onto it,
-- previewing a statement against it, or serving a live version from it —
-- requires a connection that runs every query as the CALLER rather than as the
-- shared service account.
--
-- ── Which direction this defaults, and why ────────────────────────────────────
--
-- DEFAULT false, i.e. "not governed", i.e. the publish gate does not fire.
--
-- That is the direction that does not silently change today's behaviour, and
-- the choice is between two different failures rather than between safe and
-- unsafe:
--
--   * DEFAULT true would mark every existing connection governed at the instant
--     this migration ran. Every endpoint published against a non-delegated
--     database — which is every endpoint the gate is about — would stop serving
--     on the next request, with no operator having asked for anything. A
--     migration is not a place to take a production outage, and an outage
--     nobody chose is one operators clear by flipping the column back off,
--     which teaches them the column means "breaks things".
--
--   * DEFAULT false leaves those endpoints exactly as they are today: running
--     as the service account, with Ranger evaluated at machine-identity
--     granularity. That exposure is real and it is UNCHANGED by this migration
--     — the gate is opt-in per database, and marking a database `governed` is
--     the deliberate act by which a workspace admin asks for it.
--
-- The programme's decision (design §4, Decision A) is a per-database flag
-- precisely because the alternatives depend on someone remembering. This column
-- is what they set ONCE, on the database whose data is actually governed;
-- everything after that is enforced by the server, at publish and at serve.
ALTER TABLE query.databases
    ADD COLUMN governed boolean NOT NULL DEFAULT false;

COMMENT ON COLUMN query.databases.governed IS
    'Engine-side row filters / column masks apply to this connection''s data. '
    'A Data API endpoint may only be published onto it — and its SQL may only be '
    'previewed against it, and a live version may only be served from it — when '
    'the connection runs queries as the caller (delegated, or '
    'options.impersonate); otherwise the query runs on a shared service '
    'credential and those filters are evaluated for the machine identity, not '
    'the consumer.';
