/**
 * `{{token}}` placeholders for a notification subject, for the editor's help
 * text and live preview.
 *
 * The SERVER is authoritative: `crates/egret-automation/src/subject.rs` does
 * the real substitution at send time and rejects an unknown token on save, so
 * a token missing from this list still works and one added here that the
 * server does not know is refused with a message naming it. This copy exists
 * so the form can show what the subject will look like before saving — a
 * preview that has to round-trip to the server is a preview nobody waits for.
 *
 * `{{name}}` style is the house convention: SQL Lab parameters, chart link
 * templates and dashboard URL templates all use it.
 */
export const SUBJECT_TOKENS: { token: string; hint: string }[] = [
  { token: "date", hint: "2026-08-13" },
  { token: "time", hint: "09:00" },
  { token: "datetime", hint: "2026-08-13 09:00" },
  { token: "day", hint: "13" },
  { token: "month", hint: "08" },
  { token: "year", hint: "2026" },
  { token: "weekday", hint: "Thursday" },
  { token: "name", hint: "the schedule's name" },
];

const pad = (n: number) => String(n).padStart(2, "0");

/**
 * Substitute the tokens in `template` for a preview.
 *
 * `at` defaults to now, in the BROWSER's timezone — the preview shows shape,
 * not the exact string a run will produce, which is rendered in the
 * schedule's own timezone by the server. An unknown token is left verbatim,
 * the same as the server does, so the preview shows the mistake rather than a
 * gap where the value should be.
 */
export function renderSubject(template: string, name: string, at: Date = new Date()): string {
  const weekday = at.toLocaleDateString("en-US", { weekday: "long" });
  const values: Record<string, string> = {
    date: `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`,
    time: `${pad(at.getHours())}:${pad(at.getMinutes())}`,
    datetime: `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`,
    day: pad(at.getDate()),
    month: pad(at.getMonth() + 1),
    year: String(at.getFullYear()),
    weekday,
    name,
  };
  return template.replace(/\{\{\s*([a-z_]+)\s*\}\}/g, (whole, key: string) =>
    key in values ? values[key] : whole,
  );
}
