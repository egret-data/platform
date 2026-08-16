import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useDatabases } from "@/api/databases";
import { useDataApiValidateEndpoint } from "@/api/generated/data-api/data-api";
import { CacheScope } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";
import { ParamEditor } from "./ParamEditor";

type Validation =
  | { ok: true; columns: { name: string; ty: string }[] }
  | { ok: false; message: string };

/**
 * The query this endpoint serves, and how callers reach it: name/description,
 * method + path, database, SQL, parameters, cache, limits and rate limit.
 * Everything the create dialog (`Endpoints.tsx`) doesn't ask for up front.
 *
 * One column at `max-w-3xl` — deliberately. A `grid` two-column layout pads
 * the short sections (Limits, Rate limit) to the height of the tall one
 * beside them (Parameters), and CSS `columns` fights the flex container this
 * page sits in and clips the form at the fold; both were tried on a
 * comparable editor and both were wrong.
 */
export default function Definition() {
  const { ws, id, form, setForm, dirty } = useEndpointDetail();
  const { data: databases } = useDatabases(ws);
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
    <div className="max-w-3xl space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Definition</h2>
        <p className="text-sm text-muted-foreground">
          The query this endpoint serves, and how callers reach it.
        </p>
      </div>

      <div className="space-y-4">
        <div className="space-y-1.5">
          <Label htmlFor="def-name">Name</Label>
          <Input id="def-name" value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="def-description">Description</Label>
          <Textarea
            id="def-description"
            rows={2}
            value={form.description}
            onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <Label htmlFor="def-method">Method</Label>
            <Select value={form.method} onValueChange={(v) => setForm((f) => ({ ...f, method: v }))}>
              <SelectTrigger id="def-method" aria-label="Method">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="GET">GET</SelectItem>
                <SelectItem value="POST">POST</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="def-path">Path</Label>
            <Input id="def-path" value={form.path} onChange={(e) => setForm((f) => ({ ...f, path: e.target.value }))} />
          </div>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="def-db">Database</Label>
          <Select value={form.database_id} onValueChange={(v) => setForm((f) => ({ ...f, database_id: v }))}>
            <SelectTrigger id="def-db" aria-label="Database">
              <SelectValue placeholder="Choose a database" />
            </SelectTrigger>
            <SelectContent>
              {(databases ?? []).map((d) => (
                <SelectItem key={d.id} value={d.id}>
                  {d.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="def-sql">SQL</Label>
          <Textarea
            id="def-sql"
            className="font-mono text-xs"
            rows={8}
            value={form.sql}
            onChange={(e) => setForm((f) => ({ ...f, sql: e.target.value }))}
          />
        </div>
      </div>

      <ParamEditor params={form.params} onChange={(params) => setForm((f) => ({ ...f, params }))} />

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Cache</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <label htmlFor="def-cache-enabled" className="flex cursor-pointer items-center gap-2 text-sm">
            <Switch
              id="def-cache-enabled"
              checked={!!form.cache.enabled}
              onCheckedChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, enabled: v } }))}
            />
            Cache responses
          </label>
          {form.cache.enabled && (
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor="def-cache-ttl">TTL (seconds)</Label>
                <Input
                  id="def-cache-ttl"
                  type="number"
                  min={0}
                  value={form.cache.ttl_seconds ?? 0}
                  onChange={(e) =>
                    setForm((f) => ({ ...f, cache: { ...f.cache, ttl_seconds: Number(e.target.value) } }))
                  }
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="def-cache-scope">Scope</Label>
                <Select
                  value={form.cache.scope ?? CacheScope.per_principal}
                  onValueChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, scope: v as CacheScope } }))}
                >
                  <SelectTrigger id="def-cache-scope" aria-label="Cache scope">
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
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Limits</CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <Label htmlFor="def-max-rows">Max rows</Label>
            <Input
              id="def-max-rows"
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
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="def-timeout">Timeout (seconds)</Label>
            <Input
              id="def-timeout"
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
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Rate limit</CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-3 gap-3">
          <div className="space-y-1.5">
            <Label htmlFor="def-rl-per-key">Per caller (qps)</Label>
            <Input
              id="def-rl-per-key"
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
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="def-rl-burst">Burst</Label>
            <Input
              id="def-rl-burst"
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
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="def-rl-per-endpoint">Endpoint-wide (qps)</Label>
            <Input
              id="def-rl-per-endpoint"
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
          </div>
        </CardContent>
      </Card>

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
