import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FilePlus, FileText, FolderPlus, FolderUp, Loader2, PackageOpen, Upload, X } from "lucide-react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { getWorkflowExportNamespaceFilesUrl, useWorkflowSearchNamespaceFiles } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ReasonButton } from "@/components/workflows/ReasonButton";
import type { Enablement } from "@/components/workflows/draftState";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { cn } from "@/lib/utils";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { DeleteDialog, OneFieldDialog, OverwriteDialog } from "./FileDialogs";
import { FileTree, useTreeRows, type TreeAction, type TreeEntry } from "./FileTree";
import { NamespaceFileEditor } from "./NamespaceFileEditor";
import { RevisionsDrawer } from "./RevisionsDrawer";
import { baseName, checkName, isDirPath, isUnder, joinPath, movedPath, parentDir, renamedPath } from "./nsFilePaths";
import {
  FILES_MANAGE_REASON,
  countFilesUnder,
  createDirectory,
  deletePath,
  fileErrorMessage,
  isFileChanged,
  movePath,
  serverSentence,
  writeFile,
} from "./nsFilesApi";
import { readOpenTabs, writeOpenTabs, type OpenTabs } from "./openTabs";

export interface NamespaceFilesProps {
  ws: string;
  namespace: string;
  /** Every write is off, each saying `readOnlyReason` (W7d spec §4.5). The server still checks. */
  readOnly: boolean;
  readOnlyReason?: string;
  /**
   * Open a file somewhere else: the flow editor adds a `file:<path>` tab
   * (W7d Task 7). Without it the files open in tabs beside the tree,
   * remembered per namespace (the namespace page).
   */
  onOpenFile?: (path: string) => void;
  /** A move or rename took `from` (a file, or a folder and all it holds) to `to`. */
  onPathMoved?: (from: string, to: string) => void;
  /** A delete removed `path` (a file, or a folder and all it holds). */
  onPathDeleted?: (path: string) => void;
  /**
   * A restore from the tree's Revisions wrote a new revision of `path`. The
   * host's open copy reads it when it has no unsaved edits (the namespace
   * page does this for its own tabs).
   */
  onPathRestored?: (path: string) => void;
  /**
   * With `onOpenFile`: the host's open files with unsaved edits. Moving or
   * renaming them is refused until they are saved or closed; a delete says
   * their edits are lost. The namespace page tracks its own.
   */
  dirtyPaths?: readonly string[];
}

/**
 * THE NAMESPACE FILES EXPLORER (W7d spec §4.3), one component for the
 * namespace page's Files tab and the flow editor's Files tab. Remounted per
 * workspace and namespace, so no state crosses from one to another.
 */
export function NamespaceFiles(props: NamespaceFilesProps) {
  return <FilesExplorer key={`${props.ws}\u0000${props.namespace}`} {...props} />;
}

type Dialog =
  | { kind: "new-file" | "new-folder"; dir: string }
  | { kind: "rename" | "move"; entry: TreeEntry }
  | { kind: "delete"; entry: TreeEntry; count: number | null; countError: string | null; busy: boolean; error: string | null }
  | null;

interface Picked {
  file: File;
  /** The path below the import folder: the file's name, or its folder-relative path. */
  rel: string;
}

/** An import of more files than this asks first: each file is its own upload. */
export const IMPORT_CONFIRM_ABOVE = 100;

/** Why the writes are off while an import runs. */
const IMPORT_RUNNING = "Wait for the import to finish.";

const filesCount = (n: number) => (n === 1 ? "1 file" : `${n} files`);

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

