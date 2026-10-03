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
  const task = detail.getByRole("list", { name: "训练任务", exact: true }).locator(":scope > li"); await expect(task).toHaveCount(1);
  await task.getByText("查看证据检查", { exact: true }).click();
  await expect(task).toContainText("尚未记录训练结果");
  await task.getByLabel("训练记录", { exact: true }).fill("练习完成 <img src=x onerror=alert(1)>");
  await task.getByLabel("实际练习分钟", { exact: true }).fill("20");
  await task.getByRole("button", { name: "记录完成", exact: true }).click();
  await task.getByRole("button", { name: "返回修改", exact: true }).click();
  await task.getByRole("button", { name: "记录完成", exact: true }).click();
  await task.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await expect(detail).toContainText("已记录完成 · 20 分钟");
  await detail.getByText("查看证据检查", { exact: true }).click();
  await expect(detail).toContainText("已有文字材料，内容尚待核验");
  await detail.getByRole("link", { name: "回看原始训练记录", exact: true }).click();
  await expect(detail.locator("[id^=training-note-]")).toBeFocused();
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
  await expect(detail.getByText("查看证据检查", { exact: true })).toHaveCount(0);
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
  await detail.getByText("查看证据检查", { exact: true }).click();
  await expect(detail).toContainText("训练已取消，不作为完成证据");
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

test("completed training without a note remains missing evidence and does not assess a skill", async ({ page }) => {
  const panel = await start(page); await skill(panel, "证据空白练习");
  const detail = await generate(panel, "证据空白练习");
  await detail.getByRole("button", { name: "记录完成", exact: true }).click();
  await detail.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await expect(detail).toContainText("已记录完成");
  await detail.getByText("查看证据检查", { exact: true }).click();
  await expect(detail).toContainText("已记录完成，但缺少文字材料");
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("当前版本尚未自评");
});

