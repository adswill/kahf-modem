// Scenario matrix: every scenario from tools/stress-lib.mjs run in-process against the
// real routes with a fake store and a fixed clock. One node:test case per scenario.
// Failing cases print the last request and response. No network, no wrangler.
import { test } from "node:test";
import { scenarios, makeInProcess, runScenario } from "../tools/stress-lib.mjs";

for (const sc of scenarios) {
  test(`${sc.group} | ${sc.name}`, async (t) => {
    const out = await runScenario(sc, (s) => makeInProcess({ slow: !!s.slow }));
    if (out.result === "SKIP") return t.skip(out.actual);
    if (out.result === "FAIL") {
      throw new Error(`expected: ${sc.expected}\nactual: ${out.actual}\n${out.detail ?? ""}`);
    }
  });
}
