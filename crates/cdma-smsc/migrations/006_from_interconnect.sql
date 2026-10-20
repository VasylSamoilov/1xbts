-- A message that arrived from another operator must never be sent back out to
-- one. Without this the hub and a site ping-pong a message for a number the
-- site does not actually serve: the site takes the delivery, fails to resolve
-- it locally, and offers it straight back to the hub.
ALTER TABLE sms_submissions
    ADD COLUMN IF NOT EXISTS from_interconnect BOOLEAN NOT NULL DEFAULT FALSE;
