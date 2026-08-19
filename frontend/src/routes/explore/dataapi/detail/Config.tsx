import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
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
 * ONE `Setting | Details | Value` table, its four groups being `ConfigTableGroup`
 * rows. This replaced four `ConfigSection` cards, each framing a table of one or
 * two rows — read down the page, that was a column of mostly-empty boxes. The
 * identity fields (name, description, method, path) were a separate form above
 * those cards and are now ordinary rows, so every setting on the tab reads the
 * same way and the Details column can explain them.
 *
 * One column, deliberately: a two-column grid pads the short groups to the height
 * of the tall one beside them, and CSS `columns` fights the flex container this
 * page sits in and clips the form at the fold. Both were tried on this editor and
 * both were wrong.
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

      {/* Removing the page-level cap did NOT help the controls on its own:
          `ConfigTable` is `table-fixed`, so all surplus width goes to the one
          unsized column — Details — and Value stays pinned wherever its `<th>`
          puts it. This tab owns the two widest controls of any detail section
          (see Description and Path below), so it opts into a wider one.

          `lg:` and not plain `w-96`, which is a measurement, not a hunch. Below
          `lg` the Details column is hidden but the two sized columns do NOT
          shrink to fit — `table-fixed` honours their pixel widths and the
          shadcn `Table` wrapper (`overflow-x-auto`) scrolls instead. At a 500px
          viewport the content box is 356px, so the table already scrolled 140px
          at `w-52`; a flat `w-96` would have pushed that to 316px. Gating the
          widening at `lg` leaves the narrow regime exactly as it was and takes
          the space only where there is space to take.

          Between those two regimes there is a third, and it has a PRE-EXISTING
          defect worth knowing about before anyone "fixes" the widths here.
          `ConfigTableGroup` spans `colSpan={3}`, which keeps the table three
          columns wide even where Details is `display:none` — so fixed layout
          hands the leftover to a phantom third column with no first-row cell,
          and it lands as dead space to the RIGHT of Value. Measured at a 1010px
          viewport (content box 706px): columns 288/0/208 with 210px dead.
          Deleting the four group rows from that same live table gives 410/0/296
          and no dead space. This is not caused by the width pass — the old
          `max-w-3xl` (768px) never bound below `lg`, where the sidebar already
          holds the content box under ~720px — and `lg:` gating neither creates
          nor cures it. Fixing it means making Value the unsized column below
          `lg`, which changes the shared default for eleven other consumers and
          belongs in its own change. */}
      <ConfigTable valueClassName="w-52 lg:w-96" detailsClassName="max-w-prose">
        <ConfigTableGroup label="Endpoint" />
        <ConfigTableRow
          label="Name"
          htmlFor="cfg-name"
          details="Shown wherever this endpoint is listed, and used as the operation summary in the generated OpenAPI document."
        >
          <Input
            id="cfg-name"
            className="w-full"
            value={form.name}
            onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
          />
        </ConfigTableRow>
        {/* Description and Path are the two widest controls on the tab. At the
            shared `w-52` (208px) a 2-row Textarea showed about 50 characters
            and the Path input about 25 before it scrolled — the reason this
            table passes a wider `valueClassName` above. Measured after: both
            controls render 368px at a 1728px viewport. */}
        <ConfigTableRow
          label="Description"
          htmlFor="cfg-description"
          details="Free text explaining what the endpoint returns. It becomes the operation description in the generated OpenAPI document."
        >
          <Textarea
            id="cfg-description"
            className="w-full"
            rows={2}
            value={form.description}
            onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
          />
        </ConfigTableRow>
        <ConfigTableRow label="Method" htmlFor="cfg-method" details="The HTTP verb callers use.">
          <Select value={form.method} onValueChange={(v) => setForm((f) => ({ ...f, method: v }))}>
            <SelectTrigger id="cfg-method" className="w-full" aria-label="Method">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="GET">GET</SelectItem>
              <SelectItem value="POST">POST</SelectItem>
            </SelectContent>
          </Select>
        </ConfigTableRow>
        <ConfigTableRow
          label="Path"
          htmlFor="cfg-path"
          details="Appended to the workspace's Data API base URL to form the URL callers call."
        >
          <Input
            id="cfg-path"
            className="w-full"
            value={form.path}
            onChange={(e) => setForm((f) => ({ ...f, path: e.target.value }))}
          />
        </ConfigTableRow>

        <ConfigTableGroup label="Cache" />
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

        <ConfigTableGroup label="Limits" />
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

        <ConfigTableGroup label="Rate limit" />
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
            {validateMutation.isPending ? "Validating…" : "Validate saved draft"}
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
