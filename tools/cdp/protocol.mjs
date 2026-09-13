export class ConnectionClosed extends Error {}

export class ProtocolConnection {
  static async connect(url, responseTimeoutMs) {
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", () => reject(new Error(`cannot connect to ${url}`)), {
        once: true,
      });
    });
    return new ProtocolConnection(socket, responseTimeoutMs);
  }

  constructor(socket, responseTimeoutMs) {
    this.socket = socket;
    this.responseTimeoutMs = responseTimeoutMs;
    this.nextId = 1;
    this.pending = new Map();
    this.inflight = new Set();
    this.listeners = new Set();
    this.closed = new Promise((resolveClosed) => {
      this.resolveClosed = resolveClosed;
    });
    socket.addEventListener("message", (event) => this.dispatch(JSON.parse(String(event.data))));
    socket.addEventListener("close", () => {
      const error = new ConnectionClosed("CDP connection closed");
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        const message = {
          error: { code: -32000, message: error.message },
          id: pending.id,
        };
        if (pending.sessionId) message.sessionId = pending.sessionId;
        pending.resolve(message);
      }
      this.pending.clear();
      this.resolveClosed(error);
    });
  }

  send(method, params = {}, sessionId = "") {
    const id = this.nextId++;
    const message = { id, method, params };
    if (sessionId) message.sessionId = sessionId;

    const response = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} response timed out`));
      }, this.responseTimeoutMs);
      this.pending.set(id, { id, reject, resolve, sessionId, timer });
      this.socket.send(JSON.stringify(message));
    });
    response.catch(() => {});
    this.inflight.add(response);
    response.then(
      () => this.inflight.delete(response),
      () => this.inflight.delete(response),
    );
    return response;
  }

  async drain() {
    while (this.inflight.size) {
      await Promise.allSettled([...this.inflight]);
    }
  }

  listen(listener) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  close() {
    this.socket.close();
  }

  dispatch(message) {
    if (typeof message.id === "number") {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      pending.resolve(message);
      return;
    }
    for (const listener of this.listeners) listener(message);
  }
}
