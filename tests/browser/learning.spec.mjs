import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";
const panelOf = page => page.getByRole("region", { name: "学习管理", exact: true });
async function start(page, account) {
  await page.goto("/"); await login(page, account ?? await createAccount());
  const panel = panelOf(page); await expect(panel).toContainText("暂无学习计划"); return panel;
}
async function skill(panel, name) {
  await panel.getByLabel("技能名称", { exact: true }).fill(name);
  await panel.getByRole("button", { name: "保存技能", exact: true }).click();
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText(name);
  await expect(panel.getByRole("button", { name: "保存技能", exact: true })).toBeDisabled();
}
async function generate(panel, name) {
  await panel.getByRole("group", { name: "学习目标", exact: true }).getByLabel(name, { exact: true }).check();
  await panel.getByRole("button", { name: "生成学习计划", exact: true }).click();
  const detail = panel.getByLabel("学习计划详情", { exact: true }); await expect(detail).toBeVisible(); return detail;
}
test("learning skills, self assessments, plans and explicit results survive reload and clean up", async ({ page }, testInfo) => {
  const owner = await createAccount(), other = await createAccount();
  const panel = await start(page, owner); await skill(panel, "Rust 基础");
  await panel.getByRole("combobox", { name: "自评技能", exact: true }).selectOption({ label: "Rust 基础" });
  await panel.getByLabel("自评分数", { exact: true }).fill("40");
  await panel.getByRole("button", { name: "保存自评", exact: true }).click();
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("当前自评 40 分");
  await panel.getByLabel("技能名称", { exact: true }).fill("Rust 异步");
  await panel.getByRole("group", { name: "前置技能", exact: false }).getByLabel("Rust 基础", { exact: true }).check();
  await panel.getByRole("button", { name: "保存技能", exact: true }).click();
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("Rust 异步");
  const progress = panel.getByRole("region", { name: "学习进度", exact: true });
  const metric = name => progress.getByText(name, { exact: true }).locator("..").locator("dd");
  await expect(metric("启用技能")).toHaveText("2");
  await expect(metric("当前版本已自评")).toHaveText("1 / 2");
  const detail = await generate(panel, "Rust 异步");
  await expect(metric("待记录训练")).toHaveText("1");
  await expect(detail).toContainText("前置技能未满足");
  const task = detail.getByRole("list", { name: "训练任务", exact: true }).locator("li"); await expect(task).toHaveCount(1);
  await task.getByLabel("训练记录", { exact: true }).fill("练习完成 <img src=x onerror=alert(1)>");
  await task.getByLabel("实际练习分钟", { exact: true }).fill("20");
  await task.getByRole("button", { name: "记录完成", exact: true }).click();
  await task.getByRole("button", { name: "返回修改", exact: true }).click();
  await task.getByRole("button", { name: "记录完成", exact: true }).click();
  await task.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await expect(detail).toContainText("已记录完成 · 20 分钟");
  await expect(metric("今日完成")).toHaveText("1");
  await expect(metric("今日记录用时（分钟）")).toHaveText("20");
  await expect(metric("待记录训练")).toHaveText("0");
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("当前自评 40 分");
  await expect(detail.locator("img,script")).toHaveCount(0);
  expect(await panel.evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("learning-management.png");
  await panel.screenshot({ path: screenshot }); await testInfo.attach("learning-management", { path: screenshot, contentType: "image/png" });
  await page.reload(); await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(detail).toContainText("已记录完成 · 20 分钟");
  await expect(metric("今日完成")).toHaveText("1");
  await expect(metric("今日记录用时（分钟）")).toHaveText("20");
  await expect(metric("待记录训练")).toHaveText("0");
  await panel.getByRole("button", { name: "修改 Rust 基础", exact: true }).click();
  await panel.getByLabel("技能名称", { exact: true }).fill("Rust 新基础");
  await panel.getByRole("button", { name: "保存技能", exact: true }).click();
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).not.toContainText("当前自评 40 分");
  await panel.getByRole("button", { name: "删除 Rust 新基础", exact: true }).click();
  await panel.getByRole("button", { name: "保留技能", exact: true }).click();
  await panel.getByRole("button", { name: "删除 Rust 新基础", exact: true }).click();
  await panel.getByRole("button", { name: "确认删除技能", exact: true }).click();
  await expect(panel.getByRole("list", { name: "学习计划历史", exact: true })).toContainText("计划已失效");
  await expect(metric("今日完成")).toHaveText("0");
  await expect(metric("今日记录用时（分钟）")).toHaveText("0");
  await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(detail).toContainText("正文及训练结果已清除"); await expect(detail).not.toContainText("练习完成");
  await detail.getByRole("button", { name: "删除学习计划", exact: true }).click();
  await detail.getByRole("button", { name: "确认删除学习计划", exact: true }).click();
  await expect(panel.getByRole("list", { name: "学习计划历史", exact: true })).toContainText("已删除");
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await expect(panel).toHaveCount(0); await login(page, other);
  await expect(panel).toContainText("暂无技能"); await expect(panel).toContainText("暂无学习计划");
});

