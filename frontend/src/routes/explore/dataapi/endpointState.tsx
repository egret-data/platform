import { CircleAlert, CircleCheck, CircleDashed, CircleSlash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import type { DataApiEndpointDto, DataApiEndpointVersionDto } from "@/api/generated/model";

/**
 * WHAT AN ENDPOINT IS, IN WORDS, IN ONE PLACE.
 *
 * The list page said "Live" on every row. The six-tab editor the author
 * changes the endpoint on said nothing at all — not on Query, not on Config,
 * not anywhere but the row they had already left. This module is the words,
 * shared, so the two surfaces cannot drift into describing the same endpoint
 * differently.
 *
 * The two surfaces still differ in what they KNOW, and the wording differs
 * with it rather than pretending otherwise: a list row has the endpoint and
 * nothing else, while the detail shell has already loaded the version history
 * and can therefore name the served version and say whether the draft has
 * moved past it.
 */

/** The two columns every state word is derived from. */
export type EndpointStateFacts = Pick<DataApiEndpointDto, "state" | "live_version_id">;

/**
 * The LIST row's badge word — unchanged, deliberately.
 *
 * `disabled` is the one state where `live_version_id` carries information the
 * state does not. `EndpointStore::disable` (`endpoints.rs`) has no state guard
 * and never touches `live_version_id`, so a draft can be disabled directly: a
 * `disabled` row means either "was serving, paused" or "never served at all",
 * and those want different next actions (Enable vs. Publish). A bare
 * "Disabled" reads as the first in both cases.
 *
 * `live` and `draft` need no such split on a list row: a fresh draft is
 * created with `live_version_id` null (migration 0121) and `publish`
 * (`lifecycle.rs`) is the only writer that sets it, atomically with
 * `state = 'live'`, so for those two the id adds nothing the word has not
 * already said.
 */
export function endpointStateLabel(e: EndpointStateFacts): string {
  if (e.state === "disabled") {
    return e.live_version_id ? "Disabled · was live" : "Disabled · never published";
  }
  return e.state.length ? e.state.charAt(0).toUpperCase() + e.state.slice(1) : e.state;
}

/**
 * The EDITOR header's word, which says two more things than a list row can.
 *
 * - **Which version is serving.** The author is looking at a draft; the number
 *   is the only thing on the page that belongs to the contract a consumer is
 *   actually calling. Proven live: mid-divergence the router answered
 *   `{"one":1}` at `version:1` while the draft on screen read `SELECT 42`.
 * - **That the draft has moved past it.** This is the state the whole finding
 *   is about — Save and Publish had different consequences and, before this,
 *   the same signal.
 *
 * `Draft · never published` rather than the list's bare `Draft`: on the
 * surface where the author is writing SQL, "nothing is serving this yet" is
 * the fact that decides what they do next.
 */
export function endpointHeaderStateLabel(
  e: EndpointStateFacts,
  { liveVersion, diverged = false }: { liveVersion?: number | null; diverged?: boolean } = {},
): string {
  if (e.state === "live") {
    return `Live${liveVersion ? ` · v${liveVersion}` : ""}${diverged ? " · unpublished changes" : ""}`;
  }
  if (e.state === "draft") return "Draft · never published";
  return endpointStateLabel(e);
}

/**
 * Which tier the pill paints — the label's own shape, as data, so the tint and
 * the glyph cannot disagree with the words.
 */
export type EndpointStateTier = "live" | "diverged" | "draft" | "disabled";

export function endpointStateTier(e: EndpointStateFacts, diverged = false): EndpointStateTier {
  if (e.state === "live") return diverged ? "diverged" : "live";
  if (e.state === "disabled") return "disabled";
  return "draft";
}

/**
 * A distinct glyph per tier, because **state must not be carried by colour
 * alone**. The word already separates the tiers for anyone reading; the glyph
 * separates them at a glance without asking the reader to tell a green chip
 * from an amber one.
 */
const TIER_ICON: Record<EndpointStateTier, typeof CircleCheck> = {
  live: CircleCheck,
  diverged: CircleAlert,
  draft: CircleDashed,
  disabled: CircleSlash2,
};

/**
 * The tint, and why the INK is `--foreground` on all four.
 *
 * Measured in Chromium by painting the composited colour into a 1×1 canvas and
 * reading the pixel back — never by parsing `oklch()`, which yields confident
 * garbage in exactly the 2.5–2.6 band the real failures live in:
 *
 *   text-success     on bg-success/15       3.72 light / 7.00 dark
 *   text-destructive on bg-destructive/15   3.68 light / 5.84 dark
 *   text-highlight   on --background        2.56 light / 9.68 dark
 *   text-foreground  on bg-success/15      14.76 light / 15.95 dark
 *   text-foreground  on bg-highlight/15    15.80 light / 15.68 dark
 *   text-foreground  on bg-destructive/15  13.81 light / 16.53 dark
 *   text-foreground  on bg-muted           16.96 light / 14.68 dark
 *
 * This text is 12px (`text-xs`), so the bar is WCAG 2.2 1.4.3's 4.5:1 — not
 * the 3:1 of 1.4.11, which governs non-text. The three tinted inks fail it in
 * light mode; `--foreground` clears it by a factor of three on every tint. So
 * the tint stays (it is the recognition cue the list row already trained) and
 * the ink does not. The tint is decoration: the word and the glyph are what
 * identify the state, so 1.4.11 does not bind on the fill either.
 *
 * `--highlight` for the diverged tier and not `--destructive`: an endpoint
 * serving an older contract than its draft is not broken, it is behind.
 * `--accent` is reserved for hover/focus and is not a state colour at all.
 */
const TIER_TONE: Record<EndpointStateTier, string> = {
  live: "border-transparent bg-success/15 text-foreground",
  diverged: "border-transparent bg-highlight/15 text-foreground",
  draft: "border-transparent bg-muted text-foreground",
  disabled: "border-transparent bg-destructive/15 text-foreground",
};

/**
 * The endpoint's state, as a pill: tint + glyph + word.
 *
 * `aria-hidden` on the glyph, and no `aria-label` on the pill: the accessible
 * name is the visible word, computed the ordinary way, so what a screen reader
 * announces and what the page shows are the same string by construction.
 */
export function EndpointStateBadge({
  endpoint,
  liveVersion,
  diverged = false,
  className,
}: {
  endpoint: EndpointStateFacts;
  /** The version number currently served, when the caller knows it. */
  liveVersion?: number | null;
  diverged?: boolean;
  className?: string;
}) {
  const tier = endpointStateTier(endpoint, diverged);
  const Icon = TIER_ICON[tier];
  return (
    <Badge className={cn(TIER_TONE[tier], className)}>
      <Icon aria-hidden="true" data-testid="endpoint-state-icon" />
      {endpointHeaderStateLabel(endpoint, { liveVersion, diverged })}
    </Badge>
  );
}

/** The version `endpoints.live_version_id` points at, as the server marked it. */
export function liveVersionOf(
  versions: DataApiEndpointVersionDto[],
): DataApiEndpointVersionDto | undefined {
  return versions.find((v) => v.is_live);
}

/**
 * Has the saved draft moved since the live version was frozen?
 *
 * **A strict `>`, and that is the whole correctness argument.** `publish`
 * (`lifecycle.rs`) inserts the version row and writes
 * `endpoints.updated_at = now()` inside ONE transaction, and Postgres `now()`
 * is the transaction timestamp — so immediately after a publish the two stamps
 * are exactly equal. A `>=` would report every freshly published endpoint as
 * diverged, which is the failure mode of a signal nobody would then trust.
 *
 * **Only for a `live` endpoint.** `disable` writes `updated_at = now()` and
 * leaves `live_version_id` alone (`endpoints.rs`), so the bare timestamp
 * comparison says "the row moved after the version was frozen" for every
 * endpoint anyone has ever paused. Divergence is a claim about what a live
 * endpoint is serving right now; a disabled one is serving nothing, and its
 * badge already says which of the two disabled states it is in.
 *
 * **Why the timestamp and not a field-by-field diff against the snapshot.**
 * The snapshot is not the draft: publish normalises, adds the frozen column
 * union per statement, and stamps `param_gate`, so a structural comparison
 * would have to know which differences are real — and would report a false
 * divergence the day publish learns to normalise one more thing. The stamp
 * answers a narrower question exactly: *this endpoint has been edited since
 * the version a consumer is receiving was frozen.* That sentence is true even
 * when the edit was to a field the snapshot does not carry (a rename), which
 * is why the copy says "unpublished changes" and not "the SQL differs".
 */
export function draftIsAheadOfLive(
  endpoint: Pick<DataApiEndpointDto, "state" | "updated_at">,
  liveVersion: Pick<DataApiEndpointVersionDto, "created_at"> | undefined,
): boolean {
  if (endpoint.state !== "live" || !liveVersion) return false;
  const draft = Date.parse(endpoint.updated_at);
  const frozen = Date.parse(liveVersion.created_at);
  // `Date.parse` answers NaN on anything it cannot read, and every comparison
  // with NaN is false — so an unparseable stamp reports "not diverged" rather
  // than raising a warning nobody can act on.
  return draft > frozen;
}
