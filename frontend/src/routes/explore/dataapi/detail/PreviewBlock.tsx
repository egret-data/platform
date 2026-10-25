import { forwardRef, useImperativeHandle, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Caveat } from "./Caveat";
import { useDataApiPreviewEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiPreviewDto, StatementSource } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/** The cap the backend applies (`preview::PREVIEW_MAX_ROWS`). Named so the copy
 *  below and the server cannot disagree about the number on screen. */
const PREVIEW_MAX_ROWS = 100;

/**
 * Runs the ON-SCREEN statement and shows the rows, without saving or publishing.
 *
 * ONE statement RUNS, but the request describes the whole endpoint's SHAPE:
 * `POST /preview` takes the source on screen — the SQL, or the script that
 * builds it — plus the other statements' text as `siblings`, and an endpoint
 * has N of them since M2. `Query` owns which one is on screen and passes both
 * down — see [`PreviewBlockProps`].
 *
 * **Test values are NOT part of the endpoint.** They live in this component's
 * own state, never in the shared `form`, so they cannot be saved by accident and
 * `dirty` cannot latch on typing one. That is also why they are inputs of their
 * own rather than a column added to `ParamEditor`'s rows: a test value beside a
 * declaration invites the reader to think it is stored with the endpoint.
 *
 * This duplicates `TryIt`'s input row, and that is accepted: the two sections
 * are never on screen together and they answer different questions. A trial
 * authenticates a real token for a chosen SERVICE ACCOUNT, is authorized by that
 * account's invoke grant, spends the rate limit and runs the PUBLISHED version;
 * a preview authenticates nothing, checks no grant, and runs the SQL on screen
 * under the endpoint's connection identity — the author only when that
 * connection is delegated or impersonating, and the same shared service account
 * otherwise (`egret-query/src/execution/mod.rs`, `delegation_for`).
 */
/** What `Query` can drive from outside: run the preview. The SQL editor's
 *  ⌘↵ command calls this, so the affordance that makes the split pane worth
 *  having reaches the button without lifting this component's state (the test
 *  values, the result and the error all stay private — see the doc above for
 *  why that matters). */
export interface PreviewHandle {
  run: () => void;
}

export interface PreviewBlockProps {
  /**
   * The statement to run — its source as it is on screen right now, handed
   * down rather than read from `form`.
   *
   * `POST /preview` runs ONE statement, not an endpoint. An endpoint has N of
   * them (M2), and this component cannot know which one the author is looking
   * at — `Query` owns that selection, so `Query` supplies the source.
   *
   * A `StatementSource`, not a `string`: package S gave a statement two
   * sources, `{sql}` **or** `{script}`, and the preview route reads the same
   * two-case shape with the same hand-written reader — both keys, neither key
   * or an unknown key is a 422. Taking the whole source and spreading it into
   * the body is what keeps that exact; a `sql: string` prop would have had to
   * carry `""` for a scripted statement, which is a body the server refuses
   * and, before it refused, a preview of an empty statement.
   */
  source: StatementSource;
  /**
   * The OTHER statements' SQL, in any order — empty for a one-statement
   * endpoint.
   *
   * Not a second thing to run. Whether a parameter arrives as a scalar or a
   * list is a property of the whole endpoint (one `{% for %}` anywhere makes it
   * a list), so a preview that saw only `sql` judged that from the statement on
   * screen while publish judged it from all of them: green here, refused there.
   * The server reads these for that decision alone — it never renders, binds or
   * executes them.
   *
   * Sent unfiltered rather than "only the ones that mention a parameter": the
   * rule is the server's, and a client that pre-filtered would be a second copy
   * of it that can go stale.
   */
  siblings: string[];
  /**
   * The name of that statement, or `undefined` for a one-statement endpoint.
   *
   * `undefined` is not "unknown" — it means the endpoint has a single statement
   * whose name no caller ever reads (its `data` is an array, not a keyed
   * object), so naming it here would put a word on screen that looks like part
   * of the contract and is not one. With several, the note has to say WHICH of
   * them the button runs, because the button runs exactly one.
   */
  statementName?: string;
}

