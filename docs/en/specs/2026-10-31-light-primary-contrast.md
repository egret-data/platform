# Light-theme `--primary`: white text at 4.5:1

## Premise, as measured

The reported issue was low Badge contrast in the dark theme. The new opt-in check
`frontend/tests/e2e/badge-variant-contrast.spec.ts` measured every `Badge` variant and
every `StatusBadge` tone in Chromium, in both themes, on `--background`, `--card` and
`--muted`. Every dark-theme reading cleared 4.5:1; the lowest was 5.53. The live
Governance pages measured at least 7.76:1 in dark. **The dark premise does not reproduce.**

The check found a failure in the light theme instead. `Badge variant="default"` and
`Button variant="default"` show white `--primary-foreground` on `--primary`
`oklch(0.59 0.15 185)`, which measures **3.50:1**. These labels are 12–13px medium text,
so the 4.5:1 bar applies (WCAG 2.2 1.4.3).

## Decision (owner)

Darken the light `--primary`, keeping the hue and keeping white text. Change only the
token. Dark stays unchanged.

## What was chosen, and why 0.46

Lightness 0.46 is the binding value when every light pair built on `--primary` is
measured. The plain fill would clear 4.5:1 near L 0.52. The pairs measured after the
change, with the worst surface for each:

| pair | ratio |
|---|---|
| white on `bg-primary` (Badge, Button default) | 5.85 |
| white on `bg-primary/90` (hover fill, Dashboard edit bar) | 4.80 |
| `text-primary` (links, Button `link`) | 5.65 |
| `text-primary` on `bg-primary/10` (avatars, active settings item, SQL Lab chip) | 4.88 |
| `text-foreground` on `bg-primary/15` (StatusBadge primary) | 13.69 |
| `outline-primary` as a non-text indicator (1.4.11, 3:1) | 5.65 |

At L 0.50 the plain fill reached 4.95:1, but the hover fill measured 4.17:1. That is
why the token goes below the first passing value.

`--ring` is a neutral and does not derive from `--primary`. The chart accents
(`var(--primary)` in SQL Lab's result chart, the Gantt view and the chart table's
bars) become a darker teal and keep their hue.

## Not changed

- The outline badge border is the shared `--border` token, measured at 1.2–1.4:1. The
  word inside the badge identifies it, so 1.4.11 does not apply. It is reported only.
- The historical spec `2026-06-23-metabase-dashboard-style-design.md` quotes the old
  value. It is a record of that design pass and is left as written.
