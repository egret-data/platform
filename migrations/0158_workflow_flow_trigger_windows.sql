-- 0158: origin/main ends at 0157 (W13a) and the unmerged SP4a branch holds
-- 0154/0155, so 0158 collides with neither in either merge order.
--
-- Workflow W13b: the `dependsOn` window of a Flow trigger -- Kestra's
-- `MultipleConditionWindow`, one row per listener. See
-- docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md §5.2-§5.3.
--
-- The spec's table, plus the CHECK `window_start < window_end` (W13b Task 4):
-- `trigger::window::TimeWindow::bounds` never returns an empty window
-- (`BoundsError::Empty`), and a row that no instant is strictly inside could
-- never count an event, so it is unrepresentable rather than remembered.
--
-- `outputs` is Kestra's `MultipleConditionWindow.outputs` (the source
-- execution's outputs, keyed namespace -> flowId). egret writes it NULL: a
-- fired execution's trigger record is the window-free shape (spec §5.4), and
-- nothing reads the window's outputs.
--
-- **Deleting a listener cascades its window row** -- a deliberate divergence
-- from Kestra, which keeps it (user decision D1, spec §5.2). A listener row is
-- deleted when its trigger is removed from the flow (`scheduler::sync`) or
-- the flow row is deleted (0157's cascade from `workflow.flows`); a flow
-- re-created with the same trigger id must not inherit stale results.
CREATE TABLE workflow.flow_trigger_windows (
  flow_id      uuid        NOT NULL,
  trigger_id   text        NOT NULL,
  window_start timestamptz NOT NULL,
  window_end   timestamptz NOT NULL,
  results      jsonb       NOT NULL,   -- {condition key -> true}; only true is stored
  outputs      jsonb,
  PRIMARY KEY (flow_id, trigger_id),
  FOREIGN KEY (flow_id, trigger_id)
    REFERENCES workflow.flow_listeners (flow_id, trigger_id) ON DELETE CASCADE,
  CONSTRAINT workflow_flow_trigger_windows_start_before_end
    CHECK (window_start < window_end)
);

-- The scheduler pass's sweep: `DELETE ... WHERE window_end <= now()`.
CREATE INDEX workflow_flow_trigger_windows_end ON workflow.flow_trigger_windows (window_end);