export const PreviewBlock = forwardRef<PreviewHandle, PreviewBlockProps>(function PreviewBlock(
  { source, siblings, statementName },
  ref,
) {
  const { ws, id, form } = useEndpointDetail();
  const previewMutation = useDataApiPreviewEndpoint();
  // Keyed by the declaration's POSITION, not by its name — a name is not unique
  // while the author is editing (see the render comment below).
  const [values, setValues] = useState<Record<number, string>>({});
  const [result, setResult] = useState<DataApiPreviewDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function onPreview() {
    // ⌘↵ reaches this without passing the button, so the button's own
    // `disabled` is not the guard — re-check here or a held shortcut fires a
    // second run over the first.
    if (previewMutation.isPending) return;
    // Clear BOTH outcomes, not just the one this run is about to set —
    // otherwise a failing second run leaves the first run's table on screen
    // beside the error, showing two answers to "what happened" at once.
    setResult(null);
    setError(null);
    try {
      // One list per NAME, matching the serving path: a repeated parameter is a
      // list there too, and this is the wire format the route parses. A blank
      // field is omitted rather than sent as `[""]`, so an unfilled optional
      // parameter behaves like it was never supplied.
      //
      // The state above is keyed by position, so two declarations sharing a name
      // hold two values while the server can only be told one. The LAST FILLED
      // one wins (a later blank does not erase an earlier value, because blanks
      // are skipped): it is the row the author reached most recently, and the
      // one nearest the button they pressed. Little rides on the choice —
      // `validate_preview` refuses a duplicate declaration outright
      // (`ValidationError::DuplicateParameter`), so a body like this cannot
      // produce a green preview whichever value it carries.
      const sent: Record<string, string[]> = {};
      form.params.forEach((p, i) => {
        const v = values[i];
        if (v !== undefined && v !== "") sent[p.name] = [v];
      });
      const res = await previewMutation.mutateAsync({
        workspace: ws,
        id,
        data: {
          // Spread, so the body carries EXACTLY the one source key the
          // statement has — `sql` for a written one, `script` for a generated
          // one. A body with both is refused, and a body with neither has
          // nothing to run.
          ...source,
          // The shape input, not a second statement — see `PreviewBlockProps`.
          siblings,
          params: form.params,
          values: sent,
          // Required by the route, and it is the DRAFT's `output` rather than
          // the saved endpoint's — a transform added since the last Save is
          // exactly the state preview exists to show. The server checks its
          // `transform` with publish's own rule and applies none of it; see the
          // note the answer's `transform_declared` turns on below.
          output: form.output,
          database_id: form.database_id,
        },
      });
      // The mutator throws on any non-2xx, so this is the only branch reached at
      // runtime — the check just narrows TS's view of the response union, the
      // same trick every other section in this feature uses.
      if (res.status !== 200) {
        setError("Could not run the preview.");
        return;
      }
      setResult(res.data);
    } catch (e) {
      // The engine's own message, verbatim — that is what the route returns and
      // it is the only thing an author can act on. But the message can be EMPTY:
      // `UnauthorizedError` (api/client.ts) carries none at all on a 401, and
      // `ApiError.from` falls back to `res.statusText`, which is always `""`
      // over HTTP/2 — so an axum extractor rejection, an ingress 413, or a
      // 502 HTML page all arrive with nothing to say. Without the `||` fallback
      // this set state, cleared the previous rows, and rendered NOTHING, which
      // is indistinguishable from a dead button. `TryIt` guards this the same way.
      setError(
        (e instanceof Error ? e.message : String(e)) ||
          "Could not run the preview. Your session may have expired — reload the page and try again.",
      );
    }
  }

  // No dependency array on purpose: the handle must close over the CURRENT
  // form and test values, and this component re-renders on every keystroke in
  // either. A frozen array here would run yesterday's draft.
  useImperativeHandle(ref, () => ({ run: () => void onPreview() }));

  return (
    // A panel, not a block appended to a scrolling page: the header (the button
    // and the shortcut that reaches it) stays put while the note, the test
    // values and the rows scroll underneath.
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-1 pb-2">
        <h3 className="text-sm font-medium">Preview</h3>
        <Button
          type="button"
          size="sm"
          onClick={onPreview}
          disabled={previewMutation.isPending}
          title="Run the preview (⌘/Ctrl + ↵ in the editor)"
        >
          {/* Names the VERSION it runs, like its two siblings do. Beside
              "Validate saved draft" and "Publish saved draft", a bare "draft"
              reads as the same saved thing those two mean by it — and spec §3.4's
              stated failure is exactly that inference: preview, see green,
              publish, get something else. */}
          {previewMutation.isPending ? "Running…" : "Preview unsaved draft"}
        </Button>
        {/* The shortcut is the whole reason the editor and the results are on
            screen together; unwritten, it is a feature only its author knows
            about. Named beside the button it fires, not in a help dialog. */}
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono">⌘↵</kbd>
          in the editor
        </span>
      </div>

      <div className="min-h-0 flex-1 space-y-3 overflow-auto px-1 pt-3">
        {/* The note travels WITH the button — and the HEADLINE is what that
            rule is about. A reader who has to scroll, or open something, to
            learn what the button does has already pressed it, so the one
            sentence that changes what an author concludes from a green run is
            on screen with nothing clicked. The other four are behind it: as
            standing prose this was ~570 characters over one button, in the
            panel that also has to hold the rows.

            A native `<details>` (the shared `Caveat`), never `ui/collapsible`:
            `Query` renders inside the shell's `<fieldset disabled={trashed}>`,
            which kills a Radix trigger, and reading is not editing. */}
        <Caveat
          summary="A green preview does not prove a consumer can call this endpoint"
          className="max-w-prose text-xs"
        >
          Preview runs the {statementName ? <>statement <span className="font-mono">{statementName}</span></> : "SQL"}{" "}
          and parameters as they are on screen right now — including edits you
          have not saved — against the connection above, and returns up to {PREVIEW_MAX_ROWS}{" "}
          rows. It does not save, publish, spend a rate limit, fill the cache, or appear in Activity.
          Column names come from the engine, not from the published contract — see Response for the names a
          consumer receives. It runs under the connection&apos;s own identity — as you only when
          that connection delegates or impersonates, otherwise as the shared service account — and it
          authenticates no token, checks no invoke grant and spends no rate limit. Try it is what answers
          whether a consumer can call this endpoint.
        </Caveat>

        {form.params.length > 0 && (
          <div className="grid grid-cols-2 gap-3">
            {/* Keyed and id'd by INDEX, not by `p.name`. A name is not unique while
                the author is editing: two freshly-added rows are both blank, and a
                typo can duplicate an existing name — which with a name key gave
                duplicate React keys, duplicate DOM ids, and one input visibly
                driving both rows. Index is safe here because this list is not
                reorderable and each input is fully controlled from `values` (see
                `ParamEditor`, which DOES need generated row ids: its rows can be
                removed from the middle and hold uncontrolled Radix/local state).

                `values` is keyed by index for the same reason. Keying the STATE by
                name was the same collision one level down, and rendering by index
                did not fix it: two rows sharing a name still read and wrote one
                entry, so the second field echoed the first keystroke by keystroke
                — including the ordinary case of two freshly-added rows, which are
                both blank. A row id generated the way `ParamEditor` does it would
                also work, but this component does not OWN `form.params` (it reads
                them from context, and they change under it as the author edits the
                declarations), so keeping such ids in step would take the
                `useEffect`-that-calls-`setState` shape this feature avoids.

                Considered and deliberately left, in the shape index keying gives
                it: a test value belongs to a SLOT, not to a declaration, so
                removing or reordering a declaration leaves each surviving value
                where it was and it now sits beside whichever declaration slid into
                that position. (Keying by name instead traded this for a value that
                outlives its declaration and reappears when a new one reuses the
                name — and cost the collision above.) Cosmetic either way: the
                value is on screen, is never saved, and is one keystroke to
                clear. */}
            {form.params.map((p, i) => (
              <div key={i} className="space-y-1.5">
                <Label htmlFor={`preview-param-${i}`}>
                  {p.name}
                  {p.required && <span className="text-destructive"> *</span>}
                </Label>
                <Input
                  id={`preview-param-${i}`}
                  value={values[i] ?? ""}
                  placeholder={p.default ?? undefined}
                  onChange={(e) => setValues((prev) => ({ ...prev, [i]: e.target.value }))}
                />
              </div>
            ))}
          </div>
        )}

        {/* Inline, not a toast — an author mid-edit needs the message to stay on
            screen while they fix the SQL it is about.

            `whitespace-pre-wrap`, and it is what makes the monospace font mean
            anything. Refusals here are no longer all one-liners: a transform's
            Starlark parse error and its run-time traceback are ASCII diagrams
            whose caret line has to sit UNDER the token it points at —

              error: Parse error: unexpected new line, expected symbol ':'
               --> transform.star:1:34
                |
              1 | def transform(params, statements)
                |

            Measured in Chrome on this branch before this changed: the default
            `white-space: normal` collapsed every newline and every run of
            spaces, so the whole diagram printed as one line ending
            `... | 1 | def transform(params, statements) | |`, with the caret
            row reduced to a stray pipe and the alignment — the only thing it
            carries — gone. `pre-wrap`, not `pre`: a long line still wraps at
            the container edge rather than scrolling the page sideways
            (`docs/en/ui-conventions.md` §10). Covered by
            `tests/e2e/data-api-transform-refusal-legibility.spec.ts`, which
            counts the rendered line boxes in a real browser; jsdom has no
            layout and no Tailwind, so it would pass either way. */}
        {error && (
          <Alert variant="destructive">
            <AlertDescription className="whitespace-pre-wrap font-mono text-xs">
              {error}
            </AlertDescription>
          </Alert>
        )}

        {result && (
          <div className="space-y-2">
            {/* The one case where the gap between what preview shows and what a
                consumer receives is not a renaming but a different value of a
                different shape — so it is stated ON the rows, not in the note
                beside the button, which an author has already scrolled past.

                Preview cannot run the script and does not pretend to: it runs
                ONE statement, and `transform(params, statements)` is given
                every statement's rows keyed by name. Handing it this one
                statement with the others empty would refuse endpoints that
                serve fine, and — worse — return a green answer computed from
                `[]`. `Try it` drives the real serving router, so that is where
                the transformed response actually exists. */}
            {result.transform_declared && (
              <Alert>
                <AlertDescription className="text-xs">
                  This endpoint builds its response with a transform. The rows below are the
                  statement&rsquo;s own — the script&rsquo;s <em>input</em>, not the response a
                  consumer receives. Preview runs one statement and the script is given every
                  statement&rsquo;s rows, so it cannot run the script here; Try it runs the
                  published endpoint for real and shows the transformed response.
                </AlertDescription>
              </Alert>
            )}
            {/* The template the statement's script built, `{{ }}` intact.
                Showing it is the point of previewing a script at all: a script
                that branches emits a different statement per parameter value,
                and without this the only way to see which one a draft produced
                was to publish it and read the engine's error.

                Above the rows, not below them: it is what RAN, and an author
                reading an unexpected result reads the statement first.

                `whitespace-pre-wrap` for the reason the error alert above has
                it — a generated statement is many lines and a normal
                `white-space` collapses every one of them into a paragraph.
                Its own `overflow-x-auto` box, so a long line scrolls here
                rather than sideways-scrolling the page
                (`docs/en/ui-conventions.md` §10). */}
            {result.generated_sql != null && (
              <div className="space-y-1">
                <p className="text-xs font-medium">SQL this script built</p>
                <p className="text-xs text-muted-foreground">
                  What <span className="font-mono">build_sql</span> returned for the test values
                  above — a template, before any render, so{" "}
                  <span className="font-mono">{"{{ }}"}</span> is still there and is bound the
                  same way a statement you typed would be. Another value can build another
                  statement; this is the one your values reach.
                </p>
                <pre className="overflow-x-auto whitespace-pre-wrap rounded-md border border-border bg-muted/40 p-2 font-mono text-xs">
                  {result.generated_sql}
                </pre>
              </div>
            )}
            <p className="text-xs text-muted-foreground">
              {result.truncated
                ? `Showing the first ${PREVIEW_MAX_ROWS} rows — there are more.`
                : `${result.row_count} row${result.row_count === 1 ? "" : "s"}.`}
            </p>
            {/* Wide results scroll inside their own container; the page body must
                never scroll horizontally. */}
            <div className="overflow-x-auto rounded-md border border-border">
              <table className="w-full text-xs">
                <thead className="sticky top-0 bg-muted/60">
                  <tr>
                    {result.columns.map((c) => (
                      <th key={c.name} className="px-2 py-1.5 text-left font-medium">
                        {c.name}
                        <span className="ml-1 font-normal text-muted-foreground">{c.type}</span>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {result.rows.map((row, ri) => (
                    <tr key={ri} className="border-t border-border">
                      {row.map((cell, ci) => (
                        <td key={ci} className="px-2 py-1 font-mono">
                          {cell === null ? (
                            <span className="text-muted-foreground">null</span>
                          ) : (
                            String(cell)
                          )}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}
      </div>
    </div>
  );
});
