import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";

const panelOf = page => page.getByRole("region", { name: "RSS 订阅", exact: true });
async function add(panel, name = "测试订阅 <script>不执行</script>") {
  await expect(panel).toContainText("公网采集已关闭");
  await panel.getByLabel("订阅名称", { exact: true }).fill(name);
  await panel.getByLabel("RSS 来源地址").fill("https://example.com/rss");
  await panel.getByRole("button", { name: "添加订阅", exact: true }).click();
  await expect(panel.locator(".feedList").first()).toContainText(name);
}

test("RSS real management, private previews and deletion survive reload", async ({ page }, testInfo) => {
  const owner = await createAccount(), other = await createAccount();
  await page.goto("/"); await login(page, owner);
  const panel = panelOf(page);
  await add(panel);
  await expect(panel.locator("script")).toHaveCount(0);
  await panel.getByRole("button", { name: "预览采集", exact: true }).click();
  const review = panel.getByLabel("RSS 采集确认", { exact: true });
  await expect(review).toContainText("待确认");
  await expect(review.getByRole("button", { name: "确认采集一次" })).toBeDisabled();
  await expect(review.getByRole("checkbox")).toBeDisabled();
  await panel.getByRole("button", { name: "查看条目" }).click();
  await expect(panel.getByLabel("RSS 条目", { exact: true })).toContainText("暂无已保存条目");
  await page.reload();
  await panel.getByRole("button", { name: "审阅采集" }).click();
  await expect(review.locator("ul")).toContainText("待确认");
  await review.getByRole("button", { name: "取消采集预览" }).click();
  await expect(review.getByRole("heading", { name: "已取消" })).toBeVisible();
  await panel.getByRole("button", { name: "编辑", exact: true }).click();
  await panel.getByLabel("订阅名称", { exact: true }).fill("已停用的订阅");
  await panel.getByLabel("启用订阅", { exact: true }).uncheck();
  await panel.getByRole("button", { name: "保存订阅" }).click();
  await expect(panel.locator(".feedList").first()).toContainText("版本 2");
  await expect(panel.getByRole("button", { name: "预览采集", exact: true })).toBeDisabled();
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("rss-subscriptions.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("rss-subscriptions", { path: screenshot, contentType: "image/png" });
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0); await login(page, other);
  await expect(panel).toContainText("暂无订阅"); await expect(panel).toContainText("暂无采集记录");
  await page.getByRole("button", { name: "退出登录", exact: true }).click(); await login(page, owner);
  await panel.getByRole("button", { name: "删除订阅", exact: true }).click();
  await panel.getByRole("button", { name: "保留订阅" }).click();
  await expect(panel.locator(".feedList").first()).toContainText("已停用的订阅");
  await panel.getByRole("button", { name: "删除订阅", exact: true }).click();
  await panel.getByRole("button", { name: "确认删除订阅" }).click();
  await expect(panel).toContainText("暂无订阅");
  await expect(panel.locator(".feedList").nth(1)).toContainText("已取消");
});

test("RSS lost preview retries the same ID, lost confirmation only queries", async ({ page }) => {
  await page.goto("/"); await login(page, await createAccount());
  const panel = panelOf(page); await add(panel, "核对订阅");
  const ids = []; let drop = true, draft;
  await page.route("**/api/feed-subscriptions/*/collections", async route => {
    ids.push(route.request().postDataJSON().request_id);
    const response = await route.fetch(); expect(response.status()).toBe(201); draft = await response.json();
    if (drop) { drop = false; return route.abort("failed"); }
    return route.fulfill({ response });
  });
  await panel.getByRole("button", { name: "预览采集", exact: true }).click();
  await expect(panel.getByRole("button", { name: "重试原管理操作" })).toBeEnabled();
  await panel.getByRole("button", { name: "重试原管理操作" }).click();
  await expect(panel.getByLabel("RSS 采集确认", { exact: true })).toContainText("待确认");
  expect(ids).toHaveLength(2); expect(ids[0]).toBe(ids[1]);
  await page.route("**/api/feeds/config", route => route.fulfill({ json: { execution_enabled: true, mode: "public" } }));
  await panel.getByRole("button", { name: "刷新 RSS" }).click();
  await expect(panel).toContainText("公网采集已启用");
  let confirmations = 0;
  // Browser fault fixture only: never enable real source traffic in the smoke stack.
  await page.route("**/api/feed-collections/*/confirm", async route => {
    confirmations += 1;
    expect(route.request().headers()["x-requested-with"]).toBe("personal-ai");
    expect(route.request().postDataJSON()).toEqual({ accepted_digest: draft.digest, acknowledge_source_request: true });
    await route.abort("failed");
  });
  const review = panel.getByLabel("RSS 采集确认", { exact: true });
  await expect(review.getByRole("button", { name: "确认采集一次" })).toBeDisabled();
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "确认采集一次" }).click();
  await expect(panel.getByRole("button", { name: "核对原记录" })).toBeEnabled();
  await expect(panel.getByRole("button", { name: "重试原管理操作" })).toHaveCount(0);
  await page.route(`**/api/feed-collections/${draft.plan.request_id}`, route => route.fulfill({ json: { ...draft, status: "unknown", reason: "unknown" } }));
  await panel.getByRole("button", { name: "核对原记录" }).click();
  await expect(review).toContainText("来源可能已收到请求，不会自动重试");
  await review.getByRole("button", { name: "查询原请求" }).click();
  await expect(panel.getByRole("button", { name: "刷新 RSS" })).toBeEnabled();
  expect(confirmations).toBe(1);
});

