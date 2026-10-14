import type { NsFileStatsDto } from "@/api/generated/model";

/**
 * W9b's namespace file paths as the server stores them (W9b spec §3, §9): a
 * leading `/`, a directory's path ends with `/`, the root is `/`. The server
 * judges every path rule (`_files`, `_flows`, a `.vN` suffix, the length);
 * these helpers only build paths and refuse what no path can be.
 */

/** The last segment: `load.py` of `/etl/load.py`, `etl` of `/etl/`. */
export function baseName(path: string): string {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  return trimmed.slice(trimmed.lastIndexOf("/") + 1);
}

/** The directory holding `path`: `/etl/` of `/etl/load.py` and of `/etl/sub/`; `/` at the top. */
export function parentDir(path: string): string {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  return trimmed.slice(0, trimmed.lastIndexOf("/") + 1) || "/";
}

/** `name` (which may hold `/`) inside directory `dir`; a directory's path ends with `/`. */
export function joinPath(dir: string, name: string, isDirectory = false): string {
  const inner = name.replace(/^\/+/, "").replace(/\/+$/, "");
  return `${dir}${inner}${isDirectory ? "/" : ""}`;
}

export const isDirPath = (path: string) => path.endsWith("/");

/** `path` is directory `dir` itself or inside it. */
export function isUnder(path: string, dir: string): boolean {
  return path.startsWith(dir);
}

/** Where `path` lands when moved into directory `destDir`: its name kept. */
export function movedPath(path: string, destDir: string): string {
  return joinPath(destDir, baseName(path), isDirPath(path));
}

/** `path` renamed to `name` in the same directory. */
export function renamedPath(path: string, name: string): string {
  return joinPath(parentDir(path), name, isDirPath(path));
}

/**
 * What no path can be, said beside the field before anything is sent. A
 * `nested` name (create) may hold `/` to make folders on the way; a rename
 * may not (moving is "Move to…"). Every other rule is the server's, and its
 * message is shown as it says it (W9b §9, the `_files` refusal among them).
 */
export function checkName(name: string, { nested }: { nested: boolean }): string | null {
  const trimmed = name.trim().replace(/^\/+/, "").replace(/\/+$/, "");
  if (!trimmed) return "Enter a name.";
  if (!nested && trimmed.includes("/")) return "A name cannot contain /. To put it in another folder, use Move to… instead.";
  const segments = trimmed.split("/");
  if (segments.some((s) => s === "")) return "The name has an empty folder name between two slashes; remove one of them.";
  if (segments.some((s) => s === "." || s === "..")) return 'A folder or file name cannot be "." or "..".';
  return null;
}

/** Folders first, then files, each by name (case-insensitive, then exact): Kestra's explorer order. */
export function sortChildren(entries: NsFileStatsDto[]): NsFileStatsDto[] {
  const name = (e: NsFileStatsDto) => baseName(e.path);
  return [...entries].sort((a, b) => {
    const da = a.type === "Directory" ? 0 : 1;
    const db = b.type === "Directory" ? 0 : 1;
    if (da !== db) return da - db;
    const la = name(a).toLowerCase();
    const lb = name(b).toLowerCase();
    if (la !== lb) return la < lb ? -1 : 1;
    return name(a) < name(b) ? -1 : name(a) > name(b) ? 1 : 0;
  });
}

const extensionOf = (path: string) => {
  const name = baseName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};

/**
 * Monaco's language id for a file, from its extension. egret's Monaco chunk
 * registers only `python`, `sql` and `yaml` (`src/lib/monaco.ts`, pinned by
 * its test); Monaco shows any other id as plain text, so a `.js` file is
 * still opened as `javascript` and gains highlighting the day that chunk
 * registers it.
 */
const LANGUAGES: Record<string, string> = {
  py: "python",
  sql: "sql",
  yml: "yaml",
  yaml: "yaml",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  json: "json",
  sh: "shell",
  bash: "shell",
  md: "markdown",
  xml: "xml",
  html: "html",
  htm: "html",
  css: "css",
  ini: "ini",
  properties: "ini",
  r: "r",
  rb: "ruby",
  java: "java",
  go: "go",
  rs: "rust",
  ps1: "powershell",
  bat: "bat",
  tf: "hcl",
};

export function languageFor(path: string): string {
  if (baseName(path).toLowerCase() === "dockerfile") return "dockerfile";
  return LANGUAGES[extensionOf(path)] ?? "plaintext";
}

const IMAGES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
  avif: "image/avif",
};

/** Formats no text editor can show: offered for download only, never read into the page. */
const BINARIES = new Set([
  "zip", "gz", "tgz", "tar", "bz2", "xz", "7z", "rar", "jar", "war", "whl", "egg",
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods",
  "parquet", "avro", "orc", "arrow", "feather", "ion", "pickle", "pkl", "npy", "npz", "h5", "db", "sqlite",
  "exe", "dll", "so", "dylib", "bin", "class", "pyc", "o", "a",
  "mp3", "mp4", "mov", "avi", "wav", "ogg", "webm", "flac",
  "ttf", "otf", "woff", "woff2",
]);

export type FileKind = "text" | "image" | "binary";

/**
 * What a file opens as, by extension (W7d spec §4.3): an image is previewed,
 * a known binary format is offered for download only, and anything else is
 * read and judged by its bytes ({@link classifyBytes}).
 */
export function fileKindFor(path: string): FileKind {
  const ext = extensionOf(path);
  if (ext in IMAGES) return "image";
  if (BINARIES.has(ext)) return "binary";
  return "text";
}

/** The image type to label the bytes with: W9b serves every file as `application/octet-stream`. */
export function imageMime(path: string): string {
  return IMAGES[extensionOf(path)] ?? "application/octet-stream";
}

/**
 * Bytes as text when they are UTF-8 without a NUL byte, else binary. A
 * byte-order mark is kept in the text (`ignoreBOM`), so saving writes back
 * the bytes that were read.
 */
export function classifyBytes(bytes: Uint8Array): { kind: "text"; text: string } | { kind: "binary" } {
  if (bytes.includes(0)) return { kind: "binary" };
  try {
    return { kind: "text", text: new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes) };
  } catch {
    return { kind: "binary" };
  }
}
