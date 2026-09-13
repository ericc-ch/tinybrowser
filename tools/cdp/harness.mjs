import { readFile } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import vm from "node:vm";

import { ProtocolConnection } from "./protocol.mjs";

export class HarnessUnsupported extends Error {}
export class ProtocolUnsupported extends Error {}
export class CommandError extends Error {}

let activeRunner = null;

// One worker runs one control script, so an unhandled rejection inside the
// script is a test failure. content_shell routes these to its own completion
// handler; killing the worker would lose the already-logged output.
process.on("unhandledRejection", (reason) => {
  const error = reason instanceof Error ? reason : new Error(String(reason));
  if (activeRunner) activeRunner.reject(error);
  else throw error;
});

const STABILIZE_NAMES = [
  "id",
  "nodeId",
  "objectId",
  "scriptId",
  "timestamp",
  "backendNodeId",
  "parentId",
  "frameId",
  "loaderId",
  "baseURL",
  "documentURL",
  "styleSheetId",
  "executionContextId",
  "executionContextUniqueId",
  "openerId",
  "targetId",
  "browserContextId",
  "sessionId",
  "receivedBytes",
  "ownerNode",
  "guid",
  "requestId",
  "openerFrameId",
  "parentFrameId",
  "issueId",
  "initiatingFrameId",
  "pipelineId",
  "debuggerId",
];

function commandResult(message, method) {
  if (message.error) {
    throw new CommandError(`${method}: ${JSON.stringify(message.error)}`);
  }
  return message.result;
}

function runControlScript(source, context, file, corpusRoot) {
  const baseURL = /^https?:/.test(file) ? new URL(file) : pathToFileURL(file);
  return vm.runInContext(`${source}\n//# sourceURL=${file}`, context, {
    filename: file,
    importModuleDynamically(specifier) {
      const moduleURL = new URL(specifier, baseURL);
      if (moduleURL.protocol === "file:") {
        const moduleFile = fileURLToPath(moduleURL);
        if (moduleFile === corpusRoot || moduleFile.startsWith(`${corpusRoot}${sep}`)) {
          return import(moduleURL);
        }
      }
      throw new HarnessUnsupported(`Node cannot import control module: ${moduleURL.href}`);
    },
  });
}

class TestRunner {
  static get stabilizeNames() {
    return STABILIZE_NAMES;
  }

  static extendStabilizeNames(extended) {
    return [...STABILIZE_NAMES, ...extended];
  }

  static wrapPromiseWithTimeout(promise, timeout, label) {
    if (!timeout) return promise;
    let timer;
    const error = new Error(`Timed out at ${label}`);
    const timeoutPromise = new Promise((resolveTimeout) => {
      timer = setTimeout(resolveTimeout, timeout);
    });
    return Promise.race([
      promise.then((result) => {
        clearTimeout(timer);
        return result;
      }),
      timeoutPromise.then(() => Promise.reject(error)),
    ]);
  }

  constructor(input) {
    this.connection = input.connection;
    this.testBaseURL = input.testBaseURL;
    this.targetBaseURL = input.targetBaseURL;
    this.fixtureOrigin = input.fixtureOrigin;
    this.blankURL = input.blankURL;
    this.testFile = input.testFile;
    this.corpusRoot = input.corpusRoot;
    this.context = input.context;
    this.protocolTimeout = input.protocolTimeout;
    this.stopOnUnsupported = input.stopOnUnsupported;
    this.lines = [];
    this.stableValues = new Map();
    this.sessions = new Map();
    this.unsupportedMethods = new Set();
    this.completed = false;
    this.completion = new Promise((resolveCompletion, rejectCompletion) => {
      this.resolveCompletion = resolveCompletion;
      this.rejectCompletion = rejectCompletion;
    });
    this.completion.catch(() => {});
    this.browserSessionValue = new Session(this, "");
  }

  createSessionFor(sessionId) {
    return new Session(this, sessionId);
  }

  createChildTargetManagerFor(session) {
    return new ChildTargetManager(this, session);
  }

  completeTest() {
    if (this.completed) return;
    this.completed = true;
    this.resolveCompletion();
  }

  reject(error) {
    if (this.completed) return;
    this.completed = true;
    this.rejectCompletion(error);
  }

