interface Props {
  columns: string[];
  value: string;
  onChange: (col: string) => void;
}

/** Overlay shown on each chart card while a filter is selected, to map the filter to a column. */
export function CardFilterMapOverlay({ columns, value, onChange }: Props) {
  return (
    <div className="dash-nodrag absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-card/85 p-3 text-center backdrop-blur-[1px]">
      <div className="text-xs font-medium text-muted-foreground">Column to filter on</div>
      <select aria-label="column to filter on" className="h-8 w-48 max-w-full rounded border bg-background px-2 text-sm"
        value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Select a column…</option>
        {columns.map((c) => <option key={c} value={c}>{c}</option>)}
      </select>
    </div>
  );
}
