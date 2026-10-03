// 修改可能已在服务器提交，即使响应丢失；开始和结束都清除其他面板的旧评分视图。
export const feedValueInputsChanging = "feed-value-inputs-changing";
export async function changingFeedValueInputs<T>(work: () => Promise<T>, current: () => boolean): Promise<T> {
  const announce = () => { if (current()) window.dispatchEvent(new Event(feedValueInputsChanging)); };
  announce();
  try { return await work(); }
  finally { announce(); }
}
