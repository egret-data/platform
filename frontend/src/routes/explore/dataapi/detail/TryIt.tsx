import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import { useDataApiListServiceAccounts, useDataApiTryEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiServiceAccountDto, TryRequestParams, TryResponse } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/**
 * Best-effort extraction of the refusal's machine-readable code/message from
 * `TryResponseBody`. Typed as an open map (`{[key: string]: unknown}`)
 * because the route hands the serving path's body back VERBATIM — rows and
 * `meta` on success, an error shape on a refusal — so this file cannot lean
 * on a generated model for it. Handles both the documented top-level
 * `{code, message, request_id}` shape and an `{error: {code, message}}`
 * wrapper (the shape `ErrorBody` uses elsewhere in this API), rather than
 * assuming one and silently showing nothing useful if the body arrives
 * shaped the other way.
 */
function extractRefusal(body: Record<string, unknown>): { code?: string; message?: string } {
  const err = body.error;
  if (err && typeof err === "object") {
    const e = err as Record<string, unknown>;
    return {
      code: typeof e.code === "string" ? e.code : undefined,
      message: typeof e.message === "string" ? e.message : undefined,
    };
  }
  return {
    code: typeof body.code === "string" ? body.code : undefined,
    message: typeof body.message === "string" ? body.message : typeof err === "string" ? err : undefined,
  };
}

/** Copies a value to the clipboard — same "Copy"/"Copied" idiom `WarehouseDetail.tsx` uses. */
function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/**
 * `TryIt` — a real trial run against the real serving router (spec §3.2), not
 * a simulation. The `/try` route mints a genuine 60-second service-account
 * token server-side and drives it through the actual serving path, so a
 * green result here is evidence a real consumer's call would work, not a
 * guess. The route itself always answers `200` once the trial ran — the
 * endpoint's OWN status (`200`, a `403 not_granted` refusal, …) lives inside
 * the response body, and is rendered here as a RESULT, never as a page-level
 * failure.
 */
