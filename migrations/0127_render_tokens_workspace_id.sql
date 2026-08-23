-- The render-token path is a fourth reader of ChartRepo::get/DashboardRepo::get
-- (crate/egret-automation/src/render/payload.rs), and the token it authorizes
-- on carried no workspace: `RenderGrant { kind, object_id, owner_id }`. A token
-- minted for a schedule in workspace 1, naming a chart in workspace 2, would
-- resolve that chart's title/spec/rows through /render/payload and /render/data
-- with nothing to compare against — proven live in the scoping probe
-- (.superpowers/sdd/chart-target-scoping-probe.md, "Surface 2").
--
-- The schedule's workspace is known at mint time (`s.workspace_id` in
-- `render_image_report` / `render_alert_attachment`); it just was not carried
-- into the token row, so `peek`/`redeem` had nothing to hand back to the reader
-- on the other side of the request boundary. This closes that gap.
--
-- TRUNCATE rather than backfill: every row here has a ~60s TTL
-- (`RENDER_TOKEN_TTL`), so any row still present is either already expired (and
-- `prune_expired` / the `expires_at > now()` predicate on every read already
-- ignore it) or belongs to an in-flight render that will simply see its token
-- as not-found and fail closed — the same outcome a restart already produces.
TRUNCATE automation.render_tokens;

ALTER TABLE automation.render_tokens
  ADD COLUMN workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE;
