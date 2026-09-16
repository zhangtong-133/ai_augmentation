import { defineConfig } from "@playwright/test";

for (const key of ["E2E_API_URL", "E2E_WEB_URL", "E2E_GATEWAY_URL"]) {
  const url = new URL(process.env[key] || "http://invalid");
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1") {
    throw new Error("Use make browser-test with its isolated loopback environment.");
  }
}
if (!process.env.E2E_ADMIN_TOKEN) throw new Error("Missing isolated test credentials.");

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  workers: 1,
  retries: 0,
  forbidOnly: Boolean(process.env.CI),
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    browserName: "chromium",
    channel: "chromium",
    headless: true,
    locale: "zh-CN",
    actionTimeout: 10_000,
    navigationTimeout: 15_000,
    // 不导出跟踪记录或认证状态，避免其中包含请求凭据。
    trace: "off",
    video: "off",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "next-desktop", use: { baseURL: process.env.E2E_WEB_URL, viewport: { width: 1280, height: 900 } } },
    { name: "nginx-mobile", use: { baseURL: process.env.E2E_GATEWAY_URL, viewport: { width: 390, height: 844 } } },
  ],
});
