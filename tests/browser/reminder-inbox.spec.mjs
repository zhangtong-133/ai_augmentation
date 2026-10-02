import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { createAccount, login } from "./account.mjs";

const reminder = () => ({ request_id: randomUUID(), title: "复习提醒", body: "<script>私有提醒正文</script>", delivered_at_unix_ms: String(Date.now()), revision: "0", read_at_unix_ms: null, archived_at_unix_ms: null });

test("reminder inbox recovers ambiguous updates, archives, restores and clears expired sessions", async ({ page }, testInfo) => {
  const account = await createAccount();
  let item = reminder(); let writes = 0; let expired = false; let conflict = false;
  await page.route("**/api/reminders**", async route => {
    if (expired) return route.fulfill({ status: 401, json: {} });
    if (route.request().method() === "PUT") {
      writes++;
      const body = route.request().postDataJSON();
      expect(body.revision).toBe(item.revision);
      if (conflict) { conflict = false; return route.fulfill({ status: 409, json: {} }); }
      item = { ...item, revision: String(Number(item.revision) + 1), read_at_unix_ms: body.read ? item.read_at_unix_ms || String(Date.now()) : null, archived_at_unix_ms: body.archived ? String(Date.now()) : null };
      if (writes === 1) return route.abort("failed");
      return route.fulfill({ json: item });
    }
    const archived = new URL(route.request().url()).searchParams.get("archived") === "true";
    return route.fulfill({ json: { items: archived === Boolean(item.archived_at_unix_ms) ? [item] : [], next_cursor: null } });
  });
  await page.goto("/"); await login(page, account);
  const box = page.getByRole("region", { name: "提醒收件箱", exact: true });
  await expect(box).toContainText("未读");
  await expect(box.locator("script")).toHaveCount(0);
  await box.getByRole("button", { name: "标记已读", exact: true }).click();
  await expect(box).toContainText("请先刷新核对保存结果");
  await expect(box.getByRole("button", { name: "归档提醒", exact: true })).toBeDisabled();
  await box.getByRole("button", { name: "刷新收件箱" }).click();
  await expect(box.getByRole("button", { name: "标记未读", exact: true })).toBeEnabled();
  expect(writes).toBe(1);
  conflict = true;
  await box.getByRole("button", { name: "归档提醒", exact: true }).click();
  await expect(box).toContainText("提醒已在其他页面更新");
  await box.getByRole("button", { name: "刷新收件箱" }).click();
  await box.getByRole("button", { name: "归档提醒", exact: true }).click();
  await expect(box).toContainText("暂无已投递提醒。");
  await box.getByLabel("查看已归档提醒").check();
  await expect(box.getByRole("button", { name: "恢复到收件箱" })).toBeEnabled();
  const shot = testInfo.outputPath("reminder-inbox.png");
  await box.screenshot({ path: shot });
  await testInfo.attach("reminder-inbox", { path: shot, contentType: "image/png" });
  await box.getByRole("button", { name: "恢复到收件箱" }).click();
  await expect(box).toContainText("暂无已归档提醒。");
  await box.getByLabel("查看已归档提醒").uncheck();
  await box.getByRole("button", { name: "标记未读", exact: true }).click();
  await expect(box.getByRole("button", { name: "标记已读", exact: true })).toBeEnabled();
  await page.reload();
  await expect(box.getByRole("button", { name: "标记已读", exact: true })).toBeEnabled();
  expired = true;
  await box.getByRole("button", { name: "刷新收件箱" }).click();
  await expect(box).toHaveCount(0);
  await expect(page.getByRole("region", { name: "定时提醒", exact: true })).toContainText("登录已失效");
});

test("late reminder mutations cannot restore a previous account inbox", async ({ page }) => {
  const first = await createAccount(); const second = await createAccount();
  const item = reminder(); let oldAccount = true; let release; let started;
  const held = new Promise(resolve => { release = resolve; });
  const sent = new Promise(resolve => { started = resolve; });
  await page.route("**/api/reminders**", async route => {
    if (route.request().method() === "PUT") { started(); await held; return route.fulfill({ json: { ...item, revision: "1", archived_at_unix_ms: String(Date.now()) } }).catch(() => {}); }
    return route.fulfill({ json: { items: oldAccount ? [item] : [], next_cursor: null } });
  });
  await page.goto("/"); await login(page, first);
  const box = page.getByRole("region", { name: "提醒收件箱", exact: true });
  await box.getByRole("button", { name: "归档提醒", exact: true }).click(); await sent;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  oldAccount = false; await login(page, second); release();
  await expect(box).toContainText("暂无已投递提醒。");
  await expect(box).not.toContainText(item.body);
});
