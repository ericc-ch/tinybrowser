import { chromium } from "playwright";
import { expect, test } from "./fixtures";
import { decodePng, pixel } from "./png";

test("screenshot renders layout, colors, and text", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  await page.goto(daemon.shotUrl);
  const png = await page.screenshot();
  expect(png.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");

  const image = decodePng(png);
  expect(image.width).toBe(800);
  expect(image.height).toBe(600);
  // The red block fills the first 100x50 CSS pixels.
  expect(pixel(image, 50, 25)).toEqual([255, 0, 0]);
  // The blue block sits directly below it.
  expect(pixel(image, 25, 75)).toEqual([0, 0, 255]);
  // The white area right of the blocks is untouched.
  expect(pixel(image, 400, 25)).toEqual([255, 255, 255]);
  // The paragraph text paints dark pixels around y=116..140.
  let dark = 0;
  for (let y = 100; y < 160; y++) {
    for (let x = 0; x < 400; x++) {
      const [r, g, b] = pixel(image, x, y);
      if (r < 128 && g < 128 && b < 128) dark++;
    }
  }
  expect(dark).toBeGreaterThan(20);

  await browser.close();
});
