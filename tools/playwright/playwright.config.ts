import { defineConfig } from "@playwright/test";

// The suite connects to a tinybrowser daemon over CDP; it never launches a
// bundled browser, so no `playwright install` step is needed.
export default defineConfig({
  testDir: ".",
  testMatch: /.*\.spec\.ts/,
  timeout: 30_000,
  workers: 1,
  fullyParallel: false,
  reporter: [["list"]],
  use: {
    actionTimeout: 5_000,
  },
});
