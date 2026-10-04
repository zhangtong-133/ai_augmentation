"use client";
import { LearningLocalModel } from "./learning-local-model";
import { LearningModelConnection } from "./learning-model-connection";
import { useState } from "react";
import { type ModelReviewPreview, type ModelPreviewLoader } from "./learning-types";
export function LearningModelPreview({ path, locked, load }: { path: string; locked: boolean; load: ModelPreviewLoader }) {
  const [showSharing, setShowSharing] = useState(true);
  const [preview, setPreview] = useState<ModelReviewPreview | null>(null);
  return <section aria-label="模型分享预览">
    <button disabled={locked} onClick={() => { setShowSharing(true); setPreview(null); load(path, setPreview); }}>预览模型分享材料</button>
    {preview && <div><h5>模型分享预览（未发送）</h5>
      <p>拟分享技能名称、训练要求和四项证据。仅查看预览不会调用模型或消耗订阅额度；发送须单独授权并执行。</p>
      <p>模型建议仍需你逐项核验，不会自动修改自评分数。以下为完整分享内容，关闭或刷新后清除本次预览。</p>
      {showSharing && <pre className="feedText" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{JSON.stringify(preview, null, 2)}</pre>}
      <LearningModelConnection path={path.replace(/model-preview$/, "model-authorizations")} locked={locked} load={load} hideSharing={() => setShowSharing(false)} />
      <LearningLocalModel path={path.replace(/model-preview$/, "local-model-authorizations")} locked={locked} load={load} hideSharing={() => setShowSharing(false)} />
      <button disabled={locked} onClick={() => setPreview(null)}>关闭分享预览</button>
    </div>}
  </section>;
}
