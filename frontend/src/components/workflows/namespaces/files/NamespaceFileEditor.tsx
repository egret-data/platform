import { useEffect, useId, useRef } from "react";
import { Download, History, Loader2, Save } from "lucide-react";
import { getWorkflowDownloadNamespaceFileUrl } from "@/api/generated/workflow/workflow";
import MonacoEditor from "@/components/monaco/MonacoEditor";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { StaleSaveDialog } from "@/components/workflows/StaleSaveDialog";
import type { Enablement } from "@/components/workflows/draftState";
import { baseName } from "./nsFilePaths";
import { FILES_MANAGE_REASON, serverSentence } from "./nsFilesApi";
import { MAX_OPEN_BYTES, useNamespaceFile, type NamespaceFile } from "./useNamespaceFile";

export interface NamespaceFileEditorProps {
  ws: string;
  namespace: string;
  /** The file's stored path (a leading `/`). */
  path: string;
  /** Saving is off, with `readOnlyReason` shown (W7d spec §4.5). */
  readOnly: boolean;
  readOnlyReason?: string;
  /** Tells the host whether the text has unsaved edits (a tab's dot, "Save all"). */
  onDirtyChange?: (dirty: boolean) => void;
  /** Show the file's revisions (the host's drawer); no button without it. */
  onOpenRevisions?: () => void;
  /**
   * Changing it asks the file to read its latest revision again, unless it
   * has unsaved edits (a restore from the revisions drawer just wrote one).
   */
  reloadSignal?: number;
}

/**
 * ONE OPEN NAMESPACE FILE (W7d spec §4.3): {@link useNamespaceFile} and
 * {@link NamespaceFileView}. The namespace page renders one per open tab;
 * W7d Task 7 renders one per `file:<path>` tab, or lifts the hook and
 * renders the view itself.
 */
export function NamespaceFileEditor({ ws, namespace, path, readOnly, readOnlyReason, onDirtyChange, onOpenRevisions, reloadSignal }: NamespaceFileEditorProps) {
  const file = useNamespaceFile(ws, namespace, path);
  const onDirty = useRef(onDirtyChange);
  useEffect(() => {
    onDirty.current = onDirtyChange;
  });
  useEffect(() => onDirty.current?.(file.dirty), [file.dirty]);
  // A pane that goes away holds no unsaved edits any more.
  useEffect(() => () => onDirty.current?.(false), []);

  const seen = useRef(reloadSignal);
  const { dirty, reload } = file;
  useEffect(() => {
    if (reloadSignal === seen.current) return;
    seen.current = reloadSignal;
    if (!dirty) void reload();
  }, [reloadSignal, dirty, reload]);

  return (
    <NamespaceFileView
      ws={ws}
      namespace={namespace}
      path={path}
      file={file}
      readOnly={readOnly}
      readOnlyReason={readOnlyReason}
      onOpenRevisions={onOpenRevisions}
    />
  );
}

const sizeText = (n: number) =>
  n < 1024 ? (n === 1 ? "1 byte" : `${n} bytes`) : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;

/**
 * What a {@link NamespaceFile} shows: its path and revision, Save (with
 * Ctrl/Cmd+S), Revisions and Download, then the text in Monaco (language from
 * the extension), an image's preview, or a binary's download offer. A save
 * refused with 409 `file_changed` opens W7a's stale-save dialog with the
 * server's reason, which says whether the file changed or was deleted:
 * Reload reads the latest revision (a deleted file then says it is gone),
 * Overwrite writes the text without `if_revision` (a deleted file is written
 * again).
 */
