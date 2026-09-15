import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";

async function createAccount() {
  const email = `${randomUUID()}@browser.example`;
  const password = randomUUID();
  // Node fetch avoids Playwright request diagnostics recording the admin header.
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
async function login(page, account) {
  await page.getByLabel("邮箱", { exact: true }).fill(account.email);
  await page.getByLabel("密码", { exact: true }).fill(account.password);
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("heading", { name: "欢迎回来，浏览器验收" })).toBeVisible();
}
function metric(page, label) {
  return page.locator(".overviewMetrics > div").filter({ has: page.getByText(label, { exact: true }) }).locator("dd");
}
async function upload(page, name, buffer) {
  await page.getByLabel("Markdown 文件", { exact: false }).setInputFiles({ name, mimeType: "text/markdown", buffer });
  await page.getByRole("button", { name: "导入文档", exact: true }).click();
}

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
  await expect(page.getByLabel("Markdown 文件", { exact: false })).toHaveValue("");
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

  // Only this failure is simulated; normal requests use the real API/database.
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
