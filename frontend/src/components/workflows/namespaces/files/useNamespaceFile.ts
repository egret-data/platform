import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { classifyBytes, fileKindFor, imageMime, languageFor } from "./nsFilePaths";
import { fileErrorMessage, isFileChanged, readLatest, writeFile } from "./nsFilesApi";

/** The largest file opened in the page (text or image); a larger one is offered for download only. */
export const MAX_OPEN_BYTES = 5 * 1024 * 1024;

/** A file as it was read: its revision, and what the page can show of it. */
export type LoadedFile =
  | { kind: "text"; revision: number; size: number; language: string }
  | { kind: "image"; revision: number; size: number; url: string | null }
  | { kind: "binary"; revision: number; size: number; tooLarge: boolean };

export interface NamespaceFile {
  /** `loading` before the first answer and while a reload runs. */
  status: "loading" | "error" | "ready";
  /** What went wrong reading the file, as a sentence (FE-COPY-2), when `status` is `error`. */
  error: string | null;
  /** The HTTP status of that failure: 404 means the file is not there (moved or deleted). */
  errorStatus: number | null;
  file: LoadedFile | null;
  /** The editor's text (a text file), starting as the text read. */
  text: string;
  setText: (text: string) => void;
  /** The text differs from the revision it was read at, compared by content (FE-STATE-4). */
  dirty: boolean;
  /** Grows by one each time content is read: key the editor with it so it takes the new text. */
  loadId: number;
  saving: boolean;
  /** Why the last save failed, other than a changed file (FE-ACTION-3: the text is kept). */
  saveError: string | null;
  /** The server's sentence while a 409 `file_changed` waits for Reload or Overwrite. */
  conflict: string | null;
  /** Save with `if_revision` = the revision read; true when written. */
  save: () => Promise<boolean>;
  /** Save without `if_revision` (the stale-save dialog's Overwrite). Rejects on failure, after a toast. */
  overwrite: () => Promise<void>;
  /** Read the latest revision again, discarding the edits. */
  reload: () => Promise<void>;
  dismissConflict: () => void;
}

interface Snapshot {
  status: NamespaceFile["status"];
  error: string | null;
  errorStatus: number | null;
  file: LoadedFile | null;
  /** The text of the revision read: what `dirty` compares with. */
  base: string;
  text: string;
  loadId: number;
}

const decode = (bytes: Uint8Array | null) => (bytes ? classifyBytes(bytes) : null);

/**
 * ONE NAMESPACE FILE, READ ONCE AND SAVED WITH ITS REVISION (W7d spec §4.3,
 * §3.2). For the namespace page's file tabs and, in W7d Task 7, the flow
 * editor's `file:<path>` tabs.
 *
 * - **Read:** `stats` for the revision N, then the bytes AT N
 *   (`?revision=N`), so the text and the revision a save sends back always
 *   belong together. What it opens as comes from the extension, then the
 *   bytes (`fileKindFor`, `classifyBytes`). A known binary is not read.
 * - **Save:** the text, with `if_revision=N`. Written, the file is at N+1:
 *   the server wrote `max + 1` and `max` was N, the latest live revision.
 * - **409 `file_changed`** (changed, or deleted, since it was read): `conflict`
 *   holds the server's sentence for W7a's stale-save dialog. Reload reads
 *   the latest again; Overwrite resends without `if_revision`, then reads
 *   the revision it produced. A 409 WITHOUT that code (a folder now at the
 *   path, a busy path) is a plain failure in `saveError`, not the dialog.
 * - The content is not a react-query query on purpose: an invalidation must
 *   never replace text being edited. The hook refreshes the file's
 *   revisions list after a write.
 */
