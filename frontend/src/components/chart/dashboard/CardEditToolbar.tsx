interface Props {
  cardType: string;
  onSettings: () => void;
  onDuplicate: () => void;
  onReplace: () => void;
  onRemove: () => void;
}

export function CardEditToolbar({ cardType, onSettings, onDuplicate, onReplace, onRemove }: Props) {
  const btn = "rounded bg-background/80 px-1 text-xs hover:bg-accent";
  return (
    <div className="dash-nodrag absolute right-1 top-1 z-10 flex gap-1">
      <button type="button" aria-label="card settings" className={btn} onClick={onSettings}>⚙</button>
      <button type="button" aria-label="duplicate card" className={btn} onClick={onDuplicate}>⧉</button>
      {cardType === "chart" && <button type="button" aria-label="replace card" className={btn} onClick={onReplace}>↺</button>}
      <button type="button" aria-label="remove card" className={`${btn} text-destructive`} onClick={onRemove}>✕</button>
    </div>
  );
}
