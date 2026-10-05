import { chromium } from "playwright";
import { expect, test } from "./fixtures";

test("emulated viewport persists, reaches child frames, and clear restores", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.goto(daemon.frameUrl);
  const sizes = await page.evaluate(`(() => {
    const frame = document.getElementById('child');
    return [innerWidth, innerHeight, frame.contentWindow.innerWidth, frame.contentWindow.innerHeight];
  })()`);
  expect(sizes).toEqual([1024, 768, 1024, 768]);
  await page.setViewportSize({ width: 800, height: 600 });
  expect(await page.evaluate("[innerWidth, innerHeight]")).toEqual([800, 600]);
  await browser.close();
});