export function useNamespaceFile(ws: string, namespace: string, path: string, { enabled = true }: { enabled?: boolean } = {}): NamespaceFile {
  const queryClient = useQueryClient();
  const [snap, setSnap] = useState<Snapshot>({ status: "loading", error: null, errorStatus: null, file: null, base: "", text: "", loadId: 0 });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflict, setConflict] = useState<string | null>(null);
  const latest = useRef(snap);
  useEffect(() => {
    latest.current = snap;
  });
  const abort = useRef<AbortController | null>(null);
  const imageUrl = useRef<string | null>(null);

  const revoke = () => {
    if (imageUrl.current && typeof URL.revokeObjectURL === "function") URL.revokeObjectURL(imageUrl.current);
    imageUrl.current = null;
  };

  const load = useCallback(async () => {
    abort.current?.abort();
    const controller = new AbortController();
    abort.current = controller;
    setSnap((s) => ({ ...s, status: "loading", error: null, errorStatus: null }));
    const kind = fileKindFor(path);
    try {
      const read = await readLatest(ws, namespace, path, { readBytes: kind !== "binary", maxBytes: MAX_OPEN_BYTES, signal: controller.signal });
      if (controller.signal.aborted) return;
      revoke();
      let file: LoadedFile;
      let text = "";
      const tooLarge = read.bytes === null && kind !== "binary";
      if (kind === "image" && read.bytes) {
        const url = typeof URL.createObjectURL === "function" ? URL.createObjectURL(new Blob([read.bytes as BlobPart], { type: imageMime(path) })) : null;
        imageUrl.current = url;
        file = { kind: "image", revision: read.revision, size: read.size, url };
      } else {
        const decoded = kind === "text" ? decode(read.bytes) : null;
        if (decoded?.kind === "text") {
          text = decoded.text;
          file = { kind: "text", revision: read.revision, size: read.size, language: languageFor(path) };
        } else {
          file = { kind: "binary", revision: read.revision, size: read.size, tooLarge };
        }
      }
      setSnap((s) => ({ status: "ready", error: null, errorStatus: null, file, base: text, text, loadId: s.loadId + 1 }));
    } catch (e) {
      if (controller.signal.aborted) return;
      setSnap((s) => ({
        ...s,
        status: "error",
        error: fileErrorMessage(e, "The file could not be opened"),
        errorStatus: e instanceof ApiError ? e.status : null,
      }));
    }
  }, [ws, namespace, path]);

  useEffect(() => {
    if (!enabled) return;
    void load();
    return () => {
      abort.current?.abort();
      revoke();
    };
  }, [enabled, load]);

  const setText = useCallback((text: string) => setSnap((s) => ({ ...s, text })), []);
  const refreshRevisions = useCallback(
    () => queryClient.invalidateQueries({ queryKey: workflowsKeys.nsFileRevisions(ws, namespace, path) }),
    [queryClient, ws, namespace, path],
  );

  const save = useCallback(async (): Promise<boolean> => {
    const { file, text } = latest.current;
    if (!file || file.kind !== "text") return false;
    setSaving(true);
    setSaveError(null);
    try {
      await writeFile(ws, namespace, path, new Blob([text]), file.revision);
      setSnap((s) => (s.file?.kind === "text" ? { ...s, base: text, file: { ...s.file, revision: file.revision + 1, size: new Blob([text]).size } } : s));
      void refreshRevisions();
      return true;
    } catch (e) {
      if (isFileChanged(e)) setConflict(e.message);
      else setSaveError(fileErrorMessage(e, "The file was not saved"));
      return false;
    } finally {
      setSaving(false);
    }
  }, [ws, namespace, path, refreshRevisions]);

  const overwrite = useCallback(async () => {
    const { file, text } = latest.current;
    if (!file || file.kind !== "text") return;
    setSaving(true);
    setSaveError(null);
    try {
      await writeFile(ws, namespace, path, new Blob([text]));
      // The revision the write produced is not in its answer: read the latest pair again. If
      // someone wrote in between, `base` is THEIR text, so the editor shows unsaved edits.
      const read = await readLatest(ws, namespace, path, { readBytes: true, maxBytes: MAX_OPEN_BYTES });
      const decoded = decode(read.bytes);
      const base = decoded?.kind === "text" ? decoded.text : text;
      setSnap((s) => (s.file?.kind === "text" ? { ...s, base, file: { ...s.file, revision: read.revision, size: read.size } } : s));
      setConflict(null);
      void refreshRevisions();
    } catch (e) {
      toast.error(fileErrorMessage(e, "The file was not overwritten"));
      throw e;
    } finally {
      setSaving(false);
    }
  }, [ws, namespace, path, refreshRevisions]);

  const reload = useCallback(async () => {
    setConflict(null);
    setSaveError(null);
    await load();
  }, [load]);

  return {
    status: snap.status,
    error: snap.error,
    errorStatus: snap.errorStatus,
    file: snap.file,
    text: snap.text,
    setText,
    dirty: snap.file?.kind === "text" && snap.text !== snap.base,
    loadId: snap.loadId,
    saving,
    saveError,
    conflict,
    save,
    overwrite,
    reload,
    dismissConflict: useCallback(() => setConflict(null), []),
  };
}
