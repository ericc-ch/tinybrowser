import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

test("playwright drives navigation and evaluation", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);

  const context = browser.contexts()[0];
  expect(context, "connectOverCDP must expose the initial context").toBeDefined();
  const page = context.pages()[0];
  expect(page, "the initial about:blank tab must be attached").toBeDefined();

  await page.goto(daemon.pageUrl);
  expect(await page.title()).toBe("tiny");
  expect(await page.evaluate(() => (window as { fromLib?: number }).fromLib)).toBe(7);
  await page.waitForFunction(() => (window as { ready?: boolean }).ready === true);
  expect(await page.evaluate(() => (window as { payload?: string }).payload)).toBe(
    "payload",
  );
  expect(await page.content()).toContain("lib.js");

  await page.close();
  await browser.close();
});
