-- 保存授权时的固定计划版本，后续部署不能用新规则执行旧授权。
ALTER TABLE agent_plans ADD COLUMN version TEXT NOT NULL DEFAULT 'knowledge-search-v1'
    CHECK (length(version) BETWEEN 1 AND 64);
