import type { Cell } from "@/api/notebooks";
import { deriveToc } from "./tocModel";

export function TableOfContentsPanel({
  cells,
  onJump,
}: {
  cells: Cell[];
  onJump: (cellId: string) => void;
}) {
  const toc = deriveToc(cells);
  if (toc.length === 0) {
    return <p className="px-3 py-2 text-xs text-muted-foreground">No headings yet.</p>;
  }
  return (
    <nav aria-label="Table of contents" className="flex flex-col py-1">
      {toc.map((e) => (
        <button
          key={e.id}
          type="button"
          onClick={() => onJump(e.id)}
          className="truncate rounded px-2 py-1 text-left text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
          style={{ paddingLeft: `${0.5 + (e.level - 1) * 0.6}rem` }}
          title={e.label}
        >
          {e.label}
        </button>
      ))}
    </nav>
  );
}
