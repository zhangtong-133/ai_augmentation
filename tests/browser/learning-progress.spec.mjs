import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";
const root = page => page.getByRole("region", { name: "学习管理", exact: true });
const card = page => page.getByRole("region", { name: "学习进度", exact: true });
async function start(page, account) {
  await page.goto("/"); await login(page, account ?? await createAccount());
  await expect(card(page).getByText("当前版本已自评", { exact: true }).locator("..")).toContainText("0 / 0");
}
function fixture() {
  return { timezone: "UTC", as_of_unix_ms: "1790812800000", day_start_unix_ms: "1790812800000", day_end_unix_ms: "1790899200000", revision: "9007199254740993", target_score: 70, enabled_skills: 3, assessed_skills: 2, target_reached_skills: 1, ready_plans: 2, historical_plans: 1, pending_tasks: 4, completed_tasks: 7, cancelled_tasks: 2, completed_today: 2, cancelled_today: 1, recorded_minutes_today: 35 };
}
test("learning progress errors stay separate from management and expiry clears all private data", async ({ page }, testInfo) => {
  await start(page);
  await page.route("**/api/learning/progress", route => route.fulfill({ status: 503, json: { error: { code: "learning_unavailable" } } }));
  await card(page).getByRole("button", { name: "刷新学习进度", exact: true }).click();
  await expect(card(page).getByRole("alert")).toContainText("暂不可用");
  await expect(card(page).locator("dd")).toHaveCount(0);
  await root(page).getByLabel("技能名称", { exact: true }).fill("仍能保存的私有技能");
  await root(page).getByRole("button", { name: "保存技能", exact: true }).click();
  await expect(root(page).getByRole("list", { name: "技能列表", exact: true })).toContainText("仍能保存的私有技能");
  await page.route("**/api/learning/progress", route => route.fulfill({ json: fixture() }));
  await card(page).getByRole("button", { name: "刷新学习进度", exact: true }).click();
  await expect(card(page)).toContainText("学习版本 9007199254740993");
  await expect(card(page).getByRole("alert")).toHaveCount(0);
  expect(await card(page).evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("learning-progress.png"); await card(page).screenshot({ path: screenshot }); await testInfo.attach("learning-progress", { path: screenshot, contentType: "image/png" });
  await page.route("**/api/learning/progress", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await card(page).getByRole("button", { name: "刷新学习进度", exact: true }).click();
  await expect(root(page).getByRole("alert")).toContainText("登录已失效");
  await expect(card(page).locator("dd")).toHaveCount(0);
  await expect(root(page)).not.toContainText("仍能保存的私有技能");
  await expect(card(page).getByRole("button", { name: "刷新学习进度", exact: true })).toBeDisabled();
  await expect(root(page).getByRole("button", { name: "保存技能", exact: true })).toBeDisabled();
});
test("late progress reads cannot populate a different account", async ({ page }) => {
  const first=await createAccount(), second=await createAccount(); await start(page, first);
  let release, received; const wait=new Promise(r => { release=r; }); const arrival=new Promise(r => { received=r; }); let delay=true;
  await page.route("**/api/learning/progress", async route => {
    if (!delay) return route.continue(); delay=false; received(); await wait;
    try { await route.fulfill({ json: fixture() }); } catch { /* abandoned account request */ }
  });
  await card(page).getByRole("button", { name: "刷新学习进度", exact: true }).click(); await arrival;
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, second); release();
  await expect(card(page).getByText("当前版本已自评", { exact: true }).locator("..")).toContainText("0 / 0");
  await expect(card(page)).not.toContainText("9007199254740993");
});
