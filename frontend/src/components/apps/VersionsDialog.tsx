import { useState } from "react";
import { History, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { type AppVersionSummary, useAppVersions, useRestoreVersion } from "@/api/apps";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button, buttonVariants } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { RelativeTime } from "@/components/explore/RelativeTime";

/** A workspace member id, shortened the same way the backend shortens ids for display elsewhere
 * (there is no name lookup at this layer — a full id is shown on hover via the title attribute). */
function shortId(id: string | null | undefined): string | null {
  return id ? id.slice(0, 8) : null;
}

export default function VersionsDialog({
  ws,
  appId,
  open,
  onOpenChange,
}: {
  ws: string;
  appId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { data: versions } = useAppVersions(ws, appId);
  const restore = useRestoreVersion(ws, appId);
  const [confirming, setConfirming] = useState<AppVersionSummary | null>(null);

  async function doRestore(v: AppVersionSummary) {
    try {
      const restored = await restore.mutateAsync({ versionId: v.id });
      setConfirming(null);
      toast.success(`Published version ${restored.version_no}`);
    } catch (e) {
      toast.error(String((e as Error)?.message ?? e));
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle>Versions</DialogTitle>
            <DialogDescription>
              Every version ever published, newest first. Restoring one publishes it again as a new
              version — it never moves or removes anything.
            </DialogDescription>
          </DialogHeader>
          {!versions || versions.length === 0 ? (
            <p className="text-sm text-muted-foreground">Nothing has been published yet.</p>
          ) : (
            <ul className="max-h-96 divide-y overflow-y-auto">
              {versions.map((v) => (
                <li key={v.id} className="flex items-center justify-between gap-3 py-2 text-sm">
                  <div className="min-w-0">
                    <div className="font-medium">v{v.version_no}</div>
                    <div className="text-xs text-muted-foreground">
                      <RelativeTime iso={v.published_at} />
                      {shortId(v.published_by) && <> &middot; by {shortId(v.published_by)}</>}
                      {v.restored_from && <> &middot; restored from an earlier version</>}
                    </div>
                  </div>
                  <Button size="sm" variant="outline" onClick={() => setConfirming(v)}>
                    <History className="h-4 w-4" />
                    Restore
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </DialogContent>
      </Dialog>

      <AlertDialog open={confirming != null} onOpenChange={(o) => !o && setConfirming(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Restore v{confirming?.version_no}?</AlertDialogTitle>
            <AlertDialogDescription>
              This publishes v{confirming?.version_no}&apos;s source again as a brand-new version. It does
              not move or remove any existing version — the app's history only ever grows.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "default" })}
              disabled={restore.isPending}
              onClick={() => confirming && doRestore(confirming)}
            >
              {restore.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Restore this version
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
