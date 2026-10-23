import { STATUS_TONE } from "@/components/ui/status-badge";

/**
 * THE TINTED STATUS CHIP, AND WHY ITS INK IS `--foreground` ON EVERY TINT.
 *
 * Data API shipped this chip in five places — the endpoint list's state badge,
 * Versions' `Live`, Clients' `Active`/`Expired`, Activity's status code, Try
 * It's outcome — each writing the pair out by hand as `bg-<token>/15
 * text-<token>`. Coloured ink on a wash of its own hue is the whole defect:
 *
 *   text-success     on bg-success/15        3.72 light / 7.00 dark
 *   text-destructive on bg-destructive/15    3.68 light / 5.84 dark
 *   text-highlight   on bg-highlight/15      2.24 light / 7.82 dark
 *
 * This text is 12px (`text-xs`), so the bar is WCAG 2.2 1.4.3's 4.5:1 for text
 * — not the 3:1 of 1.4.11, which governs non-text. Three of the six readings
 * fail it. `--foreground` on the SAME tints clears it by a factor of three in
 * both themes and does not move dark mode down anywhere it was already fine:
 *
 *   text-foreground  on bg-success/15       14.76 light / 15.95 dark
 *   text-foreground  on bg-destructive/15   13.81 light / 16.53 dark
 *   text-foreground  on bg-highlight/15     15.80 light / 15.68 dark
 *   text-foreground  on bg-muted            16.96 light / 14.68 dark
 *
 * Measured in Chromium by painting each computed colour over the composited
 * surface into a 1×1 canvas and reading the pixel back
 * (`tests/e2e/dataapi-badge-ink.spec.ts`) — never by parsing `oklch()` into
 * three numbers, which reads L, C and H as R, G and B and yields confident
 * garbage in exactly the 2.2–2.6 band the real failures live in.
 *
 * **So the tint stays and the ink does not.** The tint is the recognition cue
 * these lists already trained, and it is decoration: at every call site the
 * WORD ("Live", "Active", "Expired") or the NUMBER (`200`, `500`) is what
 * identifies the state, so nothing here rests on colour alone and 1.4.11 does
 * not bind on the fill either. `endpointState.tsx`'s pill adds a third channel,
 * a per-tier glyph, because its five tiers are more than a colour can carry.
 *
 * **Not a token change.** `--success` and `--destructive` also serve SOLID-fill
 * roles — six control-plane surfaces paint `backgroundColor: var(--success)`
 * with `color: var(--success-foreground)` inline, and `Badge`/`Button`'s
 * `destructive` variants fill with `--destructive` — so moving either token to
 * rescue the ink would break the pairing it was designed for. Measured through
 * `var(--token)` rather than through utility classes, because a Tailwind
 * utility no source file uses is never emitted and silently falls back to the
 * INHERITED colour: a first pass "measured" `text-success-foreground` at 3.97
 * and was really reading `--foreground`. Read correctly, the solid pairs are
 * `--success-foreground` on `--success` 4.43 light / 7.96 dark and
 * `--destructive-foreground` on `--destructive` 4.69 / 2.83 — the first already
 * a hair under 4.5 and the second well under it in dark, so neither token has
 * room to give in the direction the chip would want. Both are outside this
 * file's scope, and both are the reason the fix here is the ink.
 *
 * **`border-transparent` is part of the tone, not decoration.** `Badge`'s cva
 * base carries `border`, and its `outline` variant is the only one that leaves
 * a visible one; a chip that passes only `bg-…` inherits the default variant's
 * `bg-primary` underneath. Every value here is complete enough to hand straight
 * to `className`.
 *
 * The values now live in `components/ui/status-badge.tsx` (`STATUS_TONE`), the
 * one status badge every area uses (KI-222); this map is the Data API's view of
 * it, kept so the tests that bind each call site to a tone keep reading it.
 */
export const CHIP_TONE = {
  /** A healthy, serving, current state. */
  success: STATUS_TONE.success,
  /** A stopped, expired or failing state. */
  destructive: STATUS_TONE.destructive,
  /** Behind, stale, worth a look — never `destructive`, which claims broken. */
  highlight: STATUS_TONE.highlight,
  /** Deliberately the ABSENCE of a state colour: nothing to report. */
  neutral: STATUS_TONE.neutral,
} as const;

export type ChipTone = keyof typeof CHIP_TONE;
