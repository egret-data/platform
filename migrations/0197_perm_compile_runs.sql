-- DG #3 (Governance Overview), spec §3.3: the last compile of each profile.
--
-- Until now the only trace of a compile was `perm.compiled.compiled_at` per
-- compiled policy, so nothing recorded that a profile's last compile FAILED
-- and the profile is still serving its previous compile. One row per profile:
--
-- * success is upserted by the compile funnel (`recompile_locked_service_in_tx`)
--   on the compile's own transaction, so a compile whose caller rolls back
--   leaves no row (nothing changed on the engine);
-- * failure is upserted only by the entry points no write waits on (the
--   pool-based `recompile_service` and the manual compile route), after their
--   transaction rolled back. A compile that fails inside a grant, rule, tag or
--   policy write is not recorded: that write is refused with the error, and the
--   engine keeps the previous compile.
--
-- `last_ok_at` and the counts survive a failure, so the Overview can say when
-- the profile last compiled. The CHECK makes an error row without its text
-- unrepresentable.
CREATE TABLE perm.compile_runs (
    service_id   bigint PRIMARY KEY REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    outcome      text NOT NULL CHECK (outcome IN ('ok', 'error')),
    attempted_at timestamptz NOT NULL,
    error        text,
    last_ok_at   timestamptz,
    created      integer NOT NULL DEFAULT 0,
    updated      integer NOT NULL DEFAULT 0,
    deleted      integer NOT NULL DEFAULT 0,
    unchanged    integer NOT NULL DEFAULT 0,
    skipped      integer NOT NULL DEFAULT 0,
    CONSTRAINT compile_runs_error_says_why CHECK ((outcome = 'error') = (error IS NOT NULL))
);
