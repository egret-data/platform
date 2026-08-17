import { useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import { useDataApiPreviewEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiPreviewDto } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/** The cap the backend applies (`preview::PREVIEW_MAX_ROWS`). Named so the copy
 *  below and the server cannot disagree about the number on screen. */
const PREVIEW_MAX_ROWS = 100;

/**
 * Runs the ON-SCREEN draft and shows the rows, without saving or publishing.
 *
 * **Test values are NOT part of the endpoint.** They live in this component's
 * own state, never in the shared `form`, so they cannot be saved by accident and
 * `dirty` cannot latch on typing one. That is also why they are inputs of their
 * own rather than a column added to `ParamEditor`'s rows: a test value beside a
 * declaration invites the reader to think it is stored with the endpoint.
 *
 * This duplicates `TryIt`'s input row, and that is accepted: the two sections
 * are never on screen together and they run as DIFFERENT principals — preview as
 * the author, a trial as a service account.
 */
export function PreviewBlock() {
  const { ws, id, form } = useEndpointDetail();
  const previewMutation = useDataApiPreviewEndpoint();
  const [values, setValues] = useState<Record<string, string>>({});
  const [result, setResult] = useState<DataApiPreviewDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function onPreview() {
    // Clear BOTH outcomes, not just the one this run is about to set —
    // otherwise a failing second run leaves the first run's table on screen
    // beside the error, showing two answers to "what happened" at once.
    setResult(null);
    setError(null);
    try {
      // One list per name, matching the serving path: a repeated parameter is a
      // list there too. A blank field is omitted rather than sent as `[""]`, so
      // an unfilled optional parameter behaves like it was never supplied.
      const sent: Record<string, string[]> = {};
      for (const p of form.params) {
        const v = values[p.name];
        if (v !== undefined && v !== "") sent[p.name] = [v];
      }
      const res = await previewMutation.mutateAsync({
        workspace: ws,
        id,
        data: {
          sql: form.sql,
          params: form.params,
          values: sent,
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
      // it is the only thing an author can act on.
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="space-y-3 border-t border-border pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium">Preview</h3>
        <Button type="button" size="sm" onClick={onPreview} disabled={previewMutation.isPending}>
          {previewMutation.isPending ? "Running…" : "Preview draft"}
        </Button>
      </div>

      <SettingsNote>
        Preview runs as you, against the database above, and returns up to {PREVIEW_MAX_ROWS} rows.
        It does not save, publish, spend a rate limit, fill the cache, or appear in Activity. Column
        names come from the engine, not from the published contract — see Response for the names a
        consumer receives. And because it runs as you rather than as a service account, a green
        preview does not prove a consumer can call this endpoint; Try it answers that.
      </SettingsNote>

      {form.params.length > 0 && (
        <div className="grid grid-cols-2 gap-3">
          {form.params.map((p) => (
            <div key={p.name} className="space-y-1.5">
              <Label htmlFor={`preview-param-${p.name}`}>
                {p.name}
                {p.required && <span className="text-destructive"> *</span>}
              </Label>
              <Input
                id={`preview-param-${p.name}`}
                value={values[p.name] ?? ""}
                placeholder={p.default ?? undefined}
                onChange={(e) => setValues((prev) => ({ ...prev, [p.name]: e.target.value }))}
              />
            </div>
          ))}
        </div>
      )}

      {/* Inline, not a toast — an author mid-edit needs the message to stay on
          screen while they fix the SQL it is about. */}
      {error && (
        <Alert variant="destructive">
          <AlertDescription className="font-mono text-xs">{error}</AlertDescription>
        </Alert>
      )}

      {result && (
        <div className="space-y-2">
          <p className="text-xs text-muted-foreground">
            {result.truncated
              ? `Showing the first ${PREVIEW_MAX_ROWS} rows — there are more.`
              : `${result.row_count} row${result.row_count === 1 ? "" : "s"}.`}
          </p>
          {/* Wide results scroll inside their own container; the page body must
              never scroll horizontally. */}
          <div className="max-h-72 overflow-auto rounded-md border border-border">
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
  );
}
