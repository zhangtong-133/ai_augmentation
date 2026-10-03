-- 仅保存模型分享/费用上限同意，不创建金额预留或可执行任务。
CREATE TABLE feed_value_reviews (
 user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 id UUID NOT NULL CHECK(id<>'00000000-0000-0000-0000-000000000000'),
 status TEXT NOT NULL CHECK(status IN ('draft','authorized','cancelled','expired','invalidated')),
 snapshot JSONB CHECK(octet_length(snapshot::text)<=4194304),
 pricing JSONB NOT NULL CHECK(octet_length(pricing::text)<=4096),
 digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
 amount BIGINT CHECK(amount>0),
 created_ms BIGINT NOT NULL CHECK(created_ms>=0),
 expires_ms BIGINT NOT NULL CHECK(expires_ms>created_ms AND expires_ms-created_ms<=300000),
 approved_ms BIGINT CHECK(approved_ms>=created_ms AND approved_ms<expires_ms),
 PRIMARY KEY(user_id,id),
 CHECK ((status IN ('draft','authorized'))=(snapshot IS NOT NULL)),
 CHECK (status<>'authorized' OR approved_ms IS NOT NULL),
 CHECK (status<>'draft' OR approved_ms IS NULL)
);
CREATE INDEX feed_value_creation ON feed_value_reviews(user_id,created_ms);
CREATE TABLE feed_value_audit (
 user_id UUID NOT NULL,
 request_id UUID NOT NULL,
 sequence BIGINT GENERATED ALWAYS AS IDENTITY,
 event TEXT NOT NULL CHECK(event IN ('draft','authorized','cancelled','expired','invalidated')),
 at_ms BIGINT NOT NULL,
 PRIMARY KEY(user_id,request_id,sequence),
 FOREIGN KEY(user_id,request_id) REFERENCES feed_value_reviews(user_id,id) ON DELETE CASCADE
);
CREATE FUNCTION audit_feed_value_review() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
  INSERT INTO feed_value_audit(user_id,request_id,event,at_ms) VALUES(NEW.user_id,NEW.id,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER feed_value_audit_status AFTER INSERT OR UPDATE ON feed_value_reviews
 FOR EACH ROW EXECUTE FUNCTION audit_feed_value_review();
-- 保守作废该用户全部未执行预览，避免来源改名/停用、条目刷新或偏好改变后旧同意继续使用。
CREATE FUNCTION invalidate_feed_value_reviews() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL WHERE user_id=OLD.user_id AND status IN ('draft','authorized');
  RETURN OLD;
 END IF;
 UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL WHERE user_id=NEW.user_id AND status IN ('draft','authorized');
 RETURN NEW;
END;
$$;
CREATE TRIGGER feed_value_source_changed AFTER UPDATE OR DELETE ON feed_subscriptions
 FOR EACH ROW EXECUTE FUNCTION invalidate_feed_value_reviews();
CREATE TRIGGER feed_value_entry_changed AFTER INSERT OR UPDATE OR DELETE ON feed_entries
 FOR EACH ROW EXECUTE FUNCTION invalidate_feed_value_reviews();
CREATE TRIGGER feed_value_preferences_changed AFTER INSERT OR UPDATE OR DELETE ON feed_brief_preferences
 FOR EACH ROW EXECUTE FUNCTION invalidate_feed_value_reviews();
