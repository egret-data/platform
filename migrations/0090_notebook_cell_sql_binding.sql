-- Notebook P2: SQL cell + DataFrame binding. `sql_target` = the engine the SQL
-- runs on (e.g. 'trino'/'starrocks'); `sql_into` = the kernel variable the
-- result Daft DataFrame is bound to. Both NULL for code/markdown cells.
-- (Re-adds sql_target, dropped in 0047, alongside the new sql_into.)
ALTER TABLE analytics.notebook_cells ADD COLUMN sql_target text;
ALTER TABLE analytics.notebook_cells ADD COLUMN sql_into text;
