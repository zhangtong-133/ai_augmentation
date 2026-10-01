import { test, expect } from "@playwright/test";
import { createAccount, login } from "./account.mjs";
const panelOf = page => page.getByRole("region", { name: "Daily Brief 日报", exact: true });
async function start(page, account) {
  await page.goto("/"); await login(page, account ?? await createAccount());
  const panel = panelOf(page); await expect(panel).toContainText("暂无日报"); return panel;
}

test("daily briefs persist preferences, generation and deletion with private account history", async ({ page }) => {
  const owner = await createAccount(), other = await createAccount();
  const panel = await start(page, owner);
  await panel.getByLabel("日报关键词", { exact: false }).fill("Rust\nAI");
  await expect(panel.getByRole("button", { name: "生成今日日报" })).toBeDisabled();
  await panel.getByRole("button", { name: "保存日报偏好" }).click();
  await expect(panel.getByLabel("日报关键词", { exact: false })).toHaveValue("ai\nrust");
  await panel.getByRole("button", { name: "生成今日日报" }).click();
  const detail = panel.getByLabel("日报详情", { exact: true });
  await expect(detail).toContainText("当天暂无符合条件");
  await page.reload();
  await expect(panel.getByLabel("日报关键词", { exact: false })).toHaveValue("ai\nrust");
  await panel.getByRole("button", { name: "查看日报", exact: true }).click();
  await expect(detail).toContainText("ai、rust");
  await panel.getByRole("button", { name: "删除日报", exact: true }).click();
  await panel.getByRole("button", { name: "保留日报", exact: true }).click();
  await panel.getByRole("button", { name: "删除日报", exact: true }).click();
  await panel.getByRole("button", { name: "确认删除日报", exact: true }).click();
  await expect(panel.getByRole("list", { name: "日报历史" })).toContainText("已删除");
  await panel.getByRole("button", { name: "查看日报", exact: true }).click();
  await expect(detail).toContainText("正文已清除");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await expect(panel).toHaveCount(0); await login(page, other);
  await expect(panel).toContainText("暂无日报");
  await expect(panel.getByLabel("日报关键词", { exact: false })).toHaveValue("");
});

test("lost daily brief generation retries the original ID and preference recovery queries saved state", async ({ page }) => {
  const panel = await start(page);
  let drop = true; const ids = [];
  await page.route("**/api/feed-briefs", async route => {
    if (route.request().method() !== "POST") return route.continue();
    ids.push(route.request().postDataJSON());
    const response = await route.fetch(); expect(response.status()).toBe(201);
    if (drop) { drop = false; return route.abort("failed"); }
    return route.fulfill({ response });
  });
  await panel.getByRole("button", { name: "生成今日日报" }).click();
  await expect(panel.getByRole("button", { name: "重试原日报操作" })).toBeEnabled();
  await panel.getByRole("button", { name: "重试原日报操作" }).click();
  await expect(panel.getByLabel("日报详情", { exact: true })).toContainText("已生成");
  expect(ids).toHaveLength(2); expect(ids[0]).toEqual(ids[1]);
  await expect(panel.getByRole("list", { name: "日报历史" }).locator("li")).toHaveCount(1);
  await page.route("**/api/feed-brief-preferences", async route => {
    if (route.request().method() !== "PUT") return route.continue();
    const response = await route.fetch(); expect(response.status()).toBe(200); await route.abort("failed");
  });
  await panel.getByLabel("日报关键词", { exact: false }).fill("Saved");
  await panel.getByRole("button", { name: "保存日报偏好" }).click();
  await expect(panel.getByRole("button", { name: "核对原日报操作" })).toBeEnabled();
  await panel.getByRole("button", { name: "核对原日报操作" }).click();
  await expect(panel.getByLabel("日报关键词", { exact: false })).toHaveValue("saved");
  await expect(panel.getByRole("button", { name: "核对原日报操作" })).toHaveCount(0);
});

