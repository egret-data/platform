import {
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleHelp,
  CircleMinus,
  CirclePlay,
  CircleSlash2,
  CircleX,
  type LucideIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * THE WORKFLOW STATE VOCABULARY: Kestra's states on egret's tokens (spec §8.1).
 *
 * Kestra colours a state by hue. egret maps each GROUP of states onto one of
 * its tokens, and every chip also carries the state's word and a group glyph,
 * so a state is never colour alone (FE-A11Y-4).
 *
 * **The token is the tint; the ink is `--foreground` on every tone.** Coloured
 * ink on a wash of its own hue fails 4.5:1 for this 12px text in light mode;
 * `routes/explore/dataapi/chipTone.ts` holds the measurements. The same rule
 * is applied here rather than imported because that map has no `primary` and
 * no `muted-foreground` tone, and both groups below need one.
 *
 * `muted` and `muted-foreground` are two different tones on purpose: pending
 * (`muted`, not started yet) and neutral (`muted-foreground`, finished without
 * an outcome) are told apart by the spec's table.
 */
export type StateTone = "muted" | "primary" | "success" | "highlight" | "destructive" | "neutral";
type Tone = StateTone;

const TONE_CLASS: Record<Tone, string> = {
  muted: "border-transparent bg-muted text-foreground",
  primary: "border-transparent bg-primary/15 text-foreground",
  success: "border-transparent bg-success/15 text-foreground",
  highlight: "border-transparent bg-highlight/15 text-foreground",
  destructive: "border-transparent bg-destructive/15 text-foreground",
  neutral: "border-transparent bg-muted-foreground/15 text-foreground",
};

interface StateLook {
  tone: Tone;
  icon: LucideIcon;
}

const PENDING: StateLook = { tone: "muted", icon: CircleDashed };
const RUNNING: StateLook = { tone: "primary", icon: CirclePlay };
const SUCCESS: StateLook = { tone: "success", icon: CircleCheck };
const WARNING: StateLook = { tone: "highlight", icon: CircleAlert };
const FAILED: StateLook = { tone: "destructive", icon: CircleX };
const NEUTRAL: StateLook = { tone: "neutral", icon: CircleMinus };
const UNKNOWN: StateLook = { tone: "neutral", icon: CircleHelp };

/**
 * The seventeen Kestra execution states, by §8.1's groups.
 *
 * `RESUBMITTED` doubles as this module's marker in
 * `scripts/assert-workflows-lazy.mjs`, which proves the chip stays out of the
 * entry chunk.
 *
 * Maps, not object literals: the state is a server string, and a plain
 * object would answer `constructor` or `toString` from its prototype.
 */
const EXECUTION_STATES = new Map<string, StateLook>(Object.entries({
  CREATED: PENDING,
  SUBMITTED: PENDING,
  QUEUED: PENDING,
  RESTARTED: PENDING,
  RESUBMITTED: PENDING,
  RUNNING,
  RETRYING: RUNNING,
  KILLING: RUNNING,
  SUCCESS,
  WARNING,
  PAUSED: WARNING,
  BREAKPOINT: WARNING,
  FAILED,
  KILLED: FAILED,
  CANCELLED: NEUTRAL,
  SKIPPED: NEUTRAL,
  RETRIED: NEUTRAL,
}));

/**
 * A flow's own state (`workflow.flows.state`), shown as the one thing it
 * decides: whether the flow runs (the user's ruling, 2026-10-10). Every
 * execution runs the saved draft, and `state` gates nothing but `disabled`,
 * so `draft` and `live` are both **Active**; the version number is shown on
 * its own, never in this chip.
 */
const ACTIVE: StateLook & { word: string } = { tone: "success", icon: CircleCheck, word: "Active" };
const FLOW_STATES = new Map<string, StateLook & { word: string }>([
  ["draft", ACTIVE],
  ["live", ACTIVE],
  ["disabled", { tone: "neutral", icon: CircleSlash2, word: "Disabled" }],
]);

/** The flow-list filter's values: whether a flow runs. */
export type FlowActivity = "active" | "disabled";

/** `draft` and `live` run; `disabled` does not. An unknown state is not counted as either. */
export function flowActivity(state: string): FlowActivity | undefined {
  if (state === "disabled") return "disabled";
  return state === "draft" || state === "live" ? "active" : undefined;
}

/**
 * The tone an execution state is drawn in, for a mark that is not a chip (the
 * gantt's bars). The mark beside it must still say the state in words.
 */
export function executionStateTone(state: string): StateTone {
  return (EXECUTION_STATES.get(state) ?? UNKNOWN).tone;
}

/** `SUCCESS` → `Success`, `draft` → `Draft`. */
function stateWord(state: string): string {
  return state.charAt(0).toUpperCase() + state.slice(1).toLowerCase();
}

/**
 * A state as a chip: tint, glyph and word.
 *
 * A state this table does not know (a newer engine's) is shown exactly as the
 * server wrote it, on the neutral tone with a question glyph, rather than
 * hidden or guessed at. Lookups are per `kind`, so `SUCCESS` is not a flow
 * state and `live` is not an execution state.
 *
 * The glyph is `aria-hidden` and the chip has no `aria-label`: its accessible
 * name is the visible word, so what a screen reader announces and what the
 * page shows are the same string.
 */
export function StateChip({ state, kind }: { state: string; kind: "execution" | "flow" }) {
  const flowLook = kind === "flow" ? FLOW_STATES.get(state) : undefined;
  const look = kind === "execution" ? EXECUTION_STATES.get(state) : flowLook;
  const { tone, icon: Icon } = look ?? UNKNOWN;
  const word = flowLook ? flowLook.word : look ? stateWord(state) : state;
  return (
    <Badge variant="outline" data-state={state} className={cn(TONE_CLASS[tone])}>
      <Icon aria-hidden="true" />
      {word}
    </Badge>
  );
}
