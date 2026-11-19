// The Durable Object behind the relays' WebSockets (one for the world, named "mesh"). It holds a Mesh
// (mesh.js) in RAM: sockets, presence, routing, signed plans. Nothing is written to disk.
// A relay is let in by the Worker (routes.js) only after its signed hello and its registration were
// checked; the object trusts the x-relay header the Worker sets, and nothing else reaches it from outside.
import { Mesh, meshOp } from "./mesh.js";
import { signText } from "./sign.js";

const TICK_MS = 3000;
export const FRAMES_PER_MINUTE = 600;

/** The planner's seed: the UTC day, so every relay can compute the same local plan if the server is silent. */
export const seedFor = (nowMs) => Math.floor(nowMs / 86_400_000) >>> 0;

export class MeshObject {
  constructor(state, env) {
    this.env = env;
    this.mesh = new Mesh({ sign: (t) => signText(env.PROJECT_KEY, t), seed: seedFor(Date.now()) });
    this.rate = new Map(); // relay -> { minute, n }
    this.timer = null;
  }

  /** Counts frames per relay per minute; false when over the limit. */
  allow(call, now) {
    const minute = Math.floor(now / 60_000);
    const r = this.rate.get(call);
    if (!r || r.minute !== minute) {
      this.rate.set(call, { minute, n: 1 });
      return true;
    }
    return ++r.n <= FRAMES_PER_MINUTE;
  }

  startTicks() {
    if (this.timer) return;
    this.timer = setInterval(async () => {
      const now = Date.now();
      this.mesh.seed = seedFor(now);
      this.mesh.tick(now);
      await this.mesh.publish(now);
      if (this.mesh.hub.sockets.size === 0) {
        clearInterval(this.timer);
        this.timer = null;
      }
    }, TICK_MS);
  }

  async fetch(request) {
    const url = new URL(request.url);
    const call = request.headers.get("x-relay");
    if (url.pathname === "/socket") {
      if (!call || request.headers.get("upgrade") !== "websocket") return new Response("expected a websocket", { status: 426 });
      const pair = new WebSocketPair();
      const [client, server] = [pair[0], pair[1]];
      server.accept();
      const socket = { send: (m) => server.send(JSON.stringify(m)), close: () => { try { server.close(1000, "closed"); } catch { /* already closed */ } } };
      this.mesh.connect(call, socket, Date.now());
      server.addEventListener("message", (ev) => {
        const now = Date.now();
        if (!this.allow(call, now)) return socket.send({ t: "slow" });
        let msg;
        try { msg = JSON.parse(String(ev.data)); } catch { return; }
        try {
          const reply = this.mesh.onFrame(call, msg, now);
          if (reply) socket.send(reply);
        } catch (e) {
          socket.send({ t: "error", why: String(e?.message ?? e).slice(0, 120) });
        }
      });
      server.addEventListener("close", () => this.mesh.disconnect(call, socket));
      server.addEventListener("error", () => this.mesh.disconnect(call, socket));
      this.startTicks();
      return new Response(null, { status: 101, webSocket: client });
    }
    if (url.pathname === "/op" && request.method === "POST") {
      const { op, args } = await request.json();
      const now = args?.nowMs ?? Date.now();
      let out;
      if (op === "frames" && !this.allow(args.call, now)) out = { error: "too many frames" };
      else {
        try { out = meshOp(this.mesh, op, args, now); }
        catch { return new Response(JSON.stringify({ error: "unknown op" }), { status: 400 }); }
      }
      return new Response(JSON.stringify(out), { headers: { "content-type": "application/json" } });
    }
    return new Response("not found", { status: 404 });
  }
}
