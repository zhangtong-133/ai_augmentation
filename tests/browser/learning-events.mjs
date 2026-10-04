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

function textFrame(sequence, event, detail, request = id) {
  return `event: ${event}\nid: ${sequence}\ndata: ${JSON.stringify({ protocol_version: "learning-text-v1", request_id: request, sequence: String(sequence), detail })}\n\n`;
}
const livePrefix = textFrame(0, "status", { status: "running", terminal: false }) + textFrame(1, "delta", { text: "私有临时正文 <script> 中文，尚未核验。" });
// A controllable browser-local stream drives the real reader/component lifecycle.
// The smoke suite separately exercises actual Redis, API and both proxies.
async function live(page, body = livePrefix, status = 200) {
  await page.evaluate(({ body, status }) => {
    const original = window.fetch.bind(window);
    const state = { calls: 0, cancelled: false, controller: null, stale: null };
    window.__learningText = state;
    window.fetch = async (url, options) => {
      if (!String(url).endsWith("/text-events")) return original(url, options);
      state.calls++;
      if (status !== 200) return new Response("{}", { status });
      state.cancelled = false;
      return new Response(new ReadableStream({
        start(controller) {
          state.controller = controller; state.stale = controller;
          controller.enqueue(new TextEncoder().encode(body));
          options.signal.addEventListener("abort", () => {
            state.cancelled = true;
            try { controller.error(new DOMException("Aborted", "AbortError")); } catch { /* already ended */ }
            if (state.controller === controller) state.controller = null;
          }, { once: true });
        },
        cancel() { state.cancelled = true; state.controller = null; },
      }), { headers: { "content-type": "text/event-stream" } });
    };
  }, { body, status });
}
async function finishLive(page, body, end = true) {
  await page.evaluate(({ body, end }) => {
    const controller = window.__learningText.controller;
    if (!controller) return;
    controller.enqueue(typeof body === "string" ? new TextEncoder().encode(body) : Uint8Array.from(body));
    if (end) { controller.close(); window.__learningText.controller = null; }
  }, { body, end });
}
async function textUI(page) {
  const ui = await setup(page, route => route.fulfill({ contentType: "text/event-stream", body: finalFrame }));
  await live(page);
  await ui.history.getByRole("button", { name: "观察临时文本", exact: true }).click();
  const region = ui.history.getByRole("region", { name: "模型临时文本", exact: true });
  await expect(region).toContainText("私有临时正文 <script> 中文");
  return { ...ui, region };
}
test("live text is plain temporary output and saved advice comes from the original request", async ({ page }, testInfo) => {
  const ui = await textUI(page);
  await expect(ui.region).toContainText("未校验的临时文本");
  await expect(ui.region.locator("script")).toHaveCount(0);
  await expect(ui.history.getByRole("region", { name: "模型核验建议", exact: true })).toHaveCount(0);
  const screenshot = testInfo.outputPath("learning-temporary-text.png");
  await ui.history.screenshot({ path: screenshot }); await testInfo.attach("learning-temporary-text", { path: screenshot, contentType: "image/png" });
  await finishLive(page, textFrame(2, "clear", {}), false);
  await expect(ui.region.locator("pre")).toHaveCount(0);
  ui.state.item = { ...ui.state.item, status: "succeeded", preview: null, advice: advice() };
  await finishLive(page, textFrame(3, "status", { status: "succeeded", terminal: true }));
  await expect(ui.region).toHaveCount(0);
  await expect(ui.history.getByRole("region", { name: "模型核验建议", exact: true })).toContainText("从已保存记录读取的建议 <script>");
  expect(ui.state.reads).toBe(2); expect(ui.state.posts).toBe(0);
  expect(await page.evaluate(() => window.__learningText.calls)).toBe(1);
});
test("stopping live text clears the body and cannot cancel or resend execution", async ({ page }) => {
  const ui = await textUI(page);
  await ui.history.getByRole("button", { name: "停止观察临时文本", exact: true }).click();
  await expect(ui.region).toHaveCount(0); await expect(ui.history).toContainText("已停止观察");
  expect(await page.evaluate(() => window.__learningText.cancelled)).toBe(true);
  await page.evaluate(() => { try { window.__learningText.stale.enqueue(new TextEncoder().encode("late private body")); } catch { /* cancelled reader */ } });
  await expect(ui.history).not.toContainText("late private body");
  expect(ui.state.posts).toBe(0); expect(ui.state.reads).toBe(1);
});
test("source invalidation discards provisional body before querying the saved authorization", async ({ page }) => {
  const ui = await textUI(page);
  ui.state.item = { ...ui.state.item, status: "invalidated", preview: null, advice: null };
  await finishLive(page, textFrame(2, "status", { status: "invalidated", terminal: true }));
  await expect(ui.history).toContainText("来源或连接已失效");
  await expect(ui.region).toHaveCount(0); await expect(ui.history.locator("pre")).toHaveCount(0);
  expect(ui.state.reads).toBe(2); expect(ui.state.posts).toBe(0);
});
test("session loss during live text clears every private learning view", async ({ page }) => {
  const ui = await textUI(page);
  await finishLive(page, textFrame(2, "closed", { reason: "session_unavailable" }));
  await expect(ui.panel).toContainText("登录已失效，请重新登录。");
  await expect(ui.panel).not.toContainText("私有临时正文"); await expect(ui.panel).not.toContainText("fixture-local");
  expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
});
test("reopening a request clears live text and ignores its old reader", async ({ page }) => {
  const ui = await textUI(page);
  await ui.history.getByRole("button", { name: "读取模型授权历史", exact: true }).click();
  await ui.history.getByRole("button", { name: "查看模型授权", exact: true }).click();
  expect(await page.evaluate(() => window.__learningText.cancelled)).toBe(true);
  await page.evaluate(() => { try { window.__learningText.stale.enqueue(new TextEncoder().encode("late old request")); } catch { /* unmounted */ } });
  await expect(ui.history).not.toContainText("私有临时正文"); await expect(ui.history).not.toContainText("late old request");
  await expect(ui.history).toContainText("已保存授权，尚未执行"); expect(ui.state.reads).toBe(2);
});
test("account change aborts and clears the temporary reader", async ({ page }) => {
  const other = await createAccount(); const ui = await textUI(page);
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, other);
  await expect(ui.panel).toContainText("暂无学习计划"); await expect(ui.panel).not.toContainText("私有临时正文");
  expect(await page.evaluate(() => window.__learningText.cancelled)).toBe(true);
  expect(ui.state.posts).toBe(0); expect(ui.state.reads).toBe(1);
});
test("truncated, invalid UTF-8 and late protocol errors clear text without a saved-result query", async ({ page }) => {
  const ui = await setup(page, route => route.fulfill({ contentType: "text/event-stream", body: finalFrame }));
  for (const suffix of ["", [255], textFrame(2, "status", { status: "succeeded", terminal: true }) + textFrame(3, "delta", { text: "invalid late" }), textFrame(2, "delta", { text: "foreign" }, "foreign")]) {
    await live(page); await ui.history.getByRole("button", { name: "观察临时文本", exact: true }).click();
    await expect(ui.history.getByRole("region", { name: "模型临时文本", exact: true })).toContainText("私有临时正文");
    await finishLive(page, suffix);
    await expect(ui.history).toContainText("观察已中断");
    await expect(ui.history.locator("pre")).toHaveCount(0);
    expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
    expect(await page.evaluate(() => window.__learningText.calls)).toBe(1);
  }
});
test("disabled text observation preserves the independent status observation", async ({ page }) => {
  const ui = await setup(page, route => route.fulfill({ contentType: "text/event-stream", body: finalFrame }));
  await live(page, "", 503);
  await ui.history.getByRole("button", { name: "观察临时文本", exact: true }).click();
  await expect(ui.history).toContainText("临时文本当前不可用"); expect(ui.state.reads).toBe(1);
  await ui.start.click(); await expect.poll(() => ui.state.reads).toBe(2);
  expect(ui.state.events).toBe(1); expect(ui.state.posts).toBe(0);
});

test("a terminal candidate clears text but cannot query advice before clean EOF", async ({ page }) => {
  const ui = await textUI(page);
  await finishLive(page, textFrame(2, "status", { status: "succeeded", terminal: true }), false);
  await expect(ui.region.locator("pre")).toHaveCount(0);
  await expect(ui.history).toContainText("核验建议已保存");
  expect(ui.state.reads).toBe(1);
  await finishLive(page, textFrame(3, "delta", { text: "late protocol failure" }));
  await expect(ui.history).toContainText("观察已中断");
  await expect(ui.history.locator("pre")).toHaveCount(0);
  expect(ui.state.reads).toBe(1); expect(ui.state.posts).toBe(0);
});
