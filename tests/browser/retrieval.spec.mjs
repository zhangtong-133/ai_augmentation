import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { createAccount, login } from "./account.mjs";

const indexTest = process.env.E2E_INDEX === "1" ? test : test.skip;
async function upload(page, name, buffer) {
  await page.getByLabel("Markdown / PDF 文件", { exact: false }).setInputFiles({ name, mimeType: "text/markdown", buffer });
  await page.getByRole("button", { name: "导入文档", exact: true }).click();
}

test("retrieval UI separates failures, renders untrusted text and discards cancelled results", async ({ page }) => {
  const owner = await createAccount();
  const other = await createAccount();
  const unsafe = '<img src=x onerror="window.injected=true">';
  const hit = { document_id: randomUUID(), ordinal: 0, title: "夹具资料", source: "javascript:alert(1)", text: unsafe, score: 0.9 };
  const citation = { ...hit, id: 1, quote: unsafe, quote_start: 0, quote_end: Array.from(unsafe).length };
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
      await route.fulfill({ json: { status: "answered", answer: "过期答案", citations: [citation] } }).catch(() => {});
      return;
    }
    if (mode === "empty") return route.fulfill({ json: { hits: [] } });
    if (mode === "search") return route.fulfill({ json: { hits: [hit] } });
    if (mode === "insufficient") return route.fulfill({ json: { status: "insufficient_evidence", answer: null, citations: [] } });
    if (mode === "invalid") return route.fulfill({ json: { status: "answered", answer: "无引用答案", citations: [] } });
    const errors = { disabled: [503, "answering_disabled"], unavailable: [503, "retrieval_unavailable"], expired: [401, "unauthorized"], limited: [429, "answer_rate_limited"], invalid_answer: [502, "invalid_answer"], changed: [409, "answer_evidence_changed"], read_failed: [503, "answer_evidence_unavailable"] };
    if (errors[mode]) return route.fulfill({ status: errors[mode][0], json: { error: { code: errors[mode][1] } } });
    return route.fulfill({ json: { status: "answered", answer: unsafe, citations: [citation] } });
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
  await expect(panel.locator("blockquote")).toHaveText(unsafe);
  await expect(panel.locator("mark")).toHaveText(unsafe);
  await expect(panel.locator("img, a")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  mode = "search";
  await panel.getByRole("button", { name: "检索资料", exact: true }).click();
  await expect(panel).toContainText("找到 1 个核验片段");
  await expect(panel.locator(".answerText")).toHaveCount(0);
  for (const [nextMode, expected] of [["empty", "未找到匹配"], ["insufficient", "证据不足"], ["disabled", "尚未启用知识问答"], ["unavailable", "服务暂不可用"], ["expired", "登录已失效"], ["limited", "服务繁忙或模型限流"], ["invalid_answer", "未通过引用校验"], ["invalid", "服务返回无效结果"], ["changed", "资料已变化或被删除"], ["read_failed", "暂时无法复核资料"]]) {
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
  await expect(panel.locator("blockquote")).toHaveText(document.chunks[0]);
  await expect(panel.locator("mark")).toHaveText(document.chunks[0]);
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

test("citation ranges preserve Unicode and reject malformed or ambiguous evidence", async ({ page }, testInfo) => {
  const account = await createAccount();
  const text = "前🙂言：证据 e\u0301\n结尾";
  const quote = "证据 e\u0301\n";
  const citation = { document_id: randomUUID(), ordinal: 0, title: "字符范围验收", source: "unicode.md", text, score: 0.91, id: 1, quote, quote_start: 4, quote_end: 10 };
  let citations = [citation];
  let calls = 0;
  await page.route("**/api/knowledge/answer", route => {
    calls += 1;
    return route.fulfill({ json: { status: "answered", answer: "从原文得到的回答", citations } });
  });
  await page.goto("/"); await login(page, account);
  const panel = page.getByRole("region", { name: "知识检索与问答", exact: true });
  await panel.getByLabel("问题或检索内容", { exact: false }).fill("证据是什么？");
  const submit = panel.getByRole("button", { name: "生成引用答案", exact: true });
  await submit.click();
  await expect(panel.locator("blockquote")).toHaveText(quote);
  await panel.getByText("查看原文片段", { exact: true }).click();
  await expect(panel.locator("pre")).toHaveText(text);
  expect(await panel.locator("mark").textContent()).toBe(quote);
  await panel.screenshot({ path: testInfo.outputPath("citation-unicode.png") });
  const invalid = [
    { quote: undefined }, { quote_start: undefined }, { quote: "invented" },
    { quote_start: 5, quote_end: 11 }, // UTF-16 offsets must not be accepted.
    { quote_start: -1 }, { quote_start: 4.5 }, { quote_end: 9 }, { quote_end: Number.MAX_SAFE_INTEGER },
    { quote: "é", text: "e\u0301", quote_start: 0, quote_end: 1 },
    { text: " ", quote: " ", quote_start: 0, quote_end: 1 },
    { text: "🙂".repeat(401), quote: "🙂".repeat(401), quote_start: 0, quote_end: 401 },
    { text: "重复 重复", quote: "重复", quote_start: 0, quote_end: 2 },
    { text: "aaaa", quote: "aaa", quote_start: 0, quote_end: 3 },
    { id: 0 }, { id: 6 },
  ].map(change => [{ ...citation, ...change }]);
  invalid.push([citation, citation]);
  for (const values of invalid) {
    citations = values;
    await submit.click();
    await expect(panel.getByRole("alert")).toContainText("服务返回无效结果");
    await expect(panel.locator(".answerText, blockquote, mark")).toHaveCount(0);
  }
  expect(calls).toBe(invalid.length + 1);
});
