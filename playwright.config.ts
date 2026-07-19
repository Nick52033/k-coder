import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  use: { baseURL: "http://127.0.0.1:1420", channel: process.env.PLAYWRIGHT_CHANNEL },
  projects: [
    { name: "desktop", use: { viewport: { width: 1280, height: 820 } } },
    { name: "narrow", use: { viewport: { width: 700, height: 820 } } },
  ],
  webServer: { command: "pnpm dev --host 127.0.0.1", url: "http://127.0.0.1:1420", reuseExistingServer: true },
});