  log(item, title, stabilizeNames = STABILIZE_NAMES, stabilizeValues = []) {
    if (typeof item === "object") {
      this.logObject(item, title, stabilizeNames, stabilizeValues);
      return;
    }
    this.lines.push(String(item));
  }

  logObject(object, title, stabilizeNames, stabilizeValues) {
    const lines = [];
    const stableValues = this.stableValues;

    function dumpValue(value, prefix, prefixWithName) {
      if (typeof value === "object" && value !== null) {
        if (Array.isArray(value)) dumpItems(value, prefix, prefixWithName);
        else dumpProperties(value, prefix, prefixWithName);
        return;
      }
      const valueString = String(value).replaceAll("\n", " ");
      lines.push(`${prefixWithName}${valueString.length ? " " : ""}${valueString}`);
    }

    function dumpProperties(value, prefix = "", firstLinePrefix = prefix) {
      const opening = /\S$/.test(firstLinePrefix) ? `${firstLinePrefix} {` : `${firstLinePrefix}{`;
      lines.push(opening);
      for (const name of Object.keys(value).sort()) {
        let property = value[name];
        if (stabilizeValues.includes(name)) {
          if (!stableValues.has(property)) {
            stableValues.set(property, `<${typeof property} ${stableValues.size}>`);
          }
          property = stableValues.get(property);
        } else if (stabilizeNames.includes(name)) {
          property = `<${typeof property}>`;
        }
        dumpValue(property, `    ${prefix}`, `    ${prefix}${name} :`);
      }
      lines.push(`${prefix}}`);
    }

    function dumpItems(value, prefix = "", firstLinePrefix = prefix) {
      const opening = /\S$/.test(firstLinePrefix) ? `${firstLinePrefix} [` : `${firstLinePrefix}[`;
      lines.push(opening);
      value.forEach((item, index) => {
        dumpValue(item, `    ${prefix}`, `    ${prefix}[${index}] :`);
      });
      lines.push(`${prefix}]`);
    }

    dumpValue(object, "", title ?? "");
    this.lines.push(lines.join("\n"));
  }

  params(name) {
    if (name) return null;
    return new URLSearchParams();
  }

  trimURL(url) {
    return url.replace(/^.*(([^/]*[/]){3}[^/]*)$/, "...$1");
  }

  url(path) {
    if (/^(about:|chrome:|data:|file:|https?:)/.test(path)) return path;
    return new URL(path, this.targetBaseURL).href;
  }

  async runTestSuite(tests) {
    for (const test of tests) {
      this.log(`\nRunning test: ${test.name}`);
      try {
        await test();
      } catch (error) {
        this.log(`Error during test: ${error}\n${error.stack}`);
      }
    }
    this.completeTest();
  }

  checkExpectation(fail, name, message) {
    if (fail === Boolean(message.error)) {
      this.log(`PASS: ${name}`);
      return true;
    }
    this.log(`FAIL: ${name}: ${JSON.stringify(message)}`);
    this.completeTest();
    return false;
  }

  expectedSuccess(name, message) {
    return this.checkExpectation(false, name, message);
  }

  expectedError(name, message) {
    return this.checkExpectation(true, name, message);
  }

  die(message, error) {
    this.log(`${message}: ${error}\n${error.stack}`);
    this.reject(new Error(message));
  }

  fail(message) {
    this.log(`FAIL: ${message}`);
    this.completeTest();
  }

  async loadScript(path) {
    const file = resolve(dirname(this.testFile), path);
    if (file !== this.corpusRoot && !file.startsWith(`${this.corpusRoot}${sep}`)) {
      throw new HarnessUnsupported(`loadScript outside vendored corpus: ${path}`);
    }
    const source = await readFile(file, "utf8");
    return runControlScript(source, this.context, file, this.corpusRoot);
  }

  async loadScriptAbsolute(url) {
    if (new URL(url).origin !== this.fixtureOrigin) {
      throw new HarnessUnsupported(`control helper is outside the fixture server: ${url}`);
    }
    const response = await fetch(url);
    if (!response.ok) throw new HarnessUnsupported(`cannot fetch helper: ${url}`);
    return runControlScript(await response.text(), this.context, url, this.corpusRoot);
  }

