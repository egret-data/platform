ALTER TABLE dashboards
  ADD COLUMN width text NOT NULL DEFAULT 'full' CHECK (width IN ('fixed', 'full'));
