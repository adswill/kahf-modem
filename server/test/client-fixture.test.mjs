// The Rust relay client (crates/kahf-net) and this server must agree on the
// signed request format. test/fixtures/rust-client.json holds bodies the Rust
// client made; its own tests check it still makes the same bytes. Here the
// real routes accept them. Run with: npm test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { register } from "node:module";
import { fakeStore } from "../tools/fake-store.mjs";

const HOOK = `
import { readFile } from "node:fs/promises";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) {
    const source = await readFile(new URL(url), "utf8");
    return { format: "module", source, shortCircuit: true };
  }
  return nextLoad(url, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOK));
const { handleRequest } = await import("../src/routes.js");

const fixture = JSON.parse(await readFile(new URL("./fixtures/rust-client.json", import.meta.url), "utf8"));
const NOW = Date.parse("2026-11-20T12:00:00Z");
const post = (path, body) =>
  new Request("https://kahf.test" + path, { method: "POST", headers: { "content-type": "application/json" }, body });

test("server accepts the Rust client's register and schedule bodies", async () => {
  const store = fakeStore();
  const reg = await handleRequest(post("/register", fixture.register), store, NOW);
  assert.equal(reg.status, 200, await reg.clone().text());
  assert.deepEqual(await reg.json(), { ok: true, callsign: "LY1ABC", area: "N54E25", expires_after_s: 172800 });

  const sch = await handleRequest(post("/schedule", fixture.schedule), store, NOW);
  assert.equal(sch.status, 200, await sch.clone().text());
  const body = await sch.json();
  assert.equal(body.relay, "LY1ABC");
  assert.deepEqual(body.slots, [{ channel: 1, start: NOW / 1000, duration: 60, direction: "tx" }]);
  assert.deepEqual(body.unplaced, []);
});

test("server refuses the Rust client's body once a field is changed", async () => {
  const tampered = fixture.register.replace("N54E25", "N54E26");
  const res = await handleRequest(post("/register", tampered), fakeStore(), NOW);
  assert.equal(res.status, 401);
});
