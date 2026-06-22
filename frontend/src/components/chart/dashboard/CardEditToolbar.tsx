interface Props {
  cardType: string;
  onSettings: () => void;
  onDuplicate: () => void;
  onReplace: () => void;
  onRemove: () => void;
}

export function CardEditToolbar({ cardType, onSettings, onDuplicate, onReplace, onRemove }: Props) {
  const btn = "rounded px-1 text-xs hover:bg-accent";
  return (
    <div className="dash-nodrag flex items-center gap-0.5 rounded-md border bg-card px-1 py-0.5 shadow-sm">
      <button type="button" aria-label="card settings" className={btn} onClick={onSettings}>⚙</button>
      <button type="button" aria-label="duplicate card" className={btn} onClick={onDuplicate}>⧉</button>
      {cardType === "chart" && <button type="button" aria-label="replace card" className={btn} onClick={onReplace}>↺</button>}
      <button type="button" aria-label="remove card" className={`${btn} text-destructive`} onClick={onRemove}>✕</button>
    </div>
  );
}
