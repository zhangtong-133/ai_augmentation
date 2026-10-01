-- 手动 RSS 采集仓储；不启用任何联网任务。
CREATE TABLE feed_subscriptions (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    id UUID NOT NULL CHECK (id<>'00000000-0000-0000-0000-000000000000'),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    source_url TEXT NOT NULL CHECK (octet_length(source_url) BETWEEN 1 AND 2048),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    enabled BOOLEAN NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(user_id,id),
    CHECK (NOT deleted OR NOT enabled)
);
CREATE TABLE feed_collections (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL CHECK (request_id<>'00000000-0000-0000-0000-000000000000'),
    subscription_id UUID NOT NULL,
    plan JSONB NOT NULL CHECK (octet_length(plan::text)<=8192),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft','running','succeeded','failed','unknown','cancelled')),
    created_ms BIGINT NOT NULL CHECK(created_ms>0),
    claimed_ms BIGINT,
    deadline_ms BIGINT,
    finished_ms BIGINT,
    claim_id UUID,
    accepted_digest TEXT,
    reason TEXT CHECK(reason IN ('transport','parse','subscription_changed','execution_expired','unknown')),
    inserted INTEGER NOT NULL DEFAULT 0 CHECK(inserted BETWEEN 0 AND 100),
    updated INTEGER NOT NULL DEFAULT 0 CHECK(updated BETWEEN 0 AND 100),
    unchanged INTEGER NOT NULL DEFAULT 0 CHECK(unchanged BETWEEN 0 AND 100),
    PRIMARY KEY(user_id,request_id),
    FOREIGN KEY(user_id,subscription_id) REFERENCES feed_subscriptions(user_id,id) ON DELETE CASCADE,
    CHECK ((claimed_ms IS NULL)=(claim_id IS NULL)),
    CHECK ((claimed_ms IS NULL)=(deadline_ms IS NULL)),
    CHECK ((claimed_ms IS NULL)=(accepted_digest IS NULL)),
    CHECK (accepted_digest IS NULL OR accepted_digest=digest),
    CHECK (deadline_ms IS NULL OR deadline_ms=claimed_ms+60000),
    CHECK ((status IN ('draft','cancelled'))=(claimed_ms IS NULL)),
    CHECK ((status IN ('succeeded','failed','unknown','cancelled'))=(finished_ms IS NOT NULL)),
    CHECK ((status IN ('failed','unknown'))=(reason IS NOT NULL)),
    CHECK (status='succeeded' OR inserted+updated+unchanged=0),
    CHECK (inserted+updated+unchanged<=100)
);
CREATE UNIQUE INDEX feed_one_running_per_owner ON feed_collections(user_id) WHERE status='running';
CREATE INDEX feed_collection_daily ON feed_collections(user_id,claimed_ms) WHERE claimed_ms IS NOT NULL;
CREATE INDEX feed_collection_previews ON feed_collections(user_id,created_ms);
CREATE TABLE feed_entries (
    user_id UUID NOT NULL,
    subscription_id UUID NOT NULL,
    entry_key TEXT NOT NULL CHECK(entry_key ~ '^(guid|link):[0-9a-f]{64}$'),
    title TEXT NOT NULL CHECK(char_length(title)<=512),
    summary TEXT NOT NULL CHECK(char_length(summary)<=8192),
    link TEXT CHECK(octet_length(link)<=2048),
    published_at TEXT CHECK(char_length(published_at)<=256),
    content_digest TEXT NOT NULL CHECK(content_digest ~ '^[0-9a-f]{64}$'),
    first_seen_ms BIGINT NOT NULL,
    updated_ms BIGINT NOT NULL,
    last_seen_ms BIGINT NOT NULL,
    PRIMARY KEY(user_id,subscription_id,entry_key),
    FOREIGN KEY(user_id,subscription_id) REFERENCES feed_subscriptions(user_id,id) ON DELETE CASCADE,
    CHECK(title<>'' OR summary<>'')
);
-- 审计只存请求关联、状态、固定原因和数量，不复制来源、正文或原始响应。
CREATE TABLE feed_collection_audit (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    event TEXT NOT NULL CHECK(event IN ('draft','running','succeeded','failed','unknown','cancelled')),
    at_ms BIGINT NOT NULL,
    reason TEXT CHECK(reason IN ('transport','parse','subscription_changed','execution_expired','unknown')),
    inserted INTEGER NOT NULL DEFAULT 0 CHECK(inserted BETWEEN 0 AND 100),
    updated INTEGER NOT NULL DEFAULT 0 CHECK(updated BETWEEN 0 AND 100),
    unchanged INTEGER NOT NULL DEFAULT 0 CHECK(unchanged BETWEEN 0 AND 100),
    PRIMARY KEY(user_id,request_id,event),
    FOREIGN KEY(user_id,request_id) REFERENCES feed_collections(user_id,request_id) ON DELETE CASCADE
);
