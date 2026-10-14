import { useEffect, useId, useMemo, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { useQueries } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, File, FileArchive, FileImage, FileText, Folder, FolderOpen, MoreHorizontal } from "lucide-react";
import type { NsFileStatsDto } from "@/api/generated/model";
import { getWorkflowDownloadNamespaceFileUrl, getWorkflowListNamespaceDirectoryQueryOptions } from "@/api/generated/workflow/workflow";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { IconButton } from "@/components/ui/icon-button";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { baseName, fileKindFor, isDirPath, isUnder, parentDir, sortChildren } from "./nsFilePaths";
import { fileErrorMessage } from "./nsFilesApi";

/** A row's action, chosen from its menu or its key (F2 renames, Delete deletes). */
export type TreeAction = "rename" | "move" | "revisions" | "copy-path" | "delete";

export interface TreeEntry {
  path: string;
  name: string;
  isDir: boolean;
  level: number;
  revision: number;
}

export type Row =
  | { kind: "entry"; entry: TreeEntry }
  | { kind: "loading"; key: string; level: number }
  | { kind: "error"; key: string; level: number; message: string; retry: () => void };

/**
 * The root's listing and every expanded folder's, each its own query through
 * the generated query options (FE-DATA-2), flattened in tree order: folders
 * first, then files (`sortChildren`); a folder's children only when it is
 * expanded. `folders` is every folder path seen, for the Move dialog's picker.
 */
export function useTreeRows(ws: string, namespace: string, expanded: ReadonlySet<string>) {
  const dirs = useMemo(() => ["/", ...[...expanded].sort()], [expanded]);
  const results = useQueries({
    queries: dirs.map((dir) =>
      getWorkflowListNamespaceDirectoryQueryOptions(ws, namespace, dir === "/" ? undefined : { path: dir }, {
        query: { queryKey: workflowsKeys.nsDirectory(ws, namespace, dir) },
      }),
    ),
  });
  const byDir = new Map(dirs.map((dir, i) => [dir, results[i]]));
  const root = results[0];

  const rows: Row[] = [];
  const folders = new Set<string>(["/"]);
  const walk = (dir: string, level: number) => {
    const q = byDir.get(dir);
    if (!q) return;
    if (q.isError && !q.data) {
      rows.push({ kind: "error", key: `error:${dir}`, level, message: fileErrorMessage(q.error, "This folder could not be listed"), retry: () => void q.refetch() });
      return;
    }
    if (!q.data) {
      rows.push({ kind: "loading", key: `loading:${dir}`, level });
      return;
    }
    const children: NsFileStatsDto[] = q.data.status === 200 ? q.data.data : [];
    for (const c of sortChildren(children)) {
      const isDir = c.type === "Directory";
      if (isDir) folders.add(c.path);
      rows.push({ kind: "entry", entry: { path: c.path, name: baseName(c.path), isDir, level, revision: c.revision } });
      if (isDir && expanded.has(c.path)) walk(c.path, level + 1);
    }
  };
  if (root.data) walk("/", 1);
  return { root, rows, folders: [...folders].sort() };
}

export interface FileTreeProps {
  ws: string;
  namespace: string;
  rows: Row[];
  expanded: ReadonlySet<string>;
  selected: string | null;
  onSelect: (path: string) => void;
  onToggle: (dir: string) => void;
  onOpen: (path: string) => void;
  onAction: (action: TreeAction, entry: TreeEntry) => void;
  /** Drag `from` into the folder `toDir` (W9b's move). Absent: nothing can be dragged. */
  onDrop?: (from: string, toDir: string) => void;
  /** Off: writes are disabled, and say why under their names in the menu. */
  writeReason: string | null;
}

/** Can `from` be dropped into `dir`? Not where it already is, and a folder not into itself. */
export function canDropInto(from: string, dir: string): boolean {
  if (parentDir(from) === dir) return false;
  if (isDirPath(from) && isUnder(dir, from)) return false;
  return true;
}

const iconFor = (entry: TreeEntry, open: boolean) => {
  if (entry.isDir) return open ? FolderOpen : Folder;
  const kind = fileKindFor(entry.path);
  if (kind === "image") return FileImage;
  if (kind === "binary") return /\.(zip|gz|tgz|tar|jar|7z|rar|bz2|xz)$/i.test(entry.path) ? FileArchive : File;
  return FileText;
};

