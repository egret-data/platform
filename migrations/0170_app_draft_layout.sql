-- Data Apps: persist the marimo grid-layout companion file (D23) alongside the
-- draft/version source, instead of leaving it on the pod's local disk only.
--
-- marimo writes `layouts/<app>.<type>.json` with a raw filesystem write
-- (`save_layout_config`, outside its `Storage` abstraction) -- the pod patch
-- forwards its content here explicitly; it is not derived from `source`.
-- NULL means no layout (the default, vertical arrangement).
ALTER TABLE analytics.app_drafts
  ADD COLUMN layout_json text NULL CHECK (octet_length(layout_json) <= 1048576);

ALTER TABLE analytics.app_versions
  ADD COLUMN layout_json text NULL CHECK (octet_length(layout_json) <= 1048576);
