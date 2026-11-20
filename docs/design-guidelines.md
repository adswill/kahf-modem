# Design guidelines: keeping KAHF UI from looking AI-made

Summary of the [Impeccable](https://github.com/pbakaus/impeccable) design notes (Apache 2.0), as applied to KAHF. Use it for the website (`server/src/web/`) and the app (`crates/kahf-ui`). The Impeccable skill itself is local-only for the owner, so these are the parts that matter here.

## The idea

Ask an agent for a page with no steer and the same defaults appear: Inter everywhere, purple-to-blue gradients, cream page with a thin serif and a terracotta accent, cards inside cards, an icon tile above every heading, a small label above every title. Pick a visual world from the audience's own culture and commit to it, instead of the category default.

KAHF's world is amateur radio: band charts, station logbooks, rig front panels and stencilled panel lettering. The site uses a condensed panel-style display face, ruled ledger tables for facts, a dial readout in mono, and one amber for signal.

## Refuse these

- A small label or eyebrow above a heading. The heading carries its own weight.
- Section numbers (01 / 02 / 03) unless the order carries information, such as real steps.
- Rows of same-size cards (icon, heading, text) as page structure. Nested cards are always wrong. Use a ledger, a table or plain columns.
- The hero-metric template: a big number, a small label, supporting stats.
- Gradient text. Emphasis comes from weight or size.
- Glass or blur as decoration, hard offset shadows, zero-offset coloured glows.
- A coloured bar down one side of a box. Use a full border, a tint or a leading icon.
- Monospace as a costume for "technical". Use it for data, code and measurements.
- A system face as the display voice. Source and self-host a face with real character.
- Emoji or Unicode glyphs as icons. Draw icons in one stroke weight.
- A modal for a task that needs neither interruption nor protected focus.
- Choosing light or dark by category. Choose from where and when it is used. A shack at night reads dark, so the site is dark first with a paper light theme.

## Check before you ship

- Contrast: body text at least 4.5:1, large text at least 3:1. On coloured surfaces, tint secondary text from that hue.
- Type: body measure 65 to 75 characters, display text no more than 6 rem, balanced headings, clear scale and weight steps. Run the real copy at every width.
- Spacing: tight groups, generous separation, more space above a heading than below.
- Motion: one authored moment at most, respecting reduced motion. Not an identical entrance on every section.
- States: hover, disabled, loading, error, empty, keyboard focus. Real content and working controls.
- Browser surfaces: theme the text selection, caret, scrollbar, focus ring and form accents from the palette, and use tabular numerals for data.
- Copy: use the product's own language. Controls name their action. Errors name the problem and the way out.
- Every requirement is present and findable within seconds.

## Working method

1. Audit first: list the tells above in the current design, with screenshots.
2. Decide the visual world from the audience, then replace the defaults. Do not polish the discarded look.
3. Build, then look at the rendered result at desktop and phone widths, light and dark.
4. Keep claims true. Where a feature is not built yet, say so in the copy.

## Applied to the app (`kahf-ui`)

- No cards inside cards; panels are ruled sections with labels as column heads.
- Frequencies, callsigns, SNR and times in mono with tabular numerals. Labels in the text face.
- One accent for signal state; semantic colours (good, warning, alert) stay separate from it.
- Spectrum and waterfall are the focus. Everything else stays quiet around them.
