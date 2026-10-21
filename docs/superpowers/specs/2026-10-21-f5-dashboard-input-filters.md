# F5: Dashboards, lost input and lost filters

Package F5 of the Known-issues fix programme (KI-059, KI-060, KI-061, KI-063). Frontend only.

## KI-059: spaces and Enter typed into a heading or text card

The edit-mode card wrapper (`DashboardView.tsx`, `data-testid="dashcard-body"`, `role="button"`)
opens the card settings on Enter and Space and calls `preventDefault`. The keydown of the heading
input and the text textarea bubbles up to it, so the characters were never inserted.

Fix: the wrapper acts only when `e.target === e.currentTarget`, that is a key pressed on the
focused card itself. This covers input, textarea and contenteditable, and also any other focusable
control inside a card (an inline filter button). Keyboard activation of the card (FE-A11Y-3) is
unchanged.

## KI-060: a date / all-options value is lost in the URL

The all-options date picker emits an object (`{mode, op, from, to}` or `{mode, dir, n, unit}`);
the URL stored it as JSON and `useDashboardParams` read it back as a string. The read side now
decodes by the shape the parameter's control emits:

| Parameter | Value written | URL text | Read back |
|---|---|---|---|
| list types (category, location, id), `between`, `range` | string or array | string, or JSON array | JSON array when it starts with `[`, else the string |
| date `all-options` | object | JSON object | the object when it starts with `{` |
| number (not `between`) | number | `String(n)` | a number |
| everything else | string | the string | the string |

A table-driven test covers every type and operator with each value shape its control emits, through
write, read and a reload of the resulting URL.

## KI-061: a default cannot be cleared; "Reset all filters" ignores defaults

Decision, the URL encoding of a cleared filter: **the slug with an empty value** (`?order_date=`).

- Clearing a filter that has a default writes `slug=`. On read, `slug=` means "no value", and the
  default is not applied. It survives reload and a shared link because it is in the URL.
- Clearing a filter without a default removes the slug, as before.
- A required filter cannot be emptied: clearing it removes the slug, so it returns to its default
  (the cards cannot run without a value). This matches Metabase, where a required filter resets to
  its default.

"Reset all filters" is offered when any filter's current value differs from its default (blank
spellings are equal, arrays and objects compare by content), which includes a cleared default.
Reset removes every parameter slug in one URL write, leaves `refresh` and `fullscreen`, and drops a
pending (not applied) draft. `canReset` and `reset` move from `useDashboardViewControls` (which
only knew slugs, not defaults) to `useDashboardParams`.

## KI-063: `[object Object]` in text and heading cards

`interpolateParams` formats a date value the way the filter chip does (`summarize`): a range as
"from – to", relative and all-options picks in words ("Past 3 months", "After 1997-06-01"),
quarter-year as "Q2 1997". A number `between` reads "10 – 20". Lists join with ", ". A plain object
that is not a date value prints nothing, so no value shape prints `[object Object]`.
