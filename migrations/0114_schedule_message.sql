-- A user-authored message carried into the notification body, and the switch
-- that suppresses a send with nothing in it.
--
-- `message` is Superset's `description`/`context_markdown` under a name that
-- says what it is for: the schedule already has a `description`, which is the
-- line shown in the LIST, and overloading it into the mail body would make
-- editing one silently change the other.
ALTER TABLE automation.schedules
  ADD COLUMN message text,
  -- Metabase's `skip_if_empty`. Deliberately NOT defaulted to true: a report
  -- that silently stops arriving is a support ticket, so opting in is the
  -- user's choice.
  ADD COLUMN skip_if_empty boolean NOT NULL DEFAULT false;
