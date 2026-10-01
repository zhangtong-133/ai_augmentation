import { expect } from "@playwright/test";
import { randomUUID } from "node:crypto";

export async function createAccount() {
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
export async function login(page, account) {
  // 串行验收主动遵守每分钟 20 次的真实登录限流，不关闭保护或重试失败请求。
  const delay = Math.max(0, 3500 - (Date.now() - lastLoginAt));
  if (delay) await new Promise(resolve => setTimeout(resolve, delay));
  lastLoginAt = Date.now();
  await page.getByLabel("邮箱", { exact: true }).fill(account.email);
  await page.getByLabel("密码", { exact: true }).fill(account.password);
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("heading", { name: "欢迎回来，浏览器验收" })).toBeVisible();
}
