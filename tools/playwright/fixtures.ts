import { test as base } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface Daemon {
  /** HTTP discovery origin, e.g. `http://127.0.0.1:41234`. */
  origin: string;
  /** Classic-script + fetch page served by the fixture. */
  pageUrl: string;
  /** Deterministic layout page for screenshot assertions. */
  shotUrl: string;
  /** Page styled only by an external sheet. */
  styledUrl: string;
  /** Page whose external sheet 404s. */
  brokenUrl: string;
  /** The same inline-styled box without any link element. */
  plainUrl: string;
}

interface Fixtures {
  daemon: Daemon;
}

const PAGE = `<!doctype html><title>tiny</title><script src="/lib.js"></script><script>window.ready = false; fetch('/data').then(r => r.text()).then(t => { window.payload = t; window.ready = true; });</script>`;

/** Fixed colors and positions the screenshot spec probes by pixel. */
const SHOT = `<!doctype html><title>shot</title><style>
html, body { margin: 0; padding: 0; }
#red { background: #ff0000; width: 100px; height: 50px; }
#blue { background: #0000ff; width: 50px; height: 50px; }
p { margin: 16px 0 0 0; font-size: 20px; }
</style><div id="red"></div><div id="blue"></div><p>Hello screenshot</p>`;

/** Styled only by an external sheet, so the load event must wait for it. */
const STYLED = `<!doctype html><title>styled</title>
<link rel="stylesheet" href="/styles.css">
<div class="hot"></div>`;

/** A link that 404s must not hold the load event forever. */
const BROKEN = `<!doctype html><title>broken</title>
<link rel="stylesheet" href="/missing.css">
<div style="background:#123456;width:20px;height:20px"></div>`;

/** The same box without the link, to isolate the offset. */
const PLAIN = `<!doctype html><title>plain</title>
<div style="background:#123456;width:20px;height:20px"></div>`;

const STYLES = ".hot { background: #00ff00; width: 60px; height: 60px; }";

async function waitForPort(jsonPath: string, timeoutMs: number): Promise<number> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return JSON.parse(readFileSync(jsonPath, "utf8")).port as number;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  throw new Error(`daemon.json missing at ${jsonPath}`);
}

export const test = base.extend<Fixtures>({
  daemon: async ({}, use) => {
    const binary = process.env.TINYBROWSER_BIN;
    if (!binary) {
      throw new Error("TINYBROWSER_BIN is not set; run ./tools/playwright/run");
    }
    const root = mkdtempSync(join(tmpdir(), "tinybrowser-pw-"));
    const runtime = join(root, "run");
    const data = join(root, "data");
    mkdirSync(runtime, { recursive: true });
    mkdirSync(data, { recursive: true });

    const httpServer = createServer((request, response) => {
      const path = new URL(request.url ?? "/", "http://localhost").pathname;
      if (path === "/page") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(PAGE);
      } else if (path === "/shot") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(SHOT);
      } else if (path === "/styled") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(STYLED);
      } else if (path === "/broken") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(BROKEN);
      } else if (path === "/plain") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(PLAIN);
      } else if (path === "/styles.css") {
        response.writeHead(200, { "content-type": "text/css" });
        response.end(STYLES);
      } else if (path === "/lib.js") {
        response.writeHead(200, { "content-type": "text/javascript" });
        response.end("window.fromLib = 7;");
      } else if (path === "/data") {
        response.writeHead(200, { "content-type": "text/plain" });
        response.end("payload");
      } else {
        response.writeHead(404);
        response.end("not found");
      }
    });
    await new Promise<void>((resolve) => httpServer.listen(0, "127.0.0.1", resolve));
    const httpPort = (httpServer.address() as AddressInfo).port;

    // Detached so cleanup can kill the daemon and its renderer children as one
    // process group.
    const daemon = spawn(binary, ["daemon", "--profile=default"], {
      env: { ...process.env, XDG_RUNTIME_DIR: runtime, XDG_DATA_HOME: data },
      stdio: "ignore",
      detached: true,
    });
    if (daemon.pid === undefined) throw new Error("daemon did not spawn");

    const port = await waitForPort(
      join(runtime, "tinybrowser", "default", "daemon.json"),
      10_000,
    );

    await use({
      origin: `http://127.0.0.1:${port}`,
      pageUrl: `http://127.0.0.1:${httpPort}/page`,
      shotUrl: `http://127.0.0.1:${httpPort}/shot`,
      styledUrl: `http://127.0.0.1:${httpPort}/styled`,
      brokenUrl: `http://127.0.0.1:${httpPort}/broken`,
      plainUrl: `http://127.0.0.1:${httpPort}/plain`,
    });

    try {
      process.kill(-daemon.pid, "SIGKILL");
    } catch {
      // Already gone.
    }
    await new Promise<void>((resolve) => httpServer.close(() => resolve()));
    rmSync(root, { recursive: true, force: true });
  },
});

export { expect } from "@playwright/test";
