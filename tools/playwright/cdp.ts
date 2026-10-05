/** Minimal raw CDP client over the daemon's page target websocket. */

export interface RawCdp {
  (id: number, method: string, params: Record<string, unknown>): Promise<any>;
  socket: any;
}

/** Connects to the first page target and returns a send function. */
export async function cdpConnection(origin: string): Promise<any> {
  const targets = await (await fetch(`${origin}/json/list`)).json();
  const page = targets.find((target: any) => target.type === "page") ?? targets[0];
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("cdp socket failed"));
  });
  return socket;
}

let nextId = 1;

/** Sends one command and resolves with its result. */
export function cdpSend(
  socket: any,
  method: string,
  params: Record<string, unknown>,
): Promise<any> {
  const id = nextId++;
  return new Promise((resolve) => {
    const listener = (event: any) => {
      const message = JSON.parse(String(event.data));
      if (message.id === id) {
        socket.removeEventListener("message", listener);
        resolve(message.result ?? {});
      }
    };
    socket.addEventListener("message", listener);
    socket.send(JSON.stringify({ id, method, params }));
  });
}
