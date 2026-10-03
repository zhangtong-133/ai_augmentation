-- 周期时段与现有采集账本共享额度；不会创建后台任务。
CREATE TABLE feed_schedule_occurrences (
 user_id UUID NOT NULL,
 schedule_id UUID NOT NULL,
 occurrence_key TEXT NOT NULL CHECK (occurrence_key ~ '^[0-9a-f]{64}$'),
 request_id UUID NOT NULL,
 scheduled_ms BIGINT NOT NULL CHECK (scheduled_ms>0),
 dispatch_expires_ms BIGINT NOT NULL CHECK (dispatch_expires_ms>scheduled_ms AND dispatch_expires_ms<=scheduled_ms+600000),
 dispatched_ms BIGINT CHECK (dispatched_ms>=scheduled_ms AND dispatched_ms<dispatch_expires_ms),
 PRIMARY KEY(user_id,schedule_id,occurrence_key),
 UNIQUE(user_id,request_id),
 UNIQUE(user_id,schedule_id,scheduled_ms),
 FOREIGN KEY(user_id,schedule_id) REFERENCES feed_schedules(user_id,id) ON DELETE CASCADE,
 FOREIGN KEY(user_id,request_id) REFERENCES feed_collections(user_id,request_id) ON DELETE CASCADE
);
