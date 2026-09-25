import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

// Real layout geometry through the render pipeline: Playwright reads the
// element's border box for actionability and picks the click point from it,
// then our CDP `Input.dispatchMouseEvent` path hit-tests and dispatches.
test("clicks use real layout geometry", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  await page.goto(daemon.interactiveUrl);
  await expect(page.locator("#out")).toHaveText("idle");

  // The div is `width:80px; height:30px` at the body margin, so its center is
  // (48, 23); a virtual box would be nowhere near it.
  await page.mouse.click(48, 23);
  await expect(page.locator("#out")).toHaveText("clicked");

  await browser.close();
});

// Blocked on Playwright actionability, not on form semantics: the textarea now
// has an intrinsic box and exposes `value`/selection/editing, but
// `locator.focus()` still times out while waiting for the element to become
// actionable over CDP. Follow up on the CDP element-state surface before
// promoting this to a plain `test`.
test.fixme("typing into form controls", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  await page.goto(daemon.interactiveUrl);
  await page.locator("#name").focus();
  await page.keyboard.type("abc");
  await expect(page.locator("#name")).toHaveValue("abc");
  await expect(page.locator("#out")).toHaveText("typed:abc");

  await browser.close();
});
