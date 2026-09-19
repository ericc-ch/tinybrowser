import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

// Usability probe for automation clients.
//
// This test is marked `fixme` because the engine's geometry queries return
// stale boxes: after load, `getBoundingClientRect()` for `#go` (styled
// `width:80px; height:30px`) reports `[41, 1, 8, 8]` until a render pass
// forces layout — the same degenerate box Playwright's actionability check
// reads, so `page.click` retries until it times out. `document
// .elementFromPoint` already returns the right element and the CDP input
// path (`Input.dispatchMouseEvent` -> hit test -> trusted events) works; the
// missing piece is syncing layout inside geometry queries.
test.fixme("click, typing, and selectors drive the page", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  await page.goto(daemon.interactiveUrl);
  await expect(page.locator("#out")).toHaveText("idle");

  await page.click("#go");
  await expect(page.locator("#out")).toHaveText("clicked");

  await page.locator("#name").focus();
  await page.keyboard.type("abc");
  await expect(page.locator("#name")).toHaveValue("abc");
  await expect(page.locator("#out")).toHaveText("typed:abc");

  await browser.close();
});
