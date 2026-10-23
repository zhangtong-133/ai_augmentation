import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { createAccount, login } from "./account.mjs";
const base = "/api/knowledge/answer-authorizations";
const panel = page => page.getByRole("region", { name: "本地问答授权", exact: true });
const indexTest = process.env.E2E_INDEX === "1" ? test : test.skip;

for (const backend of ["local", "ollama"]) indexTest(`${backend} exact consent persists and cancels without dispatching inference`, async ({ page }, testInfo) => {
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
  if (backend === "ollama") await panel(page).getByRole("combobox", { name: "本地运行器", exact: true }).selectOption("ollama");
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
  const profile = backend === "ollama" ? "ollama-local-answer-v1" : "local-knowledge-answer-v2";
  await expect(detail).toContainText(profile);
  expect(grant.preview.request.profile).toBe(profile);
  if (backend === "ollama") {
    expect(grant.preview.request.endpoint).toBe("http://127.0.0.1:11434/api/chat");
    expect(grant.preview.request.body.model).toBe("qwen3.5:9b");
    expect(grant.preview.request.body.think).toBe(false);
    expect(grant.preview.request.body.keep_alive).toBe(30);
  }
  await panel(page).screenshot({ path: testInfo.outputPath("answer-authorization.png") });
  await detail.getByRole("button", { name: "保存一次授权" }).click();
  await expect(detail).toContainText("状态：已保存授权");
  await page.reload();
  await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
  await panel(page).getByRole("button", { name: `已保存授权 · ${grant.request_id}`, exact: true }).click();
  await expect(detail).toContainText("状态：已保存授权");
  expect(dispatches).toBe(0);
  expect(grant.execution_available).toBe(false);
  const closed = await page.evaluate(async ({ url, digest }) => {
    const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify({ digest }) });
    return { status: response.status, body: await response.json() };
  }, { url: `${base}/${grant.request_id}/execute`, digest: grant.digest });
  expect(closed.status).toBe(503);
  expect(closed.body.error.code).toBe("answer_execution_disabled");
  const unchanged = await page.evaluate(async url => (await fetch(url)).json(), `${base}/${grant.request_id}`);
  expect(unchanged.status).toBe("authorized"); expect(unchanged.execution).toBeNull();
  await detail.getByRole("button", { name: "取消该授权" }).click();
  await expect(detail).toContainText("状态：已取消");
  await expect(detail.locator("pre")).toHaveCount(0);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other);
  await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
  await expect(panel(page).getByRole("list", { name: "最近授权记录" }).locator("li")).toHaveCount(0);
  const foreign = await page.evaluate(async url => (await fetch(url)).status, `${base}/${grant.request_id}`);
  expect(foreign).toBe(404);
  expect(dispatches).toBe(1); // Only the explicit, disabled execute request above.
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
      saved = { request_id: input.request_id, status: "draft", digest: "a".repeat(64), created_at_unix_ms: Date.now(), expires_at_unix_ms: Date.now() + 600000, execution_available: false, execution: null,
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

// These fixtures exercise the browser contract; the real database/runtime flow is
// covered by the API acceptance suite, without calling a real model.
async function executionFixture(page, { initial = "authorized", available = true, mode = "normal", backend = "local" } = {}) {
  const unsafe = '<img src=x onerror="window.injected=true">';
  const quote = "会议安排在周五。";
  const hit = { document_id: randomUUID(), ordinal: 0, title: "执行夹具", source: unsafe, text: `😀${quote}尾声 ${unsafe}`, score: 1 };
  const answer = { status: "answered", answer: `${unsafe}\n会议安排在周五。[1]`, citations: [{ ...hit, id: 1, quote, quote_start: 1, quote_end: 1 + Array.from(quote).length }] };
  const create = (id = randomUUID(), status = "authorized", question = "何时开会？", runtime = backend) => ({
    request_id: id, status, digest: "a".repeat(64), created_at_unix_ms: Date.now(), expires_at_unix_ms: Date.now() + 600000,
    execution_available: available, execution: null,
    preview: { question, materials: [hit], request_sha256: "b".repeat(64), request: runtime === "ollama"
      ? { endpoint: "http://127.0.0.1:11434/api/chat", profile: "ollama-local-answer-v1", body: {
        model: "qwen3.5:9b", runner: "llamacpp", stream: true, think: false, keep_alive: 30,
        format: { type: "object" }, options: { num_ctx: 8192, num_predict: 2048, temperature: 0,
          seed: 0, top_k: 1, top_p: 1, repeat_penalty: 1, presence_penalty: 0, frequency_penalty: 0 },
        truncate: false, shift: false, messages: [{ content: hit.text }],
      } }
      : { endpoint: "http://127.0.0.1:11435/v1/chat/completions", profile: "local-knowledge-answer-v2", body: { model: "fixture", messages: [{ content: hit.text }] } } },
  });
  const control = { current: initial ? create(undefined, initial) : null, calls: [], answer, hit, release: null, read: null, reads: 0, releaseRead: null };
  const question = () => control.current.preview?.question ?? control.current.execution?.question ?? "何时开会？";
  control.complete = (result = structuredClone(answer)) => {
    const savedQuestion = question();
    const execution = control.current.execution;
    control.current.status = "consumed"; control.current.preview = null;
    control.current.execution = { request_sha256: "b".repeat(64), started_at_unix_ms: Date.now(), deadline_at_unix_ms: Date.now() + 90000,
      ...execution, status: "succeeded",
      sent_at_unix_ms: Date.now(), finished_at_unix_ms: Date.now(), question: savedQuestion, result };
  };
  control.terminal = status => {
    control.complete(null); control.current.execution.status = status; control.current.execution.question = null;
  };
  control.holdNextRead = () => {
    control.read = async route => {
      control.read = null;
      const late = structuredClone(control.current);
      late.execution.status = "succeeded"; late.execution.result = answer; late.execution.question = "何时开会？";
      late.execution.finished_at_unix_ms = Date.now();
      await new Promise(resolve => { control.releaseRead = async () => { await route.fulfill({ json: late }).catch(() => {}); resolve(); }; });
    };
  };
  await page.route("**/api/knowledge/search", route => route.fulfill({ json: { hits: [hit] } }));
  await page.route("**/api/knowledge/answer-authorizations**", async route => {
    const request = route.request(); const path = new URL(request.url()).pathname;
    control.calls.push({ method: request.method(), path, input: request.method() === "POST" ? request.postDataJSON() : null });
    if (path === base && request.method() === "POST") {
      const input = request.postDataJSON(); control.current = create(input.request_id, "draft", input.question, input.backend ?? "local");
    } else if (path === base) {
      const items = control.current ? [{ ...control.current, preview: null, execution: control.current.execution ? { ...control.current.execution, question: null, result: null } : null }] : [];
      await route.fulfill({ json: { execution_available: available, items } }); return;
    } else if (path.endsWith("/approve")) {
      expect(request.postDataJSON()).toEqual({ digest: control.current.digest, acknowledge_sharing: true, acknowledge_local_compute: true });
      control.current.status = "authorized";
    } else if (path.endsWith("/execute")) {
      expect(request.postDataJSON()).toEqual({ digest: control.current.digest });
      expect(control.current.status).toBe("authorized");
      if (mode === "target_changed") {
        await route.fulfill({ status: 409, json: { error: { code: "answer_execution_target_changed" } } }); return;
      }
      control.terminal("running"); control.current.execution.sent_at_unix_ms = null; control.current.execution.finished_at_unix_ms = null;
      if (mode === "lost") { control.terminal("unknown"); await route.abort("failed"); return; }
      if (mode === "held") {
        // A late valid body must not undo the cancellation that was already read.
        const late = structuredClone(control.current);
        late.execution.status = "succeeded"; late.execution.result = answer; late.execution.question = "何时开会？";
        await new Promise(resolve => { control.release = async () => { await route.fulfill({ status: 202, json: late }).catch(() => {}); resolve(); }; });
        return;
      }
      await route.fulfill({ status: 202, json: control.current }); return;
    } else if (path.endsWith("/cancel")) {
      if (control.current.execution) control.terminal("cancelled");
      else { control.current.status = "cancelled"; control.current.preview = null; }
    } else {
      expect(path).toBe(`${base}/${control.current.request_id}`);
      control.reads++;
      if (control.read) { await control.read(route); return; }
    }
    await route.fulfill({ json: control.current });
  });
  const owner = await createAccount(); await page.goto("/"); await login(page, owner);
  control.open = async () => {
    await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
    await panel(page).getByRole("button", { name: new RegExp(`${control.current.request_id}$`) }).click();
  };
  control.executions = () => control.calls.filter(call => call.path.endsWith("/execute")).length;
  return control;
}

test("a separate execution click persists text and Unicode citations through refresh and deletion", async ({ page }, testInfo) => {
  const fixture = await executionFixture(page, { initial: null });
  await page.getByLabel("问题或检索内容", { exact: false }).fill("何时开会？");
  await page.getByRole("button", { name: "检索资料", exact: true }).click();
  await panel(page).getByRole("checkbox").check();
  await panel(page).getByRole("button", { name: "创建精确预览" }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await detail.getByLabel("我同意将当前问题", { exact: false }).check();
  await detail.getByLabel("我理解未来执行", { exact: false }).check();
  await detail.getByRole("button", { name: "保存一次授权" }).click();
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toBeEnabled();
  expect(fixture.executions()).toBe(0);
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail).toContainText("正在执行一次问答");
  await expect(detail).toContainText("正在自动跟踪");
  await panel(page).screenshot({ path: testInfo.outputPath("answer-execution-tracking.png") });
  expect(fixture.executions()).toBe(1);
  fixture.complete();
  await expect(detail).toContainText(fixture.answer.answer);
  await expect(panel(page).getByRole("list", { name: "最近授权记录" })).toContainText("结果已保存，待核对");
  await expect(detail).toContainText("模型结果，待你核对");
  await expect(detail.locator("img, a, script")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  await detail.getByText("查看引用原文与位置", { exact: true }).click();
  await expect(detail.locator("mark")).toHaveText(fixture.answer.citations[0].quote);
  await panel(page).screenshot({ path: testInfo.outputPath("answer-execution-succeeded.png") });
  await page.reload(); await fixture.open();
  await expect(detail).toContainText(fixture.answer.answer);
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  expect(fixture.executions()).toBe(1);
  await detail.getByRole("button", { name: "删除本次结果" }).click();
  await expect(detail).toContainText("已取消执行，结果已清除");
  await expect(detail.locator("pre, blockquote, .answerText")).toHaveCount(0);
  await detail.getByRole("button", { name: "查询执行状态" }).click();
  await expect(detail.locator(".answerText")).toHaveCount(0);
  expect(fixture.executions()).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("Ollama preview requires its own confirmation before one native execution and result recovery", async ({ page }, testInfo) => {
  const fixture = await executionFixture(page, { initial: null, backend: "ollama" });
  await page.getByLabel("问题或检索内容", { exact: false }).fill("何时开会？");
  await page.getByRole("button", { name: "检索资料", exact: true }).click();
  await panel(page).getByRole("checkbox").check();
  await panel(page).getByRole("combobox", { name: "本地运行器", exact: true }).selectOption("ollama");
  await expect(panel(page).getByLabel("本地服务地址", { exact: true })).toHaveValue("http://127.0.0.1:11434");
  await expect(panel(page).getByLabel("本地模型名称", { exact: true })).toHaveValue("qwen3.5:9b");
  await panel(page).getByRole("button", { name: "创建精确预览" }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText("请求协议：ollama-local-answer-v1");
  const input = fixture.calls.find(call => call.path === base && call.method === "POST").input;
  expect(input.backend).toBe("ollama"); expect(input.endpoint).toBe("http://127.0.0.1:11434"); expect(input.model).toBe("qwen3.5:9b");
  expect(input.sources).toEqual([{ document_id: fixture.hit.document_id, ordinal: 0 }]);
  await expect(detail).toContainText("http://127.0.0.1:11434/api/chat");
  await detail.getByText("查看完整模型请求与指纹").click();
  await expect(detail).toContainText('"think": false'); await expect(detail).toContainText('"keep_alive": 30');
  await expect(detail.getByRole("button", { name: "保存一次授权" })).toBeDisabled();
  await detail.getByLabel("我同意将当前问题", { exact: false }).check();
  await expect(detail.getByRole("button", { name: "保存一次授权" })).toBeDisabled();
  await detail.getByLabel("我理解未来执行", { exact: false }).check();
  await panel(page).screenshot({ path: testInfo.outputPath("ollama-answer-preview.png") });
  await detail.getByRole("button", { name: "保存一次授权" }).click();
  expect(fixture.executions()).toBe(0);
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail).toContainText("正在执行一次问答"); fixture.complete();
  await expect(detail).toContainText(fixture.answer.answer);
  await page.reload(); await fixture.open();
  await expect(detail).toContainText(fixture.answer.answer);
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  await detail.getByRole("button", { name: "删除本次结果" }).click();
  await expect(detail).toContainText("已取消执行，结果已清除");
  expect(fixture.executions()).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("changing the runtime preset never rewrites an existing authorization or retries a mismatched execution", async ({ page }) => {
  const fixture = await executionFixture(page, { mode: "target_changed" });
  await page.getByLabel("问题或检索内容", { exact: false }).fill("何时开会？");
  await page.getByRole("button", { name: "检索资料", exact: true }).click();
  await fixture.open();
  await panel(page).getByRole("combobox", { name: "本地运行器", exact: true }).selectOption("ollama");
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText("请求协议：local-knowledge-answer-v2");
  await expect(detail).toContainText("http://127.0.0.1:11435/v1/chat/completions");
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(panel(page).getByRole("alert")).toContainText("当前执行目标与已确认请求不同");
  await panel(page).getByRole("button", { name: "查询原请求" }).click();
  await expect(detail).toContainText("请求协议：local-knowledge-answer-v2");
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toBeDisabled();
  expect(fixture.calls.filter(call => call.path === base && call.method === "POST")).toHaveLength(0);
  expect(fixture.executions()).toBe(1);
});

test("a lost execution response recovers unknown without offering another send", async ({ page }, testInfo) => {
  const fixture = await executionFixture(page, { mode: "lost" }); await fixture.open();
  await panel(page).getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(panel(page).getByRole("alert")).toContainText("系统不会自动重发");
  await panel(page).getByRole("button", { name: "查询原请求" }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText("结果未知，不会重发");
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  await panel(page).screenshot({ path: testInfo.outputPath("answer-execution-unknown.png") });
  await page.reload(); await fixture.open();
  await expect(detail).toContainText("结果未知，不会重发");
  expect(fixture.executions()).toBe(1);
});

test("cancelling an in-flight execution suppresses its late result", async ({ page }, testInfo) => {
  const fixture = await executionFixture(page, { mode: "held" }); await fixture.open();
  await panel(page).getByRole("button", { name: "执行一次", exact: true }).click();
  await expect.poll(() => typeof fixture.release).toBe("function");
  await panel(page).getByRole("button", { name: "取消该授权", exact: true }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText("已取消执行，结果已清除");
  await fixture.release();
  await expect(detail.locator("pre, blockquote, .answerText")).toHaveCount(0);
  await detail.getByRole("button", { name: "查询执行状态" }).click();
  await expect(detail).toContainText("已取消执行，结果已清除");
  await panel(page).screenshot({ path: testInfo.outputPath("answer-execution-cancelled.png") });
  expect(fixture.executions()).toBe(1);
});

test("disabled, failed and consumed grants never expose a send action", async ({ page }) => {
  const fixture = await executionFixture(page, { available: false }); await fixture.open();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText("当前部署尚未启用");
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  fixture.terminal("failed"); await fixture.open();
  await expect(detail).toContainText("执行失败，不会重发");
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  fixture.complete({ status: "insufficient_evidence", answer: null, citations: [] }); await fixture.open();
  await expect(detail).toContainText("证据不足，模型未给出答案");
  await expect(detail.locator(".answerText, blockquote")).toHaveCount(0);
  fixture.current.execution = null; await fixture.open();
  await expect(detail).toContainText("此授权已领取，不能再次执行");
  await expect(detail.getByRole("button", { name: "执行一次", exact: true })).toHaveCount(0);
  expect(fixture.executions()).toBe(0);
});

test("logout suppresses an execution body arriving in the previous account", async ({ page }) => {
  const fixture = await executionFixture(page, { mode: "held" }); await fixture.open();
  await panel(page).getByRole("button", { name: "执行一次", exact: true }).click();
  await expect.poll(() => typeof fixture.release).toBe("function");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await fixture.release();
  await expect(panel(page)).toHaveCount(0);
  await expect(page.getByText(fixture.answer.answer, { exact: true })).toHaveCount(0);
  const other = await createAccount(); await login(page, other);
  await expect(panel(page).getByRole("region", { name: "精确授权详情" })).toHaveCount(0);
  expect(fixture.executions()).toBe(1);
});

test("invalid citation positions reject the whole body and source invalidation clears a restored result", async ({ page }) => {
  const fixture = await executionFixture(page);
  fixture.complete();
  // The emoji uses two UTF-16 units, but quote positions are Unicode scalars.
  fixture.current.execution.result.citations[0].quote_start++;
  fixture.current.execution.result.citations[0].quote_end++;
  await fixture.open();
  await expect(panel(page).getByRole("alert")).toContainText("格式异常");
  await expect(panel(page).locator(".answerText, blockquote")).toHaveCount(0);
  fixture.complete();
  await panel(page).getByRole("button", { name: "查询原请求" }).click();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await expect(detail).toContainText(fixture.answer.answer);
  fixture.terminal("invalidated");
  await detail.getByRole("button", { name: "查询执行状态" }).click();
  await expect(detail).toContainText("来源已失效，结果已清除");
  await expect(detail.locator("pre, blockquote, .answerText")).toHaveCount(0);
  expect(fixture.executions()).toBe(0);
});

for (const pause of ["manual", "hidden", "offline"]) test(`${pause} pause requires an explicit resume and never cancels or repeats execution`, async ({ page, context }) => {
  const fixture = await executionFixture(page); await fixture.open();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail.getByRole("button", { name: "暂停自动跟踪" })).toBeVisible();
  if (pause === "manual") await detail.getByRole("button", { name: "暂停自动跟踪" }).click();
  else if (pause === "hidden") await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  else await context.setOffline(true);
  await expect(detail.getByRole("button", { name: "继续自动跟踪" })).toBeVisible();
  if (pause === "hidden") await page.evaluate(() => {
    delete document.hidden; document.dispatchEvent(new Event("visibilitychange"));
  });
  if (pause === "offline") await context.setOffline(false);
  await detail.getByRole("button", { name: "查询执行状态" }).click();
  await expect(detail).toContainText("当前状态已核对，自动跟踪保持暂停");
  await expect(detail.getByRole("button", { name: "继续自动跟踪" })).toBeEnabled();
  fixture.complete(); const reads = fixture.reads;
  // More than one read interval: a pause must not silently restart on focus/network recovery.
  await page.waitForTimeout(2500);
  expect(fixture.reads).toBe(reads);
  await expect(detail.locator(".answerText")).toHaveCount(0);
  await detail.getByRole("button", { name: "继续自动跟踪" }).click();
  await expect(detail).toContainText(fixture.answer.answer);
  const finishedReads = fixture.reads;
  await page.waitForTimeout(2500);
  expect(fixture.reads).toBe(finishedReads);
  expect(fixture.executions()).toBe(1);
  expect(fixture.calls.filter(call => call.path.endsWith("/cancel"))).toHaveLength(0);
});

test("reload keeps history private until opened and resumes a running execution with reads only", async ({ page }) => {
  const fixture = await executionFixture(page); await fixture.open();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail).toContainText("正在自动跟踪");
  await page.reload(); const reads = fixture.reads;
  await panel(page).getByRole("button", { name: "刷新授权记录" }).click();
  await expect(panel(page).getByRole("list", { name: "最近授权记录" })).toContainText("正在执行一次问答");
  await page.waitForTimeout(2500);
  expect(fixture.reads).toBe(reads);
  await expect(detail).toHaveCount(0);
  await panel(page).getByRole("button", { name: new RegExp(`${fixture.current.request_id}$`) }).click();
  await expect(detail).toContainText("正在自动跟踪");
  fixture.complete();
  await expect(detail).toContainText(fixture.answer.answer);
  expect(fixture.executions()).toBe(1);
});

for (const action of ["cancel", "close", "logout"]) test(`${action} discards a pending status read and its late answer`, async ({ page }) => {
  const fixture = await executionFixture(page); await fixture.open();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail).toContainText("正在自动跟踪");
  fixture.holdNextRead();
  await expect.poll(() => typeof fixture.releaseRead).toBe("function");
  if (action === "cancel") await detail.getByRole("button", { name: "取消该授权", exact: true }).click();
  else if (action === "close") await detail.getByRole("button", { name: "关闭授权详情" }).click();
  else await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await fixture.releaseRead();
  if (action === "cancel") await expect(detail).toContainText("已取消执行，结果已清除");
  else await expect(detail).toHaveCount(0);
  const reads = fixture.reads; await page.waitForTimeout(2500);
  expect(fixture.reads).toBe(reads);
  await expect(page.getByText(fixture.answer.answer, { exact: true })).toHaveCount(0);
  if (action === "logout") {
    const other = await createAccount(); await login(page, other);
    await expect(panel(page).getByRole("region", { name: "精确授权详情" })).toHaveCount(0);
  }
  expect(fixture.executions()).toBe(1);
});

for (const fault of [401, 404, 503, "digest", "citation"]) test(`automatic observation stops on ${fault} and only an explicit read can recover`, async ({ page }) => {
  const fixture = await executionFixture(page); await fixture.open();
  const detail = panel(page).getByRole("region", { name: "精确授权详情" });
  await detail.getByRole("button", { name: "执行一次", exact: true }).click();
  await expect(detail).toContainText("正在自动跟踪");
  fixture.complete();
  fixture.read = async route => {
    if (typeof fault === "number") await route.fulfill({ status: fault, json: { error: { code: "unavailable" } } });
    else {
      const invalid = structuredClone(fixture.current);
      if (fault === "digest") invalid.digest = "c".repeat(64);
      else invalid.execution.result.citations[0].quote_start++;
      await route.fulfill({ json: invalid });
    }
  };
  await expect(panel(page).getByRole("alert")).toContainText(fault === 401 ? "登录已失效" : fault === 404 ? "未找到该请求" : fault === 503 ? "自动跟踪已停止" : "状态格式异常");
  await expect(detail).toHaveCount(0);
  await expect(panel(page).locator(".answerText, blockquote")).toHaveCount(0);
  const reads = fixture.reads; await page.waitForTimeout(2500);
  expect(fixture.reads).toBe(reads);
  if ([401, 404].includes(fault)) {
    await expect(panel(page).getByRole("button", { name: "查询原请求" })).toHaveCount(0);
    if (fault === 401) await expect(panel(page).getByRole("list", { name: "最近授权记录" }).locator("li")).toHaveCount(0);
  } else {
    fixture.read = null;
    await panel(page).getByRole("button", { name: "查询原请求" }).click();
    await expect(detail).toContainText(fixture.answer.answer);
  }
  expect(fixture.executions()).toBe(1);
});
