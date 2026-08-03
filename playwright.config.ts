import { defineConfig } from "@playwright/test";

const port = process.env.KCODER_E2E_PORT ?? "1420";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  use: { baseURL: `http://127.0.0.1:${port}`, channel: process.env.PLAYWRIGHT_CHANNEL },
  projects: [
    { name: "desktop", use: { viewport: { width: 1280, height: 820 } } },
    { name: "narrow", use: { viewport: { width: 700, height: 820 } } },
  ],
  webServer: { command: `pnpm dev --host 127.0.0.1 --port ${port}`, url: `http://127.0.0.1:${port}`, reuseExistingServer: true },
});
