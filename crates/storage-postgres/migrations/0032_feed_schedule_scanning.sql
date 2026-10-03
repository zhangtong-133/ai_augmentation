-- 内部游标扫描，不启用调度进程或改变授权。
CREATE INDEX feed_schedules_active_scan ON feed_schedules(user_id,id) WHERE status='active';
CREATE INDEX feed_collections_running_scan ON feed_collections(user_id,request_id,deadline_ms) WHERE status='running';