test("daily brief text is safe, history pages navigate, invalidation clears content and expiry clears private state", async ({ page }, testInfo) => {
  const panel = await start(page);
  await panel.getByRole("button", { name: "生成今日日报" }).click();
  await expect(panel.getByLabel("日报详情", { exact: true })).toBeVisible();
  const id = "f4258f70-323a-4320-8805-2f6a69a9ceef";
  const metadata = { request_id: id, day_start_unix_ms: "1790812800000", created_at_unix_ms: "1790812800000", status: "ready" };
  await page.route("**/api/feed-briefs?*", route => route.fulfill({ json: { items: [{ ...metadata, status: "deleted" }], next_cursor: null } }));
  await page.route("**/api/feed-briefs", route => route.fulfill({ json: { items: [metadata], next_cursor: id } }));
  await page.route(`**/api/feed-briefs/${id}`, route => route.fulfill({ json: { ...metadata, preference_revision: "0", plan: { keywords: ["Rust"], candidate_count: 1, omitted_count: 0, items: [{ entry: { subscription_id: id, entry_key: "guid:" + "a".repeat(64), title: "Rust 新闻 <script>不执行</script>", summary: "<img src=x onerror=alert(1)> 每日技术摘要", link: "javascript:alert(1)" }, score: 52, freshness_points: 40, matches: [{ keyword: "rust", in_title: true, points: 12 }] }] } } }));
  await panel.getByRole("button", { name: "刷新日报" }).click();
  await panel.getByRole("button", { name: "下一页日报" }).click();
  await expect(panel.getByRole("list", { name: "日报历史" })).toContainText("已删除");
  await panel.getByRole("button", { name: "上一页日报" }).click();
  await panel.getByRole("button", { name: "查看日报", exact: true }).click();
  const detail = panel.getByLabel("日报详情", { exact: true });
  await expect(detail).toContainText("评分 52"); await expect(detail).toContainText("标题 +12");
  await expect(detail.locator("img, script, a")).toHaveCount(0);
  expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  const screenshot = testInfo.outputPath("daily-brief.png");
  await panel.screenshot({ path: screenshot });
  await testInfo.attach("daily-brief", { path: screenshot, contentType: "image/png" });
  await page.route(`**/api/feed-briefs/${id}`, route => route.fulfill({ json: { ...metadata, status: "invalidated", plan: null } }));
  await detail.getByRole("button", { name: "更新日报状态" }).click();
  await expect(detail).toContainText("正文已清除"); await expect(detail).not.toContainText("每日技术摘要");
  await page.route("**/api/feed-brief-preferences", route => route.fulfill({ status: 401, json: { error: { code: "unauthorized" } } }));
  await panel.getByRole("button", { name: "刷新日报" }).click();
  await expect(panel.getByRole("alert")).toContainText("登录已失效");
  await expect(detail).toHaveCount(0);
  await expect(panel.getByRole("list", { name: "日报历史" }).locator("li")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "生成今日日报" })).toBeDisabled();
});

test("late daily brief generation cannot enter another account", async ({ page }) => {
  const owner = await createAccount(), other = await createAccount();
  const panel = await start(page, owner);
  let release, received;
  const waiting = new Promise(resolve => { release = resolve; });
  const submitted = new Promise(resolve => { received = resolve; });
  await page.route("**/api/feed-briefs", async route => {
    if (route.request().method() !== "POST") return route.continue();
    const response = await route.fetch(); expect(response.status()).toBe(201); received();
    await waiting; await route.fulfill({ response }).catch(() => {});
  });
  await panel.getByRole("button", { name: "生成今日日报" }).click();
  await submitted;
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
  await login(page, other); release();
  await expect(panel).toContainText("暂无日报");
  await expect(panel.getByLabel("日报详情", { exact: true })).toHaveCount(0);
});

test("daily brief preference conflicts require review and a lost deletion can be verified", async ({ page }) => {
  const panel = await start(page);
  const status = await page.evaluate(async () => (await fetch("/api/feed-brief-preferences", { method: "PUT", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify({ revision: "0", keywords: ["remote"] }) })).status);
  expect(status).toBe(200);
  await panel.getByLabel("日报关键词", { exact: false }).fill("local");
  await panel.getByRole("button", { name: "保存日报偏好" }).click();
  await expect(panel.getByRole("alert")).toContainText("偏好版本已变化");
  await expect(panel.getByRole("button", { name: "生成今日日报" })).toBeDisabled();
  await panel.getByRole("button", { name: "核对原日报操作" }).click();
  await expect(panel.getByLabel("日报关键词", { exact: false })).toHaveValue("remote");
  await panel.getByRole("button", { name: "生成今日日报" }).click();
  const detail = panel.getByLabel("日报详情", { exact: true });
  await expect(detail).toContainText("本份关键词：remote");
  let deletions = 0;
  await page.route("**/api/feed-briefs/*", async route => {
    if (route.request().method() !== "DELETE") return route.continue();
    deletions += 1;
    const response = await route.fetch(); expect(response.status()).toBe(200); await route.abort("failed");
  });
  await detail.getByRole("button", { name: "删除日报", exact: true }).click();
  await detail.getByRole("button", { name: "确认删除日报" }).click();
  await expect(panel.getByRole("button", { name: "核对原日报操作" })).toBeEnabled();
  await panel.getByRole("button", { name: "核对原日报操作" }).click();
  await expect(detail).toContainText("正文已清除"); expect(deletions).toBe(1);
});
