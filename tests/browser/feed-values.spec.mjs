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

function completedFixture() {
  const item = fixture();
  item.status = "succeeded";
  item.approved_at_unix_ms = String(Date.now());
  item.candidates = [1, 2, 3].map(id => ({ id, title: `Rust ${id} <script>纯文本</script>`, subscription_id: connectionId, entry_key: `fixture-${id}` }));
  item.scores = [{ id: 2, score: 90, reason: "相关" }, { id: 1, score: 0, reason: "较少相关" }, { id: 3, score: null, reason: "证据不足" }];
  return item;
}
function readingFixture(item) {
  return { id: item.id, digest: item.digest, status: "succeeded", day_start_unix_ms: String(Date.now()), as_of_unix_ms: String(Date.now()), keywords: ["rust"], items: item.scores.map(s => ({ id: s.id, title: item.candidates[s.id - 1].title, summary: `冻结摘要 ${s.id} <img src=x onerror=alert(1)>`, link: s.id === 2 ? "https://example.com/article" : s.id === 1 ? "javascript:alert(1)" : "https://user:secret@example.com/private", rule_score: 52, model_score: s.score, reason: s.reason })) };
}
async function selectCompleted(panel, item) {
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await panel.getByRole("button", { name: `查看评分 ${item.id}`, exact: true }).click();
}

