import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";

async function createAccount() {
  const email = `${randomUUID()}@browser.example`;
  const password = randomUUID();
  // 使用 Node fetch，避免 Playwright 请求诊断记录管理员认证头。
  const headers = { authorization: `Bearer ${process.env.E2E_ADMIN_TOKEN}`, "content-type": "application/json" };
  const created = await fetch(process.env.E2E_API_URL + "/api/users", {
    method: "POST", headers, body: JSON.stringify({ email, display_name: "浏览器验收" }), signal: AbortSignal.timeout(10000),
  });
  expect(created.status).toBe(201);
  const user = await created.json();
  const updated = await fetch(`${process.env.E2E_API_URL}/api/users/${user.id}/password`, {
    method: "POST", headers, body: JSON.stringify({ password }), signal: AbortSignal.timeout(10000),
  });
  expect(updated.status).toBe(200);
  return { email, password };
}
let lastLoginAt = 0;
async function login(page, account) {
  // 串行验收主动遵守每分钟 20 次的真实登录限流，不关闭保护或重试失败请求。
  const delay = Math.max(0, 3500 - (Date.now() - lastLoginAt));
  if (delay) await new Promise(resolve => setTimeout(resolve, delay));
  lastLoginAt = Date.now();
  await page.getByLabel("邮箱", { exact: true }).fill(account.email);
  await page.getByLabel("密码", { exact: true }).fill(account.password);
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("heading", { name: "欢迎回来，浏览器验收" })).toBeVisible();
}
function metric(page, label) {
  return page.locator(".overviewMetrics > div").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
}

test("index controls require explicit submission, show progress and stop after logout", async ({ page }) => {
  const account = await createAccount();
  let current = null;
  let mode = "normal";
  let reads = 0;
  let writes = 0;
  await page.route("**/api/documents/*/index-job", async route => {
    const request = route.request();
    if (request.method() === "POST") {
      writes += 1;
      expect(request.headers()["x-requested-with"]).toBe("personal-ai");
      current = { document_id: request.url().split("/").at(-2), status: "queued", indexed_chunks: 0, total_chunks: 4, attempts: 0, error_code: null };
      return route.fulfill({ status: 202, json: current });
    }
    reads += 1;
    if (mode === "disabled") return route.fulfill({ status: 503, json: { error: { code: "indexing_disabled" } } });
    if (mode === "unavailable") return route.fulfill({ status: 503, json: { error: { code: "storage_unavailable" } } });
    if (mode === "expired") return route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } });
    return route.fulfill({ status: current ? 200 : 404, json: current ?? { error: { code: "index_job_not_found" } } });
  });
  await page.goto("/");
  await login(page, account);
  await upload(page, "index-controls.md", Buffer.from("# 索引\n\n" + "知识积累。".repeat(500)));
  const panel = page.getByRole("region", { name: "index-controls.md的索引", exact: true });
  await expect(panel).toContainText("当前目标暂无索引任务");
  expect(writes).toBe(0);
  await panel.getByRole("button", { name: "建立索引", exact: true }).click();
  await expect(panel).toContainText("等待索引 · 0/4 块");
  await expect(panel.getByRole("button", { name: "建立索引", exact: true })).toBeDisabled();
  current = { ...current, status: "running", indexed_chunks: 2 };
  await expect(panel).toContainText("正在索引 · 2/4 块");
  current = { ...current, status: "retrying", attempts: 1, error_code: "embedding_unavailable" };
  await expect(panel).toContainText("等待自动重试");
  await expect(panel).toContainText("向量模型暂不可用");
  current = { ...current, status: "failed", attempts: 3 };
  await expect(panel.getByRole("button", { name: "重试索引", exact: true })).toBeEnabled();
  expect(writes).toBe(1);
  await panel.getByRole("button", { name: "重试索引", exact: true }).click();
  await expect(panel).toContainText("等待索引");
  current = { ...current, status: "completed", indexed_chunks: 4 };
  await expect(panel).toContainText("索引完成 · 4/4 块");
  await expect(panel.getByRole("button", { name: "建立索引", exact: true })).toBeDisabled();
  const completedReads = reads;
  await page.waitForTimeout(2500);
  expect(reads).toBe(completedReads);
  expect(writes).toBe(2);
  for (const [nextMode, expected] of [["unavailable", "无法读取索引状态"], ["expired", "登录已失效"], ["disabled", "管理员尚未启用索引"]]) {
    mode = nextMode;
    await panel.getByRole("button", { name: "刷新索引状态" }).click();
    await expect(panel).toContainText(expected);
    await expect(panel.getByRole("button", { name: "建立索引", exact: true })).toBeDisabled();
  }
  mode = "normal";
  current = { ...current, status: "running", indexed_chunks: 2 };
  await panel.getByRole("button", { name: "刷新索引状态" }).click();
  await expect(panel).toContainText("正在索引");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  const loggedOutReads = reads;
  await page.waitForTimeout(2500);
  expect(reads).toBe(loggedOutReads);
});
async function upload(page, name, buffer) {
  await page.getByLabel("Markdown / PDF 文件", { exact: false }).setInputFiles({ name, mimeType: "text/markdown", buffer });
  await page.getByRole("button", { name: "导入文档", exact: true }).click();
}

