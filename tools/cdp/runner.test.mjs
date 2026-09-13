import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const RUNNER = join(import.meta.dirname, "runner.mjs");
const WORKER = join(import.meta.dirname, "test/fixture-worker.mjs");

function runRunner(args, env) {
  const child = spawn(process.execPath, [RUNNER, ...args], {
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  return {
    child,
    output: () => output,
    closed: new Promise((resolveClosed) => {
      child.once("close", (code, signal) => resolveClosed({ code, signal }));
    }),
  };
}

test("runner contains hangs and crashes", async () => {
  const directory = mkdtempSync(join(tmpdir(), "tinybrowser-cdp-runner-test-"));
  try {
    const resultsFile = join(directory, "results.json");
    const started = Date.now();
    const run = runRunner([
      "--all",
      "--jobs=4",
      "--timeout=100",
      "http/access-inspected-object.js",
      "plain/injected-script-discard.js",
      "plain/runtime/runtime-evaluate-side-effect-free-onerror.js",
      "plain/sessions/runtime-evaluate.js",
    ], {
      TINYBROWSER_BIN: "/unused",
      TINYBROWSER_CDP_RESULTS: resultsFile,
      TINYBROWSER_CDP_WORKER: WORKER,
    });
    const { code } = await run.closed;
    assert.equal(code, 1, run.output());
    assert.ok(Date.now() - started < 2_000, run.output());
    const results = JSON.parse(readFileSync(resultsFile, "utf8"));
    assert.equal(results.length, 4);
    assert.deepEqual(
      Object.fromEntries(results.map((result) => [result.id, result.status])),
      {
        "http/access-inspected-object.js": "MISSING_FIXTURE",
        "plain/injected-script-discard.js": "TIMEOUT",
        "plain/runtime/runtime-evaluate-side-effect-free-onerror.js": "PASS",
        "plain/sessions/runtime-evaluate.js": "CRASH",
      },
    );
    const missing = results.find((result) => result.id === "http/access-inspected-object.js");
    assert.match(missing.message, /fixed-port test servers/);
  } finally {
    rmSync(directory, { force: true, recursive: true });
  }
});

test("runner kills workers on termination", async () => {
  const directory = mkdtempSync(join(tmpdir(), "tinybrowser-cdp-signal-test-"));
  try {
    const pidFile = join(directory, "worker.pid");
    const run = runRunner([
      "--all",
      "--jobs=1",
      "--timeout=10000",
      "plain/injected-script-discard.js",
    ], {
      TINYBROWSER_BIN: "/unused",
      TINYBROWSER_CDP_FAKE_PID: pidFile,
      TINYBROWSER_CDP_RESULTS: join(directory, "results.json"),
      TINYBROWSER_CDP_WORKER: WORKER,
    });
    const deadline = Date.now() + 2_000;
    while (Date.now() < deadline) {
      try {
        readFileSync(pidFile, "utf8");
        break;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
        await new Promise((resolveWait) => setTimeout(resolveWait, 10));
      }
    }
    const workerPid = Number(readFileSync(pidFile, "utf8"));
    run.child.kill("SIGTERM");
    const { code } = await run.closed;
    assert.equal(code, 143, run.output());
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
    assert.throws(() => process.kill(workerPid, 0), { code: "ESRCH" });
  } finally {
    rmSync(directory, { force: true, recursive: true });
  }
});
