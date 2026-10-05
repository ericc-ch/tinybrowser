import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright";
import { expect, test } from "./fixtures";
import { decodePng, pixel } from "./png";

/** System Chrome for the pixel-parity comparison. */
const CHROME = "/etc/profiles/per-user/erickc/bin/google-chrome-dev";
const WIDTH = 800;
const HEIGHT = 600;
/** Per-channel tolerance: rasterizers antialias differently. */
const TOLERANCE = 12;

test.skip(
  !existsSync(CHROME),
  "system Chrome is required for the pixel-parity comparison",
);

interface PageCase {
  name: string;
  url: (daemon: { [key: string]: string }) => string;
  /** Fail the spec above this share of over-tolerance pixels. Calibrated
    2026-10-06 against system Chrome: text-heavy pages carry
    antialiasing-level diffs, solid pages are near zero. A structural
    regression (missing element, wrong color, shifted layout) lands an
    order of magnitude above these caps. */
  cap: number;
}

const CASES: PageCase[] = [
  { name: "shot", url: (d) => d.shotUrl, cap: 1.0 },
  { name: "styled", url: (d) => d.styledUrl, cap: 0.05 },
  { name: "gradient", url: (d) => d.gradientUrl, cap: 0.05 },
  { name: "shadow", url: (d) => d.shadowUrl, cap: 3.0 },
  { name: "imaged", url: (d) => d.imagedUrl, cap: 0.05 },
  { name: "text", url: (d) => d.textUrl, cap: 5.0 },
  { name: "filtered", url: (d) => d.filterUrl, cap: 1.0 },
];

test("chromium pixel parity", async ({ daemon }) => {
  const out = `/tmp/opencode/chromium-diff-${Date.now()}`;
  mkdirSync(out, { recursive: true });

  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];

  const chrome = await chromium.launch({
    executablePath: CHROME,
    args: [
      "--no-sandbox",
      "--hide-scrollbars",
      "--force-device-scale-factor=1",
    ],
  });
  const reference = await chrome.newContext({
    viewport: { width: WIDTH, height: HEIGHT },
  });
  const refPage = await reference.newPage();

  const rows: string[] = ["page | dim | >tol% | mean | max"];
  for (const { name, url, cap } of CASES) {
    const target = url(daemon as unknown as { [key: string]: string });
    await page.goto(target);
    const ourPng = await page.screenshot();
    const ours = decodePng(ourPng);
    await refPage.goto(target, { waitUntil: "load" });
    const theirPng = await refPage.screenshot();
    const theirs = decodePng(theirPng);
    expect(
      [theirs.width, theirs.height],
      `${name}: reference dimensions`,
    ).toEqual([ours.width, ours.height]);

    let over = 0;
    let sum = 0;
    let max = 0;
    let worst: Array<[number, number, number]> = [];
    const total = ours.width * ours.height;
    for (let y = 0; y < ours.height; y++) {
      for (let x = 0; x < ours.width; x++) {
        const a = pixel(ours, x, y);
        const b = pixel(theirs, x, y);
        const delta = Math.max(
          Math.abs(a[0] - b[0]),
          Math.abs(a[1] - b[1]),
          Math.abs(a[2] - b[2]),
        );
        sum += delta;
        if (delta > max) max = delta;
        if (delta > TOLERANCE) over++;
        if (delta >= 200) worst.push([x, y, delta]);
      }
    }
    const percent = (over / total) * 100;
    const hot =
      worst.length > 0
        ? ` hot=${worst
            .slice(0, 8)
            .map(([x, y, d]) => `(${x},${y}:${d})`)
            .join(" ")}`
        : "";
    rows.push(
      `${name} | ${ours.width}x${ours.height} | ${percent.toFixed(2)}% | ${(sum / total / 3).toFixed(2)} | ${max}${hot}`,
    );
    expect(percent, `${name}: over-tolerance share`).toBeLessThanOrEqual(cap);
    writeFileSync(join(out, `${name}-ours.png`), ourPng);
    writeFileSync(join(out, `${name}-theirs.png`), theirPng);
  }
  console.log(`chromium-diff PNGs in ${out}\n${rows.join("\n")}`);

  await browser.close();
  await chrome.close();
});