  async loadScriptModule(path) {
    const file = resolve(dirname(this.testFile), path);
    if (file !== this.corpusRoot && !file.startsWith(`${this.corpusRoot}${sep}`)) {
      throw new HarnessUnsupported(`module outside vendored corpus: ${path}`);
    }
    return import(pathToFileURL(file));
  }

  browserSession() {
    return this.browserSessionValue;
  }

  browserP() {
    return this.browserSessionValue.protocol;
  }

  async attachFullBrowserSession() {
    const response = await this.browserP().Target.attachToBrowserTarget();
    const result = commandResult(response, "Target.attachToBrowserTarget");
    return new Session(this, result.sessionId);
  }

  async createPage(options = {}) {
    const params = { url: "about:blank" };
    for (const name of ["width", "height", "enableBeginFrameControl"]) {
      if (options[name] !== undefined) params[name] = options[name];
    }
    if (options.createContextOptions) {
      const contextResponse = await this.browserP().Target.createBrowserContext(
        options.createContextOptions,
      );
      params.browserContextId =
        commandResult(contextResponse, "Target.createBrowserContext").browserContextId;
    }
    const response = await this.browserP().Target.createTarget(params);
    const targetId = commandResult(response, "Target.createTarget").targetId;
    const page = new Page(this, targetId);
    await page.navigate(options.url ?? this.blankURL);
    return page;
  }

  async start(description, options) {
    if (!description) throw new Error("Please provide a description for the test!");
    this.log(description);
    const page = await this.createPage(options);
    if (options.html) await page.loadHTML(options.html);
    const session = await page.createSession();
    return { page, session, dp: session.protocol };
  }

  startBlank(description, options = {}) {
    return this.start(description, options);
  }

  startHTML(html, description, options = {}) {
    return this.start(description, { ...options, html });
  }

  startURL(url, description, options = {}) {
    return this.start(description, { ...options, url });
  }

  startWithFrameControl(description, options = {}) {
    return this.start(description, {
      ...options,
      width: options.width ?? 800,
      height: options.height ?? 600,
      createContextOptions: {},
      enableBeginFrameControl: true,
    });
  }

  async startBlankWithTabTarget(description) {
    if (!description) throw new Error("Please provide a description for the test!");
    this.log(description);
    const response = await this.browserP().Target.createTarget({ url: "about:blank", forTab: true });
    const targetId = commandResult(response, "Target.createTarget").targetId;
    const attached = await this.browserP().Target.attachToTarget({ targetId, flatten: true });
    const sessionId = commandResult(attached, "Target.attachToTarget").sessionId;
    return { tabTargetSession: new Session(this, sessionId) };
  }

  async logStackTrace(debuggers, stackTrace, debuggerId) {
    let current = stackTrace;
    let currentDebugger = debuggerId;
    while (current) {
      if (current.description) this.log(`--${current.description}--`);
      this.logCallFrames(current.callFrames);
      if (current.parentId) {
        if (current.parentId.debuggerId) currentDebugger = current.parentId.debuggerId;
        const result = await debuggers.get(currentDebugger).getStackTrace({
          stackTraceId: current.parentId,
        });
        current = result.stackTrace ?? result.result.stackTrace;
      } else {
        current = current.parent;
      }
    }
  }

  logCallFrames(callFrames) {
    for (const frame of callFrames) {
      const name = frame.functionName || "(anonymous)";
      const url = frame.url.replace(/[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}/, "UUID");
      const location = frame.location ?? frame;
      this.log(`${name} at ${url}:${location.lineNumber}:${location.columnNumber}`);
    }
  }

  unsupported(name) {
    throw new HarnessUnsupported(`content_shell host operation is unavailable: ${name}`);
  }

  setAllowUnsafeOperations() { this.unsupported("setAllowUnsafeOperations"); }
  setDisallowedSubresourcePathSuffixes() { this.unsupported("setDisallowedSubresourcePathSuffixes"); }
  setHighlightAds() { this.unsupported("setHighlightAds"); }
  simulateScreenOrientationLockChanged() { this.unsupported("simulateScreenOrientationLockChanged"); }
  clearTrustTokenState() { this.unsupported("clearTrustTokenState"); }
  disableAutomaticDragDrop() { this.unsupported("disableAutomaticDragDrop"); }
  disableMockScreenOrientation() { this.unsupported("disableMockScreenOrientation"); }
  setAnimationRequiresRaster() { this.unsupported("setAnimationRequiresRaster"); }
  triggerTestInspectorIssue() { this.unsupported("triggerTestInspectorIssue"); }
}

