-- 单次订阅派发：API 模式不从这条路径领取，也不改动金额账本。
ALTER TABLE feed_value_reviews DROP CONSTRAINT feed_value_reviews_status_check;
ALTER TABLE feed_value_reviews DROP CONSTRAINT feed_value_reviews_check2;
ALTER TABLE feed_value_reviews ADD COLUMN scores JSONB;
ALTER TABLE feed_value_reviews ADD COLUMN dispatch_token UUID;
ALTER TABLE feed_value_reviews ADD COLUMN dispatch_deadline_ms BIGINT;
ALTER TABLE feed_value_reviews ADD COLUMN sent_ms BIGINT;
ALTER TABLE feed_value_reviews ADD CONSTRAINT feed_value_execution_status CHECK(status IN ('draft','authorized','running','succeeded','unknown','cancelled','expired','invalidated'));
ALTER TABLE feed_value_reviews ADD CONSTRAINT feed_value_execution_snapshot CHECK((status IN ('draft','authorized','running','succeeded'))=(snapshot IS NOT NULL));
ALTER TABLE feed_value_reviews ADD CHECK((status='succeeded')=(scores IS NOT NULL));
ALTER TABLE feed_value_reviews ADD CHECK(scores IS NULL OR (jsonb_typeof(scores)='array' AND octet_length(scores::text)<=65536));
ALTER TABLE feed_value_reviews ADD CHECK(status NOT IN ('running','succeeded','unknown') OR (dispatch_token IS NOT NULL AND dispatch_deadline_ms IS NOT NULL AND approved_ms IS NOT NULL AND pricing->>'kind'='subscription'));
ALTER TABLE feed_value_reviews ADD CHECK(sent_ms IS NULL OR (dispatch_token IS NOT NULL AND sent_ms>=approved_ms AND sent_ms<dispatch_deadline_ms));
ALTER TABLE feed_value_reviews ADD CHECK((dispatch_token IS NULL)=(dispatch_deadline_ms IS NULL));
ALTER TABLE feed_value_reviews ADD CHECK(dispatch_deadline_ms IS NULL OR (dispatch_deadline_ms>approved_ms AND dispatch_deadline_ms<=expires_ms));
ALTER TABLE feed_value_reviews ADD CHECK(status<>'succeeded' OR sent_ms IS NOT NULL);
ALTER TABLE feed_value_audit DROP CONSTRAINT feed_value_audit_event_check;
ALTER TABLE feed_value_audit ADD CHECK(event IN ('draft','authorized','running','succeeded','unknown','cancelled','expired','invalidated','sending'));

CREATE OR REPLACE FUNCTION invalidate_feed_value_reviews() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL,scores=NULL WHERE user_id=OLD.user_id AND status IN ('draft','authorized','running','succeeded');
  RETURN OLD;
 END IF;
 UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL,scores=NULL WHERE user_id=NEW.user_id AND status IN ('draft','authorized','running','succeeded');
 RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION subscription_connection_changed() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN
  INSERT INTO subscription_connection_audit(user_id,connection_id,revision,status,at_ms)
   VALUES(NEW.user_id,NEW.id,NEW.revision,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 IF TG_OP='UPDATE' THEN
  UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL,scores=NULL
   WHERE user_id=NEW.user_id AND status IN ('draft','authorized','running','succeeded')
    AND pricing->>'kind'='subscription' AND pricing->>'connection_id'=NEW.id::text;
 END IF;
 RETURN NEW;
END;
$$;
