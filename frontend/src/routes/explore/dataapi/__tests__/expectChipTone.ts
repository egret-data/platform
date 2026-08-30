import { expect } from "vitest";

import { CHIP_TONE, type ChipTone } from "../chipTone";

/**
 * THE ASSERTION THAT BINDS A CALL SITE TO `CHIP_TONE`, WRITTEN ONCE.
 *
 * # Why it exists
 *
 * The accessibility fix that produced `chipTone.ts` moved five Data API status
 * chips off `bg-<token>/15 text-<token>` (3.72:1 and 3.68:1 in light mode,
 * against WCAG 2.2 1.4.3's 4.5:1 for this 12px text) and onto one shared tone
 * map with `--foreground` ink. Everything then guarded the MAP: the unit tests
 * covered the endpoint row, and `tests/e2e/dataapi-badge-ink.spec.ts` imports
 * `CHIP_TONE`/`TIER_TONE` and measures their values in Chromium.
 *
 * A constant map is not a population that can shrink. Reverting
 * `TryIt.tsx`'s outcome chip to `outcomeOk ? CHIP_TONE.success : undefined`
 * left every one of those green — the browser spec still measured four healthy
 * tones, because it never looks at a call site — while a `500` went back to
 * falling through `Badge`'s DEFAULT variant, solid `bg-primary` at 3.50:1.
 * That regression is this branch's own best find and nothing but the diff was
 * holding it.
 *
 * So each call site gets its own assertion, and this is it.
 *
 * # Why the class strings are never typed here
 *
 * The expected classes are read out of the shipped `CHIP_TONE` at assert time.
 * A test that types `"bg-success/15 text-foreground"` pins the TEST's opinion
 * of the tone: change the map to rescue a contrast reading and every such test
 * fails for a change that was correct, so the numbers and the guards drift
 * apart. Read from the map, a tone change moves code and test together, and
 * what stays pinned is the only thing this file has an opinion about — WHICH
 * tone each call site reaches for.
 *
 * # Why it also asserts what is NOT there
 *
 * Presence alone is half a guard. `bg-success/15` and `bg-destructive/15` are
 * different classes, so a call site that swapped its two arms would satisfy a
 * presence-only check on neither — but a call site that painted BOTH (a
 * conditional collapsed into a concatenation) would satisfy it on both. Every
 * class belonging to another tone and not to this one is therefore required to
 * be absent, which is also what kills an arm rewired to the wrong tone.
 *
 * `border-transparent` is in all four tones, so it is excluded from that sweep
 * by construction rather than by a list someone has to maintain.
 */
function classesOf(value: string): string[] {
  return value.split(/\s+/).filter(Boolean);
}

/** The classes on an element, as a set. `getAttribute` because `className` is
 * an object, not a string, on SVG elements — and these helpers get pointed at
 * whatever `getByText` returned. */
function paintedOn(el: Element): Set<string> {
  return new Set(classesOf(el.getAttribute("class") ?? ""));
}

/**
 * Assert that `el` is painted with `CHIP_TONE[tone]` and with no other tone.
 *
 * @param label names the call site in the failure message — these run in loops
 *   and over several arms of one component, and "expected true, got false" does
 *   not say which arm.
 */
export function expectChipTone(el: Element, tone: ChipTone, label: string): void {
  const painted = paintedOn(el);
  const want = classesOf(CHIP_TONE[tone]);

  // A tone that had become the empty string would make the loop below vacuous
  // and this function would pass on anything at all.
  expect(want.length, `CHIP_TONE.${tone} is empty`).toBeGreaterThan(0);

  const got = [...painted].join(" ");
  for (const cls of want) {
    expect(painted.has(cls), `${label}: expected CHIP_TONE.${tone}'s \`${cls}\`, painted \`${got}\``).toBe(
      true,
    );
  }
  for (const [other, value] of Object.entries(CHIP_TONE)) {
    if (other === tone) continue;
    for (const cls of classesOf(value)) {
      if (want.includes(cls)) continue;
      expect(painted.has(cls), `${label}: CHIP_TONE.${other}'s \`${cls}\` must not be here`).toBe(false);
    }
  }
}

/**
 * Assert that `el` wears NONE of the shared tones.
 *
 * For the arms that are deliberately untinted — Clients' `Revoked`, which is an
 * `outline` badge on a row that is muted as a whole, because a revoked client
 * is not a state to scan for. Without this, "revoked" is the one Clients arm
 * that no assertion reaches.
 */
export function expectNoChipTone(el: Element, label: string): void {
  const painted = paintedOn(el);
  for (const [tone, value] of Object.entries(CHIP_TONE)) {
    // `border-transparent` is shared with `Badge`'s own default and secondary
    // variants, so it is not evidence of a TONE and is not asked about here.
    const distinctive = classesOf(value).filter((c) => c !== "border-transparent");
    expect(distinctive.length, `CHIP_TONE.${tone} has no distinctive class left`).toBeGreaterThan(0);
    for (const cls of distinctive) {
      expect(painted.has(cls), `${label}: must wear no tone, found CHIP_TONE.${tone}'s \`${cls}\``).toBe(
        false,
      );
    }
  }
}
