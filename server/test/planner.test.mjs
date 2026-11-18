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
