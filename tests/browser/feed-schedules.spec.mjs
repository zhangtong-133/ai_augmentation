import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

const panelOf = page => page.getByRole("region", { name: "周期 RSS", exact: true });
async function setup(page) {
  await page.goto("/"); await login(page, await createAccount());
  const feeds = page.getByRole("region", { name: "RSS 订阅", exact: true });
  await feeds.getByLabel("订阅名称", { exact: true }).fill("周期测试 <script>不执行</script>");
  await feeds.getByLabel("RSS 来源地址").fill("https://example.com/rss");
  await feeds.getByRole("button", { name: "添加订阅", exact: true }).click();
  await expect(feeds).toContainText("版本 1");
  const panel = panelOf(page);
  await panel.getByRole("button", { name: "刷新周期 RSS", exact: true }).click();
  await panel.getByLabel("周期订阅", { exact: true }).selectOption({ label: "周期测试 <script>不执行</script>" });
  const values = await page.evaluate(() => {
    const local = ms => { const d = new Date(ms); return new Date(ms - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16); };
    return [local(Date.now() + 900000), local(Date.now() + 8100000)];
  });
  await panel.getByLabel("开始时间（本地时区）", { exact: true }).fill(values[0]);
  await panel.getByLabel("结束时间（本地时区）", { exact: true }).fill(values[1]);
  await panel.getByLabel("采集间隔", { exact: true }).selectOption("1");
  return panel;
}

test("periodic RSS exact consent persists, cancellation and account isolation", async ({ page }, testInfo) => {
  const panel = await setup(page);
  await expect(panel).toContainText("本页无法确认后台是否在线");
  await panel.getByRole("button", { name: "预览周期计划", exact: true }).click();
  const review = panel.getByLabel("周期 RSS 授权确认", { exact: true });
  await expect(review).toContainText("最多 2 次");
  await expect(review).toContainText("https://example.com/rss");
  await expect(review.getByRole("button", { name: "同意周期采集", exact: true })).toBeDisabled();
  await expect(panel.locator("script")).toHaveCount(0);
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "查询周期计划", exact: true }).click();
  await expect(review.getByRole("checkbox")).not.toBeChecked();
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "同意周期采集", exact: true }).click();
  await expect(review.getByRole("heading", { name: "已授权", exact: true })).toBeVisible();
  await page.reload();
  await panel.getByRole("button", { name: "审阅周期计划", exact: true }).click();
  await expect(review).toContainText("批准时间");
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("rss-schedule-consent.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("rss-schedule-consent", { path: screenshot, contentType: "image/png" });
  await review.getByRole("button", { name: "取消周期计划", exact: true }).click();
  await expect(review.getByRole("heading", { name: "已取消", exact: true })).toBeVisible();
  await review.getByRole("button", { name: "查询周期计划", exact: true }).click();
  await expect(review.locator("ul")).toContainText("已取消");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await login(page, await createAccount());
  await expect(panel).toContainText("暂无周期计划");
  await expect(panel).not.toContainText("https://example.com/rss");
});

