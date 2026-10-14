import { useEffect, useId, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, RotateCcw } from "lucide-react";
import { toast } from "sonner";
import type { NsFileRevisionDto } from "@/api/generated/model";
import { getWorkflowDownloadNamespaceFileUrl, useWorkflowDownloadNamespaceFile, useWorkflowNamespaceFileRevisions } from "@/api/generated/workflow/workflow";
import { rawBlob } from "@/api/mutator";
import MonacoEditor from "@/components/monaco/MonacoEditor";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { EmptyState } from "@/components/explore/EmptyState";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { baseName, classifyBytes, fileKindFor, imageMime, languageFor } from "./nsFilePaths";
import { FILES_MANAGE_REASON, fileErrorMessage, isFileChanged, readRevision, serverSentence, writeFile } from "./nsFilesApi";

/**
 * The revision a restore must find: the latest one when it is live, 0 when
 * the path's latest revision is a delete (W7d Task 2: a deleted path is at
 * revision 0, and `if_revision=0` writes it again).
 */
export function currentRevision(revisions: NsFileRevisionDto[]): number {
  const latest = [...revisions].sort((a, b) => b.revision - a.revision)[0];
  return latest && !latest.deleted ? latest.revision : 0;
}

/**
 * A FILE'S REVISIONS (W7d spec §4.3, W9b's `GET …/files/revisions`): newest
 * first, deleted ones flagged. "View" reads one (`?revision=N`); "Restore"
 * writes its bytes as a new revision with `if_revision` = the revision
 * listed as current, so a write made since the list was read is not
 * silently replaced: its 409 `file_changed` asks whether to list the
 * revisions again or restore anyway (without the check). There are no
 * unsaved edits here to discard, so W7a's stale-save dialog, whose Reload
 * asks about discarding edits, is not the right question.
 */
