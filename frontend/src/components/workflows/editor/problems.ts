import { rangeOfPath, type YamlRange } from "@/components/workflows/editor/yamlPath";

export type Severity = "error" | "warning";

/** A finding from `POST …/validate`; `at` is set for a YAML parse error (the 400). */
export type ServerFinding = {
  severity: Severity;
  code: string;
  message: string;
  path: string;
  at?: { line: number; col: number };
};

/** A marker `monaco-yaml` set from the JSON schema, as you type. */
export type SchemaMarker = { severity: Severity; message: string; range: YamlRange };

/** One row of the Problems tab, and one marker in the editor. */
export type Problem = {
  key: string;
  severity: Severity;
  message: string;
  /** The server finding's path; null for a schema marker or a parse error. */
  path: string | null;
  range: YamlRange;
  /** False when the path names nothing in the source; the range is then line 1. */
  mapped: boolean;
  source: "server" | "schema";
};

/** Line 1, where a finding whose path cannot be mapped is shown (spec §5.2, §15). */
export function lineOne(source: string): YamlRange {
  const first = source.split("\n", 1)[0] ?? "";
  return { startLine: 1, startCol: 1, endLine: 1, endCol: first.length + 1 };
}

function placeServer(source: string, f: ServerFinding): { range: YamlRange; mapped: boolean } {
  if (f.at) {
    const { line, col } = f.at;
    return { range: { startLine: line, startCol: col, endLine: line, endCol: col + 1 }, mapped: true };
  }
  const range = rangeOfPath(source, f.path);
  return range ? { range, mapped: true } : { range: lineOne(source), mapped: false };
}

const RANK: Record<Severity, number> = { error: 0, warning: 1 };

/**
 * Both sources of problems (spec §5.2), placed in `source` and sorted: errors
 * first, then by position. Server paths are mapped against the CURRENT source,
 * so a marker follows the text it names while the next validation is pending.
 */
export function collectProblems(
  source: string,
  findings: readonly ServerFinding[],
  markers: readonly SchemaMarker[],
): Problem[] {
  const server = findings.map((f, i): Problem => ({
    key: `s${i}`,
    severity: f.severity,
    message: f.message,
    path: f.at ? null : f.path || null,
    ...placeServer(source, f),
    source: "server",
  }));
  const schema = markers.map((m, i): Problem => ({
    key: `m${i}`,
    severity: m.severity,
    message: m.message,
    path: null,
    range: m.range,
    mapped: true,
    source: "schema",
  }));
  return [...server, ...schema].sort(
    (a, b) =>
      RANK[a.severity] - RANK[b.severity] ||
      a.range.startLine - b.range.startLine ||
      a.range.startCol - b.range.startCol,
  );
}