test("periodic lost writes retain IDs and query approval without replay", async ({ page }) => {
  const panel = await setup(page);
  const ids = []; let drop = true, draft;
  await page.route("**/api/feed-subscriptions/*/schedules", async route => {
    const body = route.request().postDataJSON(); ids.push(body.schedule_id);
    expect(typeof body.starts_at_unix_ms).toBe("string");
    const response = await route.fetch(); expect(response.status()).toBe(201); draft = await response.json();
    if (drop) { drop = false; return route.abort("failed"); }
    await route.fulfill({ response });
  });
  await panel.getByRole("button", { name: "预览周期计划", exact: true }).click();
  await panel.getByRole("button", { name: "重试原周期操作", exact: true }).click();
  const review = panel.getByLabel("周期 RSS 授权确认", { exact: true });
  await expect(review).toContainText("待同意");
  expect(ids).toHaveLength(2); expect(ids[0]).toBe(ids[1]);
  let approvals = 0;
  await page.route("**/api/feed-schedules/*/approve", async route => {
    approvals++;
    expect(route.request().headers()["x-requested-with"]).toBe("personal-ai");
    expect(route.request().postDataJSON()).toEqual({ accepted_digest: draft.digest, acknowledge_recurring_source_requests: true });
    const response = await route.fetch(); expect(response.status()).toBe(200);
    await route.abort("failed");
  });
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "同意周期采集", exact: true }).click();
  await expect(panel.getByRole("button", { name: "核对原计划", exact: true })).toBeEnabled();
  await expect(panel.getByRole("button", { name: "重试原周期操作", exact: true })).toHaveCount(0);
  await panel.getByRole("button", { name: "核对原计划", exact: true }).click();
  await expect(review.getByRole("heading", { name: "已授权", exact: true })).toBeVisible();
  expect(approvals).toBe(1);
  await page.route("**/api/feed-schedules", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await panel.getByRole("button", { name: "刷新周期 RSS", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(review).toHaveCount(0);
  await expect(panel).not.toContainText("https://example.com/rss");
  await expect(panel.getByRole("button", { name: "预览周期计划", exact: true })).toBeDisabled();
});

test("periodic stale plans cannot approve and history cursors navigate", async ({ page }) => {
  const panel = await setup(page);
  await panel.getByRole("button", { name: "预览周期计划", exact: true }).click();
  const review = panel.getByLabel("周期 RSS 授权确认", { exact: true });
  await expect(review).toContainText("待同意");
  const feeds = page.getByRole("region", { name: "RSS 订阅", exact: true });
  await feeds.getByRole("button", { name: "编辑", exact: true }).click();
  await feeds.getByLabel("启用订阅", { exact: true }).uncheck();
  await feeds.getByRole("button", { name: "保存订阅", exact: true }).click();
  await expect(feeds).toContainText("版本 2");
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "同意周期采集", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("计划已失效");
  await panel.getByRole("button", { name: "核对原计划", exact: true }).click();
  await expect(review.getByRole("heading", { name: "已取消", exact: true })).toBeVisible();
  // Pagination transport fixture avoids exceeding the real 20/day preview quota.
  let saved;
  await page.route("**/api/feed-schedules?after=*", route => route.fulfill({ json: { items: [{ ...saved, status: "expired" }], next_cursor: null } }));
  await page.route("**/api/feed-schedules", async route => {
    const response = await route.fetch(); const data = await response.json(); saved = data.items[0];
    await route.fulfill({ json: { items: [saved], next_cursor: saved.plan.input.schedule_id } });
  });
  await panel.getByRole("button", { name: "刷新周期 RSS", exact: true }).click();
  await panel.getByRole("button", { name: "下一页周期计划", exact: true }).click();
  await expect(panel.locator(".feedList")).toContainText("已到期");
  await expect(panel.getByRole("button", { name: "下一页周期计划", exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "上一页周期计划", exact: true }).click();
  await expect(panel.locator(".feedList")).toContainText("已取消");
});

test("periodic expired preview disables consent and late responses cannot cross accounts", async ({ page }) => {
  const panel = await setup(page);
  let draft;
  await page.route("**/api/feed-subscriptions/*/schedules", async route => {
    const response = await route.fetch(); draft = await response.json();
    await route.fulfill({ json: { ...draft, plan: { ...draft.plan, approval_expires_at_unix_ms: "1" } } });
  });
  await panel.getByRole("button", { name: "预览周期计划", exact: true }).click();
  const review = panel.getByLabel("周期 RSS 授权确认", { exact: true });
  await expect(review).toContainText("预览已过期");
  await expect(review.getByRole("checkbox")).toBeDisabled();
  await expect(review.getByRole("button", { name: "同意周期采集", exact: true })).toBeDisabled();
  let release, received;
  const waiting = new Promise(resolve => { release = resolve; });
  const submitted = new Promise(resolve => { received = resolve; });
  await page.route(`**/api/feed-schedules/${draft.plan.input.schedule_id}`, async route => {
    const response = await route.fetch(); received();
    await waiting; await route.fulfill({ response }).catch(() => {});
  });
  await review.getByRole("button", { name: "查询周期计划", exact: true }).click();
  await submitted;
  const other = await createAccount();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other); release();
  await expect(panel).toContainText("暂无周期计划");
  await expect(review).toHaveCount(0);
  await expect(panel).not.toContainText("https://example.com/rss");
});
