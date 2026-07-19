// The seam between the UI and the server. The UI programs against `Conn` only, so
// the mock and the WebSocket transport are interchangeable. Unlike the server's
// synchronous in-browser stand-in, the real transport is push: a query is answered
// by a later message, a perform streams events back, and unsolicited narration can
// arrive any time. So the interface is send-and-subscribe, not request-and-return,
// and it speaks the wire vocabulary directly (`ClientMsg` out, `ServerMsg` in).

import type { ClientMsg } from "./bindings/ClientMsg";
import type { ServerMsg } from "./bindings/ServerMsg";
import { MockWorld } from "./mockWorld";

export interface Conn {
  send(msg: ClientMsg): void;
  subscribe(cb: (msg: ServerMsg) => void): void;
  onOpen(cb: () => void): void;
  // A server-initiated close (an `@quit`, a shutdown) arrives as a socket close, not
  // a `ServerMsg`, so the UI observes it here rather than through `subscribe`.
  onClose(cb: () => void): void;
  close(): void;
}

// The live WebSocket transport. Sends are JSON envelopes; every inbound text frame
// is one `ServerMsg` fanned out to subscribers.
export class WsConn implements Conn {
  private ws: WebSocket;
  private subs: ((msg: ServerMsg) => void)[] = [];
  private opens: (() => void)[] = [];
  private closes: (() => void)[] = [];

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.ws.onopen = () => this.opens.forEach((f) => f());
    this.ws.onclose = () => this.closes.forEach((f) => f());
    this.ws.onmessage = (e) => {
      let msg: ServerMsg;
      try {
        msg = JSON.parse(e.data as string);
      } catch {
        // One bad frame is a server bug, not a reason to tear down the session:
        // mirror the transport's own tolerance and skip it.
        console.warn("unparseable server frame; skipping", e.data);
        return;
      }
      this.subs.forEach((f) => f(msg));
    };
  }

  send(msg: ClientMsg) {
    this.ws.send(JSON.stringify(msg));
  }
  subscribe(cb: (msg: ServerMsg) => void) {
    this.subs.push(cb);
  }
  onOpen(cb: () => void) {
    this.opens.push(cb);
  }
  onClose(cb: () => void) {
    this.closes.push(cb);
  }
  close() {
    this.ws.close();
  }
}

// The offline path: the same push interface answered synchronously by the in-browser
// `MockWorld`. `send` runs the message and feeds the replies straight back to
// subscribers; `onOpen` fires on a microtask so the UI has registered its callbacks
// before the bootstrap runs.
export class MockConn implements Conn {
  private world = new MockWorld();
  private subs: ((msg: ServerMsg) => void)[] = [];
  private opens: (() => void)[] = [];
  private closes: (() => void)[] = [];

  constructor() {
    queueMicrotask(() => this.opens.forEach((f) => f()));
  }

  send(msg: ClientMsg) {
    for (const reply of this.world.handle(msg)) {
      this.subs.forEach((f) => f(reply));
    }
  }
  subscribe(cb: (msg: ServerMsg) => void) {
    this.subs.push(cb);
  }
  onOpen(cb: () => void) {
    this.opens.push(cb);
  }
  onClose(cb: () => void) {
    this.closes.push(cb);
  }
  close() {
    this.closes.forEach((f) => f());
  }
}
