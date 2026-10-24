-- KI-092: a SQL Lab user can clear their own query history.
--
-- Clearing hides the caller's rows from the History list; it does not delete
-- them. A query row is also what its status, results and CSV are served from,
-- and what the workspace's query count reads, so the rows stay and only the
-- History list (`QueryStore::list_for_user`) skips the hidden ones.
ALTER TABLE query.queries
    ADD COLUMN hidden_from_history boolean NOT NULL DEFAULT false;
