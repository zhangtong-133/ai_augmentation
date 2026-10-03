import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

const endpoint = /\/api\/subscription-connections(?:[/?].*)?$/;
const id = "00000000-0000-4000-8000-000000000001";
const panelOf = page => page.getByRole("region", { name: "ChatGPT 订阅连接", exact: true });
const fixture = () => ({ id, label: "本机 <script>不执行</script>", revision: "1", status: "active", models: ["fixture-model"], valid_until_unix_ms: String(Date.now() + 3600000) });
async function setup(page) {
  await page.goto("/"); await login(page, await createAccount());
  return panelOf(page);
}

test("subscription connection conflict requires renewed consent and lost writes reconcile without replay", async ({ page }, testInfo) => {
  let item = fixture(), writes = 0;
  await page.route(endpoint, async route => {
    const request = route.request();
    if (request.method() === "POST") {
      writes++;
      expect(request.headers()["x-requested-with"]).toBe("personal-ai");
      expect(request.postDataJSON()).toEqual({ revision: item.revision });
      if (writes === 1) {
        item = { ...item, revision: "2" };
        return route.fulfill({ status: 409, json: {} });
      }
      item = { ...item, revision: "3", status: "revoked" };
      return route.abort("failed");
    }
    return route.fulfill({ json: new URL(request.url()).pathname.endsWith(id) ? item : { items: [item], next_cursor: null } });
  });
  const panel = await setup(page);
  await panel.getByRole("button", { name: `查看 ${item.label}`, exact: true }).click();
  const review = panel.getByRole("region", { name: "订阅连接详情", exact: true });
  const revoke = review.getByRole("button", { name: "确认撤销连接", exact: true });
  await expect(revoke).toBeDisabled();
  await expect(panel.locator("script")).toHaveCount(0);
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("subscription-connections.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("subscription-connections", { path: screenshot, contentType: "image/png" });
  await review.getByRole("checkbox").check(); await revoke.click();
  await expect(panel.getByRole("alert")).toContainText("版本已变化");
  await expect(revoke).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "刷新连接", exact: true })).toBeDisabled();
  await review.getByRole("button", { name: "核对连接", exact: true }).click();
  await expect(review).toContainText("版本：2");
  await expect(review.getByRole("checkbox")).not.toBeChecked();
  await expect(revoke).toBeDisabled(); expect(writes).toBe(1);
  await review.getByRole("checkbox").check(); await revoke.click();
  await expect(panel.getByRole("alert")).toContainText("撤销结果未知");
  await review.getByRole("button", { name: "核对连接", exact: true }).click();
  await expect(review.getByRole("heading")).toContainText("已撤销");
  await expect(revoke).toHaveCount(0); expect(writes).toBe(2);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await page.unroute(endpoint);
  await login(page, await createAccount());
  await expect(panel).toContainText("暂无订阅连接");
  await expect(panel).not.toContainText(item.label);
});

test("subscription pagination refreshes from first page and unauthorized responses clear metadata", async ({ page }) => {
  const first = fixture(), second = { ...fixture(), id: "00000000-0000-4000-8000-000000000002", label: "第二页连接", status: "expired" };
  let unauthorized = false;
  const cursors = [];
  await page.route(endpoint, route => {
    if (unauthorized) return route.fulfill({ status: 401, json: {} });
    const url = new URL(route.request().url());
    if (url.pathname.endsWith(id)) return route.fulfill({ json: first });
    cursors.push(url.searchParams.get("after"));
    return route.fulfill({ json: url.searchParams.has("after") ? { items: [second], next_cursor: null } : { items: [first], next_cursor: id } });
  });
  const panel = await setup(page);
  await panel.getByRole("button", { name: "下一页连接", exact: true }).click();
  await expect(panel).toContainText(second.label); await expect(panel).toContainText("已到期");
  await expect(panel).not.toContainText(first.label);
  await panel.getByRole("button", { name: "刷新连接", exact: true }).click();
  await panel.getByRole("button", { name: `查看 ${first.label}`, exact: true }).click();
  expect(cursors).toEqual([null, id, null]);
  unauthorized = true;
  await panel.getByRole("button", { name: "核对连接", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(panel).not.toContainText(first.label);
  await expect(panel.getByRole("region", { name: "订阅连接详情", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "刷新连接", exact: true })).toBeDisabled();
});

test("subscription late detail responses cannot reappear after account switch", async ({ page }) => {
  const item = fixture(); let release, started, completed;
  const requested = new Promise(resolve => { started = resolve; });
  const pending = new Promise(resolve => { release = resolve; });
  const finished = new Promise(resolve => { completed = resolve; });
  await page.route(endpoint, async route => {
    if (new URL(route.request().url()).pathname.endsWith(id)) {
      started(); await pending;
      await route.fulfill({ json: item }).catch(() => {}); completed();
    } else await route.fulfill({ json: { items: [item], next_cursor: null } });
  });
  const panel = await setup(page);
  await panel.getByRole("button", { name: `查看 ${item.label}`, exact: true }).click();
  await requested;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await page.unroute(endpoint);
  await login(page, await createAccount());
  await expect(panel).toContainText("暂无订阅连接");
  release(); await finished;
  await expect(panel).not.toContainText(item.label);
  await expect(panel.getByRole("region", { name: "订阅连接详情", exact: true })).toHaveCount(0);
});

test("subscription list failures and malformed data require explicit refresh", async ({ page }) => {
  let reads = 0;
  await page.route(endpoint, route => {
    reads++;
    if (reads === 1) return route.fulfill({ status: 503, json: {} });
    if (reads === 2) return route.fulfill({ json: { items: [{ ...fixture(), revision: 1 }], next_cursor: null } });
    return route.fulfill({ json: { items: [], next_cursor: null } });
  });
  const panel = await setup(page);
  await expect(panel.getByRole("alert")).toContainText("服务暂不可用");
  await expect(panel).not.toContainText("暂无订阅连接");
  expect(reads).toBe(1);
  await panel.getByRole("button", { name: "刷新连接", exact: true }).click();
  await expect(panel.getByRole("alert")).toContainText("连接数据不完整");
  await expect(panel.getByRole("button", { name: /^查看 / })).toHaveCount(0);
  await panel.getByRole("button", { name: "刷新连接", exact: true }).click();
  await expect(panel).toContainText("暂无订阅连接");
  await expect(panel.getByRole("alert")).toHaveCount(0); expect(reads).toBe(3);
});
