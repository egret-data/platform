CREATE TABLE dashboard_tabs (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    name         text NOT NULL,
    position     int  NOT NULL DEFAULT 0
);
CREATE INDEX dashboard_tabs_dashboard_idx ON dashboard_tabs (dashboard_id);

ALTER TABLE dashboard_cards
    ADD COLUMN tab_id uuid REFERENCES dashboard_tabs(id) ON DELETE CASCADE;
