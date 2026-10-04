-- 允许独立本地评分终态；保持 API 金额模式不可从单次本地/订阅路径派发。
DO $$
DECLARE names TEXT[];
BEGIN
 SELECT array_agg(conname::text) INTO names FROM pg_constraint
 WHERE conrelid='feed_value_reviews'::regclass AND contype='c'
  AND pg_get_constraintdef(oid) LIKE '%pricing%kind%subscription%';
 IF cardinality(names) IS DISTINCT FROM 1 THEN RAISE EXCEPTION 'unexpected scoring dispatch constraint'; END IF;
 EXECUTE format('ALTER TABLE feed_value_reviews DROP CONSTRAINT %I',names[1]);
END $$;
ALTER TABLE feed_value_reviews ADD CONSTRAINT feed_value_execution_mode CHECK(
 status NOT IN ('running','succeeded','unknown') OR
 (dispatch_token IS NOT NULL AND dispatch_deadline_ms IS NOT NULL AND approved_ms IS NOT NULL
  AND pricing->>'kind' IN ('subscription','local'))
);
