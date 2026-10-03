import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";
const id = "00000000-0000-4000-8000-000000000091";
function frame(sequence, detail, request = id) {
  return `event: ${"reason" in detail ? "closed" : "status"}\nid: ${sequence}\ndata: ${JSON.stringify({ protocol_version: "learning-status-v1", request_id: request, sequence: String(sequence), detail })}\n\n`;
}
const finalFrame = frame(0, { status: "succeeded", terminal: true });
async function setup(page, events) {
  await page.goto("/"); await login(page, await createAccount());
  const panel = page.getByRole("region", { name: "学习管理", exact: true });
  await expect(panel).toContainText("暂无学习计划");
  const state = { events: 0, reads: 0, posts: 0, item: { request_id: id, plan_id: id, task_id: id, connection_id: "fixture-local", connection_revision: "1", model: "fixture-model", status: "authorized", created_at_unix_ms: "1", expires_at_unix_ms: String(Date.now() + 300000), preview: { evidence: "旧私有材料" }, advice: null } };
  await page.route("**/api/learning/model-authorizations**", async route => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() !== "GET") state.posts++;
    if (path.endsWith("/events")) { state.events++; return events(route, state); }
    if (path.endsWith("model-authorizations")) return route.fulfill({ json: { items: [state.item], next_cursor: null } });
    state.reads++; return route.fulfill({ json: state.item });
  });
  const history = panel.getByRole("region", { name: "模型核验授权历史", exact: true });
  await history.getByRole("button", { name: "读取模型授权历史", exact: true }).click();
  await history.getByRole("button", { name: "查看模型授权", exact: true }).click();
  return { panel, history, state, start: history.getByRole("button", { name: "观察执行状态", exact: true }) };
}
function advice() {
  return Object.fromEntries(["explanation", "work", "verification", "limitations"].map(field => [field, { verdict: "supported", reason: "从已保存记录读取的建议 <script>", citations: [{ field, quote: "证据引用" }] }]));
}
test("observing execution queries saved advice after terminal without any model POST", async ({ page }, testInfo) => {
  let release; const held = new Promise(resolve => { release = resolve; });
  const ui = await setup(page, async (route, state) => {
    await held; state.item = { ...state.item, status: "succeeded", preview: null, advice: advice() };
    await route.fulfill({ contentType: "text/event-stream", body: frame(0, { status: "running", terminal: false }) + frame(1, { status: "succeeded", terminal: true }) });
  });
  await ui.start.click();
  await expect(ui.history.getByRole("button", { name: "停止观察执行状态", exact: true })).toBeVisible();
  await expect(ui.history.locator("pre")).toHaveCount(0);
  const screenshot = testInfo.outputPath("learning-status-observation.png");
  await ui.history.screenshot({ path: screenshot }); await testInfo.attach("learning-status-observation", { path: screenshot, contentType: "image/png" });
  release();
  await expect(ui.history.getByRole("region", { name: "模型核验建议", exact: true })).toContainText("从已保存记录读取的建议 <script>");
  expect(ui.state.events).toBe(1); expect(ui.state.reads).toBe(2); expect(ui.state.posts).toBe(0);
  await expect(ui.history.locator("script")).toHaveCount(0);
});
test("stopping observation discards late events without cancelling authorization", async ({ page }) => {
  let release, arrived; const held = new Promise(resolve => { release = resolve; }), received = new Promise(resolve => { arrived = resolve; });
  const ui = await setup(page, async route => { arrived(); await held; try { await route.fulfill({ contentType: "text/event-stream", body: finalFrame }); } catch { /* aborted */ } });
  await ui.start.click(); await received;
  await ui.history.getByRole("button", { name: "停止观察执行状态", exact: true }).click(); release();
  await expect(ui.history).toContainText("已停止观察");
  await expect(ui.history).toContainText("已保存授权，尚未执行");
  await ui.history.getByRole("button", { name: "核对模型授权状态", exact: true }).click();
  expect(ui.state.events).toBe(1); expect(ui.state.reads).toBe(2); expect(ui.state.posts).toBe(0);
});
test("invalid, truncated, and rate-limited observations stay manual without retries", async ({ page }) => {
  const replies = [
    { contentType: "text/event-stream", body: frame(0, { status: "succeeded", terminal: true }, "foreign") },
    { contentType: "text/event-stream", body: frame(0, { status: "running", terminal: false }) },
    { status: 429, json: {} },
  ];
  const ui = await setup(page, route => route.fulfill(replies.shift()));
  for (let n = 0; n < 3; n++) {
    await ui.start.click();
    await expect(ui.history).toContainText(n === 2 ? "观察连接已达上限" : "观察已中断");
    expect(ui.state.events).toBe(n + 1); expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
    await expect(ui.history.locator("pre")).toHaveCount(0);
  }
});
test("observation timeout reads the original authorization once and does not reconnect", async ({ page }) => {
  const ui = await setup(page, route => route.fulfill({ contentType: "text/event-stream", body: frame(0, { reason: "observation_timeout" }) }));
  await ui.start.click();
  await expect.poll(() => ui.state.reads).toBe(2);
  await expect(ui.start).toBeEnabled(); expect(ui.state.events).toBe(1); expect(ui.state.posts).toBe(0);
});
test("session loss in an open observation clears the entire private learning panel", async ({ page }) => {
  const ui = await setup(page, route => route.fulfill({ contentType: "text/event-stream", body: frame(0, { reason: "session_unavailable" }) }));
  await ui.start.click(); await expect(ui.panel).toContainText("登录已失效，请重新登录。");
  await expect(ui.history).toHaveCount(0); await expect(ui.panel).not.toContainText("fixture-local");
  expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
});
test("late observation cannot enter another account or a reopened request", async ({ page }) => {
  let release, arrived; const held = new Promise(resolve => { release = resolve; }), received = new Promise(resolve => { arrived = resolve; });
  const ui = await setup(page, async route => { arrived(); await held; try { await route.fulfill({ contentType: "text/event-stream", body: finalFrame }); } catch { /* unmounted */ } });
  await ui.start.click(); await received;
  await ui.history.getByRole("button", { name: "读取模型授权历史", exact: true }).click();
  await ui.history.getByRole("button", { name: "查看模型授权", exact: true }).click(); release();
  await expect(ui.history).toContainText("已保存授权，尚未执行"); expect(ui.state.reads).toBe(2);
  const other = await createAccount();
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, other);
  await expect(ui.panel).toContainText("暂无学习计划"); await expect(ui.panel).not.toContainText("fixture-local");
  expect(ui.state.posts).toBe(0);
});

test("account change discards an in-flight status response", async ({ page }) => {
  const other = await createAccount();
  let release, arrived; const held = new Promise(resolve => { release = resolve; }), received = new Promise(resolve => { arrived = resolve; });
  const ui = await setup(page, async route => { arrived(); await held; try { await route.fulfill({ contentType: "text/event-stream", body: finalFrame }); } catch { /* account unmounted */ } });
  await ui.start.click(); await received;
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, other); release();
  await expect(ui.panel).toContainText("暂无学习计划"); await expect(ui.panel).not.toContainText("fixture-local");
  await expect(ui.panel.getByRole("region", { name: "模型核验建议", exact: true })).toHaveCount(0);
  expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
});
test("unauthorized initial observation clears private state without an extra lookup", async ({ page }) => {
  const ui = await setup(page, route => route.fulfill({ status: 401, json: {} }));
  await ui.start.click(); await expect(ui.panel).toContainText("登录已失效，请重新登录。");
  await expect(ui.history).toHaveCount(0); expect(ui.state.reads).toBe(1); expect(ui.state.events).toBe(1);
});
