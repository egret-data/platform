/** The SQL to execute: the selected text when it has non-whitespace content, else the full editor text. */
export function pickRunSql(fullText: string, selectedText: string | null | undefined): string {
  const sel = selectedText ?? "";
  return sel.trim() ? sel : fullText;
}