function FilesExplorer({
  ws,
  namespace,
  readOnly,
  readOnlyReason = FILES_MANAGE_REASON,
  onOpenFile,
  onPathMoved,
  onPathDeleted,
  onPathRestored,
  dirtyPaths: hostDirty,
}: NamespaceFilesProps) {
  const queryClient = useQueryClient();
  const pageMode = !onOpenFile;
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const { root, rows, folders } = useTreeRows(ws, namespace, expanded);

  const [search, setSearch] = useState("");
  const q = useDebounced(search.trim(), 250);
  const searching = search.trim() !== "";
  const searchQuery = useWorkflowSearchNamespaceFiles(
    ws,
    namespace,
    { q },
    { query: { queryKey: workflowsKeys.nsFileSearch(ws, namespace, q), enabled: q !== "" } },
  );

  // The namespace page's open tabs; `dirty` is each tab's report.
  const [tabs, setTabs] = useState<OpenTabs>(() => (pageMode ? readOpenTabs(ws, namespace) : { tabs: [], active: null }));
  useEffect(() => {
    if (pageMode) writeOpenTabs(ws, namespace, tabs);
  }, [pageMode, ws, namespace, tabs]);
  const [dirty, setDirty] = useState<Record<string, boolean>>({});
  const dirtyPaths = pageMode ? Object.keys(dirty).filter((p) => dirty[p]) : (hostDirty ?? []);
  useUnsavedChangesWarning(pageMode && dirtyPaths.length > 0);
  const [reloads, setReloads] = useState<Record<string, number>>({});
  const [revisionsFor, setRevisionsFor] = useState<string | null>(null);

  const [dialog, setDialog] = useState<Dialog>(null);
  const [importing, setImporting] = useState<{ done: number; total: number } | null>(null);
  const [importErrors, setImportErrors] = useState<string[]>([]);
  const [confirmImport, setConfirmImport] = useState<Picked[] | null>(null);
  const [overwrite, setOverwrite] = useState<{ items: { path: string; file: File }[]; busy: boolean } | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    // `webkitdirectory` (every current browser) makes the picker choose a folder and give each file its
    // path below it (`webkitRelativePath`). React types do not know the attribute.
    folderInput.current?.setAttribute("webkitdirectory", "");
  }, []);
  const noteId = useId();

  // While an import runs every write waits for it: a second import, a new
  // file or a move would race the uploads into the same folders.
  const writeReason = readOnly ? readOnlyReason : importing ? IMPORT_RUNNING : null;
  const write: Enablement = writeReason ? { ok: false, reason: writeReason } : { ok: true };
  /** Where New file, New folder and the imports put things: the selected folder, or the selected file's. */
  const targetDir = selected ? (isDirPath(selected) ? selected : parentDir(selected)) : "/";

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: workflowsKeys.namespace(ws, namespace) });
    await queryClient.invalidateQueries({ queryKey: workflowsKeys.namespaces(ws) });
  };
  const expandTo = (path: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      for (let d = isDirPath(path) ? path : parentDir(path); d !== "/"; d = parentDir(d)) next.add(d);
      return next;
    });
  const toggle = (dir: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(dir)) next.delete(dir);
      else next.add(dir);
      return next;
    });

  const openFile = (path: string) => {
    setSelected(path);
    if (onOpenFile) onOpenFile(path);
    else setTabs((t) => ({ tabs: t.tabs.includes(path) ? t.tabs : [...t.tabs, path], active: path }));
  };

  const dirtyUnder = (path: string) => dirtyPaths.filter((p) => p === path || (isDirPath(path) && isUnder(p, path)));
  const blockedByEdits = (path: string) => {
    const held = dirtyUnder(path);
    if (held.length === 0) return null;
    return `${held.map(baseName).join(", ")} ${held.length === 1 ? "has" : "have"} unsaved edits. Save or close ${held.length === 1 ? "it" : "them"} first.`;
  };

  const afterMove = async (from: string, to: string) => {
    const remap = (p: string) => (p === from || (isDirPath(from) && isUnder(p, from)) ? to + p.slice(from.length) : p);
    setExpanded((s) => new Set([...s].map(remap)));
    setSelected((s) => (s ? remap(s) : s));
    setTabs((t) => ({ tabs: [...new Set(t.tabs.map(remap))], active: t.active ? remap(t.active) : null }));
    onPathMoved?.(from, to);
    expandTo(to);
    toast.success(`Moved ${from} to ${to}.`);
    await refresh();
  };

  const move = async (from: string, to: string): Promise<string | null> => {
    const held = blockedByEdits(from);
    if (held) return held;
    try {
      await movePath(ws, namespace, from, to);
    } catch (e) {
      return fileErrorMessage(e, `${baseName(from)} was not moved`);
    }
    await afterMove(from, to);
    return null;
  };

  const onDrop = async (from: string, toDir: string) => {
    const failure = await move(from, movedPath(from, toDir));
    if (failure) toast.error(failure);
  };

  const closeTabs = (under: string) => {
    const gone = (p: string) => p === under || (isDirPath(under) && isUnder(p, under));
    setTabs((t) => {
      const kept = t.tabs.filter((p) => !gone(p));
      return { tabs: kept, active: t.active && !gone(t.active) ? t.active : (kept[kept.length - 1] ?? null) };
    });
    setDirty((d) => Object.fromEntries(Object.entries(d).filter(([p]) => !gone(p))));
  };

  const startDelete = (entry: TreeEntry) => {
    setDialog({ kind: "delete", entry, count: entry.isDir ? null : 1, countError: null, busy: false, error: null });
    if (!entry.isDir) return;
    countFilesUnder(ws, namespace, entry.path).then(
      (count) => setDialog((d) => (d?.kind === "delete" && d.entry.path === entry.path ? { ...d, count } : d)),
      (e: unknown) =>
        setDialog((d) => (d?.kind === "delete" && d.entry.path === entry.path ? { ...d, countError: serverSentence(fileErrorMessage(e, "the search failed")) } : d)),
    );
  };

  const confirmDelete = async () => {
    if (dialog?.kind !== "delete") return;
    const { entry } = dialog;
    setDialog({ ...dialog, busy: true, error: null });
    try {
      await deletePath(ws, namespace, entry.path);
    } catch (e) {
      setDialog((d) => (d?.kind === "delete" ? { ...d, busy: false, error: fileErrorMessage(e, `${entry.name} was not deleted`) } : d));
      return;
    }
    setDialog(null);
    closeTabs(entry.path);
    setExpanded((s) => new Set([...s].filter((p) => !isUnder(p, entry.path))));
    setSelected((s) => (s && (s === entry.path || (entry.isDir && isUnder(s, entry.path))) ? null : s));
    onPathDeleted?.(entry.path);
    toast.success(entry.isDir ? `Deleted the folder ${entry.path}.` : `Deleted ${entry.path}.`);
    await refresh();
  };

  const copyPath = async (path: string) => {
    try {
      if (!navigator.clipboard) throw new Error("no clipboard");
      await navigator.clipboard.writeText(path);
      toast.success(`Copied ${path}.`);
    } catch {
      toast.error(`The path could not be copied: the browser did not allow it. Copy it by hand: ${path}`);
    }
  };

  const onAction = (action: TreeAction, entry: TreeEntry) => {
    setSelected(entry.path);
    if (action === "revisions") setRevisionsFor(entry.path);
    else if (action === "copy-path") void copyPath(entry.path);
    else if (writeReason) return;
    else if (action === "delete") startDelete(entry);
    else setDialog({ kind: action, entry });
  };

  const importFiles = async (picked: Picked[]) => {
    if (picked.length === 0 || importing) return;
    const dir = targetDir;
    setImportErrors([]);
    setImporting({ done: 0, total: picked.length });
    const existing: { path: string; file: File }[] = [];
    const failed: string[] = [];
    let written = 0;
    for (const [i, { file, rel }] of picked.entries()) {
      const path = joinPath(dir, rel);
      try {
        // `if_revision=0`: an import never replaces a file without asking.
        await writeFile(ws, namespace, path, file, 0);
        written += 1;
      } catch (e) {
        if (isFileChanged(e)) existing.push({ path, file });
        else failed.push(fileErrorMessage(e, `${path} was not imported`));
      }
      setImporting({ done: i + 1, total: picked.length });
    }
    setImporting(null);
    if (written > 0) toast.success(`Imported ${filesCount(written)} into ${dir}.`);
    setImportErrors(failed);
    if (existing.length > 0) setOverwrite({ items: existing, busy: false });
    if (dir !== "/") expandTo(dir);
    await refresh();
  };

  const confirmOverwrite = async () => {
    if (!overwrite) return;
    setOverwrite({ ...overwrite, busy: true });
    const failed: string[] = [];
    let written = 0;
    for (const { path, file } of overwrite.items) {
      try {
        await writeFile(ws, namespace, path, file);
        written += 1;
      } catch (e) {
        failed.push(fileErrorMessage(e, `${path} was not overwritten`));
      }
    }
    setOverwrite(null);
    if (written > 0) toast.success(`Overwrote ${filesCount(written)}.`);
    setImportErrors((errors) => [...errors, ...failed]);
    for (const { path } of overwrite.items) setReloads((r) => ({ ...r, [path]: (r[path] ?? 0) + 1 }));
    await refresh();
  };

  const pick = (input: HTMLInputElement, relative: boolean) => {
    const files = [...(input.files ?? [])];
    input.value = "";
    const picked = files.map((file) => ({ file, rel: (relative && file.webkitRelativePath) || file.name }));
    if (picked.length > IMPORT_CONFIRM_ABOVE) setConfirmImport(picked);
    else void importFiles(picked);
  };

  const searchBody = () => {
    if (searchQuery.isError) return <p role="alert" className="text-sm text-destructive">{fileErrorMessage(searchQuery.error, "The search failed")}</p>;
    if (q !== search.trim() || !searchQuery.data) return <ListSkeleton rows={3} cols={1} />;
    const found = searchQuery.data.status === 200 ? searchQuery.data.data : [];
    if (found.length === 0) {
      return (
        <div className="flex flex-col items-start gap-2">
          <p className="text-sm text-muted-foreground">No files match “{search.trim()}”.</p>
          <Button type="button" size="sm" variant="outline" onClick={() => setSearch("")}>
            Clear search
          </Button>
        </div>
      );
    }
    return (
      <ul aria-label="Search results" className="flex-1 overflow-auto rounded-md border p-1 text-sm">
        {found.map((p) => (
          <li key={p}>
            <button
              type="button"
              className="flex w-full flex-col items-start rounded-sm px-2 py-1 text-left hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              onClick={() => {
                expandTo(p);
                openFile(p);
              }}
            >
              <span className="flex items-center gap-1">
                <FileText aria-hidden className="size-4 text-muted-foreground" />
                {baseName(p)}
              </span>
              <span className="font-mono text-xs text-muted-foreground">{p}</span>
            </button>
          </li>
        ))}
      </ul>
    );
  };

  const treeBody = () => {
    // Error first (FE-STATE-1). A refused namespace (`a..b`, a `_files` segment) is W9b's 400.
    if (root.isError && !root.data) {
      if (root.error instanceof ApiError && root.error.status === 400) {
        return (
          <Alert variant="destructive">
            <AlertDescription>
              The namespace {namespace} cannot hold files: {serverSentence(root.error.message)} Files live in a namespace whose name has no
              empty segment and no _files segment; open another namespace.
            </AlertDescription>
          </Alert>
        );
      }
      return (
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{fileErrorMessage(root.error, "The files could not be listed")}</span>
            <Button type="button" size="sm" variant="outline" onClick={() => void root.refetch()}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      );
    }
    if (!root.data) return <ListSkeleton rows={6} cols={1} />;
    if (searching) return searchBody();
    if (rows.length === 0) {
      return (
        <EmptyState
          variant="dashed"
          icon={FileText}
          title="No files yet"
          description="Files here are scripts and data this namespace's flows read, for example through namespaceFiles on a script task."
          action={
            <span className="flex flex-wrap justify-center gap-2">
              <ReasonButton enablement={write} reasonId={noteId} onClick={() => setDialog({ kind: "new-file", dir: "/" })}>
                <FilePlus aria-hidden />
                New file
              </ReasonButton>
              <ReasonButton enablement={write} reasonId={noteId} variant="outline" onClick={() => fileInput.current?.click()}>
                <Upload aria-hidden />
                Import files
              </ReasonButton>
            </span>
          }
        />
      );
    }
    return (
      <FileTree
        ws={ws}
        namespace={namespace}
        rows={rows}
        expanded={expanded}
        selected={selected}
        onSelect={setSelected}
        onToggle={toggle}
        onOpen={openFile}
        onAction={onAction}
        onDrop={writeReason ? undefined : (from, dir) => void onDrop(from, dir)}
        writeReason={writeReason}
      />
    );
  };

  const exportHref = getWorkflowExportNamespaceFilesUrl(ws, namespace);

  return (
    <div className="flex h-full min-h-[32rem] flex-col gap-3 lg:flex-row">
      <div className={cn("flex min-h-0 min-w-0 flex-col gap-2", pageMode ? "lg:w-80 lg:shrink-0" : "flex-1")}>
        <div className="flex flex-wrap gap-2">
          <ReasonButton enablement={write} reasonId={noteId} variant="outline" onClick={() => setDialog({ kind: "new-file", dir: targetDir })}>
            <FilePlus aria-hidden />
            New file
          </ReasonButton>
          <ReasonButton enablement={write} reasonId={noteId} variant="outline" onClick={() => setDialog({ kind: "new-folder", dir: targetDir })}>
            <FolderPlus aria-hidden />
            New folder
          </ReasonButton>
          <ReasonButton enablement={write} reasonId={noteId} variant="outline" onClick={() => fileInput.current?.click()}>
            <Upload aria-hidden />
            Import files
          </ReasonButton>
          <ReasonButton enablement={write} reasonId={noteId} variant="outline" onClick={() => folderInput.current?.click()}>
            <FolderUp aria-hidden />
            Import a folder
          </ReasonButton>
          <Button asChild size="sm" variant="outline">
            <a href={exportHref} download={`${namespace}_files.zip`}>
              <PackageOpen aria-hidden />
              Export as zip
            </a>
          </Button>
        </div>
        {writeReason ? (
          <p id={noteId} className="text-xs text-muted-foreground">
            {writeReason}
          </p>
        ) : (
          <p className="text-xs text-muted-foreground">
            New files and imports go into <span className="font-mono">{targetDir}</span>. A .zip is imported as one file.
          </p>
        )}
        <input ref={fileInput} type="file" multiple tabIndex={-1} className="sr-only" aria-label="Choose files to import" onChange={(e) => pick(e.currentTarget, false)} />
        <input ref={folderInput} type="file" multiple tabIndex={-1} className="sr-only" aria-label="Choose a folder to import" onChange={(e) => pick(e.currentTarget, true)} />
        <div className="grid gap-1">
          <Label htmlFor={`${noteId}-search`}>Search files</Label>
          <Input id={`${noteId}-search`} type="search" placeholder="Path contains…" value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
        {importing ? (
          <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground tabular-nums">
            <Loader2 aria-hidden className="size-4 animate-spin" />
            Importing {importing.done} of {filesCount(importing.total)}…
          </p>
        ) : null}
        {importErrors.length > 0 ? (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-col items-start gap-2">
              <span>{importErrors.length === 1 ? "1 file was not imported:" : `${importErrors.length} files were not imported:`}</span>
              <ul className="list-disc pl-4">
                {importErrors.map((m) => (
                  <li key={m}>{m}</li>
                ))}
              </ul>
              <Button type="button" size="sm" variant="outline" onClick={() => setImportErrors([])}>
                Dismiss
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}
        {treeBody()}
      </div>

      {pageMode ? (
        <OpenFiles
          ws={ws}
          namespace={namespace}
          tabs={tabs}
          setTabs={setTabs}
          dirty={dirty}
          setDirty={setDirty}
          reloads={reloads}
          readOnly={readOnly}
          readOnlyReason={readOnlyReason}
          onSelect={(p) => setSelected(p)}
          onOpenRevisions={setRevisionsFor}
        />
      ) : null}

      {dialog?.kind === "new-file" || dialog?.kind === "new-folder" ? (
        <OneFieldDialog
          title={dialog.kind === "new-file" ? "New file" : "New folder"}
          description={`Created in ${dialog.dir}.`}
          label="Name"
          initial=""
          help="Use / to create it inside new folders, for example scripts/load.py."
          submitLabel={dialog.kind === "new-file" ? "Create file" : "Create folder"}
          onClose={() => setDialog(null)}
          onSubmit={async (value) => {
            const invalid = checkName(value, { nested: true });
            if (invalid) return invalid;
            const isFolder = dialog.kind === "new-folder";
            const path = joinPath(dialog.dir, value.trim(), isFolder);
            try {
              // A new file must not replace one: `if_revision=0` (W7d spec §3.2).
              if (isFolder) await createDirectory(ws, namespace, path);
              else await writeFile(ws, namespace, path, new Blob([]), 0);
            } catch (e) {
              if (isFileChanged(e)) return `${serverSentence(e.message)} Choose another name.`;
              return fileErrorMessage(e, isFolder ? "The folder was not created" : "The file was not created");
            }
            setDialog(null);
            toast.success(`Created ${path}.`);
            expandTo(path);
            if (isFolder) setSelected(path);
            else openFile(path);
            await refresh();
            return null;
          }}
        />
      ) : null}
      {dialog?.kind === "rename" ? (
        <OneFieldDialog
          title={`Rename ${dialog.entry.name}`}
          description={`In ${parentDir(dialog.entry.path)}. Every revision keeps its number.`}
          label="New name"
          initial={dialog.entry.name}
          submitLabel="Rename"
          onClose={() => setDialog(null)}
          onSubmit={async (value) => {
            const invalid = checkName(value, { nested: false });
            if (invalid) return invalid;
            const to = renamedPath(dialog.entry.path, value.trim());
            if (to === dialog.entry.path) {
              setDialog(null);
              return null;
            }
            const failure = await move(dialog.entry.path, to);
            if (!failure) setDialog(null);
            return failure;
          }}
        />
      ) : null}
      {dialog?.kind === "move" ? (
        <OneFieldDialog
          title={`Move ${dialog.entry.name}`}
          description={`Now in ${parentDir(dialog.entry.path)}. Pick or type the folder to move it to; a folder that does not exist is created.`}
          label="Destination folder"
          initial={parentDir(dialog.entry.path)}
          suggestions={folders}
          help="A path from the root, for example /scripts/."
          submitLabel="Move"
          onClose={() => setDialog(null)}
          onSubmit={async (value) => {
            let dest = value.trim();
            if (!dest.startsWith("/")) dest = `/${dest}`;
            if (!dest.endsWith("/")) dest = `${dest}/`;
            if (dest !== "/" && checkName(dest, { nested: true })) return checkName(dest, { nested: true });
            const from = dialog.entry.path;
            if (dialog.entry.isDir && isUnder(dest, from)) return "A folder cannot move into itself or into a folder inside it.";
            if (parentDir(from) === dest) return `${dialog.entry.name} is already in ${dest}. Choose another folder.`;
            const failure = await move(from, movedPath(from, dest));
            if (!failure) setDialog(null);
            return failure;
          }}
        />
      ) : null}
      {dialog?.kind === "delete" ? (
        <DeleteDialog
          name={dialog.entry.name}
          isDir={dialog.entry.isDir}
          count={dialog.count}
          countError={dialog.countError}
          unsaved={dirtyUnder(dialog.entry.path).length}
          busy={dialog.busy}
          error={dialog.error}
          onConfirm={() => void confirmDelete()}
          onClose={() => setDialog(null)}
        />
      ) : null}
      <AlertDialog open={confirmImport !== null} onOpenChange={(o) => (o ? undefined : setConfirmImport(null))}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Import {filesCount(confirmImport?.length ?? 0)}?</AlertDialogTitle>
            <AlertDialogDescription>
              They go into <span className="font-mono">{targetDir}</span>, each uploaded on its own, so this takes a while. Files that exist
              already are not replaced without asking.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button
              type="button"
              onClick={() => {
                const picked = confirmImport ?? [];
                setConfirmImport(null);
                void importFiles(picked);
              }}
            >
              Import {filesCount(confirmImport?.length ?? 0)}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {overwrite ? (
        <OverwriteDialog paths={overwrite.items.map((i) => i.path)} busy={overwrite.busy} onConfirm={() => void confirmOverwrite()} onClose={() => setOverwrite(null)} />
      ) : null}
      {revisionsFor ? (
        <RevisionsDrawer
          ws={ws}
          namespace={namespace}
          path={revisionsFor}
          readOnly={readOnly}
          readOnlyReason={readOnlyReason}
          onRestored={(p) => {
            setReloads((r) => ({ ...r, [p]: (r[p] ?? 0) + 1 }));
            onPathRestored?.(p);
          }}
          onClose={() => setRevisionsFor(null)}
        />
      ) : null}
    </div>
  );
}

