import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { useDataApiValidateEndpoint } from "@/api/generated/data-api/data-api";
import { CacheScope } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

type Validation =
  | { ok: true; columns: { name: string; ty: string }[] }
  | { ok: false; message: string };

/**
 * Everything about the endpoint as an HTTP object: the URL contract callers use,
 * and the cache / limits / rate-limit policy applied to their calls. Nothing here
 * decides what the query returns — that is `Query`.
 *
 * One column, deliberately — the shell (`EndpointDetail.tsx`) sets `max-w-3xl`
 * around the `Outlet` so every section inherits it structurally. A two-column
 * grid pads the short groups to the height of the tall one beside them; CSS
 * `columns` fights the flex container this page sits in and clips the form at
 * the fold. Both were tried on this editor and both were wrong.
 */
export default function Config() {
  const { ws, id, form, setForm, dirty } = useEndpointDetail();
  const validateMutation = useDataApiValidateEndpoint();
  const [validation, setValidation] = useState<Validation | null>(null);

  async function runValidate() {
    setValidation(null);
    try {
      const res = await validateMutation.mutateAsync({ workspace: ws, id });
      if (res.status === 200) setValidation({ ok: true, columns: res.data.columns });
    } catch (e) {
      setValidation({ ok: false, message: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Config</h2>
        <p className="text-sm text-muted-foreground">
          The endpoint as an HTTP object: the URL contract callers use, and the cache, limits, and
          rate-limit policy applied to their calls.
        </p>
      </div>

      <ConfigSection title="Endpoint">
        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="cfg-name">Name</Label>
            <Input id="cfg-name" value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="cfg-description">Description</Label>
            <Textarea
              id="cfg-description"
              rows={2}
              value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="cfg-method">Method</Label>
              <Select value={form.method} onValueChange={(v) => setForm((f) => ({ ...f, method: v }))}>
                <SelectTrigger id="cfg-method" aria-label="Method">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="GET">GET</SelectItem>
                  <SelectItem value="POST">POST</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cfg-path">Path</Label>
              <Input id="cfg-path" value={form.path} onChange={(e) => setForm((f) => ({ ...f, path: e.target.value }))} />
            </div>
          </div>
        </div>
      </ConfigSection>

      <ConfigSection title="Cache">
        <ConfigTable>
          <ConfigTableRow
            label="Enabled"
            htmlFor="cfg-cache-enabled"
            details="Serve repeat calls from a cached response instead of re-running the query."
          >
            <Switch
              id="cfg-cache-enabled"
              checked={!!form.cache.enabled}
              onCheckedChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, enabled: v } }))}
            />
          </ConfigTableRow>
          {form.cache.enabled && (
            <>
              <ConfigTableRow
                label="TTL (seconds)"
                htmlFor="cfg-cache-ttl"
                details="How long a cached response stays valid. A call after this re-runs the query."
              >
                <Input
                  id="cfg-cache-ttl"
                  className="w-full"
                  type="number"
                  min={0}
                  value={form.cache.ttl_seconds ?? 0}
                  onChange={(e) =>
                    setForm((f) => ({ ...f, cache: { ...f.cache, ttl_seconds: Number(e.target.value) } }))
                  }
                />
              </ConfigTableRow>
              <ConfigTableRow
                label="Scope"
                htmlFor="cfg-cache-scope"
                details="Per principal caches separately for each service account, so Ranger-filtered rows never cross principals. Shared caches one copy for everyone — only safe when the connection does not run queries as the caller."
              >
                <Select
                  value={form.cache.scope ?? CacheScope.per_principal}
                  onValueChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, scope: v as CacheScope } }))}
                >
                  <SelectTrigger id="cfg-cache-scope" className="w-full" aria-label="Cache scope">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={CacheScope.per_principal}>Per caller</SelectItem>
                    {/* Refused at validate time on a delegated connection — Ranger filters
                        per principal, so a shared cache would leak across identities. The
                        picker still offers it (unlike `location`): this refusal depends on
                        the DATABASE, which can change out from under a saved choice, so
                        there is no static set of values to omit here. */}
                    <SelectItem value={CacheScope.shared}>Shared</SelectItem>
                  </SelectContent>
                </Select>
              </ConfigTableRow>
            </>
          )}
        </ConfigTable>
      </ConfigSection>

      <ConfigSection title="Limits">
        <ConfigTable>
          <ConfigTableRow
            label="Max rows"
            htmlFor="cfg-max-rows"
            details="Hard cap on rows returned. A result larger than this fails the call rather than being silently truncated."
          >
            <Input
              id="cfg-max-rows"
              className="w-full"
              type="number"
              min={0}
              placeholder="unlimited"
              value={form.limits.max_rows ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  limits: { ...f.limits, max_rows: e.target.value === "" ? undefined : Number(e.target.value) },
                }))
              }
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Timeout (seconds)"
            htmlFor="cfg-timeout"
            details="How long the engine may take before the call is abandoned."
          >
            <Input
              id="cfg-timeout"
              className="w-full"
              type="number"
              min={0}
              placeholder="default"
              value={form.limits.timeout_seconds ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  limits: {
                    ...f.limits,
                    timeout_seconds: e.target.value === "" ? undefined : Number(e.target.value),
                  },
                }))
              }
            />
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>

      <ConfigSection title="Rate limit">
        <ConfigTable>
          <ConfigTableRow
            label="Per caller (qps)"
            htmlFor="cfg-rl-per-key"
            details="Sustained requests per second allowed for each client credential."
          >
            <Input
              id="cfg-rl-per-key"
              className="w-full"
              type="number"
              min={0}
              placeholder="unlimited"
              value={form.rate_limit.per_key_qps ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  rate_limit: {
                    ...f.rate_limit,
                    per_key_qps: e.target.value === "" ? undefined : Number(e.target.value),
                  },
                }))
              }
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Burst"
            htmlFor="cfg-rl-burst"
            details="How many requests may arrive at once above the sustained rate before throttling starts."
          >
            <Input
              id="cfg-rl-burst"
              className="w-full"
              type="number"
              min={0}
              placeholder="unlimited"
              value={form.rate_limit.burst ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  rate_limit: { ...f.rate_limit, burst: e.target.value === "" ? undefined : Number(e.target.value) },
                }))
              }
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Endpoint-wide (qps)"
            htmlFor="cfg-rl-per-endpoint"
            details="Ceiling across all callers of this endpoint combined, whatever their individual limits."
          >
            <Input
              id="cfg-rl-per-endpoint"
              className="w-full"
              type="number"
              min={0}
              placeholder="unlimited"
              value={form.rate_limit.per_endpoint_qps ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  rate_limit: {
                    ...f.rate_limit,
                    per_endpoint_qps: e.target.value === "" ? undefined : Number(e.target.value),
                  },
                }))
              }
            />
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>

      <div className="space-y-2 border-t border-border pt-4">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-medium">Validate</h3>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={runValidate}
            disabled={validateMutation.isPending || dirty}
            title={dirty ? "Save your changes before validating — Validate checks the saved draft." : undefined}
          >
            {validateMutation.isPending ? "Validating…" : "Validate"}
          </Button>
        </div>
        {/* Inline, not a toast: a toast vanishes before an author who is mid-edit
            reads it, and a validation failure is exactly the moment they need
            the message to still be on screen. */}
        {validation &&
          (validation.ok ? (
            <Alert>
              <AlertDescription>
                Resolves to {validation.columns.length} column{validation.columns.length === 1 ? "" : "s"}
                {validation.columns.length > 0 &&
                  `: ${validation.columns.map((c) => `${c.name} (${c.ty})`).join(", ")}`}
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive">
              <AlertDescription>{validation.message}</AlertDescription>
            </Alert>
          ))}
      </div>
    </div>
  );
}
