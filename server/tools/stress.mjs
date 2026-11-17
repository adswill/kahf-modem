// Runs the scenario matrix (tools/stress-lib.mjs) and prints one row per scenario.
//   node tools/stress.mjs                       in-process: real routes, fake store and clock
//   node tools/stress.mjs --url=http://127.0.0.1:8787   a running server (wrangler dev or deployed)
// Options: --only=<text> (scenario name or group contains text), --fails-only, --width=<n>
// Against a URL, scenarios that need the clock, the store or Content-Length tricks print SKIP.
// Remote runs write real records (random callsigns); point them at a dev server, not production.
// Exit code 1 when any scenario fails.
import { scenarios, makeInProcess, makeRemote, runScenario } from "./stress-lib.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => {
  const m = /^--([^=]+)(?:=(.*))?$/.exec(a);
  return m ? [m[1], m[2] ?? true] : [a, true];
}));
const url = typeof args.url === "string" ? args.url : null;
const only = typeof args.only === "string" ? args.only.toLowerCase() : null;
const width = Number(args.width) || 52;
const clip = (s, n) => { s = String(s).replace(/\s+/g, " "); return s.length > n ? s.slice(0, n - 3) + "..." : s; };
const pad = (s, n) => s + " ".repeat(Math.max(0, n - s.length));

const make = url ? () => makeRemote(url) : (sc) => makeInProcess({ slow: !!sc.slow });
const list = scenarios.filter((s) => !only || `${s.group} ${s.name}`.toLowerCase().includes(only));

console.log(`target: ${url ?? "in-process (fake store, fake clock)"}   scenarios: ${list.length}\n`);
console.log(`${pad("SCENARIO", width + 4)} | ${pad("EXPECTED", 38)} | ${pad("ACTUAL", width)} | RESULT`);
console.log("-".repeat(width + 4 + 38 + width + 18));

const counts = { PASS: 0, FAIL: 0, SKIP: 0 };
const failures = [];
let lastGroup = "";
for (const sc of list) {
  const out = await runScenario(sc, make);
  counts[out.result]++;
  if (out.result === "FAIL") failures.push({ sc, out });
  if (args["fails-only"] && out.result !== "FAIL") continue;
  if (sc.group !== lastGroup) { console.log(`\n[${sc.group}]`); lastGroup = sc.group; }
  const first = out.result === "FAIL" ? out.actual.split("\n")[0] : out.actual;
  console.log(`${pad(clip(sc.name, width + 4), width + 4)} | ${pad(clip(sc.expected, 38), 38)} | ${pad(clip(first, width), width)} | ${out.result}`);
}

console.log(`\nPASS ${counts.PASS}   FAIL ${counts.FAIL}   SKIP ${counts.SKIP}   total ${list.length}`);
if (failures.length) {
  console.log("\nFAILURES (exact input and response):");
  for (const { sc, out } of failures) {
    console.log(`\n- ${sc.group} | ${sc.name}\n  expected: ${sc.expected}\n  actual:   ${out.actual.split("\n")[0]}\n  ${out.detail ?? ""}`);
  }
}
process.exit(counts.FAIL ? 1 : 0);
