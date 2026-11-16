-- Workflow package 7d (spec docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md
-- Q8, D6). `labels` (0192) holds the labels beyond the flow's own `labels:`,
-- which every reader merges in from the frozen `source`. Once a person has
-- replaced an execution's labels (`POST …/executions/{id}/labels`), `labels`
-- holds the WHOLE visible list and the flow's are not merged in any more, so a
-- flow label can be removed. Every reader goes through `labels::visible_labels`.
ALTER TABLE workflow.executions
  ADD COLUMN labels_replaced boolean NOT NULL DEFAULT false;
