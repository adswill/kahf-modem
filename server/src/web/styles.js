// Inline CSS for the kahf.dev pages. One string, no external files, system fonts only.
// Warm paper, ink, hairline rules. Square corners. One green for live state and one
// red for alerts. Flat, nothing decorative.
export const CSS = String.raw`
:root {
  --bg: #f4f2ec; --fg: #1d1b15; --muted: #5c5a52; --line: #d9d5cb; --fill: #eceae2;
  --live: #1f8a65; --live-ink: #17694d; --alert: #cf2d56; --alert-ink: #b0213f; --focus: #1d1b15;
  --serif: "Iowan Old Style", "Palatino Linotype", Palatino, Georgia, serif;
  --sans: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #14130f; --fg: #ece9e0; --muted: #a39f93; --line: #34322a; --fill: #1c1b16;
    --live: #3dbe90; --live-ink: #3dbe90; --alert: #ff6b8b; --alert-ink: #ff6b8b; --focus: #ece9e0;
    color-scheme: dark;
  }
}
:root[data-theme="dark"] {
  --bg: #14130f; --fg: #ece9e0; --muted: #a39f93; --line: #34322a; --fill: #1c1b16;
  --live: #3dbe90; --live-ink: #3dbe90; --alert: #ff6b8b; --alert-ink: #ff6b8b; --focus: #ece9e0;
  color-scheme: dark;
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 17px/1.6 var(--sans); overflow-wrap: break-word; animation: paint 0.25s ease-out both; }
a { color: var(--fg); text-decoration-thickness: 1px; text-underline-offset: 3px; }
a:hover { text-decoration-thickness: 2px; }
:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; }
.mono, code, time, .num { font-family: var(--mono); font-variant-numeric: tabular-nums; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
.skip { position: absolute; left: 16px; top: -60px; background: var(--fg); color: var(--bg); padding: 8px 12px; z-index: 60; font: 13px var(--mono); }
.skip:focus { top: 8px; }
.wrap { max-width: 1120px; margin: 0 auto; padding: 0 16px; }
.wrap.wide { max-width: 1680px; }

/* header */
.top { border-bottom: 1px solid var(--line); }
.top .wrap { display: flex; align-items: baseline; gap: 8px 28px; flex-wrap: wrap; padding-top: 14px; padding-bottom: 12px; }
.brand { font: 700 22px/1 var(--serif); letter-spacing: 0.02em; text-decoration: none; }
.top nav { display: flex; gap: 4px 22px; flex-wrap: wrap; font: 12px/1.6 var(--mono); letter-spacing: 0.08em; text-transform: uppercase; }
.top nav a { color: var(--muted); text-decoration: none; padding: 6px 0; border-bottom: 1px solid transparent; }
.top nav a:hover { color: var(--fg); }
.top nav a[aria-current="page"] { color: var(--fg); border-bottom-color: var(--fg); }
.tools { margin-left: auto; }
.btn { font: 12px/1 var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--fg); background: transparent; border: 1px solid var(--line); border-radius: 0; padding: 8px 10px; cursor: pointer; }
.btn:hover { border-color: var(--fg); }
.js-only { display: none; }
.js .js-only { display: inline-block; }

/* type */
.label { font: 12px/1.4 var(--mono); letter-spacing: 0.12em; text-transform: uppercase; color: var(--muted); margin: 0 0 14px; }
h1 { font: 400 clamp(36px, 6.4vw, 60px)/1.04 var(--serif); letter-spacing: -0.015em; margin: 0 0 24px; max-width: 18em; }
h2 { font: 400 clamp(28px, 4vw, 38px)/1.1 var(--serif); letter-spacing: -0.01em; margin: 0 0 20px; }
h3 { font: 12px/1.4 var(--mono); letter-spacing: 0.12em; text-transform: uppercase; color: var(--muted); margin: 28px 0 8px; font-weight: 400; }
p { margin: 0 0 16px; max-width: 40em; }
ul.plain { margin: 0 0 16px; padding-left: 1.1em; max-width: 40em; }
ul.plain li { margin-bottom: 6px; }
.note { color: var(--muted); font-size: 15px; }
.status { font: 13px/1.5 var(--mono); }
.dot { display: inline-block; width: 8px; height: 8px; background: var(--live); margin-right: 8px; animation: beat 4s ease-in-out infinite; }
.na { color: var(--muted); font-style: italic; }
.warn { color: var(--alert-ink); }

/* home */
.hero { padding: 72px 0 56px; }
.hero .status { color: var(--muted); }
.sec { border-top: 1px solid var(--line); padding: 56px 0; }
.grid { display: grid; grid-template-columns: 1fr; gap: 32px; }
@media (min-width: 860px) {
  .grid.two { grid-template-columns: 5fr 7fr; gap: 56px; }
  .grid.even { grid-template-columns: 1fr 1fr; gap: 56px; }
}
.shot { margin: 32px 0 0; }
.shot img { display: block; max-width: 100%; height: auto; border: 1px solid var(--line); }
figcaption { font: 12px/1.5 var(--mono); color: var(--muted); margin-top: 10px; letter-spacing: 0.04em; }
.diagram { margin: 0; }
.diagram svg { display: block; width: 100%; max-width: 560px; height: auto; }
.diagram text { font-family: var(--mono); fill: var(--fg); }
.diagram .dim { fill: var(--muted); }
.diagram .box { fill: none; stroke: var(--fg); stroke-width: 1.2; }
.diagram .arrow { stroke: var(--fg); stroke-width: 1.2; fill: none; }
.diagram .rule { stroke: var(--line); stroke-width: 1; }
.diagram .carr { stroke: var(--muted); stroke-width: 0.8; }
.diagram .pilot { stroke: var(--fg); stroke-width: 1.2; }
.kv { display: grid; grid-template-columns: max-content 1fr; gap: 0 20px; margin: 0 0 16px; font: 14px/1.5 var(--mono); }
.kv dt { color: var(--muted); padding: 8px 0; border-top: 1px solid var(--line); }
.kv dd { margin: 0; padding: 8px 0; border-top: 1px solid var(--line); min-width: 0; overflow-wrap: anywhere; }
.foot { border-top: 1px solid var(--line); padding: 24px 0 40px; color: var(--muted); font-size: 14px; }
.foot p { margin: 0 0 6px; max-width: none; }

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
.mk.sel { outline: 3px solid var(--live); outline-offset: 3px; }
.mapempty { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; padding: 24px; pointer-events: none; z-index: 2; }
.mapempty[hidden] { display: none; }
.mapempty p { background: var(--bg); border: 1px solid var(--line); padding: 14px 18px; max-width: 400px; text-align: center; }
.nodepop .maplibregl-popup-content { background: var(--bg); color: var(--fg); border: 1px solid var(--line); border-radius: 0; padding: 10px 12px; }
.nodepop .maplibregl-popup-tip { border-top-color: var(--line); border-bottom-color: var(--line); }
.nodecard + .nodecard { border-top: 1px solid var(--line); margin-top: 8px; padding-top: 8px; }
.nodecard h3 { margin: 0 0 4px; font-size: 14px; display: flex; gap: 8px; align-items: baseline; }
.nodecard h3 span { font-family: var(--mono); font-weight: 700; }
.nodecard h3 em { font-style: normal; color: var(--muted); font-size: 12px; }
.nodecard p { margin: 2px 0; font-size: 13px; }
.nodecard .meta { color: var(--muted); font-size: 12px; }
@media (max-width: 860px) {
  .livestage { grid-template-columns: 1fr; }
  .livemap { min-height: 380px; height: 58vh; }
}
`;
