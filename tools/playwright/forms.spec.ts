import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

// Dogfood: drive a realistic form through Playwright's locators and verify the
// multipart body the engine sends. Covers `fill` on text/email/number/date and
// a textarea, `selectOption`, `check` on a checkbox and a radio, and submit.
test("dogfood: fill a realistic form and submit multipart", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  await page.goto(daemon.richUrl);
  await page.fill("#r-name", "Ada Lovelace");
  await page.fill("#r-email", "ada@example.com");
  await page.fill("#r-age", "36");
  await page.fill("#r-date", "1815-12-10");
  await page.fill("#r-bio", "First programmer");
  await page.selectOption("#r-color", "blue");
  await page.check("#r-agree");
  await page.check("#r-size-l");

  await expect(page.locator("#r-name")).toHaveValue("Ada Lovelace");
  await expect(page.locator("#r-email")).toHaveValue("ada@example.com");
  await expect(page.locator("#r-age")).toHaveValue("36");
  await expect(page.locator("#r-date")).toHaveValue("1815-12-10");
  await expect(page.locator("#r-bio")).toHaveValue("First programmer");
  await expect(page.locator("#r-color")).toHaveValue("blue");
  await expect(page.locator("#r-agree")).toBeChecked();
  await expect(page.locator("#r-size-l")).toBeChecked();

  await page.click("#r-submit");
  await page.waitForURL(/\/echo$/);
  const body = await page.locator("#result").textContent();
  const normalized = (body ?? "").replace(/\r\n/g, "\n");
  expect(normalized).toContain('name="name"\n\nAda Lovelace\n');
  expect(normalized).toContain('name="email"\n\nada@example.com\n');
  expect(normalized).toContain('name="age"\n\n36\n');
  expect(normalized).toContain('name="date"\n\n1815-12-10\n');
  expect(normalized).toContain('name="bio"\n\nFirst programmer\n');
  expect(normalized).toContain('name="color"\n\nblue\n');
  expect(normalized).toContain('name="agree"\n\nyes\n');
  expect(normalized).toContain('name="size"\n\nl\n');
  // A file input with no files still contributes an empty entry.
  expect(normalized).toContain('name="upload"; filename=""');

  await browser.close();
});

// The same fields through a GET submit: the entry list becomes the query
// string, decoded back into the echo page.
test("dogfood: submit a GET form and read the query", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  await page.goto(daemon.formUrl);
  await page.fill("#g-name", "Grace Hopper");
  await page.selectOption("#g-color", "red");
  await page.check("#g-agree");
  await page.click("#g-submit");

  await page.waitForURL(/\/echo\?/);
  const query = new URL(page.url()).searchParams;
  expect(query.get("name")).toBe("Grace Hopper");
  expect(query.get("color")).toBe("red");
  expect(query.get("agree")).toBe("yes");

  await browser.close();
});
