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

test("navigation resets the utility context", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  await page.goto(daemon.pageUrl);
  expect(await page.evaluate(() => (window as { fromLib?: number }).fromLib)).toBe(7);

  // The second document is a new realm; Playwright must re-create its
  // utility world instead of reusing stale handles.
  await page.goto(`${daemon.pageUrl}?second`);
  expect(await page.title()).toBe("tiny");
  expect(await page.evaluate(() => (window as { fromLib?: number }).fromLib)).toBe(7);

  await browser.close();
});

test("timer promises settle", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  const value = await page.evaluate(
    () => new Promise((resolve) => setTimeout(() => resolve(42), 0)),
  );
  expect(value).toBe(42);

  await browser.close();
});

test("failed navigation reports an error", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  await expect(page.goto("http://127.0.0.1:1/", { timeout: 5000 })).rejects.toThrow(
    /ERR_/,
  );

  await browser.close();
});
