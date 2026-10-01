import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

test("daily brief schedule opt-in persists, ambiguous saves are inspected and disabling survives reload", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/"); await login(page, owner);
  const panel = page.getByRole("region", { name: "定时日报", exact: true });
  await expect(panel).toContainText("当前状态：已停用");
  await panel.getByLabel("每日生成时间（UTC）").fill("18:30");
  await panel.getByLabel("每天自动生成日报").check();
  let writes = 0;
  await page.route("**/api/feed-brief-schedule", async route => {
    if (route.request().method() !== "PUT") return route.continue();
    writes += 1;
    const response = await route.fetch(); expect(response.status()).toBe(200);
    return writes === 1 ? route.abort("failed") : route.fulfill({ response });
  });
  await panel.getByRole("button", { name: "确认启用定时日报" }).click();
  await expect(panel).toContainText("保存结果尚未确认");
  await expect(panel.getByRole("button", { name: "确认启用定时日报" })).toBeDisabled();
  await panel.getByRole("button", { name: "刷新定时设置" }).click();
  await expect(panel).toContainText("当前状态：已启用");
  expect(writes).toBe(1);
  await page.reload();
  await expect(panel).toContainText("18:30 UTC");
  await expect(panel.getByLabel("每天自动生成日报")).toBeChecked();
  const screenshot = testInfo.outputPath("brief-schedule.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("brief-schedule", { path: screenshot, contentType: "image/png" });
  await panel.getByLabel("每天自动生成日报").uncheck();
  await panel.getByRole("button", { name: "保存停用设置" }).click();
  await expect(panel).toContainText("当前状态：已停用");
  await page.reload(); await expect(panel).toContainText("当前状态：已停用");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await expect(panel).toContainText("09:00 UTC");
  await expect(panel).toContainText("当前状态：已停用");
  await page.route("**/api/feed-brief-schedule", route => route.fulfill({ status: 401, json: {} }));
  await panel.getByRole("button", { name: "刷新定时设置" }).click();
  await expect(panel).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Daily Brief 日报", exact: true })).toContainText("登录已失效");
});

test("late brief schedule reads cannot replace another account settings", async ({ page }) => {
  const first = await createAccount(); const second = await createAccount();
  let release; let received;
  const held = new Promise(resolve => { release = resolve; });
  const started = new Promise(resolve => { received = resolve; });
  let reads = 0;
  await page.route("**/api/feed-brief-schedule", async route => {
    if (++reads === 1) { received(); await held; return route.fulfill({ json: { revision: "1", enabled: true, minute_utc: 120, next_run_unix_ms: "1800000000000", last_attempt_unix_ms: null, last_request_id: null, last_outcome: null } }).catch(() => {}); }
    return route.continue();
  });
  await page.goto("/"); await login(page, first); await started;
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, second); release();
  const panel = page.getByRole("region", { name: "定时日报", exact: true });
  await expect(panel).toContainText("当前状态：已停用");
  await expect(panel.getByLabel("每日生成时间（UTC）")).toHaveValue("09:00");
  await expect(panel).not.toContainText("02:00 UTC");
});
