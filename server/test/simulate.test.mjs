// Runs the fake-station simulator at small size (5 stations, 2 relays) and asserts every check passes.
// Run with: npm test (node --test). In-process, no network.
import { test } from "node:test";
import assert from "node:assert/strict";
import { runSimulation } from "../tools/simulate.mjs";

test("simulator: 5 stations and 2 relays pass every check", async () => {
  const result = await runSimulation({ stations: 5, relays: 2 });
  const failures = result.checks.filter((c) => c.failed > 0).map((c) => `${c.name}: ${c.first}`);
  assert.deepEqual(failures, []);
});