test("structured training evidence survives retries and reload, deletes without revival and stays private", async ({ page }, testInfo) => {
  const owner = await createAccount(), other = await createAccount();
  const panel = await start(page, owner); await skill(panel, "结构化证据技能");
  const detail = await generate(panel, "结构化证据技能");
  await detail.getByRole("button", { name: "记录完成", exact: true }).click();
  await detail.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await expect(detail).toContainText("已记录完成");
  await detail.getByText("补充结构化证据", { exact: true }).click();
  await expect(detail.getByRole("button", { name: "保存结构化证据", exact: true })).toBeDisabled();
  await detail.getByLabel("概念解释", { exact: true }).fill("私有解释 <img src=x onerror=alert(1)>");
  await detail.getByLabel("练习产物与独立完成范围", { exact: true }).fill("独立完成一个练习");
  await detail.getByLabel("验证步骤与结果", { exact: true }).fill("本地执行检查通过 https://example.com/private");
  await detail.getByLabel("局限与待验证问题", { exact: true }).fill("边界条件仍需检查");
  await detail.getByRole("button", { name: "保存结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "返回修改证据", exact: true }).click();
  const saves = [], deletes = []; let dropSave = true, dropDelete = true;
  await page.route("**/api/learning/plans/*/tasks/*/evidence", async route => {
    const method = route.request().method();
    (method === "POST" ? saves : deletes).push(route.request().postDataJSON());
    const response = await route.fetch(); expect(response.status()).toBe(200);
    if (method === "POST" && dropSave) { dropSave = false; return route.abort("failed"); }
    if (method === "DELETE" && dropDelete) { dropDelete = false; return route.abort("failed"); }
    return route.fulfill({ response });
  });
  await detail.getByRole("button", { name: "保存结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "确认保存证据", exact: true }).click();
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  const evidence = detail.getByRole("region", { name: "已保存的结构化证据", exact: true });
  await expect(evidence).toContainText("私有解释 <img src=x onerror=alert(1)>");
  expect(saves).toHaveLength(2); expect(saves[0]).toEqual(saves[1]);
  await expect(evidence.locator("img,script,a")).toHaveCount(0);
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("当前版本尚未自评");
  expect(await panel.evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("structured-evidence.png");
  await evidence.screenshot({ path: screenshot }); await testInfo.attach("structured-evidence", { path: screenshot, contentType: "image/png" });
  await page.reload(); await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(evidence).toContainText("边界条件仍需检查");
  await evidence.getByRole("button", { name: "删除结构化证据", exact: true }).click();
  await evidence.getByRole("button", { name: "保留证据", exact: true }).click();
  await evidence.getByRole("button", { name: "删除结构化证据", exact: true }).click();
  await evidence.getByRole("button", { name: "确认删除证据", exact: true }).click();
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  await expect(detail).toContainText("结构化证据已删除");
  expect(deletes).toHaveLength(2); expect(deletes[0]).toEqual(deletes[1]);
  await expect(detail).not.toContainText("私有解释");
  await expect(detail.getByText("补充结构化证据", { exact: true })).toHaveCount(0);
  await page.reload(); await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(detail).toContainText("结构化证据已删除");
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, other);
  await expect(panel).toContainText("暂无学习计划"); await expect(panel).not.toContainText("私有解释");
});

test("structured evidence accepts four full Chinese fields through each gateway", async ({ page }) => {
  const panel = await start(page); await skill(panel, "大文本证据");
  const detail = await generate(panel, "大文本证据");
  await detail.getByRole("button", { name: "记录完成", exact: true }).click();
  await detail.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await detail.getByText("补充结构化证据", { exact: true }).click();
  for (const label of ["概念解释", "练习产物与独立完成范围", "验证步骤与结果", "局限与待验证问题"]) {
    await detail.getByLabel(label, { exact: true }).fill("证据".repeat(1000));
  }
  await detail.getByRole("button", { name: "保存结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "确认保存证据", exact: true }).click();
  const evidence = detail.getByRole("region", { name: "已保存的结构化证据", exact: true });
  await expect(evidence.locator(".feedText")).toHaveCount(4);
  await expect(evidence.locator(".feedText").first()).toHaveText("证据".repeat(1000));
});

async function prepareEvidenceReview(page) {
  const panel = await start(page); await skill(panel, "用户核验技能");
  const detail = await generate(panel, "用户核验技能");
  await detail.getByRole("button", { name: "记录完成", exact: true }).click();
  await detail.getByRole("button", { name: "确认训练结果", exact: true }).click();
  await detail.getByText("补充结构化证据", { exact: true }).click();
  for (const label of ["概念解释", "练习产物与独立完成范围", "验证步骤与结果", "局限与待验证问题"]) await detail.getByLabel(label, { exact: true }).fill(`${label}的材料`);
  await detail.getByRole("button", { name: "保存结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "确认保存证据", exact: true }).click();
  await detail.getByText("逐项核验证据", { exact: true }).click();
  return { panel, detail };
}
async function fillReview(detail, supported = true) {
  for (const label of ["概念解释", "独立练习", "结果验证", "局限与反例"]) {
    if (supported || label !== "结果验证") await detail.getByRole("combobox", { name: `${label}判断`, exact: true }).selectOption("supported");
    await detail.getByLabel(`${label}核验理由`, { exact: true }).fill(`${label}私有理由 <img src=x>`);
  }
  await detail.getByRole("button", { name: "保存核验记录", exact: true }).click();
  await detail.getByRole("button", { name: "确认保存核验", exact: true }).click();
}
test("user review and explicit confirmation replay once and evidence deletion revokes only its assessment", async ({ page }, testInfo) => {
  const { panel, detail } = await prepareEvidenceReview(page);
  const requests = { save: [], confirm: [] };
  await page.route("**/api/learning/plans/*/tasks/*/evidence/review**", async route => {
    const kind = route.request().url().endsWith("/confirm") ? "confirm" : "save";
    requests[kind].push(route.request().postDataJSON());
    const response = await route.fetch(); expect(response.status()).toBe(200);
    if (requests[kind].length === 1) return route.abort("failed");
    return route.fulfill({ response });
  });
  await fillReview(detail);
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  const review = detail.getByRole("region", { name: "已保存的用户核验", exact: true });
  await expect(review).toContainText("概念解释私有理由 <img src=x>");
  await expect(review.locator("img,script")).toHaveCount(0);
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("尚未自评");
  await expect(review.getByRole("button", { name: "准备确认自评", exact: true })).toBeDisabled();
  await review.getByLabel("核验后的自评分数", { exact: true }).fill("68");
  await review.getByRole("button", { name: "准备确认自评", exact: true }).click();
  await review.getByRole("button", { name: "返回调整分数", exact: true }).click();
  await review.getByRole("button", { name: "准备确认自评", exact: true }).click();
  await review.getByRole("button", { name: "确认记录自评", exact: true }).click();
  await panel.getByRole("button", { name: "重试原学习操作", exact: true }).click();
  await expect(review).toContainText("已确认自评 68 分");
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("当前自评 68 分");
  expect(requests.save).toHaveLength(2); expect(requests.save[0]).toEqual(requests.save[1]);
  expect(requests.confirm).toHaveLength(2); expect(requests.confirm[0]).toEqual(requests.confirm[1]);
  const screenshot = testInfo.outputPath("user-evidence-review.png");
  await review.screenshot({ path: screenshot }); await testInfo.attach("user-evidence-review", { path: screenshot, contentType: "image/png" });
  expect(await panel.evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
  await page.reload(); await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(review).toContainText("已确认自评 68 分");
  await detail.getByRole("button", { name: "删除结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "确认删除证据", exact: true }).click();
  await expect(detail).toContainText("相关核验及其确认自评也已清除");
  await expect(detail).not.toContainText("私有理由");
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("尚未自评");
  await page.reload(); await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(detail).not.toContainText("已确认自评 68 分");
});
test("unverified dimensions never offer a score confirmation", async ({ page }) => {
  const { panel, detail } = await prepareEvidenceReview(page);
  await fillReview(detail, false);
  await expect(detail).toContainText("仍有缺少证据或待核验项");
  await expect(detail.getByRole("button", { name: "准备确认自评", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("list", { name: "技能列表", exact: true })).toContainText("尚未自评");
});
test("historical review cannot confirm against a changed skill", async ({ page }) => {
  const { panel, detail } = await prepareEvidenceReview(page);
  await fillReview(detail);
  await expect(detail.getByRole("button", { name: "准备确认自评", exact: true })).toBeVisible();
  await panel.getByRole("button", { name: "修改 用户核验技能", exact: true }).click();
  await panel.getByLabel("技能名称", { exact: true }).fill("新版本核验技能");
  await panel.getByRole("button", { name: "保存技能", exact: true }).click();
  await panel.getByRole("button", { name: "查看学习计划", exact: true }).click();
  await expect(detail).toContainText("技能版本或计划来源不可用，不能确认这条核验");
  await expect(detail.getByRole("button", { name: "准备确认自评", exact: true })).toHaveCount(0);
});

test("revoked plan sources disable new work while keeping explicit deletion available", async ({ page }) => {
  const panel = await start(page); await skill(panel, "历史来源技能");
  const detail = await generate(panel, "历史来源技能");
  await page.route("**/api/learning/plans/*", async route => {
    const response = await route.fetch();
    const saved = await response.json(); saved.source_assessments_available = false;
    await route.fulfill({ response, json: saved });
  });
  await detail.getByRole("button", { name: "更新学习计划状态", exact: true }).click();
  await expect(detail).toContainText("来源自评已撤销");
  await expect(detail.getByRole("button", { name: "记录完成", exact: true })).toBeDisabled();
  await expect(detail.getByRole("button", { name: "取消这项训练", exact: true })).toBeDisabled();
  await expect(detail.getByRole("button", { name: "删除学习计划", exact: true })).toBeEnabled();
});

test("model sharing preview is explicit, read only, and cleared with its evidence", async ({ page }, testInfo) => {
  const { detail } = await prepareEvidenceReview(page);
  const preview = detail.getByRole("region", { name: "模型分享预览", exact: true });
  await expect(preview.getByText("模型分享预览（未发送）", { exact: true })).toHaveCount(0);
  await preview.getByRole("button", { name: "预览模型分享材料", exact: true }).click();
  await expect(preview.getByText("模型分享预览（未发送）", { exact: true })).toBeVisible();
  await expect(preview.locator("pre")).toContainText("learning-model-review-v1");
  await expect(preview.locator("pre")).toContainText("概念解释的材料");
  await expect(preview.locator("pre")).toContainText("用户核验技能");
  const screenshot = testInfo.outputPath("model-review-preview.png");
  await preview.screenshot({ path: screenshot });
  await testInfo.attach("model-review-preview", { path: screenshot, contentType: "image/png" });
  await preview.getByRole("button", { name: "关闭分享预览", exact: true }).click();
  await expect(preview.locator("pre")).toHaveCount(0);
  await preview.getByRole("button", { name: "预览模型分享材料", exact: true }).click();
  await expect(preview.locator("pre")).toBeVisible();
  await detail.getByRole("button", { name: "删除结构化证据", exact: true }).click();
  await detail.getByRole("button", { name: "确认删除证据", exact: true }).click();
  await expect(preview).toHaveCount(0);
});

test("expired model preview request clears private learning data", async ({ page }) => {
  const { panel, detail } = await prepareEvidenceReview(page);
  await page.route("**/api/learning/plans/*/tasks/*/evidence/model-preview", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await detail.getByRole("button", { name: "预览模型分享材料", exact: true }).click();
  await expect(panel.getByText("登录已失效，请重新登录。", { exact: true })).toBeVisible();
  await expect(detail).toHaveCount(0);
  await expect(panel.getByText("概念解释的材料", { exact: true })).toHaveCount(0);
});