test("RSS pagination and untrusted entries stay text; session expiry clears private data", async ({ page }) => {
  await page.goto("/"); await login(page, await createAccount());
  const panel = panelOf(page); await add(panel, "条目订阅");
  await page.route("**/api/feed-subscriptions/*/entries*", route => route.fulfill({ status: 503, json: { error: { code: "feeds_unavailable" } } }));
  await panel.getByRole("button", { name: "查看条目" }).click();
  await expect(panel.getByRole("alert")).toContainText("服务暂不可用");
  await expect(panel.getByLabel("RSS 条目", { exact: true })).not.toContainText("暂无已保存条目");
  const key = "guid:" + "a".repeat(64);
  await page.route("**/api/feed-subscriptions/*/entries*", route => {
    const next = new URL(route.request().url()).searchParams.get("after");
    return route.fulfill({ json: { items: [{ entry_key: key, title: next ? "第二页条目" : "<img src=x onerror=alert(1)>", summary: "<script>不执行</script>", link: "javascript:alert(1)", published_at: null }], next_cursor: next ? null : key } });
  });
  await panel.getByRole("button", { name: "查看条目" }).click();
  const entries = panel.getByLabel("RSS 条目", { exact: true });
  await expect(entries).toContainText("<img src=x onerror=alert(1)>");
  await expect(entries.locator("img, script, a")).toHaveCount(0);
  await entries.getByRole("button", { name: "下一页条目" }).click();
  await expect(entries).toContainText("第二页条目");
  await entries.getByRole("button", { name: "上一页条目" }).click();
  await expect(entries).toContainText("<img src=x onerror=alert(1)>");
  await page.route("**/api/feeds/config", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await panel.getByRole("button", { name: "刷新 RSS" }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(panel).not.toContainText("条目订阅");
  await expect(panel.getByRole("button", { name: "添加订阅", exact: true })).toBeDisabled();
});

test("RSS subscription and collection cursors navigate real private pages", async ({ page }) => {
  await page.goto("/"); await login(page, await createAccount());
  const panel = panelOf(page);
  await expect(panel).toContainText("暂无订阅");
  const seeded = await page.evaluate(async () => {
    const headers = { "Content-Type": "application/json", "X-Requested-With": "personal-ai" };
    for (let i = 0; i < 21; i++) {
      const id = crypto.randomUUID();
      const sub = await fetch("/api/feed-subscriptions", { method: "POST", headers, body: JSON.stringify({ id, name: `分页订阅 ${i}`, source_url: "https://example.com/rss", enabled: true }) });
      if (sub.status !== 201) return false;
      const preview = await fetch(`/api/feed-subscriptions/${id}/collections`, { method: "POST", headers, body: JSON.stringify({ request_id: crypto.randomUUID() }) });
      if (preview.status !== 201) return false;
    }
    return true;
  });
  expect(seeded).toBe(true);
  await panel.getByRole("button", { name: "刷新 RSS" }).click();
  const subs = panel.locator(".feedList").first().locator(":scope > li");
  const history = panel.locator(".feedList").nth(1).locator(":scope > li");
  await expect(subs).toHaveCount(20); await expect(history).toHaveCount(20);
  const first = await subs.first().textContent();
  await panel.getByRole("button", { name: "下一页订阅" }).click();
  await expect(subs).toHaveCount(1);
  await expect(panel.getByRole("button", { name: "下一页订阅" })).toBeDisabled();
  await panel.getByRole("button", { name: "下一页采集" }).click();
  await expect(history).toHaveCount(1);
  await panel.getByRole("button", { name: "上一页订阅" }).click();
  await expect(subs).toHaveCount(20); await expect(subs.first()).toHaveText(first);
  await panel.getByRole("button", { name: "上一页采集" }).click();
  await expect(history).toHaveCount(20);
});

test("RSS late preview response cannot enter a different account", async ({ page }) => {
  const first = await createAccount(), second = await createAccount();
  let release, received;
  const waiting = new Promise(resolve => { release = resolve; });
  const submitted = new Promise(resolve => { received = resolve; });
  await page.route("**/api/feed-subscriptions/*/collections", async route => {
    const response = await route.fetch(); expect(response.status()).toBe(201); received();
    await waiting; await route.fulfill({ response }).catch(() => {});
  });
  await page.goto("/"); await login(page, first);
  const panel = panelOf(page); await add(panel, "前一账户的订阅");
  await panel.getByRole("button", { name: "预览采集", exact: true }).click();
  await submitted;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, second); release();
  await expect(panel).toContainText("暂无订阅"); await expect(panel).toContainText("暂无采集记录");
  await expect(panel.getByLabel("RSS 采集确认", { exact: true })).toHaveCount(0);
  await expect(panel).not.toContainText("前一账户的订阅");
});
