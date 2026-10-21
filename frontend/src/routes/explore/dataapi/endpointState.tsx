import type { ReactNode } from "react";
import { CircleAlert, CircleCheck, CircleDashed, CircleHelp, CircleSlash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { CHIP_TONE } from "./chipTone";
import type {
  DataApiEndpointDto,
  DataApiEndpointVersionDto,
  EndpointSnapshot,
} from "@/api/generated/model";

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
 * WHAT THE HEADER KNOWS ABOUT WHAT IS ACTUALLY SERVING.
 *
 * Four values, and every one of them is a claim the caller can support:
 *
 * - `not-serving` — the endpoint is a draft or is disabled, so no consumer is
 *   receiving anything and "is the draft ahead of what is served" has no
 *   subject. This is the STATE GUARD, and it is a distinct return value rather
 *   than a `false` folded in with the others on purpose: deleting the guard
 *   then changes what this function answers, where a test can see it. When it
 *   was folded into `false` the mutant that deleted it SURVIVED — the label and
 *   the tier both ignore divergence off the `live` state, so nothing rendered
 *   differently and nothing could fail.
 * - `unknown` — we have not been able to find out. First paint, a failed
 *   versions request, a `live` endpoint whose live version is not in the list.
 * - `in-step` — the draft is, field for field, the definition being served.
 * - `ahead` — it is not.
 *
 * `unknown` is what the whole tri-state exists for. This used to be a `boolean`
 * fed from `envelope?.status === 200 ? envelope.data : []`, so a versions
 * request that FAILED produced an empty list, an absent live version, and a
 * confident `false` — a diverged endpoint reading as in step, with no test on
 * the branch. A pill that cannot tell "no" from "I do not know" is a pill that
 * says "no" when the network is down, which is the one moment it matters.
 */
export type DraftVsServed = "not-serving" | "unknown" | "in-step" | "ahead";

/**
 * What a caller managed to learn about the live version, as a value that
 * cannot be mistaken for "there isn't one".
 *
 * `known: true, version: undefined` and `known: false` are different facts —
 * "this endpoint has nothing published" versus "we could not ask" — and
 * collapsing them into one `undefined` is exactly the defect above.
 */
export type LiveVersionLookup =
  | { known: false }
  | { known: true; version: DataApiEndpointVersionDto | undefined };

/**
 * Read a versions-list response envelope into that value.
 *
 * Anything that is not a `200` carrying an array is `known: false`. There is no
 * pagination to miss: `EndpointStore::versions` (`lifecycle.rs`) selects every
 * row for the endpoint, `ORDER BY v.version DESC`, with no `LIMIT`, so the live
 * version is in this list or it is not published.
 *
 * `is_live` is read as the server computed it (`COALESCE(e.live_version_id =
 * v.id, false)` in that same query) and never re-derived here.
 */
export function liveVersionLookup(
  envelope: { status: number; data: unknown } | undefined,
): LiveVersionLookup {
  if (!envelope || envelope.status !== 200 || !Array.isArray(envelope.data)) {
    return { known: false };
  }
  const versions = envelope.data as DataApiEndpointVersionDto[];
  return { known: true, version: versions.find((v) => v.is_live) };
}

/**
 * The served definition, projected out of a published snapshot — and the same
 * projection out of the draft row, so the two are comparable.
 *
 * These two functions must stay each other's mirror; that is the whole
 * contract, and it is why they sit next to each other rather than in the two
 * places that use them.
 *
 * **What is compared, and why exactly this set.** `publish` builds
 * `EndpointSnapshot` through `lifecycle::prospective_snapshot`, which clones
 * these fields off the LOCKED endpoint row, field for field, with nothing
 * computed in between — its own comment says "what is frozen is exactly the
 * bytes the row carries". Named rather than cited by line: the citation this
 * replaces pointed at where the builder USED to be inlined in `publish`, and
 * was already false in the commit that added it.
 * So for these nine fields the snapshot and the draft are byte-identical the
 * instant a version is published, and they diverge only when the author edits
 * one. Both sides also serialise through the SAME Rust types
 * (`ParamSpec`, `OutputSpec`, `CacheSpec`, `LimitsSpec`, `RateLimitSpec`,
 * `StatementSource`), so a value equal in the database is equal on the wire.
 *
 * **What is deliberately left out, and why none of it is an omission:**
 *
 * - `columns` on each statement — computed by the publish-time describe, not
 *   authored; the draft has no such field to compare against.
 * - `iterated` — the shape decision publish makes ACROSS every template
 *   (`EndpointSnapshot::iterated`); derived, not authored.
 * - `param_gate` — a stamp of which revision of the publish gate ran.
 * - `delegated` / `impersonate` — witnesses of the CONNECTION at publish time,
 *   not of the definition. They can change without anyone editing the endpoint,
 *   and the serving path already has its own `cache_scope_stale` answer for it.
 * - `name`, `description`, `folder_id`, `source_kind`, `source_ref` — on the
 *   draft row but not in the snapshot, because renaming an endpoint changes
 *   nothing a consumer receives. This is the direction the old timestamp rule
 *   got wrong: it lit the pill on a rename.
 *
 * **This is NOT the publish-time breaking-change diff, and the two must not be
 * merged.** Package 6 added `egret_dataapi::contract::compare_contracts`, which
 * compares the same two things and answers a different question:
 *
 * | | question | answer |
 * |---|---|---|
 * | here | is there anything to publish? | one boolean over nine fields |
 * | `contract.rs` | does publishing break a consumer? | a classified list |
 *
 * A `sql` edit lights this pill — correctly, the author has unpublished work —
 * and is not a breaking change — also correctly, the data is not the contract.
 * Collapsing them would make one of the two wrong. The load-bearing rule is the
 * other direction: **nothing in the frontend decides what is breaking.** That
 * judgement has one home, it reaches the console as data on `POST /validate`,
 * and the acknowledgement `Versions.tsx` sends back is the server's own digest.
 * A second opinion computed here is the drift this file's own comments keep
 * closing.
 *
 * **Old snapshots compare correctly too**, which was checked rather than
 * assumed. A pre-M2 snapshot carries `sql` + `columns` instead of `statements`;
 * `EndpointSnapshotRepr` (`egret-dataapi-api/src/lib.rs:404`) folds that into a
 * single statement named `main`, and migration `0129` named every pre-M2 draft
 * row's single statement `main` as well. The two agree by construction.
 */
type ServedDefinition = {
  method: string;
  path: string;
  database_id: string;
  statements: unknown[];
  params: unknown;
  output: unknown;
  cache: unknown;
  limits: unknown;
  rate_limit: unknown;
};

/** One statement, minus the frozen columns the author never wrote. */
function authoredStatement(s: Record<string, unknown>): Record<string, unknown> {
  return "script" in s
    ? { name: s.name, script: s.script }
    : { name: s.name, sql: s.sql };
}

function servedDefinitionOfSnapshot(s: EndpointSnapshot): ServedDefinition {
  return {
    method: s.method,
    path: s.path,
    database_id: s.database_id,
    statements: s.statements.map((st) => authoredStatement(st as unknown as Record<string, unknown>)),
    params: s.params,
    output: s.output,
    cache: s.cache,
    limits: s.limits,
    rate_limit: s.rate_limit,
  };
}

function servedDefinitionOfDraft(e: EndpointDefinitionFacts): ServedDefinition {
  return {
    method: e.method,
    path: e.path,
    database_id: e.database_id,
    statements: e.statements.map((st) => authoredStatement(st as unknown as Record<string, unknown>)),
    params: e.params,
    output: e.output,
    cache: e.cache,
    limits: e.limits,
    rate_limit: e.rate_limit,
  };
}

/** The draft columns a published version freezes. */
export type EndpointDefinitionFacts = Pick<
  DataApiEndpointDto,
  | "state"
  | "method"
  | "path"
  | "database_id"
  | "statements"
  | "params"
  | "output"
  | "cache"
  | "limits"
  | "rate_limit"
>;

/**
 * Structural equality over JSON values, with `undefined` properties treated as
 * absent — which is what a JSON round trip does to them, and what the optional
 * fields of an orval-generated model can differ by without differing at all.
 *
 * Key ORDER is not compared. Both sides come out of the same serde types today
 * so it already matches, but a rule that only holds while one serialiser feeds
 * both is not a rule.
 */
function sameJson(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((x, i) => sameJson(x, b[i]));
  }
  const ra = a as Record<string, unknown>;
  const rb = b as Record<string, unknown>;
  const ka = Object.keys(ra).filter((k) => ra[k] !== undefined);
  const kb = Object.keys(rb).filter((k) => rb[k] !== undefined);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => rb[k] !== undefined && sameJson(ra[k], rb[k]));
}

