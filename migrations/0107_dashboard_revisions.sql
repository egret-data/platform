-- Dashboard revision history: one immutable snapshot per successful dashboard PUT.
--
-- WHAT A REVISION IS. `snapshot` holds exactly the body of PUT /ws/{ws}/dashboards/{id} — the
-- `UpdateDashboard` DTO (egret-analytics-api/src/bi.rs): name, description, parameters, width,
-- tabs [{name, position}] and cards [{chart_id, card_type, grid_col, grid_row, size_x, size_y,
-- options, tab_index}]. That DTO is the complete, ID-FREE description of everything a dashboard
-- edit can change. Complete, because db_update writes exactly two things from it and there is no
-- partial-update path — the PUT is a whole-layout replace (dashboard.rs:3-5). ID-free, because
-- replace_layout DELETEs and re-INSERTs both the tabs and the cards table on every save and mints
-- fresh ids each time, so tabs carry no id in the DTO and cards reference their tab by INDEX.
--
-- ID-FREE IS WHAT MAKES REVERT CHEAP. There are no ids inside a snapshot that could collide with
-- the ids currently in the tables, so nothing has to be remapped: revert deserialises the snapshot
-- back into an UpdateDashboard and runs the ordinary update path. DashboardRepo::duplicate needs
-- its tab_map for the opposite reason — it copies ROWS, and rows carry ids.
--
-- WHAT IS DELIBERATELY NOT IN A SNAPSHOT: folder_id, deleted_at, created_by. Those are changed by
-- other endpoints (PATCH .../folder, POST .../trash) and are not part of a layout edit. Reverting
-- a layout must not silently move the dashboard back into a folder someone has since deleted, or
-- pull it out of the trash.
--
-- WHY NOT audit.audit_log (0076). It is the only audit-like table in the repo and it is the wrong
-- vehicle twice over. Its writes go through a bounded channel that DROPS on backpressure
-- (AuditSink, egret-audit/src/lib.rs), so the history would have silent holes; and its `detail`
-- column carries no entity content, so nothing in that table can reconstruct a dashboard.
-- Revisions are written synchronously on the request path and must be complete enough to restore.
--
-- RETENTION IS COUNT-BASED, NOT AGE-BASED, and is applied inline at insert. audit_log prunes by
-- age (ts < now() - interval) because it is one global firehose where age is the only axis. A
-- per-entity history has the opposite shape: under a 90-day rule a dashboard saved 400 times in an
-- afternoon keeps all 400, and a dashboard saved twice a year keeps NONE — the second being
-- exactly the dashboard whose history matters most. Keeping the newest N per dashboard bounds the
-- worst case AND guarantees the rarely-edited dashboard a usable history. N = 20; see
-- DashboardRepo::MAX_REVISIONS_PER_DASHBOARD for why 20 and not Metabase's 15.
CREATE TABLE analytics.dashboard_revisions (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES analytics.dashboards(id) ON DELETE CASCADE,
    created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- 'edit'     a PUT
    -- 'revert'   a PUT produced by reverting to an earlier revision
    -- 'baseline' the pre-feature state, captured once on a dashboard's first PUT so that the very
    --            first edit after this ships is still undoable. Mirrors Metabase's is_creation /
    --            is_reversion flags, which exist for the same two reasons.
    kind         text NOT NULL DEFAULT 'edit' CHECK (kind IN ('baseline','edit','revert')),
    snapshot     jsonb NOT NULL
);
CREATE INDEX dashboard_revisions_dash_idx ON analytics.dashboard_revisions (dashboard_id, created_at DESC, id DESC);
