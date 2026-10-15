import { chromium, type Page } from "@playwright/test";

import { expect, test } from "./fixtures";

test("javascript: iframe src does not run in a cross-origin child", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);

  const leaked = await page.evaluate(async (victimUrl: string) => {
    const messages: unknown[] = [];
    window.addEventListener("message", (event) => {
      messages.push(event.data);
    });
    const iframe = document.createElement("iframe");
    const loaded = new Promise<void>((resolve) => {
      iframe.addEventListener("load", () => resolve(), { once: true });
    });
    iframe.src = victimUrl;
    document.body.appendChild(iframe);
    await loaded;
    const readableAfterHttp = iframe.contentDocument !== null;
    iframe.src = "javascript:parent.postMessage(document.body.innerHTML,'*')";
    await Promise.race([
      new Promise<void>((resolve) => {
        iframe.addEventListener("load", () => resolve(), { once: true });
      }),
      new Promise<void>((resolve) => {
        setTimeout(resolve, 200);
      }),
    ]);
    let childRead: string | null = "readable";
    try {
      childRead = iframe.contentWindow.document.body.textContent;
    } catch {
      childRead = null;
    }
    return {
      readableAfterHttp,
      readableAfterJs: iframe.contentDocument !== null,
      childRead,
      messages,
    };
  }, daemon.victimUrl);

  expect(leaked.readableAfterHttp).toBe(false);
  expect(leaked.readableAfterJs).toBe(false);
  expect(leaked.childRead).toBeNull();
  expect(leaked.messages).toEqual([]);

  await browser.close();
});

test("javascript: iframe src still runs in a same-origin child", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(daemon.pageUrl);

  const ran = await page.evaluate(async () => {
    const iframe = document.createElement("iframe");
    const loaded = new Promise<void>((resolve) => {
      iframe.addEventListener("load", () => resolve(), { once: true });
    });
    document.body.appendChild(iframe);
    await loaded;
    const replaced = new Promise<void>((resolve) => {
      iframe.addEventListener("load", () => resolve(), { once: true });
    });
    iframe.src = "javascript:'<body>JS RAN</body>'";
    await replaced;
    return iframe.contentDocument && iframe.contentDocument.body.textContent;
  });

  expect(ran).toContain("JS RAN");

  await browser.close();
});

async function opaqueParentJavascriptChild(page: Page) {
  return page.evaluate(async () => {
    const frame = document.createElement("iframe");
    const loaded = new Promise<void>((resolve) => {
      frame.addEventListener("load", () => resolve(), { once: true });
    });
    document.body.appendChild(frame);
    await loaded;
    const readable = frame.contentDocument !== null;
    const replaced = new Promise<void>((resolve) => {
      frame.addEventListener("load", () => resolve(), { once: true });
    });
    frame.src = "javascript:'<body>JS RAN</body>'";
    await replaced;
    return {
      readable,
      body: frame.contentDocument && frame.contentDocument.body.textContent,
    };
  });
}

test("javascript: iframe src runs from an opaque about:blank parent", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  const result = await opaqueParentJavascriptChild(page);

  expect(result.readable).toBe(true);
  expect(result.body).toContain("JS RAN");

  await browser.close();
});

test("javascript: iframe src runs from an opaque data: parent", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];
  await page.goto(`data:text/html,${encodeURIComponent("<!doctype html><body></body>")}`);

  const result = await opaqueParentJavascriptChild(page);

  expect(result.readable).toBe(true);
  expect(result.body).toContain("JS RAN");

  await browser.close();
});

test("opaque about:blank rejects data: and file: history URLs", async ({ daemon }) => {
  const browser = await chromium.connectOverCDP(daemon.origin);
  const page = browser.contexts()[0].pages()[0];

  const result = await page.evaluate(() => {
    const rejected = (spec: string) => {
      try {
        history.pushState({}, "", spec);
        return false;
      } catch {
        return true;
      }
    };
    const dataRejected = rejected("data:text/html,x");
    const fileRejected = rejected("file:///tmp/x");
    let fragmentOk = false;
    try {
      history.pushState({}, "", "#frag");
      fragmentOk = location.hash === "#frag";
    } catch {
      fragmentOk = false;
    }
    return { dataRejected, fileRejected, fragmentOk, href: location.href };
  });

  expect(result.dataRejected).toBe(true);
  expect(result.fileRejected).toBe(true);
  expect(result.fragmentOk).toBe(true);

  await browser.close();
});