class Page {
  constructor(testRunner, targetId) {
    this.testRunner = testRunner;
    this.id = targetId;
    this._targetId = targetId;
  }

  targetId() {
    return this.id;
  }

  async createSession() {
    const response = await this.testRunner.browserP().Target.attachToTarget({
      targetId: this.id,
      flatten: true,
    });
    const sessionId = commandResult(response, "Target.attachToTarget").sessionId;
    return new Session(this.testRunner, sessionId);
  }

  navigate(url) {
    const operation = this.navigateTo(url);
    operation.catch(() => {});
    return operation;
  }

  async navigateTo(url) {
    const session = await this.createSession();
    try {
      await session.navigate(url);
    } finally {
      await session.disconnect();
    }
  }

  loadHTML(html) {
    const operation = this.loadHTMLIntoDocument(html);
    operation.catch(() => {});
    return operation;
  }

  async loadHTMLIntoDocument(html) {
    const session = await this.createSession();
    const escaped = html.replaceAll("'", "\\'").replaceAll("\n", "\\n");
    try {
      await session.protocol.Runtime.evaluate({
        awaitPromise: true,
        expression: `(function() {
          document.write('${escaped}');

          // Wait for all external scripts to load.
          const promise = new Promise(resolve => { window._loadHTMLResolve = resolve; })
            .then(() => { delete window._loadHTMLResolve; });
          if (document.querySelector('script[src]')) {
            document.write(
                '<script>window._loadHTMLResolve(); document.currentScript.remove();</script>');
          } else {
            window._loadHTMLResolve();
          }

          document.close();
          return promise;
        })()`,
      });
    } finally {
      await session.disconnect();
    }
  }
}

class Session {
  constructor(testRunner, sessionId) {
    this.testRunner = testRunner;
    this.sessionId = sessionId;
    this.parentSessionId = "";
    this.eventHandlers = new Map();
    this.protocol = this.createProtocol();
    this.testRunner.sessions.set(sessionId, this);
    this.stopListening = this.testRunner.connection.listen((message) => {
      if ((message.sessionId ?? "") !== this.sessionId) return;
      const handlers = this.eventHandlers.get(message.method) ?? [];
      for (const handler of [...handlers]) {
        Promise.resolve()
          .then(() => handler(message))
          .catch((error) => this.testRunner.reject(error));
      }
    });
  }

  disconnect() {
    const operation = this.disconnectFromTarget();
    operation.catch(() => {});
    return operation;
  }

  async disconnectFromTarget() {
    if (!this.sessionId) return;
    await this.testRunner.connection.send(
      "Target.detachFromTarget",
      { sessionId: this.sessionId },
      this.parentSessionId,
    );
    this.dispose();
  }

  dispose() {
    this.stopListening();
    this.testRunner.sessions.delete(this.sessionId);
  }

  createChild(sessionId) {
    const session = new Session(this.testRunner, sessionId);
    session.parentSessionId = this.sessionId;
    return session;
  }

  async attachChild(targetId) {
    const response = await this.protocol.Target.attachToTarget({ targetId, flatten: true });
    return this.createChild(commandResult(response, "Target.attachToTarget").sessionId);
  }

  sendCommand(method, params) {
    const response = this.testRunner.connection
      .send(method, params, this.sessionId)
      .then((message) => {
        if (message.error?.code === -32601) {
          this.testRunner.unsupportedMethods.add(method);
          if (this.testRunner.stopOnUnsupported) {
            this.testRunner.reject(new ProtocolUnsupported(method));
          }
        }
        return message;
      });
    response.catch(() => {});
    return response;
  }

  evaluate(code, ...args) {
    const operation = this.innerEvaluate({ awaitPromise: false, userGesture: false }, code, args);
    operation.catch(() => {});
    return operation;
  }

