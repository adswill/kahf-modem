// Inline CSS for the kahf.dev pages. One string, no external files. One self-hosted display
// face (/fonts/display.woff2), system faces for text and data.
// A night-shack look: near-black warm ground, bone text, one amber for signal and the
// 17 m band, one green for live state. Flat, hairline rules, square corners. The light
// theme is the same palette on paper.
export const CSS = String.raw`
@font-face { font-family: "Big Shoulders Display"; font-weight: 700; font-style: normal; font-display: swap; src: url(/fonts/display.woff2) format("woff2"); }
:root {
  --bg: #0d0e0b; --fg: #ebe8dc; --muted: #9b978a; --line: #2b2c25; --fill: #151610;
  --amber: #f2a93b; --amber-ink: #f2a93b; --live: #3dbe90; --live-ink: #3dbe90; --alert: #ff6b8b; --alert-ink: #ff6b8b; --focus: #f2a93b;
  --display: "Big Shoulders Display", "Arial Narrow", "Helvetica Neue", Arial, sans-serif;
  --sans: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  color-scheme: dark;
}
@media (prefers-color-scheme: light) {
  :root:not([data-theme="dark"]) {
    --bg: #f4f1e8; --fg: #1b1a14; --muted: #5d5a4e; --line: #d8d3c4; --fill: #ebe7da;
    --amber: #d98a0f; --amber-ink: #8f5400; --live: #1f8a65; --live-ink: #17694d; --alert: #cf2d56; --alert-ink: #b0213f; --focus: #1b1a14;
    color-scheme: light;
  }
}
:root[data-theme="light"] {
  --bg: #f4f1e8; --fg: #1b1a14; --muted: #5d5a4e; --line: #d8d3c4; --fill: #ebe7da;
  --amber: #d98a0f; --amber-ink: #8f5400; --live: #1f8a65; --live-ink: #17694d; --alert: #cf2d56; --alert-ink: #b0213f; --focus: #1b1a14;
  color-scheme: light;
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; scroll-padding-top: 16px; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 17px/1.6 var(--sans); overflow-wrap: break-word; animation: paint 0.25s ease-out both; }
::selection { background: var(--amber); color: #0d0e0b; }
html { caret-color: var(--amber); accent-color: var(--amber); scrollbar-color: var(--muted) var(--bg); }
a { color: var(--fg); text-decoration-thickness: 1px; text-underline-offset: 3px; text-decoration-color: var(--amber); }
a:hover { text-decoration-thickness: 2px; }
:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; }
.mono, code, time, .num { font-family: var(--mono); font-variant-numeric: tabular-nums; }
code { font-size: 0.9em; background: var(--fill); padding: 1px 5px; border: 1px solid var(--line); }
pre { font: 14px/1.6 var(--mono); background: var(--fill); border: 1px solid var(--line); padding: 14px 16px; overflow-x: auto; margin: 0 0 16px; max-width: 46em; }
pre code { background: none; border: 0; padding: 0; font-size: inherit; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
.skip { position: absolute; left: 16px; top: -60px; background: var(--amber); color: #0d0e0b; padding: 8px 12px; z-index: 60; font: 13px var(--mono); }
.skip:focus { top: 8px; }
.wrap { max-width: 1160px; margin: 0 auto; padding: 0 20px; }
.wrap.wide { max-width: 1680px; }

/* header */
.top { border-bottom: 1px solid var(--line); }
.top .wrap { display: flex; align-items: center; gap: 8px 28px; flex-wrap: wrap; padding-top: 12px; padding-bottom: 10px; }
.brand { font: 700 28px/1 var(--display); letter-spacing: 0.06em; text-decoration: none; display: inline-flex; align-items: center; gap: 10px; }
.brand::before { content: ""; width: 10px; height: 10px; background: var(--amber); }
.top nav { display: flex; gap: 2px 22px; flex-wrap: wrap; font: 500 15px/1.6 var(--sans); }
.top nav a { color: var(--muted); text-decoration: none; padding: 6px 0; border-bottom: 1px solid transparent; }
.top nav a:hover { color: var(--fg); }
.top nav a[aria-current="page"] { color: var(--fg); border-bottom-color: var(--amber); }
.tools { margin-left: auto; }
.btn { font: 500 14px/1 var(--sans); color: var(--fg); background: transparent; border: 1px solid var(--line); border-radius: 0; padding: 8px 10px; cursor: pointer; }
.btn:hover { border-color: var(--amber); }
.js-only { display: none; }
.js .js-only { display: inline-block; }

/* type */
h1 { font: 700 clamp(52px, 9.4vw, 96px)/0.94 var(--display); text-transform: uppercase; margin: 0 0 24px; max-width: 12em; text-wrap: balance; }
h1 em, h2 em { font-style: normal; color: var(--amber-ink); }
h2 { font: 700 clamp(34px, 5vw, 58px)/0.98 var(--display); text-transform: uppercase; margin: 0 0 22px; max-width: 16em; text-wrap: balance; }
h3 { font: 600 18px/1.3 var(--sans); margin: 28px 0 8px; }
p { margin: 0 0 16px; max-width: 40em; }
.lede { font-size: 20px; line-height: 1.5; color: var(--fg); max-width: 34em; }
ul.plain, ol.plain { margin: 0 0 16px; padding-left: 1.2em; max-width: 40em; }
ul.plain li, ol.plain li { margin-bottom: 6px; }
.note { color: var(--muted); font-size: 15px; }
.status { font: 13px/1.5 var(--mono); }
.dot { display: inline-block; width: 8px; height: 8px; background: var(--live); margin-right: 8px; animation: beat 4s ease-in-out infinite; }
.na { color: var(--muted); font-style: italic; }
.warn { color: var(--alert-ink); }
.cta { display: flex; flex-wrap: wrap; gap: 12px; margin: 28px 0 0; }
.cta a { font: 600 16px/1 var(--sans); text-decoration: none; padding: 15px 20px; border: 1px solid var(--line); }
.cta a:hover { border-color: var(--amber); }
.cta a.primary { background: var(--amber); border-color: var(--amber); color: #0d0e0b; }
.cta a.primary:hover { background: var(--fg); border-color: var(--fg); }

/* home */
.hero { padding: 56px 0 40px; display: grid; gap: 40px; }
.dial { font: 400 clamp(40px, 8vw, 92px)/1 var(--mono); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.dial small { display: block; font-size: 0.26em; letter-spacing: 0.12em; color: var(--muted); margin: 10px 0 0 4px; }
.dial span { color: var(--amber); }
.band { margin: 0; border: 1px solid var(--line); background: var(--fill); }
.band svg { display: block; width: 100%; height: auto; }
.band figcaption { padding: 10px 14px; margin: 0; border-top: 1px solid var(--line); }
.band text { font-family: var(--mono); fill: var(--muted); font-size: 12px; }
.band .tick { stroke: var(--line); stroke-width: 1; }
.band .tick.big { stroke: var(--muted); }
.band .edge { stroke: var(--amber); stroke-width: 2; }
.band .sig { fill: var(--amber); }
.band .sigtxt { fill: var(--amber-ink); }
.band .wf { fill: var(--amber); }
.sec { border-top: 1px solid var(--line); padding: 64px 0; }
.sec.tight { padding: 40px 0; }
.grid { display: grid; grid-template-columns: 1fr; gap: 32px; }
@media (min-width: 860px) {
  .grid.two { grid-template-columns: 5fr 7fr; gap: 56px; }
  .grid.even { grid-template-columns: 1fr 1fr; gap: 56px; }
  .grid.three { grid-template-columns: repeat(3, 1fr); gap: 40px; }
  .hero { grid-template-columns: 6fr 5fr; align-items: end; gap: 56px; }
}
.ledger { margin: 0 0 24px; border-top: 2px solid var(--fg); }
.ledger > div { display: grid; grid-template-columns: 1fr; gap: 2px 28px; padding: 14px 0; border-bottom: 1px solid var(--line); }
@media (min-width: 760px) { .ledger > div { grid-template-columns: 220px minmax(0, 1fr) minmax(0, 1.4fr); align-items: baseline; } }
.ledger dt { font: 600 15px/1.4 var(--sans); color: var(--muted); }
.ledger dd { margin: 0; }
.ledger dd.v { font: 22px/1.3 var(--mono); font-variant-numeric: tabular-nums; }
.ledger dd.n { color: var(--muted); font-size: 16px; }
.shot { margin: 32px 0 0; }
.shot img { display: block; max-width: 100%; height: auto; border: 1px solid var(--line); }
figcaption { font: 12px/1.5 var(--mono); color: var(--muted); margin-top: 10px; letter-spacing: 0.04em; }
.diagram { margin: 0; }
.diagram svg { display: block; width: 100%; max-width: 560px; height: auto; }
.diagram.wide svg { max-width: 760px; }
.diagram text { font-family: var(--mono); fill: var(--fg); }
.diagram .dim { fill: var(--muted); }
.diagram .box { fill: none; stroke: var(--fg); stroke-width: 1.2; }
.diagram .hot { fill: none; stroke: var(--amber); stroke-width: 2; }
.diagram .hotfill { fill: var(--amber); stroke: none; }
.diagram .hottxt { fill: #0d0e0b; font-weight: 700; }
.diagram .arrow { stroke: var(--fg); stroke-width: 1.2; fill: none; }
.diagram .air { stroke: var(--amber); stroke-width: 1.5; fill: none; stroke-dasharray: 5 5; }
.diagram .cut { stroke: var(--alert); stroke-width: 1.5; fill: none; }
.diagram .rule { stroke: var(--line); stroke-width: 1; }
.diagram .carr { stroke: var(--muted); stroke-width: 0.8; }
.diagram .pilot { stroke: var(--amber); stroke-width: 1.2; }
.kv { display: grid; grid-template-columns: max-content 1fr; gap: 0 20px; margin: 0 0 16px; font: 14px/1.5 var(--mono); }
.kv dt { color: var(--muted); padding: 8px 0; border-top: 1px solid var(--line); }
.kv dd { margin: 0; padding: 8px 0; border-top: 1px solid var(--line); min-width: 0; overflow-wrap: anywhere; }
.steps { list-style: none; counter-reset: s; margin: 0 0 16px; padding: 0; max-width: 46em; }
.steps li { counter-increment: s; border-top: 1px solid var(--line); padding: 14px 0 14px 52px; position: relative; }
.steps li::before { content: counter(s, decimal-leading-zero); position: absolute; left: 0; top: 14px; font: 14px/1.6 var(--mono); color: var(--amber-ink); }
.steps b { display: block; font-weight: 600; }
.pagehead { padding: 56px 0 8px; }
.pagehead h1 { font-size: clamp(48px, 7.5vw, 80px); }
.callout { border: 1px solid var(--amber); background: var(--fill); padding: 16px 18px 8px; margin: 0 0 20px; max-width: 40em; }
.callout p { margin-bottom: 8px; }
.foot { border-top: 1px solid var(--line); padding: 36px 0 44px; color: var(--muted); font-size: 14px; }
.foot .cols { display: grid; gap: 24px; grid-template-columns: 1fr; }
@media (min-width: 760px) { .foot .cols { grid-template-columns: 2fr 1fr 1fr; } }
.foot .label { font: 600 15px/1.4 var(--sans); color: var(--fg); margin: 0 0 8px; }
.foot ul { list-style: none; margin: 0; padding: 0; font: 13px/2 var(--mono); }
.foot a { color: var(--muted); }
.foot a:hover { color: var(--fg); }
.foot p { margin: 0 0 6px; max-width: 34em; }

/* map page */
.maphead { padding: 24px 0 12px; display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px 28px; }
.maphead h1 { font-size: clamp(30px, 4vw, 40px); margin: 0; }
.maphead .status { margin: 0; }
.stage > * { min-width: 0; }
.stage { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0; border: 1px solid var(--line); margin-bottom: 24px; }
@media (min-width: 900px) { .stage { grid-template-columns: minmax(0, 1fr) 360px; } }
.mapbox { border-bottom: 1px solid var(--line); min-width: 0; overflow-x: auto; background: var(--bg); }
@media (min-width: 900px) { .mapbox { border-bottom: 0; border-right: 1px solid var(--line); } }
.mapbox svg { display: block; width: 100%; min-width: 600px; height: auto; }
.mapbox svg:focus-visible { outline-offset: -3px; }
.graticule { stroke: var(--line); stroke-width: 1; fill: none; }
.glabel { font: 12px var(--mono); fill: var(--muted); }
.lbl { font: 13px var(--mono); fill: var(--fg); }
.link { stroke: var(--muted); stroke-width: 1; fill: none; }
.mk { stroke: var(--fg); stroke-width: 1.5; }
.mk.relay { fill: var(--fg); }
.mk.station { fill: var(--bg); }
.area { fill: none; stroke: var(--fg); stroke-width: 1.5; stroke-dasharray: 6 5; }
.area.station { stroke-width: 1; }
.hit { fill: transparent; stroke: none; }
.marker { cursor: pointer; }
.marker.sel .mk { stroke: var(--live); stroke-width: 3; }
.marker.sel .area { stroke: var(--live); stroke-width: 2.5; }
.marker:focus-visible .mk, .marker:focus-visible .area { stroke: var(--focus); stroke-width: 3; }
.marker:focus-visible { outline: none; }
.legend { display: flex; flex-wrap: wrap; gap: 6px 22px; padding: 10px 14px; border-top: 1px solid var(--line); font: 12px/1.4 var(--mono); color: var(--muted); }
.side { display: flex; flex-direction: column; min-width: 0; }
@media (min-width: 900px) { .side { position: sticky; top: 0; align-self: start; max-height: 100vh; } }
.sidesec { padding: 14px 16px; border-bottom: 1px solid var(--line); }
.sidesec:last-child { border-bottom: 0; }
.panel { display: none; }
.panel.open, .panel.empty, .panel:target { display: block; }
.panels.sel .panel.empty, .panels:has(:target) .panel.empty { display: none; }
.panel h2 { font: 700 20px/1.2 var(--mono); margin: 0 0 2px; letter-spacing: 0.02em; }
.panel .role { font: 12px var(--mono); letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted); margin: 0 0 10px; }
.panel .kv { font-size: 13px; grid-template-columns: 1fr; gap: 0; }
.panel .kv dt { border-top: 0; padding: 8px 0 0; font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
.panel .kv dd { border-top: 0; padding: 0 0 4px; }
.panel .note { font-size: 13px; line-height: 1.5; margin-bottom: 8px; }
.panel p { max-width: none; }
.filters { display: flex; gap: 10px; flex-wrap: wrap; }
.filters label { font: 12px var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted); display: flex; flex-direction: column; gap: 4px; flex: 1 1 120px; }
.filters select { font: 14px var(--mono); color: var(--fg); background: var(--bg); border: 1px solid var(--line); border-radius: 0; padding: 7px 8px; }
.count { font: 12px var(--mono); color: var(--muted); margin: 10px 0 0; }
.list { list-style: none; margin: 0; padding: 0; overflow-y: auto; max-height: 260px; }
@media (min-width: 900px) { .list { max-height: none; flex: 1; } }
.list li { border-top: 1px solid var(--line); padding: 0 16px; }
.list li[hidden] { display: none; }
.list a { display: grid; grid-template-columns: 1fr auto; gap: 2px 10px; padding: 9px 0; text-decoration: none; font: 14px/1.4 var(--mono); }
.list a .meta:nth-child(3) { text-align: left; }
.list a:hover .cs { text-decoration: underline; }
.list .cs { font-weight: 700; }
.list .meta { color: var(--muted); font-size: 12px; text-align: right; }
.list li.sel a { border-left: 3px solid var(--live); padding-left: 8px; margin-left: -11px; }
.empty-state { padding: 40px 24px; border: 1px dashed var(--line); margin-bottom: 24px; }
.empty-state p { margin-bottom: 10px; }

@keyframes paint { from { opacity: 0; } to { opacity: 1; } }
@keyframes beat { 0%, 80%, 100% { opacity: 1; } 90% { opacity: 0.25; } }
@media (prefers-reduced-motion: reduce) {
  body { animation: none; }
  .dot { animation: none; }
  * { scroll-behavior: auto; transition: none; }
}

/* live map */
.mk { position: relative; width: 14px; height: 14px; border: 2px solid var(--bg); padding: 0; margin: 0; cursor: pointer; border-radius: 0; }
.mk.relay { background: #1f8a65; width: 16px; height: 16px; }
.mk.station { background: var(--fg); }
.mk.heard { background: var(--muted); }
.mk:focus-visible { outline: 2px solid var(--fg); outline-offset: 2px; }
.livestage { display: grid; gap: 16px; grid-template-columns: minmax(0, 1fr) 300px; align-items: start; }
.livemap { height: min(72vh, 680px); min-height: 420px; border: 1px solid var(--line); background: var(--fill); }
.liveside { display: grid; gap: 12px; align-content: start; }
.kv { display: grid; grid-template-columns: max-content 1fr; gap: 6px 14px; margin: 0; font-family: var(--mono); font-size: 13px; }
.kv dt { color: var(--muted); }
.kv dd { margin: 0; word-break: break-all; }
.maplibregl-ctrl-attrib { font-size: 11px; }
@media (max-width: 860px) {
  .livestage { grid-template-columns: 1fr; }
  .livemap { min-height: 380px; height: 58vh; }
}
`;
