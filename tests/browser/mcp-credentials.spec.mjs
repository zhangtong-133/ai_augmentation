import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

// Never capture a failure screenshot while one-time credentials are displayed.
test.use({ screenshot: "off" });

test("MCP credentials require consent, reveal once and revoke real host access", async ({ page }, testInfo) => {
  const account = await createAccount();
  await page.goto("/"); await login(page, account);
  const panel = page.getByRole("region", { name: "MCP 宿主授权", exact: true });
  await expect(panel).toContainText("尚无宿主授权");
  await panel.getByLabel("宿主名称", { exact: true }).fill("桌面宿主 <script>不执行</script>");
  await expect(panel.getByRole("button", { name: "创建专用凭据", exact: true })).toBeDisabled();
  const consent = panel.getByRole("checkbox");
  await consent.check();
  await panel.getByLabel("凭据有效期").selectOption("1");
  await expect(consent).not.toBeChecked();
  await consent.check();
  await panel.getByRole("button", { name: "创建专用凭据", exact: true }).dblclick();
  const delivery = panel.getByRole("region", { name: "一次性凭据", exact: true });
  await expect(delivery).toBeVisible();
  await expect(panel.locator("script")).toHaveCount(0);
  await expect(delivery.getByLabel("凭据明文")).toHaveCount(0);
  await delivery.getByRole("button", { name: "显示一次性凭据" }).click();
  const token = await delivery.getByLabel("凭据明文").inputValue();
  // Keep the disposable token out of assertion diagnostics and screenshots.
  expect(/^pai_mcp_[0-9a-f]{64}$/.test(token)).toBe(true);
  const headers = { authorization: "Bearer " + token };
  expect((await fetch(process.env.E2E_API_URL + "/api/mcp/tools", { headers })).status).toBe(200);
  expect(await page.evaluate(() => [...Object.values(localStorage), ...Object.values(sessionStorage)].some(value => value.includes("pai_mcp_")))).toBe(false);
  await delivery.getByRole("button", { name: "隐藏明文" }).click();
  await expect(delivery.getByLabel("凭据明文")).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await expect(delivery).toHaveCount(0);
  await panel.getByRole("button", { name: "刷新凭据列表" }).click();
  await expect(panel.locator(".mcpList > li")).toHaveCount(1);
  await page.reload();
  await expect(panel.locator(".mcpList > li")).toHaveCount(1);
  await expect(delivery).toHaveCount(0);
  await panel.getByRole("button", { name: /^撤销 桌面宿主/ }).click();
  const confirmation = panel.getByRole("region", { name: "确认撤销凭据" });
  await confirmation.getByRole("button", { name: "保留凭据" }).click();
  expect((await fetch(process.env.E2E_API_URL + "/api/mcp/tools", { headers })).status).toBe(200);
  await panel.getByRole("button", { name: /^撤销 桌面宿主/ }).click();
  await confirmation.getByRole("button", { name: "确认撤销", exact: true }).click();
  await expect(panel.locator(".mcpList")).toContainText("已撤销");
  expect((await fetch(process.env.E2E_API_URL + "/api/mcp/tools", { headers })).status).toBe(401);
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("mcp-credentials.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("mcp-credentials", { path: screenshot, contentType: "image/png" });
});

test("MCP ambiguous creation is never retried and revocation retries the same credential", async ({ page }) => {
  const account = await createAccount();
  let creates = 0, dropRevoke = true;
  const revoked = [];
  await page.route("**/api/mcp/credentials", async route => {
    if (route.request().method() !== "POST") return route.continue();
    creates += 1;
    expect(route.request().headers()["x-requested-with"]).toBe("personal-ai");
    expect(route.request().postDataJSON()).toEqual({ host_name: "结果未知的宿主", expires_in_days: 7, acknowledge_embedding_cost: true });
    const response = await route.fetch(); expect(response.status()).toBe(201);
    return route.abort("failed");
  });
  await page.route("**/api/mcp/credentials/*/revoke", async route => {
    revoked.push(route.request().url());
    const response = await route.fetch(); expect(response.status()).toBe(200);
    if (dropRevoke) { dropRevoke = false; return route.abort("failed"); }
    return route.fulfill({ response });
  });
  await page.goto("/"); await login(page, account);
  const panel = page.getByRole("region", { name: "MCP 宿主授权", exact: true });
  await expect(panel).toContainText("尚无宿主授权");
  await panel.getByLabel("宿主名称", { exact: true }).fill("结果未知的宿主");
  await panel.getByRole("checkbox").check();
  await panel.getByRole("button", { name: "创建专用凭据", exact: true }).click();
  const recovery = panel.getByRole("region", { name: "核对创建结果" });
  await expect(recovery).toBeVisible();
  await expect(recovery.getByRole("button")).toBeDisabled();
  await expect(panel.getByRole("button", { name: "创建专用凭据", exact: true })).toBeDisabled();
  expect(creates).toBe(1);
  await expect(panel.getByRole("button", { name: "刷新凭据列表" })).toBeEnabled();
  await panel.getByRole("button", { name: "刷新凭据列表" }).click();
  await expect(panel.locator(".mcpList > li")).toHaveCount(1);
  await panel.getByRole("button", { name: "撤销 结果未知的宿主", exact: true }).click();
  await panel.getByRole("button", { name: "确认撤销", exact: true }).click();
  await expect(panel.getByRole("button", { name: "重试撤销同一凭据" })).toBeEnabled();
  await panel.getByRole("button", { name: "重试撤销同一凭据" }).click();
  await expect(panel.locator(".mcpList")).toContainText("已撤销");
  expect(revoked.length).toBe(2); expect(revoked[0]).toBe(revoked[1]);
  await recovery.getByRole("button").click();
  await expect(panel.getByRole("checkbox")).not.toBeChecked();
  expect(creates).toBe(1);
  await page.route("**/api/mcp/credentials", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await panel.getByRole("button", { name: "刷新凭据列表" }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(panel.locator(".mcpList > li")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "创建专用凭据", exact: true })).toBeDisabled();
});

test("MCP late creation responses cannot reveal a previous account credential", async ({ page }) => {
  const first = await createAccount(), second = await createAccount();
  let release;
  const waiting = new Promise(resolve => { release = resolve; });
  let submitted;
  const received = new Promise(resolve => { submitted = resolve; });
  await page.route("**/api/mcp/credentials", async route => {
    if (route.request().method() !== "POST") return route.continue();
    const response = await route.fetch(); expect(response.status()).toBe(201);
    submitted();
    await waiting;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.goto("/"); await login(page, first);
  const panel = page.getByRole("region", { name: "MCP 宿主授权", exact: true });
  await expect(panel).toContainText("尚无宿主授权");
  await panel.getByLabel("宿主名称", { exact: true }).fill("前一账户的宿主");
  await panel.getByRole("checkbox").check();
  await panel.getByRole("button", { name: "创建专用凭据", exact: true }).click();
  await received;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await login(page, second);
  release();
  await expect(panel).toContainText("尚无宿主授权");
  await expect(panel.getByRole("region", { name: "一次性凭据", exact: true })).toHaveCount(0);
  await expect(panel).not.toContainText("前一账户的宿主");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, first);
  await expect(panel.locator(".mcpList")).toContainText("前一账户的宿主");
  await expect(panel.getByRole("region", { name: "一次性凭据", exact: true })).toHaveCount(0);
});

