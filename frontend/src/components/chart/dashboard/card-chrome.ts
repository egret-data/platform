import type { DashboardCard } from "@/api/dashboards";

type CardChrome = Pick<DashboardCard, "card_type" | "options">;

/**
 * Does this card paint the card chrome — border plus card background?
 *
 * Metabase's `dashcard.background` ("Show background", `getDefault: () => true`,
 * `dashboard/visualizations/Text/index.ts:67-73`), with egret's existing exceptions folded in
 * rather than bolted beside: a HEADING has always rendered chrome-less (`HeadingCard.tsx`, and the
 * `bg-dashboard` branch this replaces in `DashboardView.tsx`), and a PLACEHOLDER draws its own
 * dashed drop target.
 *
 * So the default is per card type and the stored option, when present, wins over it — in BOTH
 * directions. Generalising the heading's rendering to any card has to mean a heading can go the
 * other way too, or the toggle is a no-op on the card type that motivated it.
 *
 * `typeof stored === "boolean"` rather than a truthiness check: `options` is `Record<string,
 * unknown>` and is never key-whitelisted server-side, so a hand-edited `"false"` is reachable, and
 * coercing it would silently strip a chart card's chrome.
 */
export function showsCardBackground(card: CardChrome): boolean {
  const stored = card.options.background;
  if (typeof stored === "boolean") return stored;
  return card.card_type !== "heading" && card.card_type !== "placeholder";
}

/** The chrome classes for a card body. A chrome-less card falls back to `bg-dashboard`, not to
 *  nothing: the tile still has to paint the canvas colour beneath it, which is what the heading
 *  branch has always done. A placeholder paints neither — its own component owns its look. */
export function cardChromeClass(card: CardChrome): string {
  if (showsCardBackground(card)) return "rounded-sm border bg-card";
  return card.card_type === "placeholder" ? "" : "bg-dashboard";
}
