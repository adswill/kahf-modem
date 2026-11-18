// Styles for the kahf.dev home page only (the map page keeps the shared sheet in styles.js).
// The home page is a printed chart: a plotting sheet with a neatline, a graticule, range
// rings, great-circle passages, and a tide-table style ruler of the twelve lanes. Pale
// chart blues, navy ink, one chart-magenta for what is active. System fonts only, no
// images, no external files. Flat colour: no gradients, shadows or filters anywhere.
export const HOME_CSS = String.raw`
body[data-page="home"] {
  --paper: #f5f3ea; --sea: #dbe9ec; --sea-2: #c3dbe1; --ink: #0f2438; --ink-2: #455d70;
  --rule: #98b1bd; --mag: #a8186f; --mag-ink: #8c125c; --night: #11334f; --land: #e8dfbf;
  --bg: var(--paper); --fg: var(--ink); --muted: var(--ink-2); --line: var(--rule); --focus: var(--mag);
  --serif: "Iowan Old Style", "Palatino Linotype", Palatino, "Book Antiqua", Georgia, serif;
  background: var(--paper); color: var(--ink); accent-color: var(--mag);
  font: 17px/1.6 var(--sans); scrollbar-color: var(--rule) var(--paper);
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) body[data-page="home"] {
    --paper: #0c1b29; --sea: #12293b; --sea-2: #1a3a52; --ink: #dbe8ee; --ink-2: #93aab8;
    --rule: #34526a; --mag: #ff86c8; --mag-ink: #ff9ed3; --night: #04101b; --land: #1b3042;
  }
}
:root[data-theme="dark"] body[data-page="home"] {
  --paper: #0c1b29; --sea: #12293b; --sea-2: #1a3a52; --ink: #dbe8ee; --ink-2: #93aab8;
  --rule: #34526a; --mag: #ff86c8; --mag-ink: #ff9ed3; --night: #04101b; --land: #1b3042;
}
body[data-page="home"] ::selection { background: var(--mag); color: var(--paper); }
body[data-page="home"] a { color: var(--ink); text-decoration-color: var(--mag); text-decoration-thickness: 1.5px; text-underline-offset: 4px; }
body[data-page="home"] a:hover { color: var(--mag-ink); }
body[data-page="home"] :focus-visible { outline: 2px solid var(--mag); outline-offset: 3px; }
body[data-page="home"] .skip { background: var(--ink); color: var(--paper); }

/* masthead: the title block of the sheet */
.mast { border-bottom: 2px solid var(--ink); }
.mast .in { max-width: 1240px; margin: 0 auto; padding: 14px 20px 12px; display: flex; flex-wrap: wrap; align-items: end; gap: 10px 36px; }
.mast .brand { font: 700 30px/1 var(--serif); letter-spacing: 0.06em; text-decoration: none; color: var(--ink); }
.mast .sub { font: italic 14px/1.3 var(--serif); color: var(--ink-2); margin: 0 0 3px; flex: 1 1 220px; }
.mast nav { display: flex; flex-wrap: wrap; gap: 2px 20px; font: 13px/1.6 var(--sans); letter-spacing: 0.04em; }
.mast nav a { text-decoration: none; color: var(--ink-2); padding: 4px 0; border-bottom: 2px solid transparent; }
.mast nav a:hover { color: var(--ink); border-bottom-color: var(--mag); }
.mast .tools { margin-left: auto; }
.mast .btn { border: 1px solid var(--rule); color: var(--ink); font: 12px/1 var(--sans); letter-spacing: 0.06em; padding: 8px 11px; background: transparent; cursor: pointer; }
.mast .btn:hover { border-color: var(--mag); color: var(--mag-ink); }

/* sheet frame */
.sheet { max-width: 1240px; margin: 0 auto; padding: 0 20px; }
.plate { display: grid; grid-template-columns: minmax(0, 1fr); gap: 28px; padding: 44px 0 52px; }
@media (min-width: 980px) { .plate { grid-template-columns: minmax(0, 5fr) minmax(0, 7fr); gap: 44px; align-items: center; padding: 56px 0 64px; } }
.plate h1 { font: 400 clamp(38px, 5.4vw, 66px)/1.02 var(--serif); letter-spacing: -0.02em; margin: 0 0 22px; text-wrap: balance; max-width: 12em; }
.plate h1 em { font-style: italic; color: var(--mag-ink); }
.plate .lede { font-size: 19px; line-height: 1.55; max-width: 30em; margin: 0 0 20px; }
.plate .state { display: flex; gap: 10px; align-items: baseline; font: 14px/1.5 var(--sans); color: var(--ink-2); margin: 0 0 26px; max-width: 32em; }
.plate .state b { color: var(--ink); font-weight: 700; white-space: nowrap; }
.go { display: flex; flex-wrap: wrap; gap: 10px 14px; }
.go a { display: inline-block; font: 600 14px/1 var(--sans); letter-spacing: 0.03em; padding: 13px 18px; text-decoration: none; border: 1.5px solid var(--ink); }
.go a.main { background: var(--ink); color: var(--paper); }
.go a.main:hover { background: var(--mag); border-color: var(--mag); color: var(--paper); }
.go a.alt { color: var(--ink); }
.go a.alt:hover { border-color: var(--mag); color: var(--mag-ink); }

/* the chart */
.chart { margin: 0; }
.chart svg { display: block; width: 100%; height: auto; background: var(--sea); border: 1px solid var(--ink); }
.chart text { font-family: var(--sans); fill: var(--ink); }
.chart .it { font-family: var(--serif); font-style: italic; }
.chart .dim { fill: var(--ink-2); }
.chart .grat { stroke: var(--rule); stroke-width: 0.8; fill: none; }
.chart .neat-a { fill: var(--ink); }
.chart .neat-b { fill: var(--paper); }
.chart .night { fill: var(--night); opacity: 0.55; }
.chart .band { fill: var(--sea-2); stroke: none; }
.chart .skip-ring { fill: none; stroke: var(--ink-2); stroke-width: 1; stroke-dasharray: 5 4; }
.chart .hop-ring { fill: none; stroke: var(--ink-2); stroke-width: 1; }
.chart .arc { fill: none; stroke: var(--ink); stroke-width: 1.6; }
.chart .arc.far { stroke-dasharray: 2 5; stroke: var(--ink-2); stroke-width: 1.3; }
.chart .node { fill: var(--paper); stroke: var(--ink); stroke-width: 1.8; }
.chart .relay { fill: var(--mag); stroke: var(--paper); stroke-width: 2; }
.chart .pkt { fill: var(--mag); stroke: var(--paper); stroke-width: 1.5; }
.chart .rose line, .chart .rose path { stroke: var(--ink); fill: none; stroke-width: 1; }
.chart .rose .fill { fill: var(--ink); }
.chart .cart { fill: var(--paper); stroke: var(--ink); stroke-width: 1.2; }
.chart-cap { font: 13px/1.5 var(--sans); color: var(--ink-2); margin: 12px 0 0; display: flex; flex-wrap: wrap; gap: 4px 22px; }
.chart-cap b { color: var(--ink); font-weight: 600; }
.chart-cap .num { font-family: var(--mono); font-variant-numeric: tabular-nums; }

/* a chart draws itself once: pathLength is 1, so every line shares one dash scale */
.chart .draw { stroke-dasharray: 1; stroke-dashoffset: 1; animation: draw 1.5s cubic-bezier(0.2, 0.7, 0.2, 1) forwards; }
.chart .draw.d1 { animation-delay: 0.2s; }
.chart .draw.d2 { animation-delay: 0.7s; }
.chart .draw.d3 { animation-delay: 1.1s; }
.chart .draw.d4 { animation-delay: 1.5s; }
.chart .pop { opacity: 0; animation: pop 0.6s ease-out forwards; animation-delay: 1.2s; }
@keyframes draw { to { stroke-dashoffset: 0; } }
@keyframes pop { to { opacity: 1; } }

/* sections */
.part { border-top: 2px solid var(--ink); padding: 56px 0 60px; }
.part h2 { font: 400 clamp(30px, 4vw, 46px)/1.08 var(--serif); letter-spacing: -0.015em; margin: 0 0 18px; max-width: 18em; text-wrap: balance; }
.part h3 { font: 700 15px/1.3 var(--sans); letter-spacing: 0.02em; color: var(--ink); margin: 26px 0 6px; text-transform: none; }
.part p { max-width: 38em; }
.part .cols { display: grid; grid-template-columns: minmax(0, 1fr); gap: 28px 56px; }
@media (min-width: 900px) { .part .cols { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); } .part .cols.lean { grid-template-columns: minmax(0, 5fr) minmax(0, 7fr); } }
.part .num, .part code { font-family: var(--mono); font-variant-numeric: tabular-nums; font-size: 0.94em; }

/* the lane ruler */
.ruler { margin: 30px 0 8px; border: 1px solid var(--ink); background: var(--sea); }
.ruler svg { display: block; width: 100%; height: auto; }
.ruler text { font-family: var(--sans); fill: var(--ink); }
.ruler .dim { fill: var(--ink-2); }
.ruler .tick { stroke: var(--ink); stroke-width: 1; }
.ruler .axis { stroke: var(--ink); stroke-width: 1.5; }
.ruler .lane { cursor: pointer; }
.ruler .lane rect.bar { stroke: var(--ink); stroke-width: 1.5; transition: transform 0.18s ease-out; transform-box: fill-box; transform-origin: 50% 100%; }
.ruler .lane.relay rect.bar { fill: var(--ink); }
.ruler .lane.calling rect.bar { fill: var(--mag); stroke: var(--mag-ink); }
.ruler .lane.spare rect.bar { fill: var(--paper); }
.ruler .lane .hit { fill: transparent; }
.ruler .lane:hover rect.bar, .ruler .lane:focus-visible rect.bar, .ruler .lane.sel rect.bar { transform: scaleY(1.12); }
.ruler .lane.sel rect.bar { stroke-width: 3; }
.ruler .lane:focus-visible { outline: none; }
.ruler .lane:focus-visible rect.bar { stroke: var(--mag); stroke-width: 3; }
.ruler .lane text { font: 600 13px var(--sans); text-anchor: middle; pointer-events: none; }
.ruler .lane.relay text.nm { fill: var(--paper); }
.ruler .lane.calling text.nm { fill: var(--paper); }
.ruler .hatch { stroke: var(--ink-2); stroke-width: 1; }
.ruler .rise { transform-box: fill-box; transform-origin: 50% 100%; animation: rise 0.7s cubic-bezier(0.2, 0.7, 0.2, 1) both; }
@keyframes rise { from { transform: scaleY(0.04); opacity: 0; } to { transform: scaleY(1); opacity: 1; } }
.js .ruler.wait .rise { animation: none; opacity: 0; transform: scaleY(0.04); }
.read { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0 28px; margin: 0; font: 14px/1.45 var(--sans); padding: 14px 16px 16px; border: 1px solid var(--ink); border-top: 0; background: var(--paper); }
@media (min-width: 760px) { .read { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
.read dt { font: 12px/1.3 var(--sans); letter-spacing: 0.06em; text-transform: uppercase; color: var(--ink-2); margin-top: 8px; }
.read dd { margin: 2px 0 0; font: 600 18px/1.25 var(--mono); font-variant-numeric: tabular-nums; min-width: 0; overflow-wrap: anywhere; }
.read dd.sm { font: 15px/1.35 var(--sans); font-weight: 400; }
table.lanes { border-collapse: collapse; width: 100%; max-width: 760px; font: 14px/1.4 var(--mono); font-variant-numeric: tabular-nums; margin: 18px 0 0; }
table.lanes caption { text-align: left; font: 13px/1.4 var(--sans); color: var(--ink-2); padding-bottom: 8px; }
table.lanes th, table.lanes td { text-align: left; padding: 6px 12px 6px 0; border-top: 1px solid var(--rule); font-weight: 400; }
table.lanes th { font: 12px var(--sans); letter-spacing: 0.06em; text-transform: uppercase; color: var(--ink-2); }
table.lanes tr.sel td { background: var(--sea); }
table.lanes td:first-child { font-weight: 700; }

/* a relay's round: six slots of two seconds */
.round { margin: 26px 0 0; border: 1px solid var(--ink); background: var(--paper); }
.round .track { position: relative; display: grid; grid-template-columns: repeat(6, 1fr); }
.round .slot { padding: 10px 10px 12px; border-right: 1px solid var(--rule); font: 13px/1.35 var(--sans); min-height: 76px; }
.round .slot:last-child { border-right: 0; }
.round .slot b { display: block; font: 700 14px var(--sans); }
.round .slot span { color: var(--ink-2); }
.round .slot.b { background: var(--sea); }
.round .slot.g { background: var(--sea-2); }
.round .slot.d b { color: var(--mag-ink); }
.round .head { display: flex; justify-content: space-between; gap: 12px; flex-wrap: wrap; padding: 8px 12px; border-bottom: 1px solid var(--ink); font: 12px/1.4 var(--sans); letter-spacing: 0.05em; text-transform: uppercase; color: var(--ink-2); }
.round .play { position: absolute; top: 0; bottom: 0; left: 0; width: 2px; background: var(--mag); animation: sweep 12s linear infinite; }
@keyframes sweep { from { left: 0; } to { left: calc(100% - 2px); } }
@media (max-width: 700px) {
  .round .track { grid-template-columns: repeat(3, 1fr); }
  .round .slot { border-bottom: 1px solid var(--rule); }
  .round .play { display: none; }
}

/* the log of what you can send: a logbook, not a card grid */
.log { width: 100%; border-collapse: collapse; margin: 22px 0 0; font: 15px/1.5 var(--sans); }
.log th, .log td { text-align: left; vertical-align: top; padding: 14px 18px 14px 0; border-top: 1px solid var(--ink); }
.log tr:last-child td, .log tr:last-child th { border-bottom: 1px solid var(--ink); }
.log th { font: 400 22px/1.2 var(--serif); width: 24%; }
.log td.m { font: 14px/1.5 var(--mono); font-variant-numeric: tabular-nums; color: var(--ink-2); white-space: nowrap; width: 20%; }
@media (max-width: 760px) { .log, .log tbody, .log tr, .log th, .log td { display: block; width: auto !important; } .log th { padding-bottom: 2px; border-top: 1px solid var(--ink); } .log td { border-top: 0; padding: 2px 0 4px; } .log td.m { white-space: normal; } }

/* status: three ledgers */
.ledger { margin: 0; padding: 0; list-style: none; font: 15px/1.5 var(--sans); }
.ledger li { display: flex; gap: 14px; padding: 9px 0; border-top: 1px solid var(--rule); }
.ledger li:last-child { border-bottom: 1px solid var(--rule); }
.ledger .k { flex: none; width: 14px; height: 14px; margin-top: 5px; border: 2px solid var(--ink); }
.ledger .k.done { background: var(--ink); }
.ledger .k.part-done { background: none; position: relative; }
.ledger .k.part-done::after { content: ""; position: absolute; inset: 0; background: var(--ink); clip-path: polygon(0 0, 100% 0, 0 100%); }
.ledger .k.gap { border-color: var(--mag); }
.ledger b { font-weight: 600; }
.ledger span { color: var(--ink-2); }
.keyline { display: flex; flex-wrap: wrap; gap: 6px 26px; font: 13px/1.4 var(--sans); color: var(--ink-2); margin: 0 0 18px; }
.keyline i { display: inline-block; width: 12px; height: 12px; border: 2px solid var(--ink); vertical-align: -1px; margin-right: 8px; }
.keyline i.done { background: var(--ink); }
.keyline i.gap { border-color: var(--mag); }

/* licence block keeps its plain text */
.fine { display: grid; grid-template-columns: minmax(0, 1fr); gap: 4px 56px; }
@media (min-width: 900px) { .fine { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); } }
.fine h3 { margin-top: 22px; }

/* footer */
.base { border-top: 2px solid var(--ink); padding: 26px 0 56px; font: 14px/1.6 var(--sans); color: var(--ink-2); }
.base .in { max-width: 1240px; margin: 0 auto; padding: 0 20px; display: flex; flex-wrap: wrap; gap: 8px 32px; align-items: baseline; }
.base p { margin: 0; }
.base .sp { margin-left: auto; }

@media (prefers-reduced-motion: reduce) {
  .chart .draw { animation: none; stroke-dashoffset: 0; }
  .chart .pop { animation: none; opacity: 1; }
  .ruler .rise { animation: none; }
  .js .ruler.wait .rise { opacity: 1; transform: none; }
  .round .play { animation: none; display: none; }
  .ruler .lane rect.bar { transition: none; }
}
`;