export function NamespaceFileView({
  ws,
  namespace,
  path,
  file,
  readOnly,
  readOnlyReason = FILES_MANAGE_REASON,
  onOpenRevisions,
  staleDialog = true,
}: {
  ws: string;
  namespace: string;
  path: string;
  file: NamespaceFile;
  readOnly: boolean;
  readOnlyReason?: string;
  onOpenRevisions?: () => void;
  /**
   * `false` when the host asks the 409 question itself ({@link FileConflictDialog}), as
   * the flow editor does: its file's state outlives this view, and "Save all files"
   * saves files whose view is not mounted.
   */
  staleDialog?: boolean;
}) {
  const name = baseName(path);
  const reasonId = useId();
  const modelId = useId().replace(/[^a-zA-Z0-9]/g, "");
  const saveRef = useRef(file.save);

  const canSave: Enablement = readOnly
    ? { ok: false, reason: readOnlyReason }
    : file.file?.kind !== "text"
      ? { ok: false, reason: "Only a text file is edited here" }
      : file.saving
        ? { ok: false, reason: "Saving…" }
        : !file.dirty
          ? { ok: false, reason: "No unsaved changes" }
          : { ok: true };

  // Ctrl/Cmd+S anywhere in the pane, the editor included: Monaco binds no command to it, so the
  // key comes up to the pane, which keeps it. A listener rather than a prop: the pane is not itself a control.
  const pane = useRef<HTMLDivElement>(null);
  const canSaveRef = useRef(canSave.ok);
  useEffect(() => {
    saveRef.current = file.save;
    canSaveRef.current = canSave.ok;
  });
  useEffect(() => {
    const el = pane.current;
    if (!el) return;
    const onKeyDown = (e: globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        // The key saves this file only: a page-wide Ctrl/Cmd+S (the flow editor's) must not see it.
        e.stopPropagation();
        if (canSaveRef.current) void saveRef.current();
      }
    };
    el.addEventListener("keydown", onKeyDown);
    return () => el.removeEventListener("keydown", onKeyDown);
  }, []);

  const revision = file.file?.revision;
  const download = getWorkflowDownloadNamespaceFileUrl(ws, namespace, revision ? { path, revision } : { path });

  return (
    <div ref={pane} className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <div className="min-w-0 flex-1">
          <p className="truncate font-mono text-xs" title={path}>
            {path}
          </p>
          {file.file ? (
            <p className="text-xs text-muted-foreground tabular-nums">
              Revision {file.file.revision} · {sizeText(file.file.size)}
            </p>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {onOpenRevisions ? (
            <Button type="button" size="sm" variant="outline" onClick={onOpenRevisions}>
              <History aria-hidden />
              Revisions
            </Button>
          ) : null}
          <Button asChild size="sm" variant="outline">
            <a href={download} download={name}>
              <Download aria-hidden />
              Download
            </a>
          </Button>
          <ReasonButton enablement={canSave} reasonId={reasonId} onClick={() => void file.save()}>
            {file.saving ? <Loader2 aria-hidden className="animate-spin" /> : <Save aria-hidden />}
            Save
          </ReasonButton>
        </div>
        <div className="basis-full">
          <DisabledReasons actions={[{ id: reasonId, label: "Save", enablement: canSave.ok || file.saving ? { ok: true } : canSave }]} />
        </div>
      </div>
      {file.saveError ? (
        <Alert variant="destructive" className="m-2 w-auto">
          <AlertDescription>{file.saveError}</AlertDescription>
        </Alert>
      ) : null}
      <div className="min-h-0 flex-1">
        <FileBody file={file} name={name} download={download} modelPath={`egret-nsfile-${modelId}-${file.loadId}/${name}`} readOnly={readOnly} />
      </div>
      {staleDialog ? <FileConflictDialog path={path} file={file} /> : null}
    </div>
  );
}

/**
 * W7a's stale-save dialog for a file whose save was refused with 409
 * `file_changed`: open while `file.conflict` is set. Reload reads the latest
 * revision; Overwrite writes the text without `if_revision`.
 */
export function FileConflictDialog({ path, file }: { path: string; file: NamespaceFile }) {
  const name = baseName(path);
  return (
    <StaleSaveDialog
      open={file.conflict !== null}
      onOpenChange={(o) => (o ? undefined : file.dismissConflict())}
      title={`${name} changed since you opened it`}
      description={
        <>
          {serverSentence(file.conflict ?? "")} Reload the latest revision, or overwrite it with your text.
        </>
      }
      reloadDescription="Reloading replaces the text in the editor with the latest saved revision. Your unsaved edits are lost."
      onReload={file.reload}
      onOverwrite={file.overwrite}
    />
  );
}

function FileBody({ file, name, download, modelPath, readOnly }: { file: NamespaceFile; name: string; download: string; modelPath: string; readOnly: boolean }) {
  // Error first (FE-STATE-1).
  if (file.status === "error") {
    return (
      <div className="p-3">
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{file.error}</span>
            <Button type="button" size="sm" variant="outline" onClick={() => void file.reload()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    );
  }
  if (file.status === "loading" || !file.file) {
    return (
      <div className="space-y-2 p-3" data-testid="file-loading">
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-3/5" />
      </div>
    );
  }
  const loaded = file.file;
  if (loaded.kind === "text") {
    return (
      <MonacoEditor
        key={file.loadId}
        height="100%"
        path={modelPath}
        language={loaded.language}
        defaultValue={file.text}
        onChange={(v) => file.setText(v ?? "")}
        options={{
          ariaLabel: `Contents of ${name}`,
          readOnly,
          automaticLayout: true,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          fontSize: 13,
          tabSize: 2,
        }}
      />
    );
  }
  if (loaded.kind === "image" && loaded.url) {
    return (
      <div className="flex h-full items-center justify-center overflow-auto bg-muted/30 p-4">
        <img src={loaded.url} alt={`Preview of ${name}`} className="max-h-full max-w-full object-contain" />
      </div>
    );
  }
  const why =
    loaded.kind === "image"
      ? `${name} cannot be previewed in this browser.`
      : loaded.kind === "binary" && loaded.tooLarge
        ? `${name} is larger than ${MAX_OPEN_BYTES / 1024 / 1024} MB, so it is not opened here.`
        : `${name} is a binary file, so it cannot be shown here.`;
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm text-muted-foreground">
      <p>{why} Download it to open it on your computer.</p>
      <Button asChild size="sm" variant="outline">
        <a href={download} download={name}>
          <Download aria-hidden />
          Download {name}
        </a>
      </Button>
    </div>
  );
}