  evaluateAsync(code, ...args) {
    const operation = this.innerEvaluate({ awaitPromise: true, userGesture: false }, code, args);
    operation.catch(() => {});
    return operation;
  }

  evaluateAsyncWithUserGesture(code, ...args) {
    const operation = this.innerEvaluate({ awaitPromise: true, userGesture: true }, code, args);
    operation.catch(() => {});
    return operation;
  }

  async innerEvaluate(options, code, args) {
    let expression = code;
    if (typeof expression === "function") {
      expression = `(${expression})(${args.map((argument) => JSON.stringify(argument)).join(", ")})`;
    }
    const response = await this.protocol.Runtime.evaluate({
      expression,
      returnByValue: true,
      ...options,
    });
    if (response.error) {
      const maybeAsync = options.awaitPromise ? "async " : "";
      this.testRunner.log(
        `Error while evaluating ${maybeAsync}'${expression}': ${JSON.stringify(response.error)}`,
      );
      this.testRunner.completeTest();
      return undefined;
    }
    return response.result.result.value;
  }

  navigate(url, waitUntil = "load") {
    const operation = this.navigateTo(this.testRunner.url(url), waitUntil);
    operation.catch(() => {});
    return operation;
  }

  async navigateTo(url, waitUntil) {
    await this.protocol.Page.enable();
    await this.protocol.Page.setLifecycleEventsEnabled({ enabled: true });
    const tree = await this.protocol.Page.getFrameTree();
    const frameId = commandResult(tree, "Page.getFrameTree").frameTree.frame.id;
    const lifecycle = this.waitForEvent(
      "Page.lifecycleEvent",
      (event) => event.params.name === waitUntil && event.params.frameId === frameId,
    );
    const navigation = this.protocol.Page.navigate({ url });
    await Promise.all([lifecycle, navigation]);
  }

  createProtocol() {
    return new Proxy({}, {
      get: (_domains, domain) => new Proxy({}, {
        get: (_methods, member) => {
          const match = /^(on(ce)?|off)([A-Z][A-Za-z0-9]*)$/.exec(String(member));
          if (!match) {
            return (params = {}) => this.sendCommand(`${String(domain)}.${String(member)}`, params);
          }
          const event = `${String(domain)}.${match[3][0].toLowerCase()}${match[3].slice(1)}`;
          if (match[1] === "once") return (predicate) => this.waitForEvent(event, predicate);
          if (match[1] === "off") return (listener) => this.removeEventHandler(event, listener);
          return (listener) => this.addEventHandler(event, listener);
        },
      }),
    });
  }

  addEventHandler(event, handler) {
    const handlers = this.eventHandlers.get(event) ?? [];
    handlers.push(handler);
    this.eventHandlers.set(event, handlers);
  }

  removeEventHandler(event, handler) {
    const handlers = this.eventHandlers.get(event) ?? [];
    const index = handlers.indexOf(handler);
    if (index !== -1) handlers.splice(index, 1);
  }

  _addEventHandler(event, handler) {
    this.addEventHandler(event, handler);
  }

  _removeEventHandler(event, handler) {
    this.removeEventHandler(event, handler);
  }

  waitForEvent(event, predicate) {
    const occurrence = new Promise((resolveEvent, rejectEvent) => {
      const timer = setTimeout(() => {
        this.removeEventHandler(event, handler);
        rejectEvent(new Error(`waiting for ${event} timed out`));
      }, this.testRunner.protocolTimeout);
      const handler = (message) => {
        if (predicate && !predicate(message)) return;
        clearTimeout(timer);
        this.removeEventHandler(event, handler);
        resolveEvent(message);
      };
      this.addEventHandler(event, handler);
    });
    occurrence.catch(() => {});
    return occurrence;
  }
}

class ChildTargetManager {
  constructor(testRunner, session) {
    this.testRunner = testRunner;
    this.session = session;
    this.attachedTargets = [];
  }

  async startAutoAttach(params = {
    autoAttach: true,
    flatten: true,
    waitForDebuggerOnStart: false,
  }) {
    this.session.protocol.Target.onAttachedToTarget((event) => {
      this.attachedTargets.push(event.params);
    });
    await this.session.protocol.Target.setAutoAttach(params);
  }

