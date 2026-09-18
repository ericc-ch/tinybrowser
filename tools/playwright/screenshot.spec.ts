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

test("external stylesheets apply before load", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  // `goto` resolves on load, and the link sheet delays load, so the sheet is
  // in by the time the screenshot runs.
  await page.goto(daemon.styledUrl);
  const png = await page.screenshot();
  const image = decodePng(png);
  expect(pixel(image, 30, 30)).toEqual([0, 255, 0]);
  await browser.close();
});

test("a failed stylesheet does not hold the load event", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  const boundsOf = async (url: string) => {
    await page.goto(url);
    const image = decodePng(await page.screenshot());
    let minX = 9999;
    let minY = 9999;
    let maxX = 0;
    let maxY = 0;
    let count = 0;
    for (let y = 0; y < image.height; y++) {
      for (let x = 0; x < image.width; x++) {
        if (pixel(image, x, y).join(",") === "18,52,86") {
          count++;
          minX = Math.min(minX, x);
          minY = Math.min(minY, y);
          maxX = Math.max(maxX, x);
          maxY = Math.max(maxY, y);
        }
      }
    }
    return { minX, minY, maxX, maxY, count };
  };
  const plain = await boundsOf(daemon.plainUrl);
  const broken = await boundsOf(daemon.brokenUrl);
  expect(plain.count).toBe(400);
  expect(broken.count).toBe(400);
  // Body's 8px UA margin places the box; the anonymous root adds nothing.
  expect(plain.minX).toBe(8);
  expect(plain.minY).toBe(8);
  // The 404 must not hold the load event (`goto` would have timed out), and
  // the link element must not shift the inline-styled box.
  expect(broken.minX).toBe(plain.minX);
  expect(broken.minY).toBe(plain.minY);
  await browser.close();
});
