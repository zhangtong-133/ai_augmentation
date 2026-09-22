"use client";

import { useEffect, useRef, useState } from "react";

export function requestError(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 403) return "请求被拒绝，请刷新页面检查登录状态。";
  if (status === 404) return "对话已删除或无权访问，请刷新对话列表。";
  if (status === 409) return "已达到额度上限，或请求 ID 已用于其他内容。请核对历史后再操作。";
  if (status === 400 || status === 422) return "输入格式不符合要求，请核对内容。";
  return "服务暂不可用，请稍后重试。";
}

// 未确认的写入冻结原始请求；只有显式重试才再次发送，不生成新请求 ID。
export function useIdempotentPost<T>(path: string, onSuccess: (value: T) => void) {
  const [pending, setPending] = useState<Record<string, string | number> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const controller = useRef<AbortController | null>(null);
  useEffect(() => () => { controller.current?.abort(); controller.current = null; }, []);

  async function submit(input: Record<string, string | number>) {
    if (controller.current) return;
    const payload = pending ?? { ...input, request_id: crypto.randomUUID() };
    setPending(payload); setBusy(true); setError("");
    const active = new AbortController(); controller.current = active;
    try {
      const response = await fetch(path, {
        method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify(payload), signal: AbortSignal.any([active.signal, AbortSignal.timeout(10000)]),
      });
      if (!response.ok) throw new Error(requestError(response.status));
      const value: T = await response.json();
      if (controller.current !== active) return;
      setPending(null); onSuccess(value);
    } catch (e) {
      if (controller.current === active) setError(e instanceof Error && e.name === "Error" ? e.message : "结果未确认。请求可能已成功，请重试原请求或先核对历史。");
    } finally { if (controller.current === active) { controller.current = null; setBusy(false); } }
  }
  function discard() { if (!controller.current) { setPending(null); setError(""); } }
  return { pending, busy, error, submit, discard };
}
