import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, resolve, sep } from "node:path";

const CONTENT_TYPES = new Map([
  [".css", "text/css"],
  [".gif", "image/gif"],
  [".html", "text/html"],
  [".js", "text/javascript"],
  [".json", "application/json"],
  [".mjs", "text/javascript"],
  [".png", "image/png"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain"],
  [".wasm", "application/wasm"],
  [".xml", "application/xml"],
]);

// Chromium's WebTest servers expose the inspector-protocol trees at
// `/inspector-protocol/`. One worker serves exactly one tree, so that prefix
// maps to the tree root and everything else resolves relative to it.
const MOUNT = "/inspector-protocol/";

export async function startFixtureServer(root) {
  const missing = new Set();
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (url.pathname === "/blank.html") {
      response.writeHead(200, { "content-type": "text/html" });
      response.end("<!doctype html><title></title>");
      return;
    }

    let decoded;
    try {
      decoded = decodeURIComponent(
        (url.pathname.startsWith(MOUNT) ? url.pathname.slice(MOUNT.length) : url.pathname)
          .replace(/^\//, ""),
      );
    } catch {
      response.writeHead(400, { "content-type": "text/plain" });
      response.end("invalid path");
      return;
    }
    const file = resolve(root, decoded);
    if (file !== root && !file.startsWith(`${root}${sep}`)) {
      response.writeHead(403, { "content-type": "text/plain" });
      response.end("forbidden");
      return;
    }

    if ([".cgi", ".php", ".pl", ".py"].includes(extname(file))) {
      missing.add(url.pathname);
      response.writeHead(501, { "content-type": "text/plain" });
      response.end("dynamic fixture unsupported");
      return;
    }

    try {
      if (!statSync(file).isFile()) throw new Error("not a file");
      response.writeHead(200, {
        "content-type": CONTENT_TYPES.get(extname(file)) ?? "application/octet-stream",
      });
      createReadStream(file).pipe(response);
    } catch {
      missing.add(url.pathname);
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("not found");
    }
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("fixture server has no port");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: () => new Promise((resolveClose, rejectClose) =>
      server.close((error) => error ? rejectClose(error) : resolveClose())),
    missingRequests: () => [...missing].filter((path) => path !== "/favicon.ico").sort(),
  };
}
