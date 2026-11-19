import test from 'node:test';
import assert from 'node:assert/strict';
import { plan, localPlan, CELLS, LANES, SLOTS } from '../src/planner.js';

// The same input as twin_input() in crates/kahf-core/src/planner.rs.
const input = Array.from({ length: 20 }, (_, i) => ({ id: 10 + i, queueBytes: ((i * 37) % 11) * 250, urgency: i % 3 }));
const line = (p) => p.grants.map((g) => `${g.relay}:${g.lane}:${g.slot}`).join(',');

// Expected strings come from the Rust planner (cargo test -p kahf-core known_plans -- --nocapture).
test('plans match the Rust planner exactly', () => {
  assert.equal(line(plan(77, 42, input)), '12:0:0,14:1:0,17:2:0,18:3:0,24:4:0,25:5:0,26:6:0,29:7:0,12:0:1,14:1:1,15:3:1,17:2:1,23:6:1,24:4:1,25:5:1,29:7:1,12:0:2,15:3:2,19:1:2,20:2:2,22:4:2,23:6:2,27:5:2,29:7:2,15:3:3,16:0:3,18:1:3,20:2:3,22:4:3,23:6:3,26:7:3,27:5:3,11:0:4,13:4:4,15:3:4,18:1:4,20:2:4,23:6:4,26:7:4,28:5:4,11:0:5,12:2:5,14:3:5,17:4:5,18:1:5,26:7:5,28:5:5,29:6:5');
  assert.equal(line(plan(78, 42, input)), '11:0:0,15:1:0,17:2:0,18:3:0,20:4:0,23:5:0,25:6:0,26:7:0,15:1:1,17:2:1,18:3:1,20:4:1,23:5:1,24:0:1,26:7:1,27:6:1,12:3:2,15:1:2,17:2:2,20:4:2,23:5:2,24:0:2,27:6:2,29:7:2,12:3:3,14:0:3,15:1:3,16:2:3,22:4:3,23:5:3,28:6:3,29:7:3,12:3:4,14:0:4,18:1:4,19:2:4,22:4:4,26:5:4,28:6:4,29:7:4,11:2:5,12:3:5,13:4:5,14:0:5,18:1:5,25:6:5,26:5:5,29:7:5');
  assert.equal(line(localPlan(80, 42, input.map((r) => r.id))), '12:2:0,12:2:1,16:2:2,20:3:2,24:4:2,16:2:3,20:3:3,24:4:3,20:3:4,28:5:4,28:5:5');
});

test('no clash, no idle relay holds a cell, input order does not matter', () => {
  for (let e = 0; e < 200; e++) {
    const rs = input.map((r, i) => ({ ...r, queueBytes: (e + i) % 3 === 0 ? 0 : 100 * (1 + ((e * i) % 17)) }));
    const p = plan(e, 9, rs);
    const cells = new Set();
    const perSlot = new Set();
    for (const g of p.grants) {
      assert.ok(g.lane < LANES && g.slot < SLOTS);
      assert.ok(!cells.has(`${g.lane}/${g.slot}`));
      assert.ok(!perSlot.has(`${g.relay}/${g.slot}`));
      cells.add(`${g.lane}/${g.slot}`);
      perSlot.add(`${g.relay}/${g.slot}`);
      assert.ok(rs.find((r) => r.id === g.relay).queueBytes > 0);
    }
    assert.deepEqual(p, plan(e, 9, [...rs].reverse()));
    assert.ok(p.grants.length <= CELLS);
  }
});

// ---- bonding and hopping: the same strings as known_bonds_and_hops_for_the_js_twin in planner.rs ----
import { bond, bondCount, hopGrants } from "../src/planner.js";

test("bond, bond_count and hop grants give exactly what the Rust planner gives", () => {
  const rs = Array.from({ length: 20 }, (_, i) => ({ id: 10 + i, queueBytes: ((i * 37) % 11) * 250, urgency: i % 3 }));
  const p = plan(77, 42, rs);
  const wants = rs.map((r) => [r.id, 1 + (r.id % 4)]);
  const b = bond(p, wants).map((x) => `${x.relay}:${x.slot}:${x.first}:${x.count}`).join(",");
  assert.equal(b, "11:4:0:1,11:5:0:1,12:0:0:1,12:1:0:1,12:2:0:1,12:5:2:1,13:4:7:2,14:0:1:1,14:1:1:1,14:5:3:2,15:1:3:2,15:2:3:2,15:3:3:2,15:4:3:2,16:3:0:1,17:0:2:1,17:1:2:1,17:5:7:2,18:0:3:2,18:3:1:1,18:4:1:1,18:5:1:1,19:2:1:1,20:2:2:1,20:3:2:1,20:4:2:1,22:2:7:2,22:3:7:2,23:1:10:1,23:2:10:1,23:3:10:1,23:4:10:1,24:0:8:1,24:1:8:1,25:0:9:1,25:1:9:1,26:0:10:1,26:3:11:1,26:4:11:1,26:5:11:1,27:2:9:1,27:3:9:1,28:4:9:1,28:5:9:1,29:0:11:1,29:1:11:1,29:2:11:1,29:5:10:1");
  const h = hopGrants(p, [[10, 11], [12, 13], [14, 15], [10, 15], [16, 17], [18, 19]]).map((x) => `${x.relay}>${x.target}:${x.lane}:${x.slot}`).join(",");
  assert.equal(h, "12>13:0:0,14>15:1:0,16>17:0:3,18>19:3:0");
  assert.equal([[25, 50], [5, 5000], [10, 500], [16, 500], [30, 500], [9, 108], [15, 108], [21, 108]].map(([s, n]) => bondCount(s, n)).join(","), "1,1,2,3,4,2,3,4");
});
