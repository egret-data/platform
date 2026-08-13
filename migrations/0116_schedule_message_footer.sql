-- A closing note under the result, so the notification can be topped and
-- tailed around what it delivers.
--
-- `message` (0114) is the opening note and keeps that meaning — no rename, no
-- backfill, and every existing notification renders exactly as it did. What
-- changes is that the picture now sits BETWEEN the two, inline in the body
-- rather than only as an attachment, which is the whole point of having a
-- footer at all: "here is why you are getting this" / picture / "here is what
-- to do about it".
ALTER TABLE automation.schedules
  ADD COLUMN message_footer text;
