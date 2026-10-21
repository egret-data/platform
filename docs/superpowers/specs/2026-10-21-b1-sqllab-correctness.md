# B1: SQL Lab correctness

Package B1 of the known-issues fix programme
(`docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`).

## Rows and what each one does

| Row | Change |
|---|---|
| KI-154, KI-081 | `run_query` no longer wraps a single query as `SELECT * FROM (<q>) AS _egret_lim LIMIT n`. A new `execution::limit` reads the SQL with `sqlparser` (GenericDialect for Trino, MySqlDialect for StarRocks, as the read-only gate does). Exactly one plain query with no `LIMIT`/`OFFSET`/`FETCH` of its own gets `\nLIMIT n+1` appended: its `ORDER BY` stays outermost and the connector's `n + 1` fetch sees the extra row, so `truncated` is exact. A query with its own limit is sent unchanged (the connector's fetch cap still decides `truncated`). SQL that cannot be read (an engine-only construct, or comment forms the engines and `sqlparser` read differently: `/*!`, a nested `/*`) keeps the old wrapper, now with `n + 1`. |
| KI-087 | The appended limit adds no line before the user's SQL, so engine positions are right. Where a wrapper remains (the unreadable fallback, and the `LIMIT 0` describe that dataset create and refresh use) the engine's `line N:` / `at line N, column` is mapped back by one line. The doubled `engine: engine:` came from `BiError::from(EngineError::Engine(m))` formatting the inner error; it now keeps the engine's own words. |
| KI-095 | Dataset create strips a trailing `;` (tokenizer-aware, so `';'` inside a string stays) before storing virtual SQL. Introspected columns with a repeated name are refused with a 400 that names the column, instead of the 500 from the unique key. |
| KI-211 | A SQL Lab run that hits the outer timeout fails with "the query ran longer than the N s limit for SQL Lab queries … the server setting `engine.query_timeout_secs`"; Trino's own "exceeded maximum time limit" gets the same hint. |
| KI-079 | The ⌘K/Ctrl+K listener is in the capture phase and stops the event, so Monaco never sees it as a chord prefix. Monaco chords that start with ⌘K are therefore not available in SQL Lab. |
| KI-078 | A cancelled run shows "Query cancelled" in the Results panel and "Cancelled after N s" in the status line and chip. |
| KI-086 | A refused submit is shown in the Results panel's error style under "Query not started", with the server's `error` text only. |
| KI-004 | `double`/`real`/`float` cells are shown rounded to 15 significant digits, which removes binary noise and nothing a double holds reliably. Copy, export, CSV and the expanded cell keep the exact value; integers and decimals are untouched. |
| KI-006 | Reproduced: with a 1,000-row result on screen, typing at 40 ms a key lost characters; at 20 ms the text was garbled. Cause: `@monaco-editor/react` writes every `value` it receives into the editor when it differs, and SQL Lab's value comes back through tab state one keystroke late. `SqlEditor` now passes `defaultValue` and writes only values it did not emit itself (Format, a snippet, a history entry). |

## Decisions

- Rounding rule for KI-004: 15 significant digits rather than a fixed number of decimals, so small values (`1.2e-10`) and large ones keep their meaning.
- The unreadable fallback keeps a wrapper rather than sending the SQL unchanged, so the engine's work stays bounded as before (on StarRocks the MySQL client may have to read an unbounded result to its end before the connection closes).
- No StarRocks engine is available on the demo stack: the StarRocks path is covered by unit tests of the MySQL dialect and by reading the connector, not live.
