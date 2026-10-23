import type React from "react";
import { Info } from "lucide-react";
import type { CardMapState } from "./card-map-state";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** The item that unmaps the card. The old native select let the user re-pick its "Select a column…"
 *  placeholder to clear the mapping; a Radix item cannot carry "", so this sentinel stands in for it
 *  and is converted back to "" before `onChange` sees it. Never a column name. */
const UNMAPPED = "__unmapped__";

interface Props {
  /** What to show, decided by `cardMapState`. */
  state: CardMapState;
  value: string;
  onChange: (col: string) => void;
}

/** Overlay shown on each card while a filter is selected in edit mode.
 *
 *  Two changes from what this used to be. The select is `w-full`, not `w-48 max-w-full`: that fixed
 *  width measured 97px inside a 125px card at 1440px — narrower than most column names, and unable
 *  to grow when the card did. And a card that cannot take the current filter now says why, instead
 *  of rendering an empty select or, for text/heading/link cards, nothing at all. Metabase does both
 *  (DashCardCardParameterMapperContent.tsx:106-215); the reasons themselves are decided in
 *  card-map-state.ts, so this component only lays them out. */
export function CardFilterMapOverlay({ state, value, onChange }: Props) {
  if (state.kind === "none") return null;

  const shell = (body: React.ReactNode) => (
    <div className="dash-nodrag absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-card/85 p-3 text-center backdrop-blur-[1px]">
      {body}
    </div>
  );

  if (state.kind !== "pick") {
    return shell(
      <div className="flex items-start gap-1.5 text-xs text-muted-foreground">
        <Info className="mt-px h-3 w-3 shrink-0" aria-hidden />
        <span>{state.text}</span>
      </div>,
    );
  }

  return shell(
    <>
      {state.header && (
        <div className="w-full truncate text-xs font-medium text-muted-foreground">{state.header}</div>
      )}
      {/* The stopPropagation on both halves is load-bearing: this overlay sits inside the card body,
          which is a click target that opens the card settings panel (`DashboardView.tsx`, the
          dashcard-body div), and React bubbles a click from the portalled list through the React
          tree to it as well. Without it, opening the picker switches the panel to the card and the
          overlay unmounts under the pointer. */}
      <Select value={value} onValueChange={(v) => onChange(v === UNMAPPED ? "" : v)}>
        <SelectTrigger size="sm" className="w-full" aria-label="column to filter on" onClick={(e) => e.stopPropagation()}>
          <SelectValue placeholder="Select a column…" />
        </SelectTrigger>
        <SelectContent onClick={(e) => e.stopPropagation()}>
          {value !== "" && <SelectItem value={UNMAPPED}>No column</SelectItem>}
          {state.columns.map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}
        </SelectContent>
      </Select>
    </>,
  );
}
