"use client";
import { useState } from "react";
import { type ModelAuthorization, type ModelPreviewLoader } from "./learning-types";
import { LearningAuthorizationDetail } from "./learning-model-authorization";
export function LearningLocalModel({ path, locked, load, hideSharing }: { path: string; locked: boolean; load: ModelPreviewLoader; hideSharing: () => void }) {
  const [endpoint, setEndpoint] = useState("http://127.0.0.1:11435");
  const [model, setModel] = useState("qwen3:4b-q4_K_M");
  const [item, setItem] = useState<ModelAuthorization | null>(null);
  const [pending, setPending] = useState<{ request_id: string; endpoint: string; model: string } | null>(null);
  function receive(value: ModelAuthorization) { setPending(null); setItem(value); }
  function create(body: NonNullable<typeof pending>) { setPending(body); setItem(null); load<ModelAuthorization>(path, receive, { method: "POST", body }); }
  return <section aria-label="本地模型授权">
    <h5>使用本地模型</h5><p>填写执行机的 loopback 地址和明确模型标签。创建草稿不发送材料；功能需由部署配置启用。</p>
    {!pending && !item && <div>
      <label>本地推理地址<input disabled={locked} value={endpoint} onChange={e => setEndpoint(e.target.value)} /></label>
      <label>本地核验模型<input disabled={locked} value={model} onChange={e => setModel(e.target.value)} /></label>
      <button disabled={locked || !endpoint || !model} onClick={() => create({ request_id: crypto.randomUUID(), endpoint, model })}>创建本地模型授权草稿</button>
    </div>}
    {pending && <div><p>本地草稿结果尚未核对，请保留原请求。</p><button disabled={locked} onClick={() => create(pending)}>重试原本地授权草稿</button><button disabled={locked} onClick={() => load<ModelAuthorization>(`/api/learning/model-authorizations/${pending.request_id}`, receive)}>查询原本地授权草稿</button></div>}
    {item && <LearningAuthorizationDetail key={`${item.request_id}:${item.status}`} item={item} locked={locked} load={load} hideSharing={hideSharing} accept={receive} />}
  </section>;
}
