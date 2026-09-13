import { readFileSync } from "node:fs";
import { dirname } from "node:path";

import { startDaemon } from "./daemon.mjs";
import { startFixtureServer } from "./fixture-server.mjs";
import {
  CommandError,
  HarnessUnsupported,
  ProtocolUnsupported,
  runInspectorTest,
} from "./harness.mjs";
import { ConnectionClosed } from "./protocol.mjs";

function workerInput() {
  const raw = process.env.TINYBROWSER_CDP_WORKER_INPUT;
  if (!raw) throw new Error("TINYBROWSER_CDP_WORKER_INPUT is not set");
  const input = JSON.parse(raw);
  if (typeof input.id !== "string" || typeof input.binary !== "string") {
    throw new Error("invalid worker input");
  }
  return input;
}

function canonicalOutput(text) {
  return `${text.replaceAll("\r\n", "\n").replace(/\n+$/, "")}\n`;
}

async function browserWebSocket(origin) {
  const response = await fetch(`${origin}/json/version`);
  if (!response.ok) throw new Error(`CDP discovery returned ${response.status}`);
  const body = await response.json();
  if (typeof body.webSocketDebuggerUrl !== "string") {
    throw new Error("CDP discovery omitted webSocketDebuggerUrl");
  }
  return body.webSocketDebuggerUrl;
}

function errorResult(input, error) {
  const normalized = error instanceof Error ? error : new Error(String(error));
  if (normalized instanceof ProtocolUnsupported) {
    return { id: input.id, message: normalized.message, status: "UNSUPPORTED_METHOD" };
  }
  if (normalized instanceof HarnessUnsupported) {
    return { id: input.id, message: normalized.message, status: "HARNESS_UNSUPPORTED" };
  }
  if (normalized instanceof ConnectionClosed) {
    return { id: input.id, message: normalized.message, status: "CRASH" };
  }
  if (normalized instanceof CommandError) {
    return { id: input.id, message: normalized.message, status: "PROTOCOL_FAILURE" };
  }
  if (/timed out/.test(normalized.message)) {
    return { id: input.id, message: normalized.message, status: "TIMEOUT" };
  }
  // CommandError and control-script exceptions mean tinybrowser did not
  // produce Chromium's behavior; runner-internal faults are reported as
  // HARNESS_FAILURE by the top-level handler instead.
  return { id: input.id, message: normalized.stack ?? normalized.message, status: "PROTOCOL_FAILURE" };
}

async function execute(input) {
  let daemon;
  let fixture;
  let result;
  try {
    fixture = await startFixtureServer(input.corpus.root);
    daemon = await startDaemon(input.binary);
    const directoryURL = new URL(`inspector-protocol/${dirname(input.testPath)}/`, `${fixture.origin}/`);
    const testURL = new URL(`inspector-protocol/${input.testPath}`, `${fixture.origin}/`).href;
    const harnessResult = await runInspectorTest({
      blankURL: new URL("inspector-protocol/resources/inspector-protocol-page.html", `${fixture.origin}/`).href,
      corpusRoot: input.corpus.root,
      fixtureOrigin: fixture.origin,
      protocolTimeout: input.protocolTimeout,
      stopOnUnsupported: input.all,
      targetBaseURL: directoryURL.href,
      testBaseURL: directoryURL.href,
      testFile: input.testFile,
      testURL,
      webSocketURL: await browserWebSocket(daemon.origin),
    });
    const actual = canonicalOutput(harnessResult.output);
    const expected = canonicalOutput(readFileSync(input.expectedFile, "utf8"));
    if (actual === expected) {
      result = { id: input.id, status: "PASS" };
    } else {
      result = {
        actual,
        expected,
        id: input.id,
        status: harnessResult.unsupportedMethods.length ? "UNSUPPORTED_METHOD" : "PROTOCOL_FAILURE",
        unsupportedMethods: harnessResult.unsupportedMethods,
      };
    }
  } catch (error) {
    result = errorResult(input, error);
  } finally {
    if (daemon) await daemon.stop();
    if (fixture) await fixture.close();
  }
  const missingRequests = fixture?.missingRequests() ?? [];
  if (
    missingRequests.length > 0 &&
    ["HARNESS_FAILURE", "PROTOCOL_FAILURE", "TIMEOUT"].includes(result.status)
  ) {
    return {
      id: input.id,
      message: `fixture server could not satisfy: ${missingRequests.join(", ")}`,
      missingRequests,
      status: "MISSING_FIXTURE",
    };
  }
  return result;
}

async function main() {
  let input;
  try {
    input = workerInput();
    return await execute(input);
  } catch (error) {
    return {
      id: input?.id ?? "unknown",
      message: error.stack ?? String(error),
      status: "HARNESS_FAILURE",
    };
  }
}

const result = await main();
if (process.send) {
  process.send({ result, type: "result" }, () => process.exit(0));
} else {
  process.exit(1);
}
