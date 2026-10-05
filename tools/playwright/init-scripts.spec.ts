import { chromium } from "playwright";
import { cdpConnection, cdpSend } from "./cdp";
import { expect, test } from "./fixtures";

test("init scripts apply before the first navigation and survive renderer swaps", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  await page.addInitScript("window.__init = 42;");
  await page.goto(daemon.pageUrl);
  expect(await page.evaluate("String(window.__init)")).toBe("42");
  // A different site re-acquires the renderer; the tab must replay the list.
  await page.goto(daemon.crossSitePageUrl);
  expect(await page.evaluate("String(window.__init)")).toBe("42");
  await browser.close();
});

test("runImmediately evaluates now; remove stops future documents", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  await page.goto(daemon.pageUrl);
  const socket = await cdpConnection(daemon.origin);
  const added = await cdpSend(socket, "Page.addScriptToEvaluateOnNewDocument", {
    source: "window.__now = 7;",
    runImmediately: true,
  });
  expect(await page.evaluate("String(window.__now)")).toBe("7");
  await cdpSend(socket, "Page.removeScriptToEvaluateOnNewDocument", {
    identifier: added.identifier,
  });
  await page.goto(daemon.pageUrl);
  expect(await page.evaluate("String(window.__now)")).toBe("undefined");
  socket.close();
  await browser.close();
});

test("init scripts run in child frames", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const context = browser.contexts()[0];
  const page = context.pages()[0];
  await page.addInitScript("window.__inframe = 3;");
  await page.goto(daemon.frameUrl);
  const value = await page.evaluate(
    "String(document.getElementById('child').contentWindow.__inframe)",
  );
  expect(value).toBe("3");
  await browser.close();
});