const indexTest = process.env.E2E_INDEX === "1" ? test : test.skip;
test("reply UI explicitly requests fixture output and restores history after reload", async ({ page }, testInfo) => {
  const account = await createAccount();
  let writes = 0;
  await page.route("**/api/conversations/*/replies", async route => {
    if (route.request().method() === "POST") writes += 1;
    return route.continue();
  });
  await page.goto("/"); await login(page, account);
  const panel = page.getByRole("region", { name: "对话与消息", exact: true });
  await panel.getByLabel("新对话标题", { exact: false }).fill("回复验收");
  await panel.getByRole("button", { name: "创建对话", exact: true }).click();
  const replies = panel.getByRole("region", { name: "显式回复", exact: true });
  await expect(replies).toContainText("暂无回复请求");
  await expect(replies.getByRole("button", { name: "请求测试回复", exact: true })).toBeDisabled();
  const text = '<img src=x onerror="window.replyInjected=true">\n测试回复';
  await panel.getByLabel("用户消息", { exact: false }).fill(text);
  await panel.getByRole("button", { name: "发送用户消息", exact: true }).click();
  await expect(replies.getByRole("button", { name: "请求测试回复", exact: true })).toBeEnabled();
  expect(writes).toBe(0);
  await replies.getByRole("button", { name: "请求测试回复", exact: true }).click();
  expect(writes).toBe(0);
  await replies.getByRole("button", { name: "确认请求测试回复", exact: true }).click();
  await expect(replies).toContainText("消息版本 1 · 已完成");
  await expect(replies.locator(".replyOutput")).toHaveText(`本地测试回复（非模型生成）：${text}`);
  await expect(replies.locator("img")).toHaveCount(0);
  expect(writes).toBe(1);
  await replies.screenshot({ path: testInfo.outputPath("replies.png") });
  await page.reload();
  await panel.getByRole("button", { name: "回复验收", exact: true }).click();
  await expect(replies.locator(".replyList li")).toHaveCount(1);
  await expect(replies).toContainText("已完成");
  expect(writes).toBe(1);
  await panel.getByRole("button", { name: "删除当前对话", exact: true }).click();
  await panel.getByRole("button", { name: "确认删除对话", exact: true }).click();
  await expect(replies).toHaveCount(0);
});

test("conversation UI retries committed requests without duplicates and isolates accounts", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  const createBodies = [];
  const messageBodies = [];
  await page.route("**/api/conversations", async route => {
    if (route.request().method() !== "POST") return route.continue();
    createBodies.push(route.request().postDataJSON());
    if (createBodies.length === 1) {
      expect((await route.fetch()).status()).toBe(200);
      return route.abort("failed");
    }
    return route.continue();
  });
  await page.route("**/api/conversations/*/messages", async route => {
    if (route.request().method() !== "POST") return route.continue();
    messageBodies.push(route.request().postDataJSON());
    if (messageBodies.length === 1) {
      expect((await route.fetch()).status()).toBe(200);
      return route.abort("failed");
    }
    return route.continue();
  });
  await page.goto("/"); await login(page, owner);
  const panel = page.getByRole("region", { name: "对话与消息", exact: true });
  await expect(panel).toContainText("暂无对话");
  await panel.getByLabel("新对话标题", { exact: false }).fill("学习计划");
  await panel.getByRole("button", { name: "创建对话", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("结果未确认");
  await expect(panel.getByLabel("新对话标题", { exact: false })).toBeDisabled();
  await panel.getByRole("button", { name: "重试创建原请求" }).click();
  await expect(panel.getByRole("heading", { name: "学习计划" })).toBeVisible();
  expect(createBodies).toHaveLength(2); expect(createBodies[0]).toEqual(createBodies[1]);
  await expect(panel.locator(".conversationList li")).toHaveCount(1);
  const thread = panel.getByRole("region", { name: "当前对话消息" });
  await expect(thread).toContainText("暂无消息");
  const text = '<img src=x onerror="window.messageInjected=true">\n中文消息';
  await thread.getByLabel("用户消息", { exact: false }).fill(text);
  await thread.getByRole("button", { name: "发送用户消息", exact: true }).click();
  await expect(thread.getByRole("alert")).toContainText("结果未确认");
  await expect(thread.getByLabel("用户消息", { exact: false })).toHaveValue(text);
  await thread.getByRole("button", { name: "刷新消息" }).click();
  await expect(thread.locator(".messageList li")).toHaveCount(1);
  await thread.getByRole("button", { name: "重试发送原请求" }).click();
  await expect(thread.getByLabel("用户消息", { exact: false })).toHaveValue("");
  await expect(thread.locator(".messageList li")).toHaveCount(1);
  expect(messageBodies).toHaveLength(2); expect(messageBodies[0]).toEqual(messageBodies[1]);
  await expect(thread.locator(".messageList p")).toHaveText(text);
  await expect(thread.locator("img")).toHaveCount(0);
  await panel.screenshot({ path: testInfo.outputPath("conversations.png") });
  await page.reload();
  await panel.getByRole("button", { name: "学习计划", exact: true }).click();
  await expect(thread.locator(".messageList li")).toHaveCount(1);
  await thread.getByLabel("用户消息", { exact: false }).fill("未发送的私有草稿");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0); await login(page, other);
  await expect(panel).toContainText("暂无对话");
  await expect(panel).not.toContainText("学习计划");
  await expect(panel.getByLabel("新对话标题", { exact: false })).toHaveValue("");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, owner);
  await panel.getByRole("button", { name: "学习计划", exact: true }).click();
  await expect(thread.getByLabel("用户消息", { exact: false })).toHaveValue("");
  await panel.getByRole("button", { name: "删除当前对话" }).click();
  await panel.getByRole("button", { name: "取消删除对话" }).click();
  await expect(thread.locator(".messageList li")).toHaveCount(1);
  await panel.getByRole("button", { name: "删除当前对话" }).click();
  await panel.getByRole("button", { name: "确认删除对话" }).click();
  await expect(panel).toContainText("暂无对话");
  await expect(thread).toHaveCount(0);
});

