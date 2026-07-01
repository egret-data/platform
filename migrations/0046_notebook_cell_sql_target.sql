-- Notebook E-P1: per-cell SQL execution target (Daft in-kernel, or a governed
-- engine id/name). NULL/'daft' = run via Daft; else the query() engine target.
ALTER TABLE notebook_cells ADD COLUMN sql_target text;
