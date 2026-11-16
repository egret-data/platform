import type { LabelDto } from "@/api/generated/model";

/**
 * THE EDIT LABELS FORM'S MODEL (package 7d D6, D10; FE-FORM-1: plain state, no form library).
 * The rows are the whole list the server stores: what Save sends replaces the execution's
 * labels, the flow's included. `system.*` rows are egret's own, shown read only and sent as
 * they are. The server checks the rest (the key pattern, the caps) and answers
 * `invalid_labels` in words that name the label.
 */
export interface LabelRow {
  key: string;
  value: string;
  system: boolean;
}

export const isSystemLabel = (key: string): boolean => key.startsWith("system.");

export function rowsFromLabels(labels: LabelDto[]): LabelRow[] {
  return labels.map((l) => ({ key: l.key, value: l.value, system: isSystemLabel(l.key) }));
}

/** Each row's errors by index: an empty key, a key an earlier row already uses, an empty value. */
export function rowErrors(rows: LabelRow[]): Map<number, { key?: string; value?: string }> {
  const errors = new Map<number, { key?: string; value?: string }>();
  const seen = new Set<string>();
  rows.forEach((row, i) => {
    const e: { key?: string; value?: string } = {};
    if (row.key === "") e.key = "Enter a key";
    else if (seen.has(row.key)) e.key = "This key is already used";
    if (row.value === "") e.value = "Enter a value";
    if (row.key !== "") seen.add(row.key);
    if (e.key || e.value) errors.set(i, e);
  });
  return errors;
}

/** Every row, system ones included, in order. */
export function labelsFromRows(rows: LabelRow[]): LabelDto[] {
  return rows.map((r) => ({ key: r.key, value: r.value }));
}

/** The rows differ from `before`, by key and value, in order (FE-STATE-4: the content, not a flag). */
export function rowsChanged(before: LabelDto[], rows: LabelRow[]): boolean {
  const after = labelsFromRows(rows);
  return after.length !== before.length || after.some((l, i) => l.key !== before[i].key || l.value !== before[i].value);
}
