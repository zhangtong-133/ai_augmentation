ALTER TABLE schedule_reminders
    ADD COLUMN revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    ADD COLUMN read_ms BIGINT CHECK (read_ms >= delivered_ms),
    ADD COLUMN archived_ms BIGINT CHECK (archived_ms >= delivered_ms);
CREATE INDEX schedule_reminders_inbox ON schedule_reminders(user_id, request_id)
    WHERE archived_ms IS NULL;
CREATE INDEX schedule_reminders_archived ON schedule_reminders(user_id, request_id)
    WHERE archived_ms IS NOT NULL;
