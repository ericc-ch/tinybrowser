import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

test("intl number formatting and locale resolution", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);
  const out = await page.evaluate(() => ({
    exp: new Intl.NumberFormat("en-US").format("1e30"),
    hex: new Intl.NumberFormat("en-US").format("0x1A"),
    negExp: new Intl.NumberFormat("en-US").format("-1.5e-3"),
    pt: new Intl.NumberFormat("pt").resolvedOptions().locale,
    plain: new Intl.NumberFormat("en-US").format(1234.5),
  }));
  console.log("intl probe", JSON.stringify(out));
  expect(out.exp).toBe("1,000,000,000,000,000,000,000,000,000,000");
  expect(out.hex).toBe("26");
  expect(out.negExp).toBe("-0.002");
  expect(out.pt).toBe("pt");
  expect(out.plain).toBe("1,234.5");
  await browser.close();
});
