import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  useDataApiGetOpenapi,
  useDataApiListEndpointVersions,
  useDataApiListServiceAccounts,
} from "@/api/generated/data-api/data-api";
import { useEndpointDetail } from "../EndpointDetail";
import { DownloadOpenApiDialog } from "../DownloadOpenApiDialog";

/**
 * Just enough of the generated OpenAPI document to show the base URL. The
 * generated hook types the 200 body as `unknown` (it is assembled as
 * `serde_json::Value` server-side — see `openapi_gen.rs`'s module comment —
 * not through utoipa, so orval has no schema to generate a model from), so
 * this is a local, minimal shape rather than a generated one.
 */
interface OpenApiDocPreview {
  servers?: { url?: string }[];
}

/**
 * What a consumer actually receives when they call this endpoint: the frozen
 * `columns` of the LIVE version (never the draft — a draft can differ from
 * what is currently served, and this section exists to show the real
 * contract), the `output.shape`/`naming` controls that decide how those
 * columns are keyed in the response body, and the served base URL plus the
 * per-service-account OpenAPI download.
 */
export default function ResponseSection() {
  const { ws, id, endpoint, form, setForm } = useEndpointDetail();

  const versionsQuery = useDataApiListEndpointVersions(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const versionsEnvelope = versionsQuery.data;
  const versions = versionsEnvelope?.status === 200 ? versionsEnvelope.data : [];
  const liveVersion = versions.find((v) => v.is_live);
  // The endpoint record says a live version exists, but this section's own
  // fetch of it hasn't landed yet — distinct from "never published", which is
  // `!endpoint.live_version_id` below. Conflating the two would flash "no
  // live version" on every load of a perfectly-live endpoint.
  const columnsLoading = !!endpoint.live_version_id && versionsQuery.isLoading;
  // react-query clears `isLoading` on ANY settle, error included — so a fetch
  // failure for a live endpoint falls straight into the `!liveVersion`
  // branch below unless checked first, and reads as "never published" to an
  // author who has already published. Same class of bug this codebase fixed
  // once across four list pages (`Endpoints.tsx`'s own `isError` branch is
  // the pattern this mirrors) — the one section whose whole purpose is
  // representing server state accurately cannot re-introduce it.
  const columnsErrored = !!endpoint.live_version_id && versionsQuery.isError;

  const { data: accountsEnvelope } = useDataApiListServiceAccounts(ws);
  const accounts = accountsEnvelope?.status === 200 ? accountsEnvelope.data : [];
  // Any service account's document carries the same `servers[0].url` — it is
  // derived from `issuer` and the workspace slug, not from which account
  // asked — so the first one is enough to preview it. The download itself
  // (`DownloadOpenApiDialog`) still makes the author choose explicitly.
  const previewAccountId = accounts[0]?.id;

  const oaQuery = useDataApiGetOpenapi(
    ws,
    { service_account_id: previewAccountId ?? "", endpoint_id: id },
    { query: { enabled: !!previewAccountId } },
  );
  const oaEnvelope = oaQuery.data;
  const doc = oaEnvelope?.status === 200 ? (oaEnvelope.data as OpenApiDocPreview) : undefined;
  const baseUrl = doc?.servers?.[0]?.url;
  // The mutator throws the real error on any non-2xx, so react-query's
  // `error` carries it (never `data`) — see `apiFetch`/`mutator.ts`. Typed as
  // `ErrorBody` by the generated hook but actually an `Error` with `.status`,
  // same cast `EndpointDetail.tsx` and `DownloadOpenApiDialog.tsx` already use.
  const oaError = oaQuery.error as unknown as (Error & { status?: number }) | null | undefined;

  const [downloadOpen, setDownloadOpen] = useState(false);

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Response</h2>
        <p className="text-sm text-muted-foreground">
          What a consumer actually receives when they call this endpoint.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Frozen columns</CardTitle>
        </CardHeader>
        <CardContent>
          {columnsLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : columnsErrored ? (
            <Alert variant="destructive">
              <AlertDescription>
                Could not load the published contract.{" "}
                {(versionsQuery.error as unknown as Error)?.message ?? "Try again in a moment."}
              </AlertDescription>
            </Alert>
          ) : !endpoint.live_version_id || !liveVersion ? (
            <p className="text-sm text-muted-foreground">
              This endpoint has no live version yet. Publish it to see the columns a consumer will receive
              — these come from the published contract, not the draft.
            </p>
          ) : (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <th className="pb-1 font-medium">Name</th>
                  <th className="pb-1 font-medium">Type</th>
                </tr>
              </thead>
              <tbody>
                {liveVersion.snapshot.columns.map((c) => (
                  <tr key={c.name} className="border-t border-border">
                    <td className="py-1 font-mono text-xs">{c.name}</td>
                    <td className="py-1 text-muted-foreground">{c.ty}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Output shape</CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <Label htmlFor="resp-shape">Shape</Label>
            <Select
              value={form.output.shape ?? "objects"}
              onValueChange={(v) => setForm((f) => ({ ...f, output: { ...f.output, shape: v } }))}
            >
              <SelectTrigger id="resp-shape" aria-label="Shape">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="objects">Objects</SelectItem>
                <SelectItem value="arrays">Arrays</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="resp-naming">Key naming</Label>
            <Select
              value={form.output.naming ?? "as_is"}
              onValueChange={(v) => setForm((f) => ({ ...f, output: { ...f.output, naming: v } }))}
            >
              <SelectTrigger id="resp-naming" aria-label="Key naming">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="as_is">As-is</SelectItem>
                <SelectItem value="camel">camelCase</SelectItem>
                <SelectItem value="snake">snake_case</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">OpenAPI document</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">
            Generated per service account — the document lists exactly the endpoints that account may
            invoke. Choose one to download this endpoint&apos;s contract.
          </p>
          <div className="flex flex-wrap items-center gap-3">
            <Button type="button" variant="outline" onClick={() => setDownloadOpen(true)}>
              Download OpenAPI
            </Button>
            {baseUrl && <span className="font-mono text-xs text-muted-foreground">{baseUrl}</span>}
          </div>
          {oaQuery.isError && (
            <Alert variant="destructive">
              <AlertDescription>
                {oaError?.status === 409
                  ? oaError.message
                  : "Could not generate a preview of the served request."}
              </AlertDescription>
            </Alert>
          )}
        </CardContent>
      </Card>

      <DownloadOpenApiDialog ws={ws} open={downloadOpen} onOpenChange={setDownloadOpen} endpointId={id} />
    </div>
  );
}
