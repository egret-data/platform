import type React from "react";
import { Info } from "lucide-react";
import type { CardMapState } from "./card-map-state";

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
      <select aria-label="column to filter on" className="h-8 w-full rounded border bg-background px-2 text-sm"
        value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Select a column…</option>
        {state.columns.map((c) => <option key={c} value={c}>{c}</option>)}
      </select>
    </>,
  );
}
