/** A dashboard heading card: a single styled line, no card chrome. In edit mode it is an inline
 *  input bound to the raw text (so {{slug}} stays literal). */
export function HeadingCard({ text, editing, autoFocus, onChange }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }) {
  if (editing && onChange) {
    return (
      <input
        aria-label="heading text"
        className="dash-nodrag h-full w-full bg-transparent px-1 text-xl font-bold text-foreground placeholder:font-normal placeholder:text-muted-foreground focus:outline-none"
        placeholder="Heading"
        autoFocus={autoFocus}
        value={text}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <div className="flex h-full items-center px-1 text-xl font-bold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
