-- Package 6 of the Data API production-readiness programme: record what a
-- publish did to the consumers of the version it replaced.
--
-- ── Why a new COLUMN and not a field inside `snapshot` ────────────────────────
--
-- `dataapi.endpoint_versions.snapshot` is the frozen contract a live consumer is
-- coding against, and this crate has refused twice already (M1's
-- `ColumnSpec::required`, M2's `statements`) to rewrite stored rows to suit a
-- newer binary. A diff is not part of the contract either: it is a fact ABOUT
-- the moment of publication, not about what a request returns. Putting it beside
-- the snapshot keeps `EndpointSnapshot` exactly what serving reads and nothing
-- more.
--
-- ── Why NULL rather than DEFAULT '[]' ────────────────────────────────────────
--
-- The two are different facts and this table has to keep them apart:
--
--   * NULL — nobody looked. Every version published before this column existed,
--     and every FIRST version of an endpoint, where there is no live contract to
--     compare against because no consumer has ever received anything.
--   * '[]' — this publish WAS compared against the live version and broke
--     nothing.
--
-- `DEFAULT '[]'` would have stamped the second claim onto every row that only
-- ever qualified for the first, retroactively, in a migration — asserting that a
-- comparison happened for versions published months before the comparison
-- existed. The whole package is about not letting a silence read as an answer,
-- so the column that records it must not start by doing exactly that.
--
-- `publish` writes a value on every publish from here on, so the ambiguity does
-- not grow: NULL after this migration means "first version", and only that.
--
-- Together with the `created_by` and `created_at` this table already carries,
-- this is the audit trail — who accepted which break, on which version, when.
-- Cheap to write at the one moment it is known; not recoverable from anything
-- else afterwards.
ALTER TABLE dataapi.endpoint_versions
    ADD COLUMN breaking_changes jsonb;

COMMENT ON COLUMN dataapi.endpoint_versions.breaking_changes IS
    'The changes this version made that a consumer of the previously-live '
    'version cannot survive, as classified by egret_dataapi::contract at '
    'publish time, and therefore what created_by explicitly acknowledged. '
    'NULL means no comparison was made — a version published before this '
    'column existed, or the first version of an endpoint, where there was no '
    'live contract to break. An empty array means a comparison was made and '
    'found nothing.';
