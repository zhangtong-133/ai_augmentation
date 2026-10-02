-- 仅保存周期采集授权，不创建后台联网任务。
CREATE TABLE feed_schedules (
 user_id UUID NOT NULL,
 id UUID NOT NULL CHECK (id <> '00000000-0000-0000-0000-000000000000'),
 subscription_id UUID NOT NULL,
 plan JSONB NOT NULL CHECK (octet_length(plan::text)<=8192),
 digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
 status TEXT NOT NULL CHECK (status IN ('draft','active','cancelled','expired')),
 created_ms BIGINT NOT NULL CHECK (created_ms>0),
 approval_expires_ms BIGINT NOT NULL CHECK (approval_expires_ms=created_ms+300000),
 ends_ms BIGINT NOT NULL CHECK (ends_ms>approval_expires_ms AND ends_ms<=created_ms+604800000),
 approved_ms BIGINT CHECK (approved_ms>=created_ms AND approved_ms<approval_expires_ms),
 PRIMARY KEY(user_id,id),
 FOREIGN KEY(user_id,subscription_id) REFERENCES feed_subscriptions(user_id,id) ON DELETE CASCADE,
 CHECK (status<>'active' OR approved_ms IS NOT NULL),
 CHECK (status<>'draft' OR approved_ms IS NULL)
);
CREATE UNIQUE INDEX feed_schedule_one_active ON feed_schedules(user_id,subscription_id) WHERE status='active';
CREATE INDEX feed_schedule_created ON feed_schedules(user_id,created_ms);
CREATE TABLE feed_schedule_audit (
 user_id UUID NOT NULL,
 schedule_id UUID NOT NULL,
 event TEXT NOT NULL CHECK (event IN ('draft','active','cancelled','expired')),
 at_ms BIGINT NOT NULL,
 PRIMARY KEY(user_id,schedule_id,event),
 FOREIGN KEY(user_id,schedule_id) REFERENCES feed_schedules(user_id,id) ON DELETE CASCADE
);
CREATE FUNCTION audit_feed_schedule() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
  INSERT INTO feed_schedule_audit VALUES(NEW.user_id,NEW.id,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER feed_schedule_audit_status AFTER INSERT OR UPDATE ON feed_schedules FOR EACH ROW EXECUTE FUNCTION audit_feed_schedule();
CREATE FUNCTION invalidate_feed_schedules() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.revision IS DISTINCT FROM OLD.revision OR NEW.deleted OR NOT NEW.enabled THEN
  UPDATE feed_schedules SET status='cancelled' WHERE user_id=NEW.user_id AND subscription_id=NEW.id AND status IN ('draft','active');
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER feed_schedule_subscription_change AFTER UPDATE ON feed_subscriptions FOR EACH ROW EXECUTE FUNCTION invalidate_feed_schedules();
