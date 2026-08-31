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
--   * '[]' — this publish WAS compared, against the version named by
--     `compared_against`, and broke nothing for the consumers of THAT version.
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
-- ── Why the baseline is NAMED, and not left to be assumed ────────────────────
--
-- `compared_against` is the version the diff was taken against — the one
-- `endpoints.live_version_id` pointed at inside the publish's own row lock. It
-- is not always the newest row in this table, and that is the whole reason it is
-- stored rather than inferred.
--
-- `rollback` moves `live_version_id` BACKWARDS and writes no row here. It is
-- deliberately not gated (see `EndpointStore::rollback`: the one gate that must
-- not exist is the one standing between an operator and the end of an outage),
-- so with v1 live and v0 rolled back to, every consumer coding against v1 has
-- already been moved off it by an act this table does not record. A publish that
-- then compares the draft against v0 can perfectly truthfully find nothing —
-- and a bare '[]' would have been read, by anyone reconstructing the incident,
-- as "publishing v2 broke nobody", full stop. It did not break the consumers of
-- v0. It says nothing at all about the consumers of v1, and without the baseline
-- beside it there is no way to tell that from the row.
--
-- So '[]' is only ever a claim about `compared_against`, and a reader can see
-- for themselves whether that was the newest version at the time. The act that
-- moved the pointer leaves its own record in the audit trail, where `rollback`
-- now emits the same classification against the version it moved off.
--
-- Together with the `created_by` and `created_at` this table already carries,
-- this is the audit trail — who accepted which break, against which baseline,
-- when. Cheap to write at the one moment it is known; not recoverable from
-- anything else afterwards.
ALTER TABLE dataapi.endpoint_versions
    ADD COLUMN breaking_changes jsonb,
    -- No FK action: `endpoint_versions` is immutable and nothing deletes from
    -- it, and a baseline that somehow went away must not take the record of the
    -- comparison with it.
    ADD COLUMN compared_against uuid REFERENCES dataapi.endpoint_versions (id);

COMMENT ON COLUMN dataapi.endpoint_versions.breaking_changes IS
    'The changes this version made that a consumer of the version named by '
    'compared_against cannot survive, as classified by egret_dataapi::contract '
    'at publish time, and therefore what created_by explicitly acknowledged. '
    'NULL means no comparison was made — a version published before this '
    'column existed, or the first version of an endpoint, where there was no '
    'live contract to break. An empty array means a comparison was made '
    'against compared_against and found nothing; it is a claim about the '
    'consumers of THAT version only, which after a rollback is not the newest '
    'version of this endpoint.';

COMMENT ON COLUMN dataapi.endpoint_versions.compared_against IS
    'The version this one was compared against at publish: whatever '
    'endpoints.live_version_id pointed at inside the publishing transaction. '
    'NULL exactly when breaking_changes is NULL. Not always the preceding '
    'version — rollback moves the live pointer backwards and writes no row '
    'here — which is why breaking_changes cannot be read without it.';
