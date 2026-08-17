-- Data API P5: what the management UI needs the access log to remember.
--
-- Three nullable columns, all nullable for the same reason: every row written
-- before this migration has no answer, and inventing one would make historical
-- rows lie about which caller they came from.
--
-- `actor_user_id` — non-null means "run from Try-it, by this person". It does
-- two jobs at once: statistics filter it OUT (a trial is not consumer traffic
-- and would pollute every percentile), while the call list shows it as the
-- audit trail of an impersonation action.
--
-- No FK, exactly as `endpoint_id` and `service_account_id` in 0123: the log must
-- survive the deletion of what it describes. A dangling uuid here is the normal
-- end state of a deleted user, not corruption.
ALTER TABLE dataapi.access_log ADD COLUMN actor_user_id uuid;

-- `request_method` / `request_path` — what the caller actually asked for.
--
-- WHY THEY ARE NEEDED AT ALL: a request that never resolved an endpoint writes a
-- row with `endpoint_id IS NULL` (see 0123's comment — "a 404 still produces a
-- row"). Without the requested URL, those rows can only be counted, never read:
-- the management UI can say "37 calls matched no endpoint" and nothing more,
-- which is exactly the situation where an operator needs to know WHICH url a
-- client is getting wrong.
--
-- The serving path holds both values at the moment it writes the row, so this
-- records something already in hand rather than reconstructing it.
--
-- NO INDEX ON request_path, deliberately. Nothing looks a row up by it: the
-- call list narrows on (workspace_id, ts) via `access_log_ws_ts_idx` and reads
-- the path out of the rows that query already returned. An index here would
-- only slow down the busiest insert in the schema.
ALTER TABLE dataapi.access_log ADD COLUMN request_method text;
ALTER TABLE dataapi.access_log ADD COLUMN request_path   text;
