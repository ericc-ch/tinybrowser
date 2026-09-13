import { fork } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const TOOLS = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(TOOLS, "../..");
const WORKER = process.env.TINYBROWSER_CDP_WORKER ?? join(TOOLS, "worker.mjs");
const VENDOR = join(ROOT, "third_party/blink-cdp");
const CORPORA = [
  {
    name: "plain",
    root: join(VENDOR, "inspector-protocol"),
  },
  {
    name: "http",
    root: join(VENDOR, "http-inspector-protocol"),
  },
];

const activeWorkers = new Set();
let runTemporaryRoot = null;

function parsePositiveInteger(raw, name) {
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 1) throw new Error(`${name} must be a positive integer`);
  return value;
}

function parseArguments(args) {
  const options = {
    all: false,
    jobs: null,
    list: false,
    patterns: [],
    timeoutMs: 3_000,
    verbose: false,
  };
  for (const argument of args) {
    if (argument === "--all") options.all = true;
    else if (argument === "--list") options.list = true;
    else if (argument === "--verbose") options.verbose = true;
    else if (argument.startsWith("--jobs=")) {
      options.jobs = parsePositiveInteger(argument.slice("--jobs=".length), "--jobs");
    } else if (argument.startsWith("--timeout=")) {
      options.timeoutMs = parsePositiveInteger(argument.slice("--timeout=".length), "--timeout");
    } else if (argument.startsWith("-")) {
      throw new Error(`unknown option: ${argument}`);
    } else {
      options.patterns.push(argument);
    }
  }
  options.jobs ??= options.all ? 4 : 1;
  return options;
}

async function filesBelow(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await filesBelow(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

async function discoverTests() {
  const tests = [];
  for (const corpus of CORPORA) {
    for (const expectedFile of await filesBelow(corpus.root)) {
      if (!expectedFile.endsWith("-expected.txt")) continue;
      const testFile = expectedFile.slice(0, -"-expected.txt".length) + ".js";
      if (!existsSync(testFile)) continue;
      const path = relative(corpus.root, testFile).split(sep).join("/");
      tests.push({
        corpus,
        expectedFile,
        id: `${corpus.name}/${path}`,
        path,
        testFile,
      });
    }
  }
  return tests.sort((left, right) => left.id.localeCompare(right.id));
}

function passingTests() {
  return readFileSync(join(TOOLS, "passing.txt"), "utf8")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
}

const ABSOLUTE_URL = /(?:https?|wss?):\/\/[^\s'"`\\<>]+/g;
// Namespace and documentation links that appear in test sources without
// causing a network request. Everything else is a Chromium test-server
// destination we cannot serve offline.
const DOCUMENTATION_HOSTS = new Set([
  "bugs.webkit.org",
  "chromium-review.googlesource.com",
  "crbug.com",
  "developer.mozilla.org",
  "drafts.csswg.org",
  "github.com",
  "html.spec.whatwg.org",
  "issues.chromium.org",
  "source.chromium.org",
  "tc39.es",
  "w3c.github.io",
  "webkit.org",
  "www.w3.org",
]);

function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:\\])\/\/[^\n]*/gm, "$1");
}

const HTML_REFERENCE = /['"`]([^'"`\s]*\.html[^'"`\s]*)['"`]/g;