export function RevisionsDrawer({
  ws,
  namespace,
  path,
  readOnly,
  readOnlyReason = FILES_MANAGE_REASON,
  onRestored,
  onClose,
}: {
  ws: string;
  namespace: string;
  path: string;
  readOnly: boolean;
  readOnlyReason?: string;
  /** A restore wrote a new revision of `path`: open copies of it may read it again. */
  onRestored?: (path: string) => void;
  onClose: () => void;
}) {
  const name = baseName(path);
  const queryClient = useQueryClient();
  const query = useWorkflowNamespaceFileRevisions(ws, namespace, { path }, { query: { queryKey: workflowsKeys.nsFileRevisions(ws, namespace, path) } });
  const revisions = query.data?.status === 200 ? [...query.data.data].sort((a, b) => b.revision - a.revision) : undefined;
  const current = revisions ? currentRevision(revisions) : 0;
  const [viewing, setViewing] = useState<number | null>(null);
  const [restoring, setRestoring] = useState<number | null>(null);
  const [conflict, setConflict] = useState<{ revision: number; message: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reasonId = useId();

  const restored = async (revision: number) => {
    toast.success(`Restored revision ${revision} of ${name} as a new revision.`);
    await queryClient.invalidateQueries({ queryKey: workflowsKeys.nsFiles(ws, namespace) });
    onRestored?.(path);
  };

  const restore = async (revision: number, ifRevision: number | undefined) => {
    setRestoring(revision);
    setError(null);
    try {
      const bytes = await readRevision(ws, namespace, path, revision);
      await writeFile(ws, namespace, path, new Blob([bytes as BlobPart]), ifRevision);
      setConflict(null);
      await restored(revision);
    } catch (e) {
      setConflict(null);
      if (ifRevision !== undefined && isFileChanged(e)) setConflict({ revision, message: e.message });
      else setError(fileErrorMessage(e, `Revision ${revision} was not restored`));
    } finally {
      setRestoring(null);
    }
  };

  const body = (() => {
    if (query.isError && !revisions) {
      return (
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{fileErrorMessage(query.error, "The revisions could not be listed")}</span>
            <Button type="button" size="sm" variant="outline" onClick={() => void query.refetch()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      );
    }
    if (!revisions) return <ListSkeleton rows={3} cols={2} />;
    if (revisions.length === 0) return <EmptyState title="No revisions" description={`${name} has no revisions to show.`} />;
    return (
      <ul aria-label={`Revisions of ${name}`} className="divide-y rounded-md border">
        {revisions.map((r) => {
          const isCurrent = r.revision === current;
          const why = readOnly ? readOnlyReason : isCurrent ? "This is the current revision" : null;
          return (
            <li key={r.revision} className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm">
              <div className="min-w-0 flex-1">
                <p className="font-medium tabular-nums">
                  Revision {r.revision}
                  {isCurrent ? <span className="ml-2 text-xs font-normal text-muted-foreground">Current</span> : null}
                  {r.deleted ? <span className="ml-2 text-xs font-normal text-muted-foreground">Deleted</span> : null}
                </p>
                <p className="text-xs text-muted-foreground tabular-nums">
                  {new Date(r.created).toLocaleString()} · {r.size === 1 ? "1 byte" : `${r.size} bytes`}
                </p>
                {why ? (
                  <p id={`${reasonId}-${r.revision}`} className="text-xs text-muted-foreground">
                    <span aria-hidden>Restore: </span>
                    {why}
                  </p>
                ) : null}
              </div>
              <Button
                type="button"
                size="sm"
                variant={viewing === r.revision ? "secondary" : "outline"}
                aria-label={`View revision ${r.revision}`}
                aria-pressed={viewing === r.revision}
                onClick={() => setViewing(viewing === r.revision ? null : r.revision)}
              >
                View
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                aria-label={`Restore revision ${r.revision}`}
                disabled={!!why || restoring !== null}
                aria-describedby={why ? `${reasonId}-${r.revision}` : undefined}
                onClick={() => void restore(r.revision, current)}
              >
                {restoring === r.revision ? <Loader2 aria-hidden className="animate-spin" /> : <RotateCcw aria-hidden />}
                Restore
              </Button>
            </li>
          );
        })}
      </ul>
    );
  })();

  return (
    <Sheet open onOpenChange={(o) => (o ? undefined : onClose())}>
      <SheetContent className="flex w-full flex-col gap-4 overflow-y-auto sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>Revisions of {name}</SheetTitle>
          <SheetDescription>
            Every saved revision of {path}, newest first. Restoring one writes its content as a new revision; nothing is lost.
          </SheetDescription>
        </SheetHeader>
        {error ? (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        {body}
        {viewing !== null ? <RevisionContent ws={ws} namespace={namespace} path={path} revision={viewing} /> : null}
        <AlertDialog open={conflict !== null} onOpenChange={(o) => (o || restoring !== null ? undefined : setConflict(null))}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{name} changed since its revisions were listed</AlertDialogTitle>
              <AlertDialogDescription>
                {serverSentence(conflict?.message ?? "")} List the revisions again to see what was written, or restore revision{" "}
                {conflict?.revision} anyway as a new revision.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={restoring !== null}>Cancel</AlertDialogCancel>
              <Button
                type="button"
                variant="outline"
                disabled={restoring !== null}
                onClick={() => {
                  setConflict(null);
                  void query.refetch();
                }}
              >
                List again
              </Button>
              <Button type="button" variant="destructive" disabled={restoring !== null} onClick={() => conflict && void restore(conflict.revision, undefined)}>
                {restoring !== null ? <Loader2 aria-hidden className="animate-spin" /> : null}
                Restore anyway
              </Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </SheetContent>
    </Sheet>
  );
}

/** One revision's content: read-only text, an image, or a download. */
function RevisionContent({ ws, namespace, path, revision }: { ws: string; namespace: string; path: string; revision: number }) {
  const name = baseName(path);
  const kind = fileKindFor(path);
  // The generated hook, asked for the bytes (`rawBlob`): a revision never changes, so it is cached as is.
  const query = useWorkflowDownloadNamespaceFile(
    ws,
    namespace,
    { path, revision },
    { query: { queryKey: workflowsKeys.nsFileRevision(ws, namespace, path, revision), enabled: kind !== "binary", staleTime: Infinity }, request: { rawBlob: true } },
  );
  const [shown, setShown] = useState<{ kind: "text"; text: string } | { kind: "image"; url: string | null } | { kind: "binary" } | null>(null);
  const blob = query.data ? rawBlob(query.data) : null;
  useEffect(() => {
    if (!blob) return;
    let url: string | null = null;
    let live = true;
    void blob.arrayBuffer().then((buf) => {
      if (!live) return;
      const bytes = new Uint8Array(buf);
      if (kind === "image") {
        url = typeof URL.createObjectURL === "function" ? URL.createObjectURL(new Blob([bytes as BlobPart], { type: imageMime(path) })) : null;
        setShown({ kind: "image", url });
      } else setShown(classifyBytes(bytes));
    });
    return () => {
      live = false;
      if (url && typeof URL.revokeObjectURL === "function") URL.revokeObjectURL(url);
    };
  }, [blob, kind, path]);

  const download = getWorkflowDownloadNamespaceFileUrl(ws, namespace, { path, revision });
  const content = (() => {
    if (query.isError) return <p role="alert" className="text-sm text-destructive">{fileErrorMessage(query.error, `Revision ${revision} could not be read`)}</p>;
    if (kind !== "binary" && !shown) return <Skeleton className="h-40 w-full" />;
    if (shown?.kind === "text") {
      return (
        <div className="h-64 overflow-hidden rounded-md border">
          <MonacoEditor
            height="100%"
            path={`egret-nsfile-revision-${revision}/${name}`}
            language={languageFor(path)}
            defaultValue={shown.text}
            options={{ ariaLabel: `Revision ${revision} of ${name}`, readOnly: true, minimap: { enabled: false }, scrollBeyondLastLine: false, fontSize: 13 }}
          />
        </div>
      );
    }
    if (shown?.kind === "image" && shown.url) return <img src={shown.url} alt={`Revision ${revision} of ${name}`} className="max-h-64 max-w-full object-contain" />;
    return (
      <p className="text-sm text-muted-foreground">
        Revision {revision} is not shown here.{" "}
        <a className="underline underline-offset-2 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none" href={download} download={name}>
          Download revision {revision}
        </a>{" "}
        to open it.
      </p>
    );
  })();
  return (
    <section aria-label={`Revision ${revision}`} className="flex flex-col gap-2">
      <h3 className="text-sm font-semibold">Revision {revision}</h3>
      {content}
    </section>
  );
}
