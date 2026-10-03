import { type EvidenceReview } from "./learning-types";
const labels: Record<EvidenceReview["state"], string> = {
  not_recorded: "尚未记录训练结果",
  cancelled: "训练已取消，不作为完成证据",
  missing_note: "已记录完成，但缺少文字材料",
  unverified: "已有文字材料，内容尚待核验",
};
export function LearningEvidenceReview({ review, noteId }: { review: EvidenceReview; noteId: string }) {
  return <details>
    <summary>查看证据检查</summary>
    <p>{labels[review.state] ?? "检查状态不可用，请更新计划状态。"}</p>
    <p>绑定技能版本 {review.skill_revision}，仅检查这份计划中的材料，不代表当前能力。完成次数和耗时不会换算成分数。</p>
    {review.result_request_id && <a href={`#${noteId}`}>回看原始训练记录</a>}
    <p>请结合原始记录逐项核验；没有材料的项目保留为缺少证据：</p>
    <ul>
      <li>概念解释：是否用自己的话准确说明了关键概念？</li>
      <li>独立练习：具体完成了什么，哪些部分需要帮助？</li>
      <li>结果验证：是否留下可复现的步骤、检查方法和结果？</li>
      <li>局限与反例：哪些情况仍不能处理，下一次准备验证什么？</li>
    </ul>
    <p>本检查不生成评分建议。需要调整自评时，请回到“显式自评”主动填写；提交前核对当前技能版本。已保存的训练记录不可修改，可在新的训练中补充材料。</p>
  </details>;
}
