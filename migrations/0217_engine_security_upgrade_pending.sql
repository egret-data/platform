-- StarRocks engine posture (security upgrade): true while a security upgrade
-- (`POST /api/control-plane/engines/{id}/security-upgrade`) has stored its
-- changes on the row (the FE root password secret, the patched FE/CN images)
-- but the apply that carries them to the cluster has not succeeded yet. The
-- route sets it before it writes anything and clears it once its apply
-- succeeds; any other successful apply of the StarRocks engine clears it too.
-- While it is set the engine reports the `notApplied` security upgrade reason,
-- and a repeat call finishes the run.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS security_upgrade_pending boolean NOT NULL DEFAULT false;