/**
 * THE FILE TREE: WAI-ARIA's tree pattern over W9b's directory listings. One
 * row takes Tab (roving `tabIndex`); Up/Down move, Right expands or enters a
 * folder, Left collapses or goes to the parent, Home/End jump, Enter opens a
 * file or toggles a folder, F2 renames, Delete deletes, and Shift+F10 (or
 * the context-menu key) opens the row's menu, whose "Move to…" is every
 * drag's keyboard equivalent (spec §4.3). A row's menu button is for the
 * mouse and takes no Tab stop.
 *
 * Dragging a row onto a folder moves it there; onto a file, into that
 * file's folder; onto the tree's empty space, to the root.
 */
export function FileTree({ ws, namespace, rows, expanded, selected, onSelect, onToggle, onOpen, onAction, onDrop, writeReason }: FileTreeProps) {
  const entries = rows.flatMap((r) => (r.kind === "entry" ? [r.entry] : []));
  const [focus, setFocus] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropDir, setDropDir] = useState<string | null>(null);
  const refs = useRef(new Map<string, HTMLDivElement>());
  const wantFocus = useRef(false);
  const helpId = useId();

  // The row that takes Tab: the focused one while it is shown, else the selection, else the first.
  const tabStop = [focus, selected].find((p) => p && entries.some((e) => e.path === p)) ?? entries[0]?.path ?? null;

  useEffect(() => {
    if (wantFocus.current && focus) {
      wantFocus.current = false;
      refs.current.get(focus)?.focus();
    }
  }, [focus, rows]);

  const moveFocus = (path: string | undefined) => {
    if (!path) return;
    wantFocus.current = true;
    setFocus(path);
    onSelect(path);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, entry: TreeEntry) => {
    const i = entries.findIndex((x) => x.path === entry.path);
    const open = entry.isDir && expanded.has(entry.path);
    switch (e.key) {
      case "ArrowDown":
        moveFocus(entries[i + 1]?.path);
        break;
      case "ArrowUp":
        moveFocus(entries[i - 1]?.path);
        break;
      case "Home":
        moveFocus(entries[0]?.path);
        break;
      case "End":
        moveFocus(entries[entries.length - 1]?.path);
        break;
      case "ArrowRight":
        if (entry.isDir && !open) onToggle(entry.path);
        else if (open && entries[i + 1]?.level === entry.level + 1) moveFocus(entries[i + 1].path);
        break;
      case "ArrowLeft":
        if (open) onToggle(entry.path);
        else if (entry.level > 1) moveFocus(parentDir(entry.path));
        break;
      case "Enter":
      case " ":
        if (entry.isDir) onToggle(entry.path);
        else onOpen(entry.path);
        break;
      case "F2":
        onAction("rename", entry);
        break;
      case "Delete":
        onAction("delete", entry);
        break;
      case "F10":
        if (!e.shiftKey) return;
        setMenuFor(entry.path);
        break;
      case "ContextMenu":
        setMenuFor(entry.path);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const dropTargetOf = (entry: TreeEntry | null) => (entry ? (entry.isDir ? entry.path : parentDir(entry.path)) : "/");
  const dragProps = (entry: TreeEntry | null) =>
    onDrop
      ? {
          onDragOver: (e: DragEvent) => {
            const dir = dropTargetOf(entry);
            if (!dragging || !canDropInto(dragging, dir)) return;
            e.preventDefault();
            e.stopPropagation();
            if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
            setDropDir(dir);
          },
          onDragLeave: () => setDropDir(null),
          onDrop: (e: DragEvent) => {
            const dir = dropTargetOf(entry);
            e.preventDefault();
            e.stopPropagation();
            const from = dragging;
            setDragging(null);
            setDropDir(null);
            if (from && canDropInto(from, dir)) onDrop(from, dir);
          },
        }
      : {};

  return (
    <div
      role="tree"
      aria-label={`Files of ${namespace}`}
      aria-describedby={helpId}
      className={cn("min-h-24 flex-1 overflow-auto rounded-md border p-1 text-sm", dropDir === "/" && "bg-accent")}
      {...dragProps(null)}
    >
      <p id={helpId} className="sr-only">
        {/* F2 and Delete do nothing while writes are off, so they are not offered then. */}
        {writeReason
          ? "Arrow keys move through the files. Enter opens a file, and Shift+F10 opens a file's actions."
          : "Arrow keys move through the files. Enter opens a file. F2 renames, Delete deletes, and Shift+F10 opens a file's actions, among them Move to…."}
      </p>
      {rows.map((row) => {
        if (row.kind === "loading") {
          return (
            <div key={row.key} role="none" style={{ paddingLeft: `${row.level * 0.75}rem` }} className="py-1">
              <Skeleton className="h-4 w-32" />
            </div>
          );
        }
        if (row.kind === "error") {
          return (
            <div key={row.key} role="none" style={{ paddingLeft: `${row.level * 0.75}rem` }} className="flex flex-col items-start gap-1 py-1">
              <span role="alert" className="text-xs text-destructive">
                {row.message}
              </span>
              <Button type="button" size="sm" variant="outline" onClick={row.retry}>
                Try again
              </Button>
            </div>
          );
        }
        const { entry } = row;
        const open = entry.isDir && expanded.has(entry.path);
        const Icon = iconFor(entry, open);
        const isSelected = selected === entry.path;
        return (
          <div
            key={entry.path}
            ref={(el) => {
              if (el) refs.current.set(entry.path, el);
              else refs.current.delete(entry.path);
            }}
            role="treeitem"
            aria-label={entry.name}
            aria-level={entry.level}
            aria-expanded={entry.isDir ? open : undefined}
            aria-selected={isSelected}
            tabIndex={tabStop === entry.path ? 0 : -1}
            title={entry.path}
            draggable={!!onDrop}
            onDragStart={(e) => {
              setDragging(entry.path);
              e.dataTransfer?.setData("text/plain", entry.path);
              if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => {
              setDragging(null);
              setDropDir(null);
            }}
            {...dragProps(entry)}
            onFocus={() => setFocus(entry.path)}
            onClick={() => {
              setFocus(entry.path);
              onSelect(entry.path);
              if (entry.isDir) onToggle(entry.path);
              else onOpen(entry.path);
            }}
            onKeyDown={(e) => onKeyDown(e, entry)}
            className={cn(
              "group flex cursor-pointer items-center gap-1 rounded-sm py-0.5 pr-1 outline-none",
              "hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
              isSelected && "bg-muted font-medium",
              entry.isDir && dropDir === entry.path && "bg-accent ring-2 ring-ring",
            )}
            style={{ paddingLeft: `${(entry.level - 1) * 0.75 + 0.25}rem` }}
          >
            {entry.isDir ? (
              open ? <ChevronDown aria-hidden className="size-4 shrink-0" /> : <ChevronRight aria-hidden className="size-4 shrink-0" />
            ) : (
              <span aria-hidden className="size-4 shrink-0" />
            )}
            <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate">{entry.name}</span>
            <RowMenu
              ws={ws}
              namespace={namespace}
              entry={entry}
              open={menuFor === entry.path}
              onOpenChange={(o) => {
                setMenuFor(o ? entry.path : null);
                if (!o) {
                  wantFocus.current = true;
                  setFocus(entry.path);
                }
              }}
              onAction={(a) => onAction(a, entry)}
              writeReason={writeReason}
            />
          </div>
        );
      })}
    </div>
  );
}

/** A row's actions. Writes are disabled with their reason written under them (FE-ACTION-4). */
function RowMenu({
  ws,
  namespace,
  entry,
  open,
  onOpenChange,
  onAction,
  writeReason,
}: {
  ws: string;
  namespace: string;
  entry: TreeEntry;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAction: (action: TreeAction) => void;
  writeReason: string | null;
}) {
  const write = (action: TreeAction, label: string) => (
    <DropdownMenuItem disabled={!!writeReason} onSelect={() => onAction(action)} className="flex-col items-start gap-0">
      <span>{label}</span>
      {writeReason ? <span className="text-xs text-muted-foreground">{writeReason}</span> : null}
    </DropdownMenuItem>
  );
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange}>
      <DropdownMenuTrigger asChild>
        <IconButton
          tabIndex={-1}
          aria-label={`Actions for ${entry.name}`}
          className="opacity-60 group-hover:opacity-100 group-focus-visible:opacity-100"
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
        >
          <MoreHorizontal aria-hidden className="size-4" />
        </IconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
        {write("rename", "Rename")}
        {write("move", "Move to…")}
        {entry.isDir ? null : (
          <>
            <DropdownMenuItem onSelect={() => onAction("revisions")}>Revisions</DropdownMenuItem>
            <DropdownMenuItem asChild>
              <a href={getWorkflowDownloadNamespaceFileUrl(ws, namespace, { path: entry.path })} download={entry.name}>
                Download
              </a>
            </DropdownMenuItem>
          </>
        )}
        <DropdownMenuItem onSelect={() => onAction("copy-path")}>Copy path</DropdownMenuItem>
        <DropdownMenuSeparator />
        {write("delete", "Delete")}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
