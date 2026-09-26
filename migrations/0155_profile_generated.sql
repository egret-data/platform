-- 0155_profile_generated.sql
-- SP4a D4. A profile is compiled from the perm model (`generated`) or
-- hand-authored (`manual`). Existing profiles are manual: they were written by
-- hand, and calling them generated would make the next compile delete policies
-- nobody asked it to.
ALTER TABLE policy.authz_services
    ADD COLUMN origin text NOT NULL DEFAULT 'manual'
    CHECK (origin IN ('generated', 'manual'));
