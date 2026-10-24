/** "1 row", "2 rows", "1,000 rows": a row count in the singular when it is one (KI-161). */
export function rowsLabel(n: number): string {
  return `${n.toLocaleString()} ${n === 1 ? "row" : "rows"}`;
}

/** Append a name clicked in the Explorer to the editor text, with a space when
 *  it would otherwise run into the word before it (KI-084): `SELECT` + a table
 *  is `SELECT t`, not `SELECTt`. No space after whitespace, an opening
 *  parenthesis or a dot, where the name already starts a new token. */
export function appendName(sql: string, name: string): string {
  return sql === "" || /[\s(.]$/.test(sql) ? sql + name : `${sql} ${name}`;
}