async function prepareReplyConversation(page, title = "回复故障验收") {
  const panel = page.getByRole("region", { name: "对话与消息", exact: true });
  await panel.getByLabel("新对话标题", { exact: false }).fill(title);
  await panel.getByRole("button", { name: "创建对话", exact: true }).click();
  await expect(panel.getByRole("heading", { name: title, exact: true })).toBeVisible();
  await expect(panel.getByRole("button", { name: "发送用户消息", exact: true })).toBeEnabled();
  await panel.getByLabel("用户消息", { exact: false }).fill("需要隔离的回复内容");
  await panel.getByRole("button", { name: "发送用户消息", exact: true }).click();
  await expect(panel.locator(".messageList li")).toHaveCount(1);
  const replies = panel.getByRole("region", { name: "显式回复", exact: true });
  await expect(replies.getByRole("button", { name: "刷新回复历史" })).toBeEnabled();
  return { panel, replies };
}

test("reply UI retries a committed request with the original ID and isolates accounts", async ({ page }) => {
  const owner = await createAccount();
  const other = await createAccount();
  const bodies = [];
  await page.route("**/api/conversations/*/replies", async route => {
    if (route.request().method() !== "POST") return route.continue();
    bodies.push(route.request().postDataJSON());
    if (bodies.length === 1) {
      expect((await route.fetch()).status()).toBe(202);
      return route.abort("failed");
    }
    return route.continue();
  });
  await page.goto("/"); await login(page, owner);
  const { panel, replies } = await prepareReplyConversation(page);
  await replies.getByRole("button", { name: "请求测试回复", exact: true }).click();
  await replies.getByRole("button", { name: "确认请求测试回复", exact: true }).click();
  await expect(replies.getByRole("alert")).toContainText("结果未确认");
  await expect(panel.getByRole("button", { name: "发送用户消息", exact: true })).toBeDisabled();
  await expect(panel.getByRole("button", { name: "删除当前对话", exact: true })).toBeDisabled();
  await expect(panel.getByRole("button", { name: "回复故障验收", exact: true })).toBeDisabled();
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await expect(replies.locator(".replyList li")).toHaveCount(1);
  expect(bodies).toHaveLength(1);
  await replies.getByRole("button", { name: "重试回复原请求" }).click();
  await expect(replies).toContainText("消息版本 1 · 已完成");
  await expect(replies.getByRole("button", { name: "重试回复原请求" })).toHaveCount(0);
  expect(bodies).toHaveLength(2); expect(bodies[0]).toEqual(bodies[1]);
  await expect(replies.locator(".replyList li")).toHaveCount(1);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await expect(panel).toContainText("暂无对话");
  await expect(page.getByRole("region", { name: "显式回复", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, owner);
  await panel.getByRole("button", { name: "回复故障验收", exact: true }).click();
  await expect(replies.locator(".replyList li")).toHaveCount(1);
  await expect(replies).toContainText("需要隔离的回复内容");
  expect(bodies).toHaveLength(2);
});

test("reply UI stops polling on failure and terminal states and retries cancellation explicitly", async ({ page }) => {
  const owner = await createAccount();
  let current = null;
  let mode = "disabled";
  let reads = 0;
  let writes = 0;
  let cancellations = 0;
  await page.route("**/api/conversations/*/replies", async route => {
    if (route.request().method() === "POST") {
      writes += 1;
      const body = route.request().postDataJSON();
      expect(body.expected_revision).toBe(1);
      current = { request_id: body.request_id, revision: 1, status: "queued", output: null, mode: "fixture" };
      return route.fulfill({ status: 202, json: current });
    }
    reads += 1;
    if (mode === "unavailable" || mode === "expired") return route.fulfill({ status: mode === "expired" ? 401 : 503, json: { error: { code: "unavailable" } } });
    return route.fulfill({ json: { enabled: mode !== "disabled", mode: "fixture", items: current ? [current] : [] } });
  });
  await page.route("**/api/conversations/*/replies/*/cancel", async route => {
    cancellations += 1;
    expect(route.request().headers()["x-requested-with"]).toBe("personal-ai");
    if (cancellations === 1) return route.fulfill({ status: 503, json: { error: { code: "unavailable" } } });
    current = { ...current, status: "cancelled" };
    return route.fulfill({ json: current });
  });
  await page.goto("/"); await login(page, owner);
  const { replies } = await prepareReplyConversation(page);
  await expect(replies).toContainText("管理员尚未启用测试回复");
  await expect(replies.getByRole("button", { name: "请求测试回复", exact: true })).toBeDisabled();
  mode = "normal";
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await replies.getByRole("button", { name: "请求测试回复", exact: true }).click();
  await replies.getByRole("button", { name: "确认请求测试回复", exact: true }).click();
  await expect(replies).toContainText("消息版本 1 · 等待执行");
  current = { ...current, status: "dispatching" };
  await expect(replies).toContainText("消息版本 1 · 正在执行");
  await replies.getByRole("button", { name: "取消此回复" }).click();
  await expect(replies.getByRole("alert")).toContainText("服务暂不可用");
  expect(cancellations).toBe(1);
  await replies.getByRole("button", { name: "取消此回复" }).click();
  await expect(replies).toContainText("消息版本 1 · 已取消");
  const stopped = reads;
  await page.waitForTimeout(2300);
  expect(reads).toBe(stopped); expect(cancellations).toBe(2);
  for (const [status, label] of [["failed", "执行失败"], ["unknown", "结果未知"]]) {
    current = { ...current, status };
    await replies.getByRole("button", { name: "刷新回复历史" }).click();
    await expect(replies).toContainText(`消息版本 1 · ${label}`);
  }
  await expect(replies).toContainText("不会自动重发");
  expect(writes).toBe(1);
  mode = "unavailable";
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await expect(replies.getByRole("alert")).toContainText("已停止自动刷新");
  await expect(replies.getByRole("button", { name: "请求测试回复", exact: true })).toBeDisabled();
  await expect(replies).not.toContainText("暂无回复请求");
  const failedReads = reads;
  await page.waitForTimeout(2300);
  expect(reads).toBe(failedReads);
  mode = "expired";
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await expect(replies.getByRole("alert")).toContainText("登录已失效");
  mode = "normal"; current = { ...current, status: "queued" };
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await expect(replies).toContainText("消息版本 1 · 等待执行");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(replies).toHaveCount(0);
  const loggedOutReads = reads;
  await page.waitForTimeout(2300);
  expect(reads).toBe(loggedOutReads); expect(writes).toBe(1);
});

test("reply UI discards delayed history after switching conversations", async ({ page }) => {
  const owner = await createAccount();
  await page.goto("/"); await login(page, owner);
  await prepareReplyConversation(page, "回复对话甲");
  const { panel, replies } = await prepareReplyConversation(page, "回复对话乙");
  let release;
  let delayRead = true;
  await page.route("**/api/conversations/*/replies", async route => {
    if (route.request().method() !== "GET") return route.continue();
    if (!delayRead) return route.continue();
    delayRead = false;
    await new Promise(resolve => { release = resolve; });
    return route.fulfill({ json: { enabled: true, mode: "fixture", items: [{ request_id: randomUUID(), revision: 1, status: "succeeded", output: "不得跨对话显示的晚到回复" }] } }).catch(() => {});
  });
  await replies.getByRole("button", { name: "刷新回复历史" }).click();
  await expect.poll(() => typeof release).toBe("function");
  await panel.getByRole("button", { name: "回复对话甲", exact: true }).click();
  await expect(panel.getByRole("heading", { name: "回复对话甲", exact: true })).toBeVisible();
  await expect(replies).toContainText("暂无回复请求");
  release();
  await expect(replies).not.toContainText("不得跨对话显示");
});

test("conversation UI rejects oversized messages and discards late reads when switching", async ({ page }) => {
  const owner = await createAccount();
  await page.goto("/"); await login(page, owner);
  const panel = page.getByRole("region", { name: "对话与消息", exact: true });
  for (const title of ["对话甲", "对话乙"]) {
    await panel.getByLabel("新对话标题", { exact: false }).fill(title);
    await panel.getByRole("button", { name: "创建对话", exact: true }).click();
    await expect(panel.getByRole("heading", { name: title })).toBeVisible();
    await expect(panel.getByRole("button", { name: "发送用户消息" })).toBeEnabled();
  }
  const thread = panel.getByRole("region", { name: "当前对话消息" });
  let writes = 0;
  let release;
  let failedRead = true;
  let delayRead = false;
  await page.route("**/api/conversations/*/messages", async route => {
    if (route.request().method() === "POST") { writes += 1; return route.continue(); }
    if (failedRead) return route.fulfill({ status: 503, json: { error: { code: "unavailable" } } });
    if (delayRead) {
      delayRead = false;
      await new Promise(resolve => { release = resolve; });
      return route.fulfill({ json: { revision:1,deleted:false,messages:[{id:randomUUID(),sequence:1,content:"不应显示的旧消息"}] } }).catch(() => {});
    }
    return route.continue();
  });
  await thread.getByRole("button", { name: "刷新消息" }).click();
  await expect(thread.getByRole("alert")).toContainText("服务暂不可用");
  await expect(thread).not.toContainText("暂无消息");
  await expect(thread.getByRole("button", { name: "发送用户消息" })).toBeDisabled();
  failedRead = false;
  await thread.getByRole("button", { name: "刷新消息" }).click();
  await expect(thread).toContainText("暂无消息");
  await thread.getByLabel("用户消息", { exact: false }).fill("中".repeat(1366));
  await thread.getByRole("button", { name: "发送用户消息" }).click();
  await expect(thread.getByRole("alert")).toContainText("4096 UTF-8 字节");
  expect(writes).toBe(0);
  delayRead = true;
  await thread.getByRole("button", { name: "刷新消息" }).click();
  await expect.poll(() => typeof release).toBe("function");
  await panel.getByRole("button", { name: "对话甲", exact: true }).click();
  await expect(thread.getByRole("heading", { name: "对话甲" })).toBeVisible();
  await expect(thread).toContainText("暂无消息");
  release();
  await expect(thread).not.toContainText("不应显示的旧消息");
  await expect(thread.getByLabel("用户消息", { exact: false })).toHaveValue("");
});

test("long memory supports explicit edits, conflicts, deletion and account isolation", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/");
  await login(page, owner);
  const panel = page.getByRole("region", { name: "长期记忆", exact: true });
  await expect(panel).toContainText("暂无记忆");
  await panel.getByLabel("记忆标题", { exact: false }).fill("回答偏好");
  await panel.getByLabel("记忆内容", { exact: false }).fill('<img src=x onerror="window.memoryInjected=true">');
  const created = page.waitForResponse(response => response.url().endsWith("/api/memories") && response.request().method() === "POST");
  await panel.getByRole("button", { name: "保存记忆", exact: true }).click();
  const response = await created;
  expect(response.status()).toBe(201);
  await expect(panel.locator("li .memoryText")).toHaveText('<img src=x onerror="window.memoryInjected=true">');
  // 页面无需消费创建响应体；通过实际列表读取持久化记录，避免依赖 CDP 响应体缓存。
  const fact = await page.evaluate(async () => (await (await fetch("/api/memories", { cache: "no-store" })).json())[0]);
  expect(fact.title).toBe("回答偏好");
  await expect(panel.locator("img")).toHaveCount(0);
  await page.reload();
  await panel.getByRole("button", { name: "编辑", exact: true }).click();
  await panel.getByLabel("记忆内容", { exact: false }).fill("中文且简洁");
  // 模拟另一个页面先完成修改，当前页面不得覆盖新版本。
  expect(await page.evaluate(async item => (await fetch(`/api/memories/${item.id}`, { method: "PUT", headers: { "content-type": "application/json", "x-requested-with": "personal-ai" }, body: JSON.stringify({ title: item.title, content: "其他页面的新版本", version: item.version }) })).status, fact)).toBe(200);
  await panel.getByRole("button", { name: "保存记忆", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("记录已被修改");
  await expect(panel.getByLabel("记忆内容", { exact: false })).toHaveValue("中文且简洁");
  await panel.getByRole("button", { name: "刷新记忆", exact: true }).click();
  await expect(panel.locator("li .memoryText")).toHaveText("其他页面的新版本");
  await panel.getByRole("button", { name: "编辑", exact: true }).click();
  await panel.getByLabel("记忆内容", { exact: false }).fill("中文且简洁");
  await panel.getByRole("button", { name: "保存记忆", exact: true }).click();
  await expect(panel.locator("li .memoryText")).toHaveText("中文且简洁");
  await panel.screenshot({ path: testInfo.outputPath("long-memory.png") });
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await login(page, other);
  await expect(panel).toContainText("暂无记忆");
  await expect(panel.getByLabel("记忆内容", { exact: false })).toHaveValue("");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, owner);
  await panel.getByRole("button", { name: "删除", exact: true }).click();
  await panel.getByRole("button", { name: "取消删除", exact: true }).click();
  await expect(panel.locator("li")).toHaveCount(1);
  await panel.getByRole("button", { name: "删除", exact: true }).click();
  await panel.getByRole("button", { name: "确认删除", exact: true }).click();
  await expect(panel).toContainText("暂无记忆");
  await page.reload();
  await expect(panel).toContainText("暂无记忆");
});

test("retrieval UI separates failures, renders untrusted text and discards cancelled results", async ({ page }) => {
  const owner = await createAccount();
  const other = await createAccount();
  const unsafe = '<img src=x onerror="window.injected=true">';
  const hit = { document_id: randomUUID(), ordinal: 0, title: "夹具资料", source: "javascript:alert(1)", text: unsafe, score: 0.9 };
  let mode = "answer";
  let calls = 0;
  let release;
  await page.route("**/api/knowledge/*", async route => {
    calls += 1;
    expect(route.request().method()).toBe("POST");
    expect(route.request().headers()["x-requested-with"]).toBe("personal-ai");
    expect(route.request().postDataJSON().query).toBe("资料中的问题");
    if (mode === "delayed") {
      await new Promise(resolve => { release = resolve; });
      await route.fulfill({ json: { status: "answered", answer: "过期答案", citations: [{ ...hit, id: 1 }] } }).catch(() => {});
      return;
    }
    if (mode === "empty") return route.fulfill({ json: { hits: [] } });
    if (mode === "search") return route.fulfill({ json: { hits: [hit] } });
    if (mode === "insufficient") return route.fulfill({ json: { status: "insufficient_evidence", answer: null, citations: [] } });
    if (mode === "invalid") return route.fulfill({ json: { status: "answered", answer: "无引用答案", citations: [] } });
    const errors = { disabled: [503, "answering_disabled"], unavailable: [503, "retrieval_unavailable"], expired: [401, "unauthorized"], limited: [429, "answer_rate_limited"], invalid_answer: [502, "invalid_answer"] };
    if (errors[mode]) return route.fulfill({ status: errors[mode][0], json: { error: { code: errors[mode][1] } } });
    return route.fulfill({ json: { status: "answered", answer: unsafe, citations: [{ ...hit, id: 1 }] } });
  });
  await page.goto("/");
  await login(page, owner);
  const panel = page.getByRole("region", { name: "知识检索与问答", exact: true });
  const input = panel.getByLabel("问题或检索内容", { exact: false });
  await input.fill(" ");
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("1–1000");
  await input.fill("问".repeat(1001));
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  expect(calls).toBe(0);
  await input.fill("资料中的问题");
  await panel.getByRole("button", { name: "生成引用答案", exact: true }).click();
  await expect(panel.locator(".answerText")).toHaveText(unsafe);
  await panel.getByText("查看原文片段", { exact: true }).click();
  await expect(panel.locator("pre")).toHaveText(unsafe);
  await expect(panel.locator("img, a")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  mode = "search";
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  await expect(panel).toContainText("找到 1 个核验片段");
  await expect(panel.locator(".answerText")).toHaveCount(0);
  for (const [nextMode, expected] of [["empty", "未找到匹配"], ["insufficient", "证据不足"], ["disabled", "尚未启用知识问答"], ["unavailable", "服务暂不可用"], ["expired", "登录已失效"], ["limited", "服务繁忙或模型限流"], ["invalid_answer", "未通过引用校验"], ["invalid", "服务返回无效结果"]]) {
    mode = nextMode;
    await panel.getByRole("button", { name: mode === "empty" ? "检索资料" : "生成引用答案", exact: true }).click();
    await expect(panel).toContainText(expected);
    await expect(panel.locator(".answerText")).toHaveCount(0);
  }
  mode = "delayed";
  await panel.getByRole("button", { name: "生成引用答案", exact: true }).click();
  await expect.poll(() => Boolean(release)).toBe(true);
  const beforeCancel = calls;
  await expect(panel.getByRole("button", { name: "检索资料", exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "停止等待" }).click();
  release();
  await expect(panel).toContainText("服务端可能仍在处理并计费");
  await expect(panel.getByRole("region", { name: "本次查询结果" })).toHaveCount(0);
  expect(calls).toBe(beforeCancel);
  mode = "answer";
  await panel.getByRole("button", { name: "生成引用答案", exact: true }).click();
  await expect(panel.locator(".answerText")).toHaveText(unsafe);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await login(page, other);
  await expect(input).toHaveValue("");
  await expect(panel.getByRole("region", { name: "本次查询结果" })).toHaveCount(0);
});

indexTest("retrieval and cited answers use real indexed evidence and isolate accounts", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/");
  await login(page, owner);
  const imported = page.waitForResponse(response => response.url().endsWith("/api/documents") && response.request().method() === "POST");
  await upload(page, "retrieval-live.md", Buffer.from("# 学习方法\n\n每天复习并核对原始资料。"));
  const summary = await (await imported).json();
  const document = await page.evaluate(async id => (await fetch(`/api/documents/${id}`)).json(), summary.id);
  const indexing = page.getByRole("region", { name: "retrieval-live.md的索引", exact: true });
  await indexing.getByRole("button", { name: "建立索引", exact: true }).click();
  await expect(indexing).toContainText("索引完成", { timeout: 30000 });
  const panel = page.getByRole("region", { name: "知识检索与问答", exact: true });
  await panel.getByLabel("问题或检索内容", { exact: false }).fill(document.chunks[0]);
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  await expect(panel).toContainText("找到 1 个核验片段");
  await panel.getByText("查看原文片段", { exact: true }).click();
  await expect(panel.locator("pre")).toHaveText(document.chunks[0]);
  await panel.getByRole("button", { name: "生成引用答案", exact: true }).click();
  await expect(panel.locator(".answerText")).toContainText("依据资料：");
  await expect(panel.getByRole("heading", { name: "[1] retrieval-live.md", exact: true })).toBeVisible();
  const detail = panel.locator("details");
  if (!(await detail.evaluate(element => element.open))) await detail.locator("summary").click();
  await expect(panel.locator("pre")).toHaveText(document.chunks[0]);
  await panel.screenshot({ path: testInfo.outputPath("retrieval-answer.png") });
  await page.reload();
  await expect(panel.getByLabel("问题或检索内容", { exact: false })).toHaveValue("");
  await expect(panel.locator(".answerText")).toHaveCount(0);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await panel.getByLabel("问题或检索内容", { exact: false }).fill(document.chunks[0]);
  const emptySearch = page.waitForResponse(response => response.url().endsWith("/api/knowledge/search"));
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  const isolatedSearch = await emptySearch;
  const isolatedBody = await isolatedSearch.json().catch(() => null);
  expect(isolatedSearch.status(), `隔离检索错误码：${isolatedBody?.error?.code ?? "无"}`).toBe(200);
  await expect(panel).toContainText("未找到匹配");
  await panel.getByRole("button", { name: "生成引用答案", exact: true }).click();
  await expect(panel).toContainText("证据不足");
  await expect(panel.locator("pre, .answerText")).toHaveCount(0);
});

indexTest("index submission completes and survives reload through real services", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/");
  await login(page, owner);
  await upload(page, "index-live.md", Buffer.from("# 索引验收\n\n" + "知识积累。".repeat(500)));
  const panel = page.getByRole("region", { name: "index-live.md的索引", exact: true });
  await expect(panel).toContainText("当前目标暂无索引任务");
  const submitted = page.waitForResponse(response => response.url().endsWith("/index-job") && response.request().method() === "POST");
  await panel.getByRole("button", { name: "建立索引", exact: true }).click();
  const response = await submitted;
  expect(response.status()).toBe(202);
  const job = await response.json();
  await expect(panel).toContainText("索引完成 · 4/4 块", { timeout: 30000 });
  await page.reload();
  await expect(panel).toContainText("索引完成 · 4/4 块");
  await expect(panel.getByRole("button", { name: "建立索引", exact: true })).toBeDisabled();
  await panel.screenshot({ path: testInfo.outputPath("index-completed.png") });
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await expect(panel).toHaveCount(0);
  expect(await page.evaluate(async id => (await fetch(`/api/documents/${id}/index-job`)).status, job.document_id)).toBe(404);
});

// 未启用公网检查时，在测试夹具启动前将用例注册为跳过。
const publicWebTest = process.env.E2E_PUBLIC_WEB === "1" ? test : test.skip;
publicWebTest("public webpage import persists, deduplicates and isolates through the UI", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/");
  await login(page, owner);
  const region = page.getByRole("region", { name: "个人知识库", exact: true });
  async function importPage() {
    await page.getByLabel("网页地址", { exact: true }).fill("https://example.com/");
    const response = page.waitForResponse(r => r.url().endsWith("/api/documents") && r.request().method() === "POST");
    await page.getByRole("button", { name: "导入网页", exact: true }).click();
    return response;
  }
  const response = await importPage();
  expect(response.status(), "Real public fetch must succeed; failures are not skipped").toBe(201);
  const document = await response.json();
  expect(document.source_type).toBe("web_page");
  expect(document.title).toBe("Example Domain");
  expect(document.source).toBe("https://example.com/");
  await expect(metric(page, "文档总数")).toHaveText("1");
  await expect(metric(page, "文本块总数")).toHaveText(String(document.chunk_count));
  await expect(page.getByLabel("网页地址", { exact: true })).toHaveValue("");
  await page.reload();
  await page.getByRole("button", { name: document.title, exact: true }).click();
  const detail = page.getByRole("region", { name: "文档详情" });
  await expect(detail.getByRole("heading", { name: "网页提取文本" })).toBeVisible();
  await expect(detail.locator("pre").first()).toContainText("Example Domain");
  await expect(detail.getByRole("link")).toHaveAttribute("href", document.source);
  const stored = await page.evaluate(async id => (await fetch(`/api/documents/${id}`)).json(), document.id);
  expect(stored.original_html).toBeUndefined();
  expect(stored.chunks.length).toBeGreaterThan(0);
  await region.screenshot({ path: testInfo.outputPath("public-web-import.png") });
  expect((await importPage()).status()).toBe(409);
  await expect(region.getByRole("alert")).toHaveText("这份内容已经导入，无需重复上传。");
  await expect(metric(page, "文档总数")).toHaveText("1");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await expect(metric(page, "文档总数")).toHaveText("0");
  expect(await page.evaluate(async id => (await fetch(`/api/documents/${id}`)).status, document.id)).toBe(404);
  expect((await importPage()).status()).toBe(201);
  await expect(metric(page, "文档总数")).toHaveText("1");
});

test("file import refreshes overview; logout and account switch clear private UI", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "登录你的工作台" })).toBeVisible();
  await expect(page.getByRole("region", { name: "今日概览" })).toHaveCount(0);
  await login(page, owner);
  await expect(metric(page, "文档总数")).toHaveText("0");

  const markdown = "# UI 验收\n\n" + "知识积累。".repeat(500);
  await upload(page, "ui-note.md", Buffer.from(markdown));
  await expect(page.getByText(/已导入「ui-note.md」/)).toBeVisible();
  await expect(metric(page, "文档总数")).toHaveText("1");
  await expect(metric(page, "文本块总数")).toHaveText("4");
  await expect(page.getByLabel("Markdown / PDF 文件", { exact: false })).toHaveValue("");
  await page.getByRole("button", { name: "ui-note.md", exact: true }).click();
  await expect(page.getByRole("region", { name: "文档详情" }).locator("pre").first()).toHaveText(markdown);
  await page.getByText("查看 4 个文本块", { exact: true }).click();
  await expect(page.getByRole("region", { name: "文档详情" }).locator("details pre")).toHaveCount(4);
  await upload(page, "duplicate.md", Buffer.from(markdown));
  await expect(page.getByRole("region", { name: "个人知识库", exact: true }).getByRole("alert")).toHaveText("这份内容已经导入，无需重复上传。");
  await expect(metric(page, "文档总数")).toHaveText("1");
  await page.reload();
  await expect(page.getByRole("button", { name: "ui-note.md", exact: true })).toBeVisible();
  await expect(metric(page, "文档总数")).toHaveText("1");
  const screenshot = testInfo.outputPath("overview.png");
  await page.getByRole("region", { name: "今日概览" }).screenshot({ path: screenshot });
  await testInfo.attach("overview", { path: screenshot, contentType: "image/png" });

  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(page.getByRole("heading", { name: "登录你的工作台" })).toBeVisible();
  for (const name of ["今日概览", "个人知识库", "文档详情"]) {
    await expect(page.getByRole("region", { name, exact: true })).toHaveCount(0);
  }
  await login(page, other);
  await expect(metric(page, "文档总数")).toHaveText("0");
  await expect(page.getByText("暂无文档。", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "ui-note.md", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await page.reload();
  await expect(page.getByRole("heading", { name: "登录你的工作台" })).toBeVisible();
  expect(errors).toEqual([]);
});

