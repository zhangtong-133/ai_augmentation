-- 工具执行前占用次数；结果未知或失败不退还，不保存输入及输出正文。
CREATE TABLE tool_daily_budgets (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day DATE NOT NULL,
    used INTEGER NOT NULL CHECK (used BETWEEN 0 AND 100),
    PRIMARY KEY (user_id, day)
);

CREATE TABLE tool_calls (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    request_id UUID NOT NULL,
    tool TEXT NOT NULL CHECK (length(tool) BETWEEN 1 AND 64),
    arguments_digest TEXT NOT NULL CHECK (arguments_digest ~ '^[0-9a-f]{64}$'),
    day DATE NOT NULL,
    status TEXT NOT NULL DEFAULT 'running'
        CHECK (status IN ('running','succeeded','failed','timed_out','busy','denied','output_rejected')),
    input_bytes INTEGER NOT NULL CHECK (input_bytes BETWEEN 2 AND 8192),
    output_bytes INTEGER CHECK (output_bytes BETWEEN 0 AND 65536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    deadline TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '60 seconds',
    finished_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, request_id),
    FOREIGN KEY (user_id, day) REFERENCES tool_daily_budgets(user_id, day),
    CHECK ((status='running') = (finished_at IS NULL)),
    CHECK ((status='succeeded') = (output_bytes IS NOT NULL))
);
CREATE INDEX tool_calls_user_day ON tool_calls(user_id, day, created_at, request_id);
