// A local copy of the server on a port, with an in-memory store, for trying the relay
// software end to end: `node tools/serve-local.mjs 8787`. Nothing is saved; no Cloudflare.
import http from "node:http";
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
http
  .createServer(async (req, res) => {
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
  })
  .listen(port, "127.0.0.1", () => console.log(`local server on http://127.0.0.1:${port}`));
