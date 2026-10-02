import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

const panel = page => page.getByRole("region", { name: "外部搜索", exact: true });
const submit = page => panel(page).getByRole("button", { name: "搜索公开资料", exact: true });
const result = () => ({ tool: "web_search", output: { provider: "searxng", untrusted: true, partial: true, results_truncated: true, results: [{ url: "https://example.org/source", title: "<b>外部资料</b>", snippet: "<img src=x onerror=alert(1)> 仅作为文字", text_truncated: true }] } });
async function enabled(page) {
  await page.route("**/api/tools", route => route.fulfill({ json: { tools: [{ name: "web_search" }] } }));
}
async function start(page, account) {
  await page.goto("/"); await login(page, account ?? await createAccount());
}
async function consent(page, query = "私有查询🙂") {
  await panel(page).getByLabel("搜索内容（1–500 个字符）").fill(query);
  await panel(page).getByRole("checkbox").check();
}

test("search is disabled by default and manifest failures never send a query", async ({ page }) => {
  let calls = 0;
  await page.route("**/api/tools/web_search", route => { calls++; return route.fulfill({ json: result() }); });
  await start(page);
  await expect(panel(page)).toContainText("管理员尚未启用外部搜索");
  await expect(submit(page)).toHaveCount(0);
  await page.route("**/api/tools", route => route.fulfill({ status: 503, json: {} }));
  await panel(page).getByRole("button", { name: "刷新搜索状态" }).click();
  await expect(panel(page).getByRole("alert")).toBeVisible();
  await enabled(page);
  await panel(page).getByRole("button", { name: "刷新搜索状态" }).click();
  await expect(submit(page)).toBeDisabled();
  expect(calls).toBe(0);
});

test("search requires fresh consent and renders bounded results as text on both gateways", async ({ page }, testInfo) => {
  await enabled(page);
  const requests = [];
  await page.route("**/api/tools/web_search", route => {
    requests.push({ body: route.request().postDataJSON(), headers: route.request().headers() });
    return route.fulfill({ json: result() });
  });
  await start(page);
  await consent(page);
  await panel(page).getByLabel("搜索内容（1–500 个字符）").fill("修改后的查询🙂");
  await expect(panel(page).getByRole("checkbox")).not.toBeChecked();
  await expect(submit(page)).toBeDisabled();
  await panel(page).getByRole("checkbox").check();
  await submit(page).click();
  await expect(panel(page)).toContainText("部分搜索引擎未响应");
  await expect(panel(page)).toContainText("仅展示前 5 条结果");
  await expect(panel(page)).toContainText("标题或摘要已截断");
  const link = panel(page).getByRole("link", { name: "<b>外部资料</b>", exact: true });
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  await expect(link).toHaveAttribute("referrerpolicy", "no-referrer");
  await expect(panel(page).locator("img, script")).toHaveCount(0);
  await expect(submit(page)).toBeDisabled();
  expect(requests).toHaveLength(1);
  expect(requests[0].body).toEqual({ query: "修改后的查询🙂", limit: 5, acknowledge_external_request: true });
  expect(requests[0].headers["idempotency-key"]).toMatch(/^[0-9a-f-]{36}$/);
  expect(requests[0].headers["x-requested-with"]).toBe("personal-ai");
  expect(await panel(page).evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("web-search.png");
  await panel(page).screenshot({ path: screenshot });
  await testInfo.attach("web-search", { path: screenshot, contentType: "image/png" });
  await panel(page).getByRole("checkbox").check(); await submit(page).click();
  await expect(panel(page).getByRole("region", { name: "外部搜索结果" })).toBeVisible();
  expect(requests).toHaveLength(2);
  expect(requests[0].headers["idempotency-key"]).not.toBe(requests[1].headers["idempotency-key"]);
});

test("invalid links and unknown responses are failures, empty results remain distinct", async ({ page }) => {
  await enabled(page);
  let reply = result(); reply.output.results[0].url = "javascript:alert(1)";
  await page.route("**/api/tools/web_search", route => route.fulfill({ json: reply }));
  await start(page); await consent(page); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("无效搜索结果");
  await expect(panel(page).getByRole("link")).toHaveCount(0);
  reply = { ...result(), output: { ...result().output, partial: false, results_truncated: false, results: [] } };
  await panel(page).getByRole("checkbox").check(); await submit(page).click();
  await expect(panel(page)).toContainText("本次未返回匹配结果");
  await page.route("**/api/tools/web_search", route => route.fulfill({ status: 429, json: {} }));
  await panel(page).getByRole("checkbox").check(); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("额度已用完或服务繁忙");
  await expect(panel(page).getByRole("region", { name: "外部搜索结果" })).toHaveCount(0);
});

test("lost responses never retry and expired sessions erase search data", async ({ page }) => {
  await enabled(page); let calls = 0;
  await page.route("**/api/tools/web_search", route => { calls++; return route.abort("failed"); });
  await start(page); await consent(page); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("结果未确认");
  await expect(submit(page)).toBeDisabled(); expect(calls).toBe(1);
  await page.route("**/api/tools/web_search", route => route.fulfill({ json: result() }));
  await panel(page).getByRole("checkbox").check(); await submit(page).click();
  await expect(panel(page)).toContainText("<b>外部资料</b>");
  await page.route("**/api/tools", route => route.fulfill({ status: 401, json: {} }));
  await panel(page).getByRole("button", { name: "刷新搜索状态" }).click();
  await expect(panel(page).getByRole("alert")).toContainText("登录已失效");
  await expect(panel(page)).not.toContainText("私有查询🙂");
  await expect(panel(page).getByRole("link")).toHaveCount(0);
  await expect(panel(page).getByRole("button", { name: "刷新搜索状态" })).toBeDisabled();
});

for (const stopFirst of [true, false]) {
test(`switching accounts discards late search responses (stop first: ${stopFirst})`, async ({ page }) => {
  await enabled(page); const first = await createAccount(), second = await createAccount();
  let release, received;
  const wait = new Promise(resolve => { release = resolve; });
  const arrival = new Promise(resolve => { received = resolve; });
  await page.route("**/api/tools/web_search", async route => {
    received(); await wait;
    try { await route.fulfill({ json: result() }); } catch { /* disposed request */ }
  });
  await start(page, first); await consent(page); await submit(page).click(); await arrival;
  if (stopFirst) {
    await panel(page).getByRole("button", { name: "停止等待搜索" }).click();
    await expect(panel(page)).toContainText("服务端可能仍在执行并计费");
    await expect(submit(page)).toBeDisabled();
  }
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, second); release();
  await expect(submit(page)).toBeDisabled();
  await expect(panel(page).getByLabel("搜索内容（1–500 个字符）")).toHaveValue("");
  await expect(panel(page).getByRole("region", { name: "外部搜索结果" })).toHaveCount(0);
  await expect(panel(page)).not.toContainText("本次请求 ID");
});
}

test("validation prevents requests and conflicts never imply recovered results", async ({ page }) => {
  await enabled(page); let calls = 0, status = 409;
  await page.route("**/api/tools/web_search", route => { calls++; return route.fulfill({ status, json: {} }); });
  await start(page); await consent(page, "文".repeat(501)); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("查询格式无效"); expect(calls).toBe(0);
  await consent(page); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("不能恢复结果正文");
  expect(calls).toBe(1); await expect(submit(page)).toBeDisabled();
  status = 404; await panel(page).getByRole("checkbox").check(); await submit(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("尚未启用外部搜索");
  await expect(submit(page)).toHaveCount(0); expect(calls).toBe(2);
});