/**
 * Does the draft on screen differ from the definition a consumer is receiving?
 *
 * **A direct comparison against the published snapshot, not a clock.** The rule
 * here used to be `endpoints.updated_at > liveVersion.created_at`, and every
 * defect it had was the same defect: a timestamp is a PROXY for the question,
 * and the proxy drifts every time anything but an edit touches the row. There
 * are six writers of `dataapi.endpoints.updated_at`, all of them read before
 * this was written:
 *
 * | writer | what it does to the pill under a timestamp rule |
 * |---|---|
 * | `endpoints.rs:1092` `update` | the one real edit — correct |
 * | `lifecycle.rs:5186` `publish` | equal stamps, so a strict `>` was needed |
 * | `endpoints.rs:819` `set_state`, via `disable` | needed the `live` guard |
 * | `endpoints.rs:819` `set_state`, via `enable` | **restores `live`** — publish, disable, enable and the pill reads "unpublished changes" forever on an endpoint nobody edited |
 * | `lifecycle.rs:5240` `rollback` | leaves `state` alone, so rolling back TO the version already live diverges it from itself |
 * | `endpoints.rs:984` `trash` / `endpoints.rs:1027` `restore` | restore returns a row as `disabled`, so this one is masked by the state guard today and stops being masked the moment anything enables it |
 *
 * A comparison against the snapshot answers the question directly and is
 * immune to all six, and to the seventh someone adds next year. It also costs
 * nothing new: the shell already fetches the version rows, snapshots and all,
 * and until now threw everything but one integer away.
 *
 * **The failure direction is a false `ahead`, never a false `in-step`.** If a
 * snapshot cannot be read, or the live version is not in the list, the answer
 * is `unknown` rather than a guess. If a future publish ever normalises one of
 * the nine compared fields instead of cloning it, the pill would say
 * "unpublished changes" on an endpoint that has none — which nags an author
 * into republishing. The opposite error hides a live contract drifting from
 * the draft on screen, which is the defect this pill was built for.
 */