  findAttachedSession(predicate) {
    const found = this.attachedTargets.find(({ targetInfo }) => predicate(targetInfo));
    return found ? this.session.createChild(found.sessionId) : null;
  }

  findAttachedSessionPrimaryMainFrame() {
    return this.findAttachedSession(
      (info) => info.type === "page" && info.subtype === undefined,
    );
  }

  findAttachedSessionPrerender() {
    return this.findAttachedSession((info) => info.type === "page" && info.subtype === "prerender");
  }
}

function sandboxFor(testFile, testURL, fixtureOrigin) {
  const timers = new Set();
  const intervals = new Set();
  function hostSetTimeout(callback, delay, ...args) {
    const timer = setTimeout(() => {
      timers.delete(timer);
      callback(...args);
    }, delay);
    timers.add(timer);
    return timer;
  }
  function hostClearTimeout(timer) {
    timers.delete(timer);
    clearTimeout(timer);
  }
  function hostSetInterval(callback, delay, ...args) {
    const interval = setInterval(callback, delay, ...args);
    intervals.add(interval);
    return interval;
  }
  function hostClearInterval(interval) {
    intervals.delete(interval);
    clearInterval(interval);
  }
  const hostDocument = new Proxy({}, {
    get() {
      throw new HarnessUnsupported("the control script requires a content_shell document");
    },
  });
  const unsupportedDevToolsAPI = new Proxy({}, {
    get() {
      throw new HarnessUnsupported("the control script requires content_shell's DevToolsAPI");
    },
    set() {
      throw new HarnessUnsupported("the control script requires content_shell's DevToolsAPI");
    },
  });
  function fixtureFetch(resource, init) {
    const rawURL = resource instanceof Request ? resource.url : resource;
    const url = new URL(rawURL, testURL);
    if (url.protocol !== "data:" && url.origin !== fixtureOrigin) {
      throw new HarnessUnsupported(`control fetch is outside the fixture server: ${url.href}`);
    }
    return resource instanceof Request ? fetch(resource, init) : fetch(url, init);
  }
  const sandbox = {
    ArrayBuffer,
    atob: (value) => Buffer.from(value, "base64").toString("binary"),
    Blob,
    btoa: (value) => Buffer.from(value, "binary").toString("base64"),
    clearInterval: hostClearInterval,
    clearTimeout: hostClearTimeout,
    console,
    crypto,
    DevToolsAPI: unsupportedDevToolsAPI,
    document: hostDocument,
    fetch: fixtureFetch,
    Headers,
    location: new URL(testURL),
    performance,
    queueMicrotask,
    Request,
    Response,
    setInterval: hostSetInterval,
    setTimeout: hostSetTimeout,
    structuredClone,
    TextDecoder,
    TextEncoder,
    URL,
    URLSearchParams,
    WebAssembly,
  };
  const context = vm.createContext(sandbox, { name: relative(process.cwd(), testFile) });
  context.globalThis = context;
  context.self = context;
  context.window = context;
  context.TestRunner = TestRunner;
  return {
    context,
    dispose() {
      for (const timer of timers) clearTimeout(timer);
      for (const interval of intervals) clearInterval(interval);
      timers.clear();
      intervals.clear();
    },
  };
}

export async function runInspectorTest(input) {
  const connection = await ProtocolConnection.connect(input.webSocketURL, input.protocolTimeout);
  const sandbox = sandboxFor(input.testFile, input.testURL, input.fixtureOrigin);
  const { context } = sandbox;
  const testRunner = new TestRunner({ ...input, connection, context });
  context.testRunner = testRunner;
  activeRunner = testRunner;
  connection.closed.then((error) => testRunner.reject(error));

  try {
    const source = await readFile(input.testFile, "utf8");
    const test = runControlScript(source, context, input.testFile, input.corpusRoot);
    if (typeof test !== "function") {
      throw new HarnessUnsupported("test source did not evaluate to a function");
    }
    Promise.resolve(test(testRunner)).catch((error) => testRunner.reject(error));
    await testRunner.completion;
    return {
      output: testRunner.lines.join("\n"),
      unsupportedMethods: [...testRunner.unsupportedMethods].sort(),
    };
  } finally {
    sandbox.dispose();
    await connection.drain();
    connection.close();
  }
}
