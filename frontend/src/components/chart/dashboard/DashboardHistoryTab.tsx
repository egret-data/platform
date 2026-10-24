import { useState } from "react";
import { Button } from "@/components/ui/button";
import { ConfirmActionDialog } from "@/components/explore/ConfirmActionDialog";
import type { DashboardRevision } from "@/api/dashboards";
import { attributionLine, revisionSummary } from "./attribution";

interface Props {
  revisions: DashboardRevision[];
  isLoading: boolean;
  /** Called with the revision's id. */
  onRevert: (revisionId: string) => void;
  reverting: boolean;
}

/** Metabase's History tab: a newest-first timeline with a revert action on every entry that is
 *  not the live one. Lives in its own file so the info sidebar stays a shell — DashboardView.tsx
 *  is touched by seven workstreams and must not grow. */
export function DashboardHistoryTab({ revisions, isLoading, onRevert, reverting }: Props) {
  // A revert replaces the live dashboard, so it is confirmed first (KI-075). It is itself a
  // revision, so the dialog can say how to undo it.
  const [pending, setPending] = useState<string | null>(null);
  if (isLoading) return <p className="py-2 text-xs text-muted-foreground">Loading history…</p>;
  if (revisions.length === 0) {
    return (
      <p className="py-2 text-xs text-muted-foreground">
        No history yet — this dashboard hasn't been saved since revisions were turned on.
      </p>
    );
  }
  return (
    <>
    <ol className="flex flex-col gap-2 py-1">
      {revisions.map((r) => (
        <li key={r.id} className="flex items-start justify-between gap-2 border-b pb-2 text-xs last:border-b-0">
          <span className="min-w-0">
            <span className="block">{revisionSummary(r.kind, r.created_by_name)}</span>
            <span className="block text-muted-foreground">{attributionLine(r.created_at, null)}</span>
          </span>
          {/* is_current comes from the API, not from this array's order. */}
          {!r.is_current && (
            <Button size="sm" variant="ghost" className="h-6 shrink-0 px-2 text-xs"
              disabled={reverting} onClick={() => setPending(r.id)}>Revert</Button>
          )}
        </li>
      ))}
    </ol>
    <ConfirmActionDialog
      open={pending !== null}
      onOpenChange={(o) => { if (!o) setPending(null); }}
      title="Revert to this version?"
      description="The dashboard goes back to how it was at this point, for everyone. The current version stays in the history, so you can revert to it again."
      confirmLabel="Revert"
      onConfirm={() => { if (pending) onRevert(pending); setPending(null); }}
    />
    </>
  );
}
