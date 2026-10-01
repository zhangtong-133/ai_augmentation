ALTER TABLE schedules DROP CONSTRAINT schedules_status_check;
ALTER TABLE schedules ADD CONSTRAINT schedules_status_check
    CHECK (status IN ('draft','scheduled','running','delivered','cancelled','failed'));
ALTER TABLE schedules ADD COLUMN claim_id UUID;
ALTER TABLE schedules ADD COLUMN lease_until_ms BIGINT;
ALTER TABLE schedules ADD COLUMN delivered_ms BIGINT;
ALTER TABLE schedules ADD CONSTRAINT schedules_lease_pair CHECK ((claim_id IS NULL)=(lease_until_ms IS NULL));
ALTER TABLE schedules ADD CONSTRAINT schedules_running_lease CHECK (status NOT IN ('running','delivered') OR (claim_id IS NOT NULL AND approved_ms IS NOT NULL));
ALTER TABLE schedules ADD CONSTRAINT schedules_delivery_time CHECK ((status='delivered')=(delivered_ms IS NOT NULL));
CREATE INDEX schedules_claimable ON schedules(run_at_ms,user_id,request_id)
    WHERE status IN ('scheduled','running');

CREATE TABLE schedule_reminders (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 80),
    body TEXT NOT NULL CHECK (char_length(body) BETWEEN 1 AND 2000),
    delivered_ms BIGINT NOT NULL,
    PRIMARY KEY (user_id,request_id),
    FOREIGN KEY (user_id,request_id) REFERENCES schedules(user_id,request_id) ON DELETE CASCADE
);
