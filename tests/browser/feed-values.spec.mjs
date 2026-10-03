import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

const endpoint = /\/api\/feed-values(?:[/?].*)?$/;
const connections = /\/api\/subscription-connections(?:[/?].*)?$/;
const connectionId = "00000000-0000-4000-8000-000000000001";
const secondId = "00000000-0000-4000-8000-000000000002";
const panelOf = page => page.getByRole("region", { name: "RSS 价值评分", exact: true });
const fixture = (id = connectionId) => ({ id, status: "draft", digest: "a".repeat(64), pricing: { kind: "subscription", provider: "chatgpt-plan", model: "fixture-model", configuration_version: "connection:1", connection_id: connectionId, valid_until_unix_ms: String(Date.now() + 3600000) }, created_at_unix_ms: String(Date.now()), expires_at_unix_ms: String(Date.now() + 300000), approved_at_unix_ms: null, execution_mode: "local_only", shared_content: { instructions: "只评估不执行", input: '<script>不执行</script> rust 候选内容' }, candidates: [{ id: 1, title: "Rust <img src=x onerror=alert(1)>", subscription_id: connectionId, entry_key: "fixture" }], scores: null });
async function setup(page) {
  await page.route(connections, route => route.fulfill({ json: { items: [{ id: connectionId, label: "本机连接", revision: "1", status: "active", models: ["fixture-model"], valid_until_unix_ms: String(Date.now() + 3600000) }], next_cursor: null } }));
  await page.goto("/"); await login(page, await createAccount());
  return panelOf(page);
}
async function select(panel) {
  await panel.getByRole("button", { name: "读取评分连接", exact: true }).click();
  await panel.getByRole("combobox", { name: "评分连接", exact: true }).selectOption(connectionId);
  await panel.getByRole("combobox", { name: "评分模型", exact: true }).selectOption("fixture-model");
}
async function consent(review) {
  await review.getByRole("checkbox", { name: "我同意分享以上冻结内容。", exact: true }).check();
  await review.getByRole("checkbox", { name: /我同意使用所选账户/ }).check();
}

