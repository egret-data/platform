import { ApiError } from "@/api/client";
import {
  workflowCreateNamespaceDirectory,
  workflowDeleteNamespaceFile,
  workflowDownloadNamespaceFile,
  workflowMoveNamespaceFile,
  workflowNamespaceFileStats,
  workflowSearchNamespaceFiles,
  workflowUploadNamespaceFile,
} from "@/api/generated/workflow/workflow";
import { rawBlob } from "@/api/mutator";
import { isUnder } from "./nsFilePaths";

/**
 * W9b's namespace file routes, through the generated fetchers (FE-DATA-2).
 * Each is called directly, not through its hook, for the reason its comment
 * names. Listings and revisions use the generated hooks (`FileTree.tsx`,
 * `RevisionsDrawer.tsx`).
 */

/** Why every write is off for a viewer without `workflow:manage` (W7d spec §4.5, FE-ACTION-4). */
export const FILES_MANAGE_REASON = "Changing namespace files needs the workflow:manage permission";

/** The 409 code of a write whose `if_revision` no longer holds (W7d spec §3.2). */
export const FILE_CHANGED = "file_changed";

/** The save was refused because the file changed, or was deleted, since it was read. */
export const isFileChanged = (e: unknown): e is ApiError => e instanceof ApiError && e.status === 409 && e.code === FILE_CHANGED;

/**
 * The server's sentence as a sentence: without `Error::Conflict`'s
 * `conflict: ` prefix, capitalised, with a full stop.
 */
export function serverSentence(message: string): string {
  const bare = message.replace(/^conflict:\s*/i, "").replace(/\.$/, "");
  return `${bare.charAt(0).toUpperCase()}${bare.slice(1)}.`;
}

/**
 * One revision's bytes (`GET …/files?path&revision`). Called directly
 * (FE-DATA-2 exception): the revision is computed at fetch time, from the
 * `stats` read just before it ({@link readLatest}).
 */
export async function readRevision(ws: string, namespace: string, path: string, revision: number, signal?: AbortSignal): Promise<Uint8Array> {
  const res = await workflowDownloadNamespaceFile(ws, namespace, { path, revision }, { rawBlob: true, signal });
  return new Uint8Array(await rawBlob(res).arrayBuffer());
}

/**
 * A file's latest revision as one consistent pair: `stats` gives the
 * revision N, then the bytes are read AT N (`?revision=N`), never "the
 * latest" again. A write between the two reads cannot hand the editor
 * revision N+1's text under revision N's number, which a save would then
 * send back as `if_revision` and silently overwrite (W7d Task 2's note).
 * `readBytes` is false for a file that is only offered for download.
 */
export async function readLatest(
  ws: string,
  namespace: string,
  path: string,
  { readBytes, maxBytes, signal }: { readBytes: boolean; maxBytes: number; signal?: AbortSignal },
): Promise<{ revision: number; size: number; bytes: Uint8Array | null }> {
  // Called directly (FE-DATA-2 exception): the download below needs this answer's revision.
  const stats = await workflowNamespaceFileStats(ws, namespace, { path }, { signal });
  if (stats.status !== 200) throw new Error("the server's answer was not a file's details");
  if (stats.data.type !== "File") throw new ApiError(400, `'${path}' is a folder, not a file`);
  const { revision, size } = stats.data;
  if (!readBytes || size > maxBytes) return { revision, size, bytes: null };
  return { revision, size, bytes: await readRevision(ws, namespace, path, revision, signal) };
}

/**
 * `POST …/files?path[&if_revision]` (multipart). With `ifRevision` the write
 * commits only if the path is still at that revision (0: it must not
 * exist), else 409 `file_changed`; without it the write overwrites (W9b).
 * Called directly (FE-DATA-2 exception): the revision is computed at call
 * time, per file.
 */
export async function writeFile(ws: string, namespace: string, path: string, content: Blob, ifRevision?: number): Promise<void> {
  await workflowUploadNamespaceFile(ws, namespace, { fileContent: content }, ifRevision === undefined ? { path } : { path, if_revision: ifRevision });
}

/** `POST …/files/directory?path`: the folder and its parents. */
export async function createDirectory(ws: string, namespace: string, path: string): Promise<void> {
  await workflowCreateNamespaceDirectory(ws, namespace, { path });
}

/** `PUT …/files?from&to`: a file, or a folder with all it holds, every revision kept (W9b §4.4). */
export async function movePath(ws: string, namespace: string, from: string, to: string): Promise<void> {
  await workflowMoveNamespaceFile(ws, namespace, { from, to });
}

/** `DELETE …/files?path`: soft delete; revisions are kept (W9b §4.3). */
export async function deletePath(ws: string, namespace: string, path: string): Promise<void> {
  await workflowDeleteNamespaceFile(ws, namespace, { path });
}

/**
 * How many live files a delete of `path` removes, for the confirmation
 * (W7d spec §4.3). A file is one. A folder's files come from W9b's search
 * route with the folder's path as `q`: search is a case-insensitive
 * substring, so `/scripts/` also finds `/old/Scripts/x`; only paths that
 * START with the folder's are counted. Called directly (FE-DATA-2
 * exception): asked once, when the confirmation opens.
 */
export async function countFilesUnder(ws: string, namespace: string, path: string): Promise<number> {
  if (!path.endsWith("/")) return 1;
  const res = await workflowSearchNamespaceFiles(ws, namespace, { q: path });
  if (res.status !== 200) throw new Error("the server's answer was not a list of paths");
  return res.data.filter((p) => isUnder(p, path)).length;
}

/**
 * A request's failure as one sentence that says what happened and what to
 * do (FE-COPY-2), with the server's own words when it sent any. `what`
 * names the attempt: "The file was not saved".
 */
export function fileErrorMessage(e: unknown, what: string): string {
  // Without `Error::Conflict`'s `conflict: ` prefix, as `serverSentence` reads it.
  const server = e instanceof Error && e.message ? e.message.replace(/^conflict:\s*/i, "").replace(/\.$/, "") : "";
  if (e instanceof ApiError) {
    switch (e.status) {
      case 403:
        return `${what}: ${server || "permission denied"}. ${FILES_MANAGE_REASON}; ask a workspace admin for it.`;
      case 400:
        return `${what}: ${server}. Correct it and try again.`;
      case 404:
        return `${what}: ${server}. It may have been moved or deleted; refresh the list.`;
      case 408:
        return `${what}: ${server}. The upload stalled; try again.`;
      case 409:
        return /try again/i.test(server) ? `${what}: ${server}.` : `${what}: ${server}. Choose another path, or change what is there first.`;
      case 413:
        return `${what}: ${server}. Make it smaller and try again.`;
      case 503:
        return `${what}: ${server}. The store is unavailable; try again in a moment.`;
      default:
        return `${what}: ${server || `the server answered ${e.status}`}. Try again.`;
    }
  }
  if (e instanceof TypeError) return `${what}: the server could not be reached. Check your connection and try again.`;
  return `${what}: ${server || "something went wrong"}. Try again.`;
}
