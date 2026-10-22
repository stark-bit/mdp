import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "tests",
  globalSetup: "./global-setup.ts",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  // a retry that passes hides an intermittent bug; a flaky test is a failing test
  retries: 0,
  forbidOnly: !!process.env.CI,
  // every test runs its own server in its own temp directory, on its own port
  fullyParallel: true,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    trace: "retain-on-failure",
    navigationTimeout: 10_000,
  },
  projects: [
    // one browser project: the server under test is the release binary itself
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
});
