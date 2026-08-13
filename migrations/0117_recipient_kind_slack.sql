-- Slack has been a `RecipientKind` in Rust since #484 — with a delivery path,
-- a vault-backed bot token and an editor field — but `recipient_kind_valid`
-- (migration 0110) still admitted only `email` and `webhook`. So every attempt
-- to save a Slack destination failed at the database with
-- `violates recipient_kind_valid`, and the whole feature was unusable through
-- the API. Nothing caught it because no test had ever created one.
--
-- The CHECK is the guarantee and the Rust enum is the error message; when the
-- two disagree the guarantee wins, silently, at the worst moment. Adding a
-- variant to `RecipientKind` means coming here.
ALTER TABLE automation.schedule_recipients
  DROP CONSTRAINT IF EXISTS recipient_kind_valid;

ALTER TABLE automation.schedule_recipients
  ADD CONSTRAINT recipient_kind_valid CHECK (kind IN ('email', 'webhook', 'slack'));
