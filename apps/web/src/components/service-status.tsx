"use client";

import { useEffect, useState } from "react";

export function ServiceStatus() {
  const [health, setHealth] = useState("检查中…");
  const [ready, setReady] = useState("检查中…");
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const controller = new AbortController();
    async function check(path: string) {
      try {
        const response = await fetch(path, { cache: "no-store", signal: controller.signal });
        return response.ok ? "正常" : "不可用";
      } catch { return "无法连接"; }
    }
    async function load() {
      setLoading(true); setHealth("检查中…"); setReady("检查中…");
      const [live, storage] = await Promise.all([check("/api/healthz"), check("/api/readyz")]);
      if (!controller.signal.aborted) { setHealth(live); setReady(storage); setLoading(false); }
    }
    void load();
    return () => controller.abort();
  }, [revision]);

  return <div aria-label="服务状态" aria-busy={loading}>
    <p role="status">API 存活：{health} · 数据库就绪：{ready}</p>
    <button disabled={loading} onClick={() => setRevision(value => value + 1)}>检查服务</button>
  </div>;
}
