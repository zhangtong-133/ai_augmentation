-- 仅保存显式离线日报，不启用采集、模型或通知。
CREATE TABLE feed_brief_preferences (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL CHECK(revision>0),
    keywords JSONB NOT NULL CHECK(jsonb_typeof(keywords)='array' AND jsonb_array_length(keywords)<=5 AND octet_length(keywords::text)<=4096)
);
CREATE TABLE feed_briefs (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
    preference_revision BIGINT NOT NULL CHECK(preference_revision>=0),
    day_start_ms BIGINT NOT NULL CHECK(day_start_ms>=0 AND day_start_ms%86400000=0),
    created_ms BIGINT NOT NULL CHECK(created_ms>=day_start_ms AND created_ms::numeric<day_start_ms::numeric+86400000),
    status TEXT NOT NULL CHECK(status IN ('ready','invalidated','deleted')),
    digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
    plan JSONB CHECK(octet_length(plan::text)<=2097152),
    PRIMARY KEY(user_id,request_id),
    CHECK((status='ready')=(plan IS NOT NULL))
);
CREATE INDEX feed_brief_daily ON feed_briefs(user_id,day_start_ms);
CREATE TABLE feed_brief_sources (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    subscription_id UUID NOT NULL,
    PRIMARY KEY(user_id,request_id,subscription_id),
    FOREIGN KEY(user_id,request_id) REFERENCES feed_briefs(user_id,request_id) ON DELETE CASCADE,
    FOREIGN KEY(user_id,subscription_id) REFERENCES feed_subscriptions(user_id,id) ON DELETE CASCADE
);
CREATE INDEX feed_brief_source_lookup ON feed_brief_sources(user_id,subscription_id);
-- 每个候选来源均登记，即使条目因排名/去重未被选中。
-- 整份失效并清除正文，不以修改后的快照冒充原始计划。
CREATE FUNCTION invalidate_feed_briefs() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR (NEW.deleted AND NOT OLD.deleted) THEN
        UPDATE feed_briefs b SET status='invalidated',plan=NULL
        WHERE b.user_id=OLD.user_id AND b.status='ready'
          AND EXISTS (SELECT 1 FROM feed_brief_sources s WHERE s.user_id=b.user_id
            AND s.request_id=b.request_id AND s.subscription_id=OLD.id);
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER feed_brief_delete_cleanup BEFORE UPDATE OF deleted OR DELETE ON feed_subscriptions
    FOR EACH ROW EXECUTE FUNCTION invalidate_feed_briefs();
