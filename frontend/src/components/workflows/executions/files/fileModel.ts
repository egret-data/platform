import { ApiError } from "@/api/client";
import { getWorkflowDownloadExecutionFileUrl } from "@/api/generated/workflow/workflow";

/**
 * What a file row and the preview drawer share (W7b spec §8): the name, the
 * size, the download link, and what a 404 means.
 */

/**
 * The file routes answer 404 for a file pruned after retention, never stored,
 * or outside the execution's loop family (a trigger's or a replay source's
 * file is served by its own execution). The UI cannot tell these apart, so it
 * says the one thing true of all of them. Also this module's marker in
 * `scripts/assert-workflows-lazy.mjs`.
 */
export const NO_LONGER_AVAILABLE = "This file is no longer available";

export const isNotFound = (e: unknown) => e instanceof ApiError && e.status === 404;

/** The last segment of a `kestra:///` URI, or the URI when it has none. */
export function fileName(uri: string): string {
  return uri.slice(uri.lastIndexOf("/") + 1) || uri;
}

/** An `.ion` file's preview rows are JSON values; any other file's are lines of text. */
export const isIonFile = (uri: string) => uri.toLowerCase().endsWith(".ion");

const UNITS = ["KiB", "MiB", "GiB", "TiB"];

/** A byte count: bytes below 1 KiB, then binary units with one decimal. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let v = bytes / 1024;
  let unit = 0;
  while (v >= 1024 && unit < UNITS.length - 1) {
    v /= 1024;
    unit++;
  }
  return `${v.toFixed(1)} ${UNITS[unit]}`;
}

/**
 * The file route, for an `<a href>`: the browser streams it with the session
 * cookie (`Content-Disposition: attachment`), so nothing is read into memory.
 */
export const downloadHref = (ws: string, executionId: string, uri: string) =>
  getWorkflowDownloadExecutionFileUrl(ws, executionId, { path: uri });