test("reading switches model and rule order without new requests and opens only safe original links", async ({ page }, testInfo) => {
  const item = completedFixture(); let reads = 0;
  await page.route(endpoint, route => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/reading")) { reads++; return route.fulfill({ json: readingFixture(item) }); }
    return route.fulfill({ json: path.endsWith(item.id) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page); await selectCompleted(panel, item); expect(reads).toBe(0);
  await panel.getByRole("button", { name: "读取评分阅读", exact: true }).click();
  const reading = panel.getByRole("region", { name: "评分阅读视图", exact: true });
  const entries = reading.getByRole("list", { name: "评分阅读条目" }).getByRole("listitem");
  await expect(entries).toHaveCount(3);
  await expect(entries.first()).toContainText("Rust 2");
  await expect(entries.nth(1)).toContainText("0 / 100");
  await expect(entries.last()).toContainText("无法评分");
  await expect(reading.locator("script,img")).toHaveCount(0);
  const links = reading.getByRole("link", { name: "打开原文", exact: true });
  await expect(links).toHaveCount(1); await expect(links).toHaveAttribute("href", "https://example.com/article");
  await expect(links).toHaveAttribute("rel", "noopener noreferrer"); await expect(links).toHaveAttribute("referrerpolicy", "no-referrer");
  await reading.getByRole("combobox", { name: "阅读排序", exact: true }).selectOption("rule");
  await expect(entries.first()).toContainText("Rust 1"); expect(reads).toBe(1);
  await reading.getByRole("combobox", { name: "阅读排序", exact: true }).selectOption("model");
  await expect(entries.first()).toContainText("Rust 2"); expect(reads).toBe(1);
  expect(await reading.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("rss-value-reading.png");
  await reading.screenshot({ path: screenshot }); await testInfo.attach("rss-value-reading", { path: screenshot, contentType: "image/png" });
});

test("reading rejects mismatched snapshots and clears content when a result becomes invalid or session expires", async ({ page }) => {
  const item = completedFixture(); let mode = "valid";
  await page.route(endpoint, route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/reading")) {
      if (mode === "invalid") return route.fulfill({ status: 409, json: {} });
      if (mode === "expired") return route.fulfill({ status: 401, json: {} });
      return route.fulfill({ json: { ...readingFixture(item), ...(mode === "wrong" ? { digest: "b".repeat(64) } : {}) } });
    }
    return route.fulfill({ json: path.endsWith(item.id) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page);
  for (const failure of ["wrong", "invalid", "expired"]) {
    mode = "valid"; await selectCompleted(panel, item);
    await panel.getByRole("button", { name: "读取评分阅读", exact: true }).click();
    await expect(panel).toContainText("冻结摘要");
    mode = failure;
    await panel.getByRole("button", { name: "读取评分阅读", exact: true }).click();
    await expect(panel.getByRole("alert")).toBeVisible();
    await expect(panel).not.toContainText("冻结摘要");
    await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toHaveCount(0);
  }
  await expect(panel).not.toContainText(item.id);
  await expect(panel.getByRole("button", { name: "刷新评分记录", exact: true })).toBeDisabled();
});

test("preference changes cancel a pending reading and discard its late response", async ({ page }) => {
  const item = completedFixture(); let release, started, completed;
  const pending = new Promise(resolve => { release = resolve; });
  const requested = new Promise(resolve => { started = resolve; });
  const finished = new Promise(resolve => { completed = resolve; });
  await page.route(endpoint, async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/reading")) {
      started(); await pending; await route.fulfill({ json: readingFixture(item) }).catch(() => {}); completed();
    } else await route.fulfill({ json: path.endsWith(item.id) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page); await selectCompleted(panel, item);
  await panel.getByRole("button", { name: "读取评分阅读", exact: true }).click(); await requested;
  await page.getByLabel("日报关键词（每行一个，最多 5 个）", { exact: true }).fill("rust");
  await page.getByRole("button", { name: "保存日报偏好", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("来源、偏好或连接正在变更");
  release(); await finished;
  await expect(panel).not.toContainText("冻结摘要");
  await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toHaveCount(0);
});

test("late reading responses cannot enter a different signed-in account", async ({ page }) => {
  const item = completedFixture(); let release, started, completed;
  const pending = new Promise(resolve => { release = resolve; });
  const requested = new Promise(resolve => { started = resolve; });
  const finished = new Promise(resolve => { completed = resolve; });
  await page.route(endpoint, async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/reading")) {
      started(); await pending; await route.fulfill({ json: readingFixture(item) }).catch(() => {}); completed();
    } else await route.fulfill({ json: path.endsWith(item.id) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page); await selectCompleted(panel, item);
  await panel.getByRole("button", { name: "读取评分阅读", exact: true }).click(); await requested;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await page.unroute(endpoint); await login(page, await createAccount());
  release(); await finished;
  await expect(panel.getByRole("region", { name: "评分阅读视图", exact: true })).toHaveCount(0);
  await expect(panel).not.toContainText("冻结摘要");
});

function localFixture(id = connectionId) {
  return { ...fixture(id), pricing: { kind: "local", profile: "local-rss-v4", endpoint: "http://127.0.0.1:11435", model: "qwen3:4b-q4_K_M", valid_until_unix_ms: String(Date.now() + 300000) } };
}
async function selectLocal(panel) {
  await panel.getByRole("combobox", { name: "评分方式", exact: true }).selectOption("local");
  await panel.getByRole("button", { name: "读取本地评分配置", exact: true }).click();
}
test("local scoring is disabled until explicit configuration read and rejects remote or cloud targets", async ({ page }) => {
  let enabled = false, writes = 0, configs = 0;
  await page.route(endpoint, route => {
    if (route.request().method() === "POST") { writes++; return route.fulfill({ status: 503, json: {} }); }
    configs++; return route.fulfill({ json: { local_enabled: enabled, execution_mode: "local_only" } });
  });
  const panel = await setup(page); expect(configs).toBe(0);
  await selectLocal(panel); await expect(panel).toContainText("本地评分未启用");
  const preview = panel.getByRole("button", { name: "预览本地分享内容", exact: true });
  await expect(preview).toBeDisabled(); enabled = true;
  await panel.getByRole("button", { name: "读取本地评分配置", exact: true }).click();
  await expect(preview).toBeEnabled();
  for (const address of ["https://127.0.0.1:11435", "http://localhost:11435", "http://127.0.0.1:11435/", "http://127.0.0.1:00080", "http://192.168.1.1:11435", "http://127.0.0.1:65536"]) {
    await panel.getByLabel("本地评分地址", { exact: true }).fill(address); await expect(preview).toBeDisabled();
  }
  await panel.getByLabel("本地评分地址", { exact: true }).fill("http://127.0.0.1:80"); await expect(preview).toBeEnabled();
  await panel.getByLabel("本地评分模型", { exact: true }).fill("qwen3:4b-cloud"); await expect(preview).toBeDisabled();
  expect(writes).toBe(0);
});
test("local lost preview preserves original target and approval needs separate sharing and compute consent", async ({ page }, testInfo) => {
  let item, approvals = 0; const previews = [], writes = [];
  await page.route(endpoint, route => {
    const req = route.request(), path = new URL(req.url()).pathname;
    if (path.endsWith("/config")) return route.fulfill({ json: { local_enabled: true, execution_mode: "local_only" } });
    if (req.method() === "POST") {
      expect(req.headers()["x-requested-with"]).toBe("personal-ai"); writes.push(path);
      if (path.endsWith("/approve-local")) {
        approvals++; expect(req.postDataJSON()).toEqual({ digest: item.digest, acknowledge_sharing: true, acknowledge_local_compute: true });
        item = { ...item, status: "authorized", approved_at_unix_ms: String(Date.now()) }; return route.abort("failed");
      }
      expect(path).toBe("/api/feed-values/local"); previews.push(req.postDataJSON());
      expect(previews.at(-1)).toEqual({ id: expect.any(String), endpoint: "http://127.0.0.1:11435", model: "qwen3:4b-q4_K_M" });
      item = localFixture(previews[0].id); return previews.length === 1 ? route.abort("failed") : route.fulfill({ json: item });
    }
    if (path.endsWith("/reading")) return route.fulfill({ json: readingFixture(item) });
    return route.fulfill({ json: item });
  });
  const panel = await setup(page); await selectLocal(panel);
  await panel.getByRole("button", { name: "预览本地分享内容", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("响应未确认");
  await expect(panel.getByLabel("本地评分地址", { exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "重试原预览", exact: true }).click();
  const review = panel.getByRole("region", { name: "评分详情", exact: true });
  const approve = review.getByRole("button", { name: "批准此次评分", exact: true });
  await expect(review).toContainText("本地分类采用固定档位：相关 80、部分相关 40、无关 0");
  await expect(review).toContainText(item.shared_content.input); await expect(review.locator("script,img")).toHaveCount(0);
  expect(previews).toHaveLength(2); expect(previews[0]).toEqual(previews[1]);
  await expect(approve).toBeDisabled();
  await review.getByRole("checkbox", { name: "我同意分享以上冻结内容。", exact: true }).check(); await expect(approve).toBeDisabled();
  await review.getByRole("checkbox", { name: "我同意使用本机计算资源，并已核对游戏显存预算。", exact: true }).check();
  const screenshot = testInfo.outputPath("rss-local-consent.png"); await panel.screenshot({ path: screenshot });
  await testInfo.attach("rss-local-consent", { path: screenshot, contentType: "image/png" });
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  await approve.click(); await expect(panel.getByRole("alert")).toContainText("响应未确认");
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await expect(review).toContainText(/make local-value OWNER=[a-f0-9-]{36} REQUEST=[a-f0-9-]{36}/); expect(approvals).toBe(1);
  item = { ...completedFixture(), id: item.id, pricing: item.pricing };
  await review.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await review.getByRole("button", { name: "读取评分阅读", exact: true }).click();
  await expect(panel.getByRole("region", { name: "评分阅读视图", exact: true })).toContainText("无法评分");
  expect(writes.some(path => /\/(run|claim|confirm|approve)$/.test(path))).toBe(false);
});
test("local cancellation clears content and malformed local receipts cannot authorize", async ({ page }) => {
  let item = localFixture(), malformed = false, cancelled = 0;
  await page.route(endpoint, route => {
    const req = route.request(), path = new URL(req.url()).pathname;
    if (path.endsWith("/config")) return route.fulfill({ json: { local_enabled: true, execution_mode: "local_only" } });
    if (path.endsWith("/cancel")) { cancelled++; item = { ...item, status: "cancelled", scores: null, shared_content: null, candidates: null }; return route.fulfill({ json: item }); }
    if (req.method() === "POST") { item = localFixture(req.postDataJSON().id); return route.fulfill({ json: item }); }
    if (path.endsWith(item.id)) return route.fulfill({ json: malformed ? { ...item, pricing: { ...item.pricing, endpoint: "https://remote.example" } } : item });
    return route.fulfill({ json: { items: [item], next_cursor: null } });
  });
  const panel = await setup(page); await selectLocal(panel);
  await panel.getByRole("button", { name: "预览本地分享内容", exact: true }).click();
  await panel.getByRole("button", { name: "取消此次评分", exact: true }).click();
  await expect(panel).toContainText("已取消"); await expect(panel).not.toContainText("rust 候选内容"); expect(cancelled).toBe(1);
  malformed = true; await panel.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("数据不完整");
  await expect(panel.getByRole("region", { name: "评分详情", exact: true })).toHaveCount(0);
});

test("real private local configuration and empty preview traverse both application proxies", async ({ page }) => {
  const panel = await setup(page); await selectLocal(panel);
  await expect(panel).toContainText("本地评分已启用");
  const pendingResponse = page.waitForResponse(response => new URL(response.url()).pathname === "/api/feed-values/local" && response.request().method() === "POST");
  await panel.getByRole("button", { name: "预览本地分享内容", exact: true }).click();
  expect((await pendingResponse).status()).toBe(400); // Missing keywords/candidates is input validation, before draft persistence.
  await expect(panel.getByRole("alert")).toContainText("输入无效");
  await panel.getByRole("button", { name: "核对原评分请求", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("原请求不存在");
  await panel.getByRole("button", { name: "关闭不存在的请求", exact: true }).click();
  await expect(panel.getByRole("button", { name: "预览本地分享内容", exact: true })).toBeEnabled();
  const status = await page.evaluate(async () => {
    const response = await fetch(`/api/feed-values/${crypto.randomUUID()}/approve-local`, { method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify({ digest: "a".repeat(64), acknowledge_sharing: true, acknowledge_local_compute: true }) });
    await response.body?.cancel(); return response.status;
  });
  expect(status).toBe(404);
});

test("historical local scoring remains readable but cannot approve or dispatch a new profile", async ({ page }) => {
  let item = localFixture(); item.pricing.profile = "local-rss-v2";
  await page.route(endpoint, route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/config")) return route.fulfill({ json: { local_enabled: true, execution_mode: "local_only" } });
    if (path === "/api/feed-values") return route.fulfill({ json: { items: [item], next_cursor: null } });
    return route.fulfill({ json: item });
  });
  const panel = await setup(page); await selectLocal(panel);
  await panel.getByRole("button", { name: "刷新评分记录", exact: true }).click();
  await panel.getByRole("button", { name: `查看评分 ${item.id}`, exact: true }).click();
  const review = panel.getByRole("region", { name: "评分详情", exact: true });
  await expect(review).toContainText("此记录使用旧评分版本");
  await expect(review.getByRole("button", { name: "批准此次评分", exact: true })).toBeDisabled();
  item = { ...item, status: "authorized", approved_at_unix_ms: String(Date.now()) };
  await review.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await expect(review).not.toContainText("make local-value");
  item = { ...completedFixture(), id: item.id, pricing: item.pricing };
  await review.getByRole("button", { name: "核对评分状态", exact: true }).click();
  await expect(review).toContainText("模型参考评分");
  await expect(review).not.toContainText("本地分类采用固定档位");
});
