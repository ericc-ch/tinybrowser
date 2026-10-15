import { chromium } from "@playwright/test";

import { expect, test } from "./fixtures";

test("a missing blob URL leaves the iframe on its current document", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);

  const result = await page.evaluate(async () => {
    const iframe = document.createElement("iframe");
    const loaded = new Promise<void>((resolve) => {
      iframe.addEventListener("load", () => resolve(), { once: true });
    });
    document.body.appendChild(iframe);
    await loaded;
    iframe.contentDocument.body.textContent = "STAY";
    iframe.src = "blob:https://example.com/00000000-0000-4000-8000-000000000000";
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 200);
    });
    return {
      readable: iframe.contentDocument !== null,
      body: iframe.contentDocument && iframe.contentDocument.body.textContent,
    };
  });

  expect(result.readable).toBe(true);
  expect(result.body).toContain("STAY");

  await browser.close();
});

test("an iframe blob URL lookup strips the fragment", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);

  const result = await page.evaluate(async () => {
    const url = URL.createObjectURL(
      new Blob(["<!doctype html><body>BLOBHIT</body>"], { type: "text/html" }),
    );
    const iframe = document.createElement("iframe");
    const loaded = new Promise<void>((resolve) => {
      iframe.addEventListener("load", () => resolve(), { once: true });
    });
    iframe.src = `${url}#page=2`;
    document.body.appendChild(iframe);
    await Promise.race([
      loaded,
      new Promise<void>((resolve) => {
        setTimeout(resolve, 1000);
      }),
    ]);
    return iframe.contentDocument && iframe.contentDocument.body.textContent;
  });

  expect(result).toContain("BLOBHIT");

  await browser.close();
});

test("a removed frame's blob URL is gone", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);

  const created = await page.evaluate(async (blobChildUrl: string) => {
    const url = await new Promise<string>((resolve) => {
      window.addEventListener(
        "message",
        (event) => {
          resolve(String(event.data));
        },
        { once: true },
      );
      const iframe = document.createElement("iframe");
      iframe.id = "blob-frame";
      iframe.src = blobChildUrl;
      document.body.appendChild(iframe);
    });
    const before = await fetch(url).then((response) => response.text());
    return { before, url };
  }, daemon.blobChildUrl);

  expect(created.before).toBe("GONE");
  await page.goto(`${daemon.pageUrl}?next`);

  const after = await page.evaluate(async (url: string) => {
    try {
      return await fetch(url).then((response) => response.text());
    } catch {
      return "gone";
    }
  }, created.url);

  expect(after).toBe("gone");

  await browser.close();
});

test("createObjectURL on about:blank is fetchable", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  const text = await page.evaluate(async () => {
    const url = URL.createObjectURL(new Blob(["OPAQUE"], { type: "text/plain" }));
    return await fetch(url).then((response) => response.text());
  });

  expect(text).toBe("OPAQUE");

  await browser.close();
});
