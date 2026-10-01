-- 只生成已保存条目的规则日报；配置缺失即禁用。与用户删除级联。
CREATE TABLE feed_brief_schedules (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL CHECK (revision > 0),
    enabled BOOLEAN NOT NULL,
    minute_utc INTEGER NOT NULL CHECK (minute_utc BETWEEN 0 AND 1439),
    next_run_ms BIGINT CHECK (next_run_ms >= 0),
    last_attempt_ms BIGINT CHECK (last_attempt_ms >= 0),
    last_request_id UUID,
    last_outcome TEXT CHECK (last_outcome IN ('generated', 'skipped')),
    CHECK (enabled = (next_run_ms IS NOT NULL)),
    CHECK ((last_attempt_ms IS NULL) = (last_outcome IS NULL)),
    CHECK ((last_outcome IS NOT DISTINCT FROM 'generated') = (last_request_id IS NOT NULL))
);
CREATE INDEX feed_brief_schedules_due ON feed_brief_schedules(next_run_ms, user_id) WHERE enabled;