test("invalid files are rejected and overview failure can be retried", async ({ page }) => {
  const owner = await createAccount();
  await page.goto("/");
  await login(page, owner);
  await expect(metric(page, "文档总数")).toHaveText("0");
  await upload(page, "invalid.md", Buffer.from([0xff, 0xfe, 0xff]));
  await expect(page.getByRole("region", { name: "个人知识库", exact: true }).getByRole("alert")).toHaveText("请将文件保存为 UTF-8 编码后重新上传。");
  await upload(page, "oversize.md", Buffer.alloc(256 * 1024 + 1, 97));
  await expect(page.getByRole("region", { name: "个人知识库", exact: true }).getByRole("alert")).toHaveText("文件太大，请选择不超过 256 KiB 的 Markdown。");
  await expect(metric(page, "文档总数")).toHaveText("0");

  // 仅模拟此故障，正常请求均使用真实 API 和数据库。
  await page.route("**/api/overview", route => route.fulfill({ status: 503, json: { error: { code: "storage_unavailable" } } }));
  await page.getByRole("button", { name: "刷新概览", exact: true }).click();
  const overview = page.getByRole("region", { name: "今日概览" });
  await expect(overview.getByRole("alert")).toHaveText("概览暂不可用，请重试。");
  await expect(metric(page, "文档总数")).toHaveCount(0);
  await page.unroute("**/api/overview");
  await overview.getByRole("button", { name: "刷新概览", exact: true }).click();
  await expect(metric(page, "文档总数")).toHaveText("0");
  await expect(overview.getByRole("alert")).toHaveCount(0);
});