/**
 * The namespace page's open files: a tab per file (a dot and "unsaved" on
 * one with edits), its editor below. Every open file stays mounted, so
 * switching tabs keeps unsaved text; closing one with edits asks first.
 */
function OpenFiles({
  ws,
  namespace,
  tabs,
  setTabs,
  dirty,
  setDirty,
  reloads,
  readOnly,
  readOnlyReason,
  onSelect,
  onOpenRevisions,
}: {
  ws: string;
  namespace: string;
  tabs: OpenTabs;
  setTabs: (update: (t: OpenTabs) => OpenTabs) => void;
  dirty: Record<string, boolean>;
  setDirty: (update: (d: Record<string, boolean>) => Record<string, boolean>) => void;
  reloads: Record<string, number>;
  readOnly: boolean;
  readOnlyReason: string;
  onSelect: (path: string) => void;
  onOpenRevisions: (path: string) => void;
}) {
  const id = useId();
  const [closing, setClosing] = useState<string | null>(null);
  const tabRefs = useRef(new Map<string, HTMLButtonElement>());
  const labels = useMemo(() => {
    const names = tabs.tabs.map(baseName);
    return Object.fromEntries(
      tabs.tabs.map((p, i) => [p, names.filter((n) => n === names[i]).length > 1 ? `${names[i]} (${parentDir(p)})` : names[i]]),
    );
  }, [tabs.tabs]);

  const activate = (path: string) => {
    setTabs((t) => ({ ...t, active: path }));
    onSelect(path);
  };
  const close = (path: string) => {
    setClosing(null);
    setDirty((d) => {
      const { [path]: _gone, ...rest } = d;
      return rest;
    });
    setTabs((t) => {
      const i = t.tabs.indexOf(path);
      const kept = t.tabs.filter((p) => p !== path);
      return { tabs: kept, active: t.active === path ? (kept[Math.min(i, kept.length - 1)] ?? null) : t.active };
    });
  };
  const askClose = (path: string) => (dirty[path] ? setClosing(path) : close(path));

  const onTabKey = (e: KeyboardEvent, path: string) => {
    const i = tabs.tabs.indexOf(path);
    const to = e.key === "ArrowRight" ? tabs.tabs[(i + 1) % tabs.tabs.length] : e.key === "ArrowLeft" ? tabs.tabs[(i - 1 + tabs.tabs.length) % tabs.tabs.length] : null;
    if (!to) return;
    e.preventDefault();
    activate(to);
    tabRefs.current.get(to)?.focus();
  };

  return (
    <div className="flex min-h-[24rem] min-w-0 flex-1 flex-col overflow-hidden rounded-md border">
      {tabs.tabs.length === 0 ? (
        <div className="flex flex-1 items-center justify-center p-4">
          <EmptyState icon={FileText} title="No file open" description="Choose a file in the tree to open it here." />
        </div>
      ) : (
        <>
          <div role="tablist" aria-label="Open files" className="flex shrink-0 overflow-x-auto border-b bg-muted/30">
            {tabs.tabs.map((p) => {
              const active = p === tabs.active;
              return (
                <div key={p} role="presentation" className={cn("flex items-center border-r", active && "bg-background")}>
                  <button
                    ref={(el) => {
                      if (el) tabRefs.current.set(p, el);
                      else tabRefs.current.delete(p);
                    }}
                    type="button"
                    role="tab"
                    id={`${id}-tab-${p}`}
                    aria-selected={active}
                    aria-controls={`${id}-panel-${p}`}
                    tabIndex={active ? 0 : -1}
                    title={p}
                    onClick={() => activate(p)}
                    onKeyDown={(e) => onTabKey(e, p)}
                    className={cn(
                      "flex items-center gap-1.5 py-1.5 pr-1 pl-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      active ? "font-medium" : "text-muted-foreground",
                    )}
                  >
                    {labels[p]}
                    {dirty[p] ? (
                      <>
                        <span aria-hidden data-testid="unsaved-dot" className="size-2 rounded-full bg-foreground" />
                        <span className="sr-only">, unsaved</span>
                      </>
                    ) : null}
                  </button>
                  <IconButton aria-label={`Close ${labels[p]}`} className="mr-1" onClick={() => askClose(p)}>
                    <X aria-hidden className="size-3.5" />
                  </IconButton>
                </div>
              );
            })}
          </div>
          {tabs.tabs.map((p) => (
            <div
              key={p}
              role="tabpanel"
              id={`${id}-panel-${p}`}
              aria-labelledby={`${id}-tab-${p}`}
              hidden={p !== tabs.active}
              className="min-h-0 flex-1"
            >
              <NamespaceFileEditor
                ws={ws}
                namespace={namespace}
                path={p}
                readOnly={readOnly}
                readOnlyReason={readOnlyReason}
                reloadSignal={reloads[p]}
                onDirtyChange={(d) => setDirty((m) => (!!m[p] === d ? m : { ...m, [p]: d }))}
                onOpenRevisions={() => onOpenRevisions(p)}
              />
            </div>
          ))}
        </>
      )}
      <AlertDialog open={closing !== null} onOpenChange={(o) => (o ? undefined : setClosing(null))}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Close {closing ? labels[closing] : ""} without saving?</AlertDialogTitle>
            <AlertDialogDescription>It has unsaved edits. Closing it discards them; the saved revision is kept.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <Button type="button" variant="destructive" onClick={() => closing && close(closing)}>
              Discard and close
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
