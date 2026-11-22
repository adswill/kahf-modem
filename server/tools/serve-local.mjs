// A local copy of the server on a port, with an in-memory store, for trying the relay
// software end to end: `node tools/serve-local.mjs 8787`. Nothing is saved; no Cloudflare.
import http from "node:http";
import { createHash } from "node:crypto";
import { register } from "node:module";
import { fakeStore } from "./fake-store.mjs";

// Node 20 cannot import .ts files, so a small load hook reads page.ts as JavaScript.
register(
  "data:text/javascript," +
    encodeURIComponent(`
import { readFile } from "node:fs/promises";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) {
    return { format: "module", source: await readFile(new URL(url), "utf8"), shortCircuit: true };
  }
  return nextLoad(url, context);
}`),
);
const { handleRequest } = await import("../src/routes.js");

const port = Number(process.argv[2] || 8787);
const store = fakeStore();
const server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = chunks.length ? Buffer.concat(chunks) : undefined;
    const request = new Request(`http://127.0.0.1:${port}${req.url}`, {
      method: req.method,
      headers: req.headers,
      body: req.method === "GET" || req.method === "HEAD" ? undefined : body,
    });
    const r = await handleRequest(request, store, Date.now(), {});
    res.writeHead(r.status, Object.fromEntries(r.headers));
    res.end(Buffer.from(await r.arrayBuffer()));
});

// A minimal RFC 6455 server side for /mesh/ws, so the relay software's push link can be tried locally.
// The signed hello is checked by the same route as in production; the frames go to the in-process Mesh.
function frame(text) {
  const p = Buffer.from(text);
  const head = p.length < 126 ? Buffer.from([0x81, p.length]) : Buffer.from([0x81, 126, p.length >> 8, p.length & 255]);
  return Buffer.concat([head, p]);
}
server.on("upgrade", async (req, socket) => {
  socket.setNoDelay(true);
  const auth = await handleRequest(
    new Request(`http://127.0.0.1:${port}${req.url}`, { headers: { ...req.headers, upgrade: "websocket" } }),
    store,
    Date.now(),
    {},
  );
  if (auth.status !== 200) {
    socket.end(`HTTP/1.1 ${auth.status} refused\r\n\r\n`);
    return;
  }
  const { call } = await auth.json();
  const accept = createHash("sha1").update(req.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  const peer = { send: (m) => socket.write(frame(JSON.stringify(m))), close: () => socket.end() };
  store.mesh.connect(call, peer, Date.now());
  let buf = Buffer.alloc(0);
  socket.on("data", (d) => {
    buf = Buffer.concat([buf, d]);
    for (;;) {
      if (buf.length < 2) return;
      let len = buf[1] & 127, off = 2;
      if (len === 126) { if (buf.length < 4) return; len = buf.readUInt16BE(2); off = 4; }
      const masked = (buf[1] & 128) !== 0;
      if (buf.length < off + (masked ? 4 : 0) + len) return;
      const mask = masked ? buf.subarray(off, off + 4) : null;
      if (masked) off += 4;
      const payload = Buffer.from(buf.subarray(off, off + len));
      if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
      const opcode = buf[0] & 15;
      buf = buf.subarray(off + len);
      if (opcode === 8) { socket.end(); return; }
      if (opcode !== 1) continue;
      try {
        const m = JSON.parse(payload.toString()); if (process.env.WSTRACE) console.log(Date.now() % 100000, call, m.t);
        const reply = store.mesh.onFrame(call, m, Date.now());
        if (reply) peer.send(reply);
      } catch { /* ignore a bad frame */ }
    }
  });
  socket.on("close", () => store.mesh.disconnect(call, peer));
  socket.on("error", () => store.mesh.disconnect(call, peer));
});
server.listen(port, "127.0.0.1", () => console.log(`local server on http://127.0.0.1:${port}`));