function referencedHTML(test, source) {
  const files = new Set();
  for (const match of source.matchAll(HTML_REFERENCE)) {
    const raw = match[1];
    if (/^(?:https?:|\/\/)/.test(raw)) continue;
    let path;
    try {
      path = decodeURIComponent(raw.split(/[?#]/, 1)[0]);
    } catch {
      continue;
    }
    const file = resolve(dirname(test.testFile), path);
    if (file.startsWith(`${test.corpus.root}${sep}`) && existsSync(file)) files.add(file);
  }
  return files;
}

function blockedURL(test) {
  const source = stripComments(readFileSync(test.testFile, "utf8"));
  const sources = [source];
  for (const file of referencedHTML(test, source)) {
    sources.push(stripComments(readFileSync(file, "utf8")));
  }
  for (const content of sources) {
    for (const match of content.matchAll(ABSOLUTE_URL)) {
      let url;
      try {
        url = new URL(match[0]);
      } catch {
        continue;
      }
      if (!DOCUMENTATION_HOSTS.has(url.hostname)) return match[0];
    }
  }
  return null;
}

function globPattern(pattern) {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${escaped.replaceAll("*", ".*")}$`);
}

function selectTests(tests, options) {
  if (options.all && options.patterns.length === 0) return tests;
  const patterns = options.patterns.length ? options.patterns : passingTests();
  if (patterns.length === 0) {
    throw new Error("tools/cdp/passing.txt is empty; use --all or provide a path pattern");
  }
  const matchers = patterns.map(globPattern);
  const selected = tests.filter((test) => matchers.some((matcher) => matcher.test(test.id)));
  if (selected.length === 0) throw new Error(`no CDP tests matched: ${patterns.join(", ")}`);
  return selected;
}

function killWorker(worker) {
  if (worker.pid === undefined) return;
  for (const target of [-worker.pid, worker.pid]) {
    try {
      process.kill(target, "SIGKILL");
    } catch (error) {
      if (error.code !== "ESRCH" && error.code !== "EPERM") {
        console.error(`cannot signal ${target}: ${error.message}`);
      }
    }
  }
}

function runOne(test, input) {
  const blocked = blockedURL(test);
  if (blocked) {
    return Promise.resolve({
      id: test.id,
      message: `requires Chromium's fixed-port test servers (${blocked})`,
      status: "MISSING_FIXTURE",
    });
  }
  return new Promise((resolveResult) => {
    const worker = fork(WORKER, [], {
      detached: true,
      env: {
        ...process.env,
        TINYBROWSER_CDP_WORKER_INPUT: JSON.stringify({
          all: input.all,
          binary: input.binary,
          corpus: test.corpus,
          expectedFile: test.expectedFile,
          id: test.id,
          protocolTimeout: Math.max(100, Math.floor(input.timeoutMs * 0.8)),
          testFile: test.testFile,
          testPath: test.path,
        }),
        TMPDIR: input.temporaryRoot,
      },
      execArgv: ["--experimental-vm-modules"],
      stdio: ["ignore", "pipe", "pipe", "ipc"],
    });
    activeWorkers.add(worker);
    let stderr = "";
    worker.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk).slice(-4_000);
    });
    worker.stdout.resume();
    let finished = false;
    const timer = setTimeout(() => {
      finish({
        id: test.id,
        message: `${test.id} exceeded ${input.timeoutMs} ms`,
        status: "TIMEOUT",
      });
    }, input.timeoutMs);

    function describe(message) {
      const tail = stderr.trim().replaceAll(/\s+/g, " ").slice(-500);
      return tail ? `${message} (worker stderr: ${tail})` : message;
    }

    function finish(result) {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      activeWorkers.delete(worker);
      killWorker(worker);
      resolveResult(result);
    }

    worker.once("message", (message) => {
      if (message?.type === "result" && message.result?.id === test.id) {
        finish(message.result);
      } else {
        finish({ id: test.id, message: describe("worker returned an invalid result"), status: "CRASH" });
      }
    });
    worker.once("error", (error) => {
      finish({ id: test.id, message: describe(error.stack ?? String(error)), status: "CRASH" });
    });
    worker.once("exit", (code, signal) => {
      finish({
        id: test.id,
        message: describe(`worker exited before reporting (code=${code}, signal=${signal})`),
        status: "CRASH",
      });
    });
  });
}

async function runWorkers(tests, jobs, input) {
  const results = new Array(tests.length);
  let next = 0;
  async function worker() {
    while (next < tests.length) {
      const index = next++;
      const result = await runOne(tests[index], input);
      results[index] = result;
      const detail = result.status === "PASS" ? "" : ` ${result.message ?? ""}`;
      console.log(`${result.status.padEnd(20)} ${result.id}${detail}`.trimEnd());
    }
  }
  await Promise.all(Array.from({ length: Math.min(jobs, tests.length) }, worker));
  return results;
}

function printDetails(results, verbose) {
  const counts = new Map();
  for (const result of results) counts.set(result.status, (counts.get(result.status) ?? 0) + 1);
  console.log("\nCDP corpus summary");
  for (const [status, count] of [...counts].sort()) {
    console.log(`${status.padEnd(20)} ${count}`);
  }
  if (!verbose) return;
  for (const result of results) {
    if (result.status === "PASS" || result.actual === undefined) continue;
    console.log(`\n--- ${result.id}: expected`);
    console.log(result.expected);
    console.log(`\n+++ ${result.id}: actual`);
    console.log(result.actual);
  }
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const tests = await discoverTests();
  const selected = selectTests(tests, options);
  if (options.list) {
    selected.forEach((test) => console.log(test.id));
    return;
  }

  const binary = process.env.TINYBROWSER_BIN;
  if (!binary) throw new Error("TINYBROWSER_BIN is not set; run ./tools/cdp/run");
  console.log(`Running ${selected.length} of ${tests.length} Blink CDP tests using ${options.jobs} worker(s)`);
  const temporaryRoot = join(ROOT, "target/cdp-tmp");
  runTemporaryRoot = temporaryRoot;
  rmSync(temporaryRoot, { recursive: true, force: true });
  mkdirSync(temporaryRoot, { recursive: true });
  try {
    const results = await runWorkers(selected, options.jobs, {
      all: options.all,
      binary,
      temporaryRoot,
      timeoutMs: options.timeoutMs,
    });
    printDetails(results, options.verbose);
    const resultsFile = process.env.TINYBROWSER_CDP_RESULTS ?? join(ROOT, "target/cdp-results.json");
    writeFileSync(resultsFile, `${JSON.stringify(results, null, 2)}\n`);
    if (results.some((result) => result.status !== "PASS")) process.exitCode = 1;
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
}

for (const [signal, exitCode] of [["SIGINT", 130], ["SIGTERM", 143]]) {
  process.once(signal, () => {
    for (const worker of activeWorkers) killWorker(worker);
    if (runTemporaryRoot) rmSync(runTemporaryRoot, { recursive: true, force: true });
    process.exit(exitCode);
  });
}

await main();
