-- 一次性本地提醒，仅保存预览/授权；后台领取和站内投递另行接入。
CREATE TABLE schedules (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    request_id UUID NOT NULL,
    version TEXT NOT NULL CHECK (version='local-reminder-once-v1'),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 80),
    body TEXT NOT NULL CHECK (char_length(body) BETWEEN 1 AND 2000),
    run_at_ms BIGINT NOT NULL CHECK (run_at_ms>0),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft','scheduled','cancelled')),
    created_ms BIGINT NOT NULL,
    approval_expires_ms BIGINT NOT NULL,
    approved_ms BIGINT,
    cancelled_ms BIGINT,
    approval JSONB CHECK (octet_length(approval::text)<=1024),
    PRIMARY KEY (user_id,request_id),
    CHECK (approval_expires_ms>created_ms AND approval_expires_ms<=run_at_ms),
    CHECK ((approved_ms IS NULL)=(approval IS NULL)),
    CHECK (status<>'draft' OR approved_ms IS NULL),
    CHECK (status<>'scheduled' OR approved_ms IS NOT NULL),
    CHECK ((status='cancelled')=(cancelled_ms IS NOT NULL))
);
CREATE INDEX schedules_due ON schedules(run_at_ms,user_id,request_id) WHERE status='scheduled';
CREATE INDEX schedules_owner_created ON schedules(user_id,created_ms);