function pdfFile(text, padding = 0, pages = 1) {
  const stream = `BT /F1 1 Tf 50 750 Td (${text}) Tj ET`;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`,
  ];
  const kids = ["3 0 R"];
  for (let page = 1; page < pages; page++) {
    const id = objects.length + 1;
    kids.push(`${id} 0 R`);
    objects.push(objects[2].replace("/Contents 5 0 R", `/Contents ${id + 1} 0 R`), objects[4]);
  }
  objects[1] = `<< /Type /Pages /Kids [${kids.join(" ")}] /Count ${pages} >>`;
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets.slice(1)) pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\n`;
  // 使用较大的合法 PDF 注释，验证各层代理的请求大小限制。
  if (padding) pdf += "%" + "x".repeat(padding) + "\n";
  pdf += `startxref\n${xref}\n%%EOF\n`;
  return Buffer.from(pdf);
}

test("PDF import extracts literal text, deduplicates and isolates; invalid PDFs leave no data", async ({ page }, testInfo) => {
  const owner = await createAccount();
  const other = await createAccount();
  await page.goto("/");
  await login(page, owner);
  const text = "PDF knowledge **literal**";
  const pdf = pdfFile(text, 2 * 1024 * 1024);
  await upload(page, "knowledge.pdf", pdf);
  await expect(page.getByText(/已导入「knowledge.pdf」/)).toBeVisible();
  await expect(metric(page, "文档总数")).toHaveText("1");
  await expect(metric(page, "文本块总数")).toHaveText("1");
  await page.reload();
  await page.getByRole("button", { name: "knowledge.pdf", exact: true }).click();
  await expect(page.getByRole("heading", { name: "PDF 提取文本" })).toBeVisible();
  await expect(page.getByRole("region", { name: "文档详情" }).locator("pre").first()).toHaveText(text);
  await page.getByText("查看 1 个文本块", { exact: true }).click();
  await expect(page.getByRole("region", { name: "文档详情" }).locator("details pre")).toHaveText(text);
  const stored = await page.evaluate(async () => {
    const documents = await (await fetch("/api/documents")).json();
    return (await fetch("/api/documents/" + documents[0].id)).json();
  });
  expect(stored.source_type).toBe("pdf");
  expect(stored.original_pdf).toBeUndefined();
  await page.getByRole("region", { name: "个人知识库", exact: true }).screenshot({ path: testInfo.outputPath("pdf-import.png") });
  const alert = page.getByRole("region", { name: "个人知识库", exact: true }).getByRole("alert");
  await upload(page, "duplicate.pdf", pdf);
  await expect(alert).toHaveText("这份内容已经导入，无需重复上传。");
  await upload(page, "broken.pdf", Buffer.from("%PDF-1.4 broken"));
  await expect(alert).toHaveText("PDF 无法读取，请检查文件是否损坏或需要密码。");
  await upload(page, "blank.pdf", pdfFile(""));
  await expect(alert).toHaveText("PDF 中没有可提取文字；扫描件请先进行 OCR。");
  await upload(page, "huge.pdf", Buffer.alloc(5 * 1024 * 1024 + 1));
  await expect(alert).toHaveText("PDF 太大，请选择不超过 5 MiB 的文件。");
  await upload(page, "too-much-text.pdf", pdfFile("x".repeat(1000), 0, 1100));
  await expect(alert).toHaveText("PDF 或提取文本过大，请拆分文件后重试。");
  await page.getByRole("button", { name: "刷新概览", exact: true }).click();
  await expect(metric(page, "文档总数")).toHaveText("1");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await expect(metric(page, "文档总数")).toHaveText("0");
  await expect(page.getByRole("button", { name: "knowledge.pdf", exact: true })).toHaveCount(0);
  await upload(page, "knowledge.pdf", pdf);
  await expect(page.getByText(/已导入「knowledge.pdf」/)).toBeVisible();
});

test("web import rejects private URLs without adding documents", async ({ page }, testInfo) => {
  await page.goto("/");
  await login(page, await createAccount());
  const region = page.getByRole("region", { name: "个人知识库", exact: true });
  await page.getByLabel("网页地址", { exact: true }).fill("http://127.0.0.1/private");
  await page.getByRole("button", { name: "导入网页", exact: true }).click();
  await expect(region.getByRole("alert")).toHaveText("仅支持公开网页，不能导入本机或内网地址。");
  await page.getByLabel("网页地址", { exact: true }).fill("https://example.com:8080/");
  await page.getByRole("button", { name: "导入网页", exact: true }).click();
  await expect(region.getByRole("alert")).toHaveText("请输入不含登录信息的 HTTP 或 HTTPS 网页地址（使用默认端口）。");
  await region.screenshot({ path: testInfo.outputPath("web-import.png") });
  await page.reload();
  await expect(metric(page, "文档总数")).toHaveText("0");
  await expect(page.getByRole("button", { name: "导入网页", exact: true })).toBeEnabled();
});