export default function TryIt() {
  const { ws, id, endpoint } = useEndpointDetail();
  const [searchParams] = useSearchParams();
  // Task 5's Access section links here with `?service_account=<id>` to
  // preselect the account a row could not answer for — read that EXACT key.
  // Nothing fails if this key were different; it would just silently not
  // preselect, which is why the contract is written down on both sides.
  const preselected = searchParams.get("service_account") ?? "";

  const accountsQuery = useDataApiListServiceAccounts(ws);
  const accountsEnvelope = accountsQuery.data;
  const accounts: DataApiServiceAccountDto[] =
    accountsEnvelope?.status === 200 ? accountsEnvelope.data : [];

  const [accountId, setAccountId] = useState(preselected);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  const tryMutation = useDataApiTryEndpoint();
  const [result, setResult] = useState<TryResponse | null>(null);
  const [tooLarge, setTooLarge] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);

  const accessHref = `/explore/${ws}/data-api/endpoints/${id}/access`;
  const activityHref = `/explore/${ws}/data-api/activity`;

  async function onRun() {
    // Clear every prior outcome up front, not just the one this branch is
    // about to set — otherwise a second run that hits `413` (or errors)
    // would leave a PREVIOUS run's success `result` Card rendered alongside
    // the new outcome, showing two different answers to "what happened" at
    // once.
    setResult(null);
    setTooLarge(false);
    setRunError(null);
    try {
      // A `Vec` per key because a repeated parameter is a LIST (`TryRequestParams`'s
      // own doc) — a blank field is simply omitted rather than sent as `[""]`, so an
      // unfilled optional parameter behaves like it was never supplied.
      const params: TryRequestParams = {};
      for (const p of endpoint.params) {
        const v = paramValues[p.name];
        if (v !== undefined && v !== "") params[p.name] = [v];
      }
      const res = await tryMutation.mutateAsync({
        workspace: ws,
        id,
        data: { service_account_id: accountId, params },
      });
      // The mutator throws on any non-2xx (see `client.ts`/`mutator.ts`) — the
      // `/try` route itself only ever resolves 200 (the trial ran); the
      // endpoint's OWN status lives in `res.data.status`, handled below. Same
      // 2xx-narrowing trick every other section in this feature uses.
      if (res.status !== 200) {
        setRunError("Could not run the trial.");
        return;
      }
      setResult(res.data);
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 413) {
        // The serving path's own body was too large for THIS route to carry
        // back — not a failure of the trial, which still ran for real.
        setTooLarge(true);
      } else {
        setRunError(err.message || "Could not run the trial.");
      }
    }
  }

  const outcomeOk = result ? result.status >= 200 && result.status < 300 : false;
  const refusal = result ? extractRefusal(result.body) : undefined;
  const notGranted = !!result && result.status === 403 && refusal?.code === "not_granted";

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Try it</h2>
        <p className="text-sm text-muted-foreground">
          Runs a real call through the actual serving router, as a chosen service account — this answers
          what a real consumer would receive, not a simulation.
        </p>
      </div>

      <SettingsNote>
        A trial is a real call, not a dry run. It spends the endpoint-wide rate-limit bucket for real —
        the per-client bucket it counts against is only this trial&apos;s own, so a click here cannot
        throttle a live consumer. It also writes to the access log and appears in Activity&apos;s call
        list under &quot;Include trial runs&quot;. And on a cached endpoint it can populate the cache real
        consumers read, including a per-account (
        <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">per_principal</code>) entry for
        this same account, not only <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">shared</code>{" "}
        ones.
      </SettingsNote>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Run as</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="try-account">Service account</Label>
            <Select value={accountId} onValueChange={setAccountId}>
              <SelectTrigger id="try-account" aria-label="Service account">
                <SelectValue placeholder="Choose a service account" />
              </SelectTrigger>
              <SelectContent>
                {accounts.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    {a.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {accounts.length === 0 && !accountsQuery.isLoading && (
              <p className="text-xs text-muted-foreground">
                This workspace has no service accounts yet. Create one, then grant it invoke, in{" "}
                <Link className="underline" to={accessHref}>
                  Access
                </Link>
                .
              </p>
            )}
          </div>

          {endpoint.params.length > 0 && (
            <div className="grid grid-cols-2 gap-3">
              {endpoint.params.map((p) => (
                <div key={p.name} className="space-y-1.5">
                  <Label htmlFor={`try-param-${p.name}`}>
                    {p.name}
                    {p.required && <span className="text-destructive"> *</span>}
                  </Label>
                  <Input
                    id={`try-param-${p.name}`}
                    value={paramValues[p.name] ?? ""}
                    placeholder={p.default ?? undefined}
                    onChange={(e) => setParamValues((prev) => ({ ...prev, [p.name]: e.target.value }))}
                  />
                </div>
              ))}
            </div>
          )}

          <Button type="button" onClick={onRun} disabled={!accountId || tryMutation.isPending}>
            {tryMutation.isPending ? "Running…" : "Run"}
          </Button>
        </CardContent>
      </Card>

      {runError && (
        <Alert variant="destructive">
          <AlertDescription>{runError}</AlertDescription>
        </Alert>
      )}

      {tooLarge && (
        <Alert>
          <AlertDescription>
            This result is too large to display here. The call still ran for real — its rate-limit,
            access-log and cache effects all happened — check{" "}
            <Link className="underline" to={activityHref}>
              Activity
            </Link>{" "}
            to see it.
          </AlertDescription>
        </Alert>
      )}

      {result && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Result</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <dl className="flex items-center gap-2 text-sm">
              <dt className="text-muted-foreground">Status</dt>
              <dd>
                <Badge className={outcomeOk ? "bg-success/15 text-success" : undefined}>{result.status}</Badge>
              </dd>
            </dl>

            {notGranted ? (
              <Alert variant="destructive">
                <AlertDescription>
                  This service account is not granted invoke on this endpoint — the most common first
                  failure.{" "}
                  <Link className="underline" to={accessHref}>
                    Grant it in Access
                  </Link>
                  .
                </AlertDescription>
              </Alert>
            ) : (
              !outcomeOk && (
                <Alert variant="destructive">
                  <AlertDescription>
                    The endpoint refused this call{refusal?.code ? ` (${refusal.code})` : ""}.{" "}
                    {refusal?.message ?? "See the body below for details."}
                  </AlertDescription>
                </Alert>
              )
            )}

            <div className="space-y-1.5">
              <Label>Body</Label>
              <pre className="max-h-64 overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs">
                {JSON.stringify(result.body, null, 2)}
              </pre>
            </div>

            <div className="space-y-1.5">
              <Label>Request</Label>
              <div className="space-y-2 rounded-md border border-border bg-muted/40 p-3 font-mono text-xs">
                <div>
                  {result.request.method} {result.request.url}
                </div>
                <div>Authorization: {result.request.authorization}</div>
                <div className="flex items-start gap-2">
                  <pre className="flex-1 overflow-x-auto whitespace-pre-wrap break-all">
                    {result.request.curl}
                  </pre>
                  <CopyButton value={result.request.curl} />
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
