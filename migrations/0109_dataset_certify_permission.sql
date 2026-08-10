-- Make dataset and metric certification mean what dashboard certification means.
--
-- WHAT WAS WRONG. 0108 introduced `dashboard:certify` as a PLATFORM permission and said, in its
-- own comment, that dataset certification rode on `dataset:manage` and was worth resolving. Two
-- defects, not one:
--
--   1. SELF-CERTIFICATION. Anyone who could edit a dataset could mark it trusted. The badge then
--      says "its author liked it", which is not information a reader can act on.
--
--   2. FORGED ATTRIBUTION — the worse of the two. `certified_by` arrived in the REQUEST BODY, as
--      a free-text field in the UI ("Certified by: Name or team"). An editor could stamp a
--      dataset as vetted BY SOMEONE ELSE. The dashboard route never had this: it takes the
--      certifier from the session and the client cannot name anyone.
--
-- Metrics carried both defects too, through `add_metric` / `update_metric`. Fixing datasets and
-- leaving metrics would have made the dataset gate cosmetic: the metric is what the semantic
-- layer actually shows a reader, so a forgeable metric badge defeats a trustworthy dataset one.
--
-- WHY ONE PERMISSION FOR BOTH. A metric belongs to a dataset; "vouching for this data" is one
-- act, not two. A separate `metric:certify` would be a second thing to grant and to forget.
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, 'dataset:certify'
FROM roles r
WHERE r.scope = 'platform' AND r.name = 'data_steward'
ON CONFLICT (role_id, permission) DO NOTHING;

-- NO BACKFILL, AND NO CLEARING. Rows already marked certified keep their flag and their recorded
-- certifier. Clearing them would be the safer-looking choice and the wrong one: it would erase
-- real review work done by real stewards, and there is no way to tell, after the fact, which
-- historical rows were self-certified and which were not. The gate applies from here forward;
-- what is already recorded stays as the record of what happened.
