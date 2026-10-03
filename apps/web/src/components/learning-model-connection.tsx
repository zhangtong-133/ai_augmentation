"use client";
import { useEffect, useState } from "react";
import { type ModelAuthorization, type ModelPreviewLoader } from "./learning-types";
import { LearningAuthorizationDetail } from "./learning-model-authorization";
type Connection = { id: string; label: string; revision: string; status: string; models: string[]; valid_until_unix_ms: string };
type Connections = { items: Connection[]; next_cursor: string | null };
export function LearningModelConnection({ path, locked, load, hideSharing }: { hideSharing: () => void; path: string; locked: boolean; load: ModelPreviewLoader }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  const [connections, setConnections] = useState<Connections | null>(null); const [selected, setSelected] = useState(""); const [model, setModel] = useState("");
  const [item, setItem] = useState<ModelAuthorization | null>(null);
  const [pending, setPending] = useState<{ request_id: string; connection_id: string; connection_revision: string; model: string } | null>(null);
  const connection = connections?.items.find(c => c.id === selected && c.status === "active" && Number(c.valid_until_unix_ms) > now);
  function list(after?: string) { setSelected(""); setModel(""); setConnections(null); setItem(null); load<Connections>(`/api/subscription-connections${after ? `?after=${encodeURIComponent(after)}` : ""}`, setConnections); }
  function receive(v: ModelAuthorization) { setPending(null); setItem(v); }
  function create(body: NonNullable<typeof pending>) { setPending(body); setItem(null); load<ModelAuthorization>(path, receive, { method: "POST", body }); }
  return <div>
    <p>先选择已登记的订阅连接和模型，再查看有期限的授权草稿。没有可用连接时，请先完成本地连接登记。</p>
    <button disabled={locked || !!pending} onClick={() => list()}>读取可用订阅连接</button>
    {connections && !pending && !item && <div>
      <label>核验订阅连接<select disabled={locked} value={selected} onChange={e => { setSelected(e.target.value); setModel(""); }}><option value="">请选择连接</option>{connections.items.filter(c => c.status === "active" && Number(c.valid_until_unix_ms) > now).map(c => <option key={c.id} value={c.id}>{c.label}</option>)}</select></label>
      <label>核验模型<select disabled={locked || !connection} value={model} onChange={e => setModel(e.target.value)}><option value="">请选择模型</option>{connection?.models.map(m => <option key={m} value={m}>{m}</option>)}</select></label>
      <button disabled={locked || !connection || !model} onClick={() => { if (connection) create({ request_id: crypto.randomUUID(), connection_id: connection.id, connection_revision: connection.revision, model }); }}>创建模型授权草稿</button>
      {connections.next_cursor && <button disabled={locked} onClick={() => list(connections.next_cursor!)}>下一页订阅连接</button>}
    </div>}
    {pending && <div><p>草稿结果尚未核对，请保留原请求。</p><button disabled={locked} onClick={() => create(pending)}>重试原授权草稿</button><button disabled={locked} onClick={() => load<ModelAuthorization>(`/api/learning/model-authorizations/${pending.request_id}`, receive)}>查询原授权草稿</button></div>}
    {item && <LearningAuthorizationDetail key={`${item.request_id}:${item.status}`} item={item} locked={locked} load={load} hideSharing={hideSharing} accept={receive} />}
  </div>;
}
