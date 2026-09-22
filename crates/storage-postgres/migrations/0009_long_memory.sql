-- 用户手动维护的长期记忆，不自动作为模型上下文。
CREATE TABLE memory_facts (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (char_length(btrim(title)) > 0 AND char_length(title) <= 80),
    content TEXT NOT NULL CHECK (char_length(btrim(content)) > 0 AND char_length(content) <= 2000),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX memory_facts_owner_order ON memory_facts(user_id, created_at DESC, id DESC);