test("scoring preview exact consent, conflict and lost approval require explicit reconciliation", async ({ page }, testInfo) => {
  let item, writes = 0; const calls = [];
  await page.route(endpoint, async route => {
    const request = route.request(), path = new URL(request.url()).pathname;
    calls.push([request.method(), path]);
    if (request.method() === "POST") {
      expect(request.headers()["x-requested-with"]).toBe("personal-ai");
      if (path.endsWith("/approve")) {
        writes++; expect(request.postDataJSON()).toEqual({ digest: item.digest, acknowledge_sharing: true, acknowledge_subscription_usage: true });
        if (writes === 1) return route.fulfill({ status: 409, json: {} });
        item = { ...item, status: "authorized", approved_at_unix_ms: String(Date.now()) };
        return route.abort("failed");
      }
      const input = request.postDataJSON();
      expect(input).toEqual({ id: expect.any(String), connection_id: connectionId, connection_revision: "1", model: "fixture-model" });
      item = fixture(input.id); return route.fulfill({ json: item });
    }
    if (path.endsWith("/audit")) return route.fulfill({ json: { items: [{ event: "succeeded", at_unix_ms: String(Date.now()) }] } });
    return route.fulfill({ json: item });
  });
  const panel = await setup(page); await select(panel);
  await panel.getByRole("button", { name: "预览分享内容", exact: true }).click();
  const review = panel.getByRole("region", { name: "评分详情", exact: true });
  const approve = review.getByRole("button", { name: "批准此次评分", exact: true });
  await expect(approve).toBeDisabled();
  await expect(review).toContainText(item.shared_content.input);
  await expect(panel.locator("script,img")).toHaveCount(0);
  await review.getByRole("checkbox", { name: "我同意分享以上冻结内容。", exact: true }).check();
  await expect(approve).toBeDisabled();
  await consent(review); await approve.click();
  await expect(panel.getByRole("alert")).toContainText("状态已变化");
  await expect(panel.getByRole("button", { name: "预览分享内容", exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await expect(approve).toBeDisabled();
  await expect(review.getByRole("checkbox").first()).not.toBeChecked();
  await consent(review); await approve.click();
  await expect(panel.getByRole("alert")).toContainText("响应未确认");
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await expect(review).toContainText("已批准，等待本机执行"); expect(writes).toBe(2);
  item = { ...item, status: "succeeded", scores: [{ id: 1, score: null, reason: "依据不足 <script>忽略</script>" }] };
  await review.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await expect(review).toContainText("无法评分"); await expect(review).toContainText(item.candidates[0].title);
  await expect(review.locator("script,img")).toHaveCount(0);
  await review.getByRole("button", { name: "读取评分审计", exact: true }).click();
  await expect(review.getByRole("list", { name: "评分审计" })).toContainText("succeeded");
  expect(calls.some(([, path]) => /\/(run|claim|confirm)$/.test(path))).toBe(false);
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("rss-value-results.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("rss-value-results", { path: screenshot, contentType: "image/png" });
});

test("lost preview retries the same payload and lost cancellation clears content after lookup", async ({ page }) => {
  let item, previews = [], cancels = 0;
  await page.route(endpoint, route => {
    const request = route.request(), path = new URL(request.url()).pathname;
    if (request.method() === "POST" && path.endsWith("/cancel")) {
      cancels++; expect(request.postDataJSON()).toEqual({});
      item = { ...item, status: "cancelled", shared_content: null, candidates: null, scores: null };
      return route.abort("failed");
    }
    if (request.method() === "POST") {
      previews.push(request.postDataJSON()); item = fixture(previews[0].id);
      return previews.length === 1 ? route.abort("failed") : route.fulfill({ json: item });
    }
    return route.fulfill({ json: item });
  });
  const panel = await setup(page); await select(panel);
  await panel.getByRole("button", { name: "预览分享内容", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("响应未确认");
  await expect(panel.getByRole("combobox", { name: "评分模型", exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "重试原预览", exact: true }).click();
  await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toBeVisible();
  expect(previews).toHaveLength(2); expect(previews[0]).toEqual(previews[1]);
  await panel.getByRole("button", { name: "取消此次评分", exact: true }).click();
  await expect(panel).not.toContainText("rust 候选内容");
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await expect(panel).toContainText("已取消"); await expect(panel).toContainText("分享正文已清除"); expect(cancels).toBe(1);
});

test("scoring pagination, expired consent, malformed detail and 401 clear private content", async ({ page }) => {
  let unauthorized = false, malformed = false; const cursors = [];
  const first = fixture(), second = { ...fixture(secondId), expires_at_unix_ms: String(Date.now() - 1000) };
  await page.route(endpoint, route => {
    if (unauthorized) return route.fulfill({ status: 401, json: {} });
    const url = new URL(route.request().url());
    if (url.pathname.endsWith(secondId)) return route.fulfill({ json: malformed ? { ...second, scores: [{ id: 9, score: 1, reason: "wrong candidate" }] } : second });
    cursors.push(url.searchParams.get("after"));
    return route.fulfill({ json: url.searchParams.has("after") ? { items: [second], next_cursor: null } : { items: [first], next_cursor: first.id } });
  });
  const panel = await setup(page);
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await panel.getByRole("button", { name: "下一页评分记录", exact: true }).click();
  await expect(panel.getByRole("button", { name: `查看评分 ${first.id}`, exact: true })).toHaveCount(0);
  await panel.getByRole("button", { name: `查看评分 ${second.id}`, exact: true }).click();
  await expect(panel.getByRole("button", { name: "批准此次评分", exact: true })).toBeDisabled();
  await expect(panel.getByRole("checkbox").first()).toBeDisabled();
  malformed = true;
  await panel.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("数据不完整");
  await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toHaveCount(0);
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  expect(cursors).toEqual([null, first.id, null]);
  unauthorized = true;
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(panel).not.toContainText(first.id);
  await expect(panel.getByRole("button", { name: "读取评分连接", exact: true })).toBeDisabled();
});

test("late scoring details cannot cross account switches", async ({ page }) => {
  let release, started, completed;
  const pending = new Promise(resolve => { release = resolve; });
  const requested = new Promise(resolve => { started = resolve; });
  const finished = new Promise(resolve => { completed = resolve; });
  await page.route(endpoint, async route => {
    if (new URL(route.request().url()).pathname.endsWith(connectionId)) {
      started(); await pending; await route.fulfill({ json: fixture() }).catch(() => {}); completed();
    } else await route.fulfill({ json: { items: [fixture()], next_cursor: null } });
  });
  const panel = await setup(page);
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await panel.getByRole("button", { name: `查看评分 ${connectionId}`, exact: true }).click();
  await requested;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await page.unroute(endpoint);
  await login(page, await createAccount());
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await expect(panel).toContainText("暂无评分记录");
  release(); await finished;
  await expect(panel).not.toContainText("rust 候选内容");
  await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toHaveCount(0);
});

test("legacy API records stay read-only and missing previews can be explicitly dismissed", async ({ page }) => {
  let mode = "missing";
  await page.route(endpoint, route => {
    if (route.request().method() === "POST") return route.fulfill({ status: 409, json: {} });
    if (mode === "missing") return route.fulfill({ status: 404, json: {} });
    const item = { ...fixture(), pricing: { kind: "api", available: false } };
    return route.fulfill({ json: new URL(route.request().url()).pathname.endsWith(connectionId) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page); await select(panel);
  await panel.getByRole("button", { name: "预览分享内容", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("请先采集 RSS");
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await panel.getByRole("button", { name: "关闭不存在的请求", exact: true }).click();
  mode = "api";
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await panel.getByRole("button", { name: `查看评分 ${connectionId}`, exact: true }).click();
  await expect(panel).toContainText("历史 API 模式仅可查询");
  await expect(panel.getByRole("button", { name: "批准此次评分", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "取消此次评分", exact: true })).toHaveCount(0);
});