test("lost learning generation and cancellation reuse original requests", async ({ page }) => {
  const panel = await start(page); await skill(panel, "数据库");
  const requests = []; let dropPlan = true, dropResult = true;
  await page.route("**/api/learning/plans", async route => {
    if (route.request().method() !== "POST") return route.continue();
    requests.push(route.request().postDataJSON()); const response = await route.fetch(); expect(response.status()).toBe(201);
    if (dropPlan) { dropPlan = false; return route.abort("failed"); } return route.fulfill({ response });
  });
  await panel.getByRole("group", { name: "学习目标", exact: true }).getByLabel("数据库", { exact: true }).check();
  await panel.getByRole("button", { name: "生成学习计划", exact: true }).click();
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  const detail = panel.getByLabel("学习计划详情", { exact: true }); await expect(detail).toBeVisible();
  expect(requests).toHaveLength(2); expect(requests[0]).toEqual(requests[1]);
  const results = [];
  await page.route("**/api/learning/plans/*/tasks/*/result", async route => {
    results.push(route.request().postDataJSON()); const response = await route.fetch(); expect(response.status()).toBe(200);
    if (dropResult) { dropResult = false; return route.abort("failed"); } return route.fulfill({ response });
  });
  await detail.getByLabel("训练记录", { exact: true }).fill("今天没有时间");
  await detail.getByRole("button", { name: "取消这项训练", exact: true }).click();
  await detail.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  await expect(detail).toContainText("已取消训练 · 0 分钟");
  expect(results).toHaveLength(2); expect(results[0]).toEqual(results[1]);
  await expect(panel.getByRole("list", { name: "学习计划历史", exact: true }).locator("li")).toHaveCount(1);
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("尚未自评");
});

test("learning precise revisions, history navigation, conflict recovery and expiry clear private state", async ({ page }) => {
  const panel = await start(page);
  const id = "ac1dfd48-f9ac-45f3-bc31-2c9cc494c413";
  const snapshot = { revision: "9007199254740993", skills: [{ skill_id: id, revision: "9007199254740993", name: "私有技能", enabled: true, deleted: false, prerequisite_ids: [] }], assessments: [] };
  await page.route("**/api/learning/snapshot", route => route.fulfill({ json: snapshot }));
  await panel.getByRole("button", { name: "刷新学习数据", exact: true }).click();
  await panel.getByRole("button", { name: "修改 私有技能", exact: true }).click();
  let body;
  await page.route(`**/api/learning/skills/${id}`, route => { body = route.request().postDataJSON(); return route.fulfill({ status: 409, json: { error: { code: "learning_conflict" } } }); });
  await panel.getByLabel("技能名称", { exact: true }).fill("修改请求");
  await panel.getByRole("button", { name: "保存技能", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("版本已变化"); expect(body.revision).toBe("9007199254740993");
  await panel.getByRole("button", { name: "核对原学习操作", exact: true }).click(); await expect(panel).toContainText("已查询当前保存状态");
  await page.route("**/api/learning/plans", route => route.fulfill({ json: { items: [{ request_id: id, created_at_unix_ms: "1790812800000", status: "invalidated" }], next_cursor: id } }));
  await page.route("**/api/learning/plans?*", route => route.fulfill({ json: { items: [{ request_id: id, created_at_unix_ms: "1790812800000", status: "deleted" }], next_cursor: null } }));
  await panel.getByRole("button", { name: "刷新学习数据", exact: true }).click();
  await panel.getByRole("button", { name: "下一页学习计划", exact: true }).click(); await expect(panel.getByRole("list", { name: "学习计划历史", exact: true })).toContainText("已删除");
  await panel.getByRole("button", { name: "上一页学习计划", exact: true }).click(); await expect(panel.getByRole("list", { name: "学习计划历史", exact: true })).toContainText("计划已失效");
  await page.route("**/api/learning/snapshot", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await panel.getByRole("button", { name: "刷新学习数据", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效"); await expect(panel).not.toContainText("私有技能");
  await expect(panel.getByRole("list", { name: "学习计划历史", exact: true }).locator("li")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "刷新学习数据", exact: true })).toBeDisabled();
});

test("late learning plan response cannot enter a new account", async ({ page }) => {
  const owner = await createAccount(), other = await createAccount(); const panel = await start(page, owner); await skill(panel, "私有晚到技能");
  let release, received; const waiting = new Promise(resolve => { release = resolve; }); const arrived = new Promise(resolve => { received = resolve; });
  await page.route("**/api/learning/plans", async route => {
    if (route.request().method() !== "POST") return route.continue();
    const response = await route.fetch(); received(); await waiting;
    try { await route.fulfill({ response }); } catch { /* old account request was aborted */ }
  });
  await panel.getByRole("group", { name: "学习目标", exact: true }).getByLabel("私有晚到技能", { exact: true }).check();
  await panel.getByRole("button", { name: "生成学习计划", exact: true }).click(); await arrived;
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, other); release();
  await expect(panel).toContainText("暂无学习计划"); await expect(panel).not.toContainText("私有晚到技能"); await expect(panel.getByLabel("学习计划详情", { exact: true })).toHaveCount(0);
});