export function draftVsServed(
  endpoint: EndpointDefinitionFacts,
  lookup: LiveVersionLookup,
): DraftVsServed {
  if (endpoint.state !== "live") return "not-serving";
  if (!lookup.known) return "unknown";
  const snapshot = lookup.version?.snapshot;
  // A `live` endpoint always has a `live_version_id` — `enable` refuses a null
  // one and `publish` is the only writer — so no live version in a list we DID
  // read means the two disagree, and the honest answer is that we cannot say.
  if (!snapshot || !Array.isArray(snapshot.statements) || snapshot.statements.length === 0) {
    return "unknown";
  }
  return sameJson(servedDefinitionOfSnapshot(snapshot), servedDefinitionOfDraft(endpoint))
    ? "in-step"
    : "ahead";
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
 *
 * **`Live · publish state unknown`, and why it is not a bare `Live`.** When the
 * versions request has not landed or has failed, this pill knows the endpoint
 * is live and knows nothing else. A bare `Live` beside a `CircleCheck` is a
 * claim — "and your draft is what it is serving" — that the caller cannot
 * support, and it is the reading a diverged endpoint would get during an
 * outage. The word says the shape of the gap rather than hiding it, and the
 * pill is already a link to the Versions tab, where the failed request has a
 * real error message and a retry. Loading and failed are one word deliberately:
 * they are the same fact to the reader ("this does not yet say"), and a
 * separate tier for a sub-second state would be a flicker, not information.
 *
 * `served` defaults to `unknown` rather than `in-step` for the same reason: a
 * caller that forgets to pass it gets a pill that says it does not know, not
 * one that says everything is fine.
 */
export function endpointHeaderStateLabel(
  e: EndpointStateFacts,
  { liveVersion, served = "unknown" }: { liveVersion?: number | null; served?: DraftVsServed } = {},
): string {
  if (e.state === "live") {
    // `== null`, not truthiness: versions are numbered from 1 today
    // (`COALESCE(MAX(version), 0) + 1`), but a `0` reaching a truthiness gate
    // prints a bare `Live` — the confident label — for a version we DO know.
    const version = liveVersion == null ? "" : ` · v${liveVersion}`;
    if (served === "ahead") return `Live${version} · unpublished changes`;
    if (served === "in-step") return `Live${version}`;
    return `Live${version} · publish state unknown`;
  }
  if (e.state === "draft") return "Draft · never published";
  return endpointStateLabel(e);
}

/**
 * Which tier the pill paints — the label's own shape, as data, so the tint and
 * the glyph cannot disagree with the words.
 */
export type EndpointStateTier = "live" | "diverged" | "unknown" | "draft" | "disabled";

export function endpointStateTier(
  e: EndpointStateFacts,
  served: DraftVsServed = "unknown",
): EndpointStateTier {
  const base = endpointStateOnlyTier(e);
  if (base !== "live") return base;
  if (served === "ahead") return "diverged";
  if (served === "in-step") return "live";
  // `unknown`, and also `not-serving` — which a `live` endpoint cannot be, so
  // reaching here means two readings of `state` disagreed and "I do not know"
  // is again the only supportable answer.
  return "unknown";
}

/**
 * The tier a surface that is NOT ASKING about divergence can support.
 *
 * **This exists because the list page cannot honestly call `endpointStateTier`
 * at all.** That function defaults `served` to `"unknown"` and, for a `live`
 * endpoint, `"unknown"` is a TIER: grey `bg-muted` and a `CircleHelp`. The list
 * does not fetch versions, so it has no `served` to pass, and the default would
 * paint every live row as a shrug. Passing `"in-step"` to get the green back
 * would be worse — it is a claim the list has no evidence for, and inventing a
 * confident answer where none was measured is the exact defect `DraftVsServed`
 * was introduced to kill.
 *
 * **The honest distinction is that "not asking" and "asked and could not tell"
 * are different facts.** `unknown` means a versions request was made and did
 * not settle the question; the reader should go look. The list never raised the
 * question, so it has nothing to report and no reason to nag. Collapsing the
 * two would be the same mistake in a new place — the mistake that turned a
 * failed request into a confident `false` and a diverged endpoint into an
 * in-step one, written up over `DraftVsServed`.
 *
 * **Its return type, not a convention, is what keeps the list honest.**
 * `EndpointStateOnlyTier` does not contain `diverged` or `unknown`, so a list
 * surface built on this function CANNOT render either — there is no argument to
 * pass and no branch to reach. A `served` parameter defaulted to something
 * benign would have made the same promise in prose and left it to be forgotten;
 * this one is checked by `tsc`.
 */
export type EndpointStateOnlyTier = Extract<EndpointStateTier, "live" | "draft" | "disabled">;

export function endpointStateOnlyTier(e: EndpointStateFacts): EndpointStateOnlyTier {
  if (e.state === "disabled") return "disabled";
  if (e.state === "live") return "live";
  return "draft";
}

/**
 * A distinct glyph per tier, because **state must not be carried by colour
 * alone**. The word already separates the tiers for anyone reading; the glyph
 * separates them at a glance without asking the reader to tell a green chip
 * from an amber one.
 *
 * Exported so tests can name a FORBIDDEN glyph by its tier instead of by its
 * class string: `CircleHelp` is an alias of lucide's `CircleQuestionMark` and
 * paints `lucide-circle-question-mark`, so a test asserting
 * `not.toBe("lucide-circle-help")` is true no matter what the component
 * renders. That assertion was written, and it passed under the mutant it was
 * written to catch.
 */
export const TIER_ICON: Record<EndpointStateTier, typeof CircleCheck> = {
  live: CircleCheck,
  diverged: CircleAlert,
  unknown: CircleHelp,
  draft: CircleDashed,
  disabled: CircleSlash2,
};

/**
 * The tint, and why the INK is `--foreground` on all of them.
 *
 * The pairs themselves, the measurements behind them and the reason the fix is
 * the ink rather than the token now live in `chipTone.ts`, because four other
 * Data API surfaces were painting the same chip by hand and one of them (this
 * pill) had already been fixed while the other four still shipped
 * `bg-success/15 text-success` at 3.72:1. A rule that lives in one file's
 * comment is a rule four files can be written without reading.
 *
 * What stays here is the mapping from TIER to tone, which is a judgement about
 * endpoints and not about colour:
 *
 * `--highlight` for the diverged tier and not `--destructive`: an endpoint
 * serving an older contract than its draft is not broken, it is behind.
 * `--accent` is reserved for hover/focus and is not a state colour at all.
 *
 * `unknown` shares `bg-muted` with `draft` — deliberately the ABSENCE of a
 * state colour, because the pill has no state to report. The two are still
 * told apart by their word and their glyph, which is what carries state here;
 * a fifth tint would imply this is a fifth thing an endpoint can BE, and it is
 * not, it is the reader's view of it failing.
 *
 * Exported so `tests/e2e/dataapi-badge-ink.spec.ts` measures what this actually
 * PAINTS rather than a string retyped in the spec — jsdom resolves no `oklch()`
 * and a class assertion returns the same answer with the tokens swapped
 * (`docs/en/ui-conventions.md` §10).
 */
export const TIER_TONE: Record<EndpointStateTier, string> = {
  live: CHIP_TONE.success,
  diverged: CHIP_TONE.highlight,
  unknown: CHIP_TONE.neutral,
  draft: CHIP_TONE.neutral,
  disabled: CHIP_TONE.destructive,
};

/**
 * The endpoint's state, as a pill: tint + glyph + word.
 *
 * `aria-hidden` on the glyph, and no `aria-label` on the pill: the accessible
 * name is the visible word, computed the ordinary way, so what a screen reader
 * announces and what the page shows are the same string by construction.
 *
 * Shared by both surfaces so the tint and the glyph cannot fork again. They did
 * fork: the list kept its own `STATE_TONE` map and a bare `<Badge>`, so one
 * endpoint read as green-ink-on-green in a table and as themed-with-a-glyph one
 * click away, and the list's copy was the one measuring 3.72:1.
 */
function StateBadge({
  tier,
  className,
  children,
}: {
  tier: EndpointStateTier;
  className?: string;
  children: ReactNode;
}) {
  const Icon = TIER_ICON[tier];
  return (
    <Badge className={cn(TIER_TONE[tier], className)}>
      <Icon aria-hidden="true" data-testid="endpoint-state-icon" />
      {children}
    </Badge>
  );
}

/** The EDITOR header's pill, which knows what is served and says so. */
export function EndpointStateBadge({
  endpoint,
  liveVersion,
  served = "unknown",
  className,
}: {
  endpoint: EndpointStateFacts;
  /** The version number currently served, when the caller knows it. */
  liveVersion?: number | null;
  served?: DraftVsServed;
  className?: string;
}) {
  return (
    <StateBadge tier={endpointStateTier(endpoint, served)} className={className}>
      {endpointHeaderStateLabel(endpoint, { liveVersion, served })}
    </StateBadge>
  );
}

/**
 * The LIST row's pill — the same tint and the same glyph, over the shorter word
 * a row can support and over a tier that cannot express divergence.
 *
 * It takes no `served` and has nowhere to put one: see `endpointStateOnlyTier`
 * for why a list that has not fetched versions must not answer that question in
 * either direction.
 *
 * **`live` still gets `CircleCheck` here, and that does not contradict
 * `endpointHeaderStateLabel`.** That function's comment refuses a bare `Live`
 * beside a check *in the header*, where the author is looking at a draft they
 * are editing and the pair would read as "and this draft is what your consumers
 * receive" — a claim about a comparison. A row carries no draft: nothing is on
 * screen for `Live ✓` to be a claim ABOUT, and it says only that the row's
 * state is the serving one, which the word already said. The check marks the
 * healthy member of {live, draft, disabled}; it does not mark agreement.
 */
export function EndpointRowStateBadge({
  endpoint,
  className,
}: {
  endpoint: EndpointStateFacts;
  className?: string;
}) {
  return (
    <StateBadge tier={endpointStateOnlyTier(endpoint)} className={className}>
      {endpointStateLabel(endpoint)}
    </StateBadge>
  );
}

/**
 * The URL a saved or typed method/path will move the endpoint to, when that is
 * NOT the URL callers use today — or `null`.
 *
 * Serving matches the LIVE route (`live_method` / `live_path`), which only
 * publish and rollback write; a new path on the draft is saved but not served
 * until the next publish (KI-023). `null` for an endpoint never published (it
 * has no URL to keep) and when the two agree. Paths are compared in the
 * spelling the server stores (`normalize_path`: one leading slash, a trailing
 * one absorbed), so typing `reports/daily` against a live `/reports/daily` is
 * not a pending move.
 */
export function pendingRoute(
  e: Pick<DataApiEndpointDto, "live_method" | "live_path">,
  method: string,
  path: string,
): { method: boolean; path: boolean; liveUrl: string } | null {
  if (!e.live_path || !e.live_method) return null;
  const canonical = `/${path.trim().replace(/^\//, "").replace(/\/$/, "")}`;
  const moved = { method: method !== e.live_method, path: canonical !== e.live_path };
  if (!moved.method && !moved.path) return null;
  return { ...moved, liveUrl: `${e.live_method} ${e.live_path}` };
}
