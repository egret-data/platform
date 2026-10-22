import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { diffConfig, type ConfigChange } from "./configDiff";

const KIND_LABEL: Record<ConfigChange["kind"], string> = {
  added: "Added",
  removed: "Removed",
  changed: "Changed",
};

const KIND_CLASS: Record<ConfigChange["kind"], string> = {
  added: "text-emerald-600",
  removed: "text-destructive",
  changed: "text-amber-600",
};

/** Compact JSON rendering of a leaf value, truncated for the diff row. */
function fmt(value: unknown): string {
  const s = JSON.stringify(value);
  if (s === undefined) return "—";
  return s.length > 80 ? `${s.slice(0, 79)}…` : s;
}

function ChangeRow({ change }: { change: ConfigChange }) {
  return (
    <div className="flex flex-col gap-0.5 border-b py-2 text-sm last:border-b-0">
      <div className="flex items-center gap-2">
        <span
          className={`w-16 shrink-0 text-xs font-medium ${KIND_CLASS[change.kind]}`}
        >
          {KIND_LABEL[change.kind]}
        </span>
        <span className="font-mono text-xs">{change.path}</span>
      </div>
      <div className="pl-16 font-mono text-xs text-muted-foreground">
        {change.kind === "added" ? (
          <span>{fmt(change.to)}</span>
        ) : change.kind === "removed" ? (
          <span className="line-through">{fmt(change.from)}</span>
        ) : (
          <span>
            <span className="line-through">{fmt(change.from)}</span>
            {" → "}
            <span className="text-foreground">{fmt(change.to)}</span>
          </span>
        )}
      </div>
    </div>
  );
}

/**
 * Modal that diffs the freshly-built `after` config against the engine's
 * stored `before` config (leaf-path `diffConfig`), colour-coded by change kind,
 * and confirms the save: saving restarts the engine (`restartNote` says how),
 * so it is never applied without this step (KI-198).
 */
export function ReviewChangesDialog({
  before,
  after,
  open,
  onOpenChange,
  onConfirm,
  restartNote,
}: {
  before: unknown;
  after: unknown;
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** Applies the changes; the dialog closes first. */
  onConfirm: () => void;
  /** What saving does to the running engine. */
  restartNote: string;
}) {
  const changes = diffConfig(before, after);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Review changes</DialogTitle>
          <DialogDescription>
            {changes.length === 0
              ? "No changes."
              : `${changes.length} field${changes.length === 1 ? "" : "s"} will change.`}
          </DialogDescription>
        </DialogHeader>

        {changes.length > 0 && (
          <ScrollArea className="max-h-72">
            <div className="pr-3">
              {changes.map((change) => (
                <ChangeRow key={change.path} change={change} />
              ))}
            </div>
          </ScrollArea>
        )}

        <p className="text-xs text-muted-foreground">{restartNote}</p>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            onClick={() => {
              onOpenChange(false);
              onConfirm();
            }}
          >
            Save changes
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
