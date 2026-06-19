import type { Rule, FormattingOperator } from "./format-types";

const CELL_ALPHA = 0.65;
const ROW_ALPHA = 0.2;
const RANGE_ALPHA = 0.75;

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  const full = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
  return [parseInt(full.slice(0, 2), 16), parseInt(full.slice(2, 4), 16), parseInt(full.slice(4, 6), 16)];
}
function rgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
function lerp(a: number, b: number, t: number): number {
  return Math.round(a + (b - a) * t);
}
function gradient(value: number, min: number, max: number, colors: string[]): string {
  if (max <= min) return rgba(colors[0], RANGE_ALPHA);
  const t = Math.min(1, Math.max(0, (value - min) / (max - min)));
  const stops = colors.map(hexToRgb);
  const seg = t * (stops.length - 1);
  const i = Math.min(stops.length - 2, Math.floor(seg));
  const localT = seg - i;
  const [r, g, b] = [0, 1, 2].map((k) => lerp(stops[i][k], stops[i + 1][k], localT)) as [number, number, number];
  return `rgba(${r}, ${g}, ${b}, ${RANGE_ALPHA})`;
}

function matchOperator(op: FormattingOperator, cell: unknown, ruleValue: unknown): boolean {
  if (op === "is-null") return cell == null;
  if (op === "not-null") return cell != null;
  if (op === "is-true") return cell === true || cell === "true";
  if (op === "is-false") return cell === false || cell === "false";
  if (cell == null) return false;
  if (op === "contains") return String(cell).includes(String(ruleValue));
  if (op === "does-not-contain") return !String(cell).includes(String(ruleValue));
  if (op === "starts-with") return String(cell).startsWith(String(ruleValue));
  if (op === "ends-with") return String(cell).endsWith(String(ruleValue));
  const cn = Number(cell), rn = Number(ruleValue);
  const numeric = Number.isFinite(cn) && Number.isFinite(rn);
  const c = numeric ? cn : String(cell);
  const r = numeric ? rn : String(ruleValue);
  switch (op) {
    case "=": return c === r;
    case "!=": return c !== r;
    case "<": return c < r;
    case ">": return c > r;
    case "<=": return c <= r;
    case ">=": return c >= r;
  }
  return false;
}

export interface Conditional {
  bg: (value: unknown, colName: string) => string | null;
  rowBg: (rowIndex: number) => string | null;
}

export function buildConditional(
  rows: unknown[][],
  colIndexByName: Record<string, number>,
  rules: Rule[],
  opts: { isPivot?: boolean } = {},
): Conditional {
  const extents: Record<string, [number, number]> = {};
  const extentOf = (col: string): [number, number] => {
    if (extents[col]) return extents[col];
    const idx = colIndexByName[col];
    let min = Infinity, max = -Infinity;
    for (const row of rows) {
      const v = Number(row[idx]);
      if (Number.isFinite(v)) { if (v < min) min = v; if (v > max) max = v; }
    }
    extents[col] = [min, max];
    return extents[col];
  };

  const bg = (value: unknown, colName: string): string | null => {
    for (const rule of rules) {
      if (!rule.columns.includes(colName)) continue;
      if (rule.type === "single") {
        if (rule.highlight_row && !opts.isPivot) continue;
        if (matchOperator(rule.operator, value, rule.value)) {
          return rgba(rule.color, CELL_ALPHA);
        }
      } else {
        const v = Number(value);
        if (!Number.isFinite(v)) continue;
        let [min, max] = extentOf(colName);
        if (rule.min_type === "custom" && typeof rule.min_value === "number") min = rule.min_value;
        if (rule.max_type === "custom" && typeof rule.max_value === "number") max = rule.max_value;
        if (rule.min_type === "all" || rule.max_type === "all") {
          const all = rule.columns.map(extentOf);
          if (rule.min_type === "all") min = Math.min(...all.map((e) => e[0]));
          if (rule.max_type === "all") max = Math.max(...all.map((e) => e[1]));
        }
        return gradient(v, min, max, rule.colors);
      }
    }
    return null;
  };

  const rowColors: (string | null)[] = rows.map((row) => {
    if (opts.isPivot) return null;
    for (const rule of rules) {
      if (rule.type !== "single" || !rule.highlight_row) continue;
      for (const col of rule.columns) {
        const idx = colIndexByName[col];
        if (idx == null) continue;
        if (matchOperator(rule.operator, row[idx], rule.value)) return rgba(rule.color, ROW_ALPHA);
      }
    }
    return null;
  });

  return { bg, rowBg: (i) => rowColors[i] ?? null };
}
