import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { createAccount, login } from "./account.mjs";
const base = "/api/knowledge/answer-authorizations";
const panel = page => page.getByRole("region", { name: "本地问答授权", exact: true });
const indexTest = process.env.E2E_INDEX === "1" ? test : test.skip;

indexTest("exact consent persists and cancels without dispatching inference", async ({ page }, testInfo) => {
  const owner = await createAccount(); const other = await createAccount();
  await page.goto("/"); await login(page, owner);
  const imported = page.waitForResponse(r => r.url().endsWith("/api/documents") && r.request().method() === "POST");
  await page.getByLabel("Markdown / PDF 文件", { exact: false }).setInputFiles({ name: "authorization.md", mimeType: "text/markdown", buffer: Buffer.from("# 授权资料\n\n会议安排在周五。") });
  await page.getByRole("button", { name: "导入文档", exact: true }).click();
  const summary = await (await imported).json();
  const document = await page.evaluate(async id => (await fetch(`/api/documents/${id}`)).json(), summary.id);
  const indexing = page.getByRole("region", { name: "authorization.md的索引", exact: true });
  await indexing.getByRole("button", { name: "建立索引", exact: true }).click();
  await expect(indexing).toContainText("索引完成", { timeout: 30000 });
  const retrieval = page.getByRole("region", { name: "知识检索与问答", exact: true });
  await retrieval.getByLabel("问题或检索内容", { exact: false }).fill(document.chunks[0]);
  await retrieval.getByRole("button", { name: "检索资料", exact: true }).click();
  await expect(retrieval).toContainText("找到 1 个核验片段");
  let dispatches = 0;
  page.on("request", r => { if (/\/api\/knowledge\/(answer|search)$|\/execute$|\/v1\/chat\/completions$/.test(new URL(r.url()).pathname)) dispatches++; });
  await panel(page).getByRole("checkbox").check();
  const created = page.waitForResponse(r => r.url().endsWith(base) && r.request().method() === "POST");
  await panel(page).getByRole("button", { name: "创建精确预览" }).click();
  const grant = await (await created).json();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText(document.chunks[0]);
  await expect(detail.getByRole("button", { name: "保存一次授权" })).toBeDisabled();
  await detail.getByLabel("我同意将当前问题", { exact: false }).check();
  await expect(detail.getByRole("button", { name: "保存一次授权" })).toBeDisabled();
  await detail.getByLabel("我理解未来执行", { exact: false }).check();
  await detail.getByText("查看完整模型请求与指纹").click();
  await expect(detail).toContainText("local-knowledge-answer-v2");
  await panel(page).screenshot({ path: testInfo.outputPath("answer-authorization.png") });
  await detail.getByRole("button", { name: "保存一次授权" }).click();
  await expect(detail).toContainText("状态：已保存授权");
  await page.reload();
  await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
  await panel(page).getByRole("button", { name: `已保存授权 · ${grant.request_id}`, exact: true }).click();
  await expect(detail).toContainText("状态：已保存授权");
  await detail.getByRole("button", { name: "取消该授权" }).click();
  await expect(detail).toContainText("状态：已取消");
  await expect(detail.locator("pre")).toHaveCount(0);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
  await expect(panel(page).getByRole("list", { name: "最近授权记录" }).locator("li")).toHaveCount(0);
  const foreign = await page.evaluate(async url => (await fetch(url)).status, `${base}/${grant.request_id}`);
  expect(foreign).toBe(404);
  expect(dispatches).toBe(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("lost preview response recovers original request and invalidation clears consent", async ({ page }) => {
  const owner = await createAccount();
  const unsafe = '<img src=x onerror="window.injected=true">';
  const hit = { document_id: randomUUID(), ordinal: 0, title: "夹具材料", source: "fixture.md", text: unsafe, score: 1 };
  let saved; let creations = 0; let approvals = 0; let invalidated = false;
  await page.route("**/api/knowledge/search", r => r.fulfill({ json: { hits: [hit] } }));
  await page.route("**/api/knowledge/answer-authorizations**", async route => {
    const request = route.request(); const path = new URL(request.url()).pathname;
    if (request.method() === "POST" && path === base) {
      creations++; const input = request.postDataJSON();
      expect(input.sources).toEqual([{ document_id: hit.document_id, ordinal: 0 }]);
      saved = { request_id: input.request_id, status: "draft", digest: "a".repeat(64), created_at_unix_ms: Date.now(), expires_at_unix_ms: Date.now() + 600000,
        preview: { question: input.question, materials: [hit], request_sha256: "b".repeat(64), request: { endpoint: "http://127.0.0.1:11435/v1/chat/completions", profile: "local-knowledge-answer-v2", body: { model: "fixture", messages: [{ content: unsafe }] } } } };
      await route.abort("failed"); return;
    }
    if (path.endsWith("/approve")) { approvals++; invalidated = true; await route.fulfill({ status: 409, json: { error: { code: "answer_authorization_conflict" } } }); return; }
    expect(path).toBe(`${base}/${saved.request_id}`);
    await route.fulfill({ json: invalidated ? { ...saved, status: "invalidated", preview: null } : saved });
  });
  await page.goto("/"); await login(page, owner);
  await page.getByLabel("问题或检索内容", { exact: false }).fill("夹具问题");
  await page.getByRole("button", { name: "检索资料", exact: true }).click();
  await panel(page).getByRole("checkbox").check();
  await panel(page).getByRole("button", { name: "创建精确预览" }).click();
  await expect(panel(page).getByRole("alert")).toContainText("网络异常");
  await expect(panel(page).getByRole("button", { name: "创建精确预览" })).toBeDisabled();
  await panel(page).getByRole("button", { name: "查询原请求" }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText(unsafe);
  await expect(detail.locator("img, a")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  await detail.getByLabel("我同意将当前问题", { exact: false }).check();
  await detail.getByLabel("我理解未来执行", { exact: false }).check();
  await detail.getByRole("button", { name: "保存一次授权" }).click();
  await expect(detail).toHaveCount(0);
  await panel(page).getByRole("button", { name: "查询原请求" }).click();
  await expect(detail).toContainText("状态：来源已失效");
  await expect(detail.getByRole("checkbox")).toHaveCount(0);
  await expect(detail.locator("pre")).toHaveCount(0);
  expect(creations).toBe(1); expect(approvals).toBe(1);
});
