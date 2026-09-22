import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import {
  useControlPlaneCatalogRegistryCatalogImpact,
  useControlPlaneCatalogRegistryUpdateRegistryCatalog,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { RegistryCatalogDto } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { ConfigSection } from "@/components/form/ConfigSection";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { FieldLabel } from "../config/FieldLabel";
import { catalogKey, catalogsQueryKey } from "../queryKeys";
import { isEgretWarehouse } from "./Catalogs";
import {
  AdvancedEditor,
  type AdvancedIssue,
  type AdvancedValue,
  parseAdvancedIssue,
} from "./kinds/AdvancedEditor";
import {
  GENERIC_REQUIRED_KEY,
  KindForm,
  cleanAdvanced,
  cleanConfig,
  submitProblem,
} from "./kinds/forms";
import { REGISTRY_KINDS } from "./kinds/pickers";

/** `advanced` off the DTO (an untyped JSON object) as the per-engine maps. */
export function advancedMaps(advanced: unknown): AdvancedValue {
  const out: AdvancedValue = {};
  const o = advanced && typeof advanced === "object" ? (advanced as Record<string, unknown>) : {};
  for (const engine of ["trino", "starrocks"] as const) {
    const m = o[engine];
    if (!m || typeof m !== "object") continue;
    const entries = Object.entries(m as Record<string, unknown>).map(
      ([k, v]) => [k, typeof v === "string" ? v : String(v)] as const,
    );
    if (entries.length > 0) out[engine] = Object.fromEntries(entries);
  }
  return out;
}

/**
 * The stored `config` as an object the forms can read.
 *
 * `RegistryCatalogDtoConfig` is `{ [key: string]: unknown }`, which TypeScript
 * is happy to let a JSON `null` inhabit at runtime. `submitProblem` guards its
 * own top-level read (see there), but the READ-MODE form does not go through
 * `submitProblem` — `JdbcForm` and its siblings each open with `const c =
 * config` and then `str(c.dialect)`. Both paths have to be safe, or fixing one
 * simply moves the blank page a few lines down.
 */
function configObject(config: unknown): Record<string, unknown> {
  return config && typeof config === "object" && !Array.isArray(config)
    ? (config as Record<string, unknown>)
    : {};
}

function asMaps(a: AdvancedValue): Record<string, Record<string, string>> {
  return {
    ...(a.trino ? { trino: a.trino } : {}),
    ...(a.starrocks ? { starrocks: a.starrocks } : {}),
  };
}

/**
 * Why egret would refuse this STORED configuration if it were saved today, or
 * `null`.
 *
 * SP3c-1 tightened `validate_config` — a non-egret iceberg catalog now needs a
 * `storage_connection_id` (`config.rs:476-478`), among others — so rows written
 * before it can no longer be parsed. `GET /catalogs` and `GET /catalogs/{id}`
 * deliberately hand back the stored column WITHOUT parsing it (SP3c task-3
 * survey), which is what makes such a row reachable at all: this page is the
 * repair path, and PUT — which does parse — is the repair.
 *
 * The instrument is `submitProblem`, the same completeness check the create
 * form runs, written in the backend's own words. It is not `validate_config`,
 * so the claim this function supports is exactly "a save would be refused for
 * this reason", not "the row is unparseable" — a narrower statement than the
 * one the banner would like to make, and the only one that is measured.
 *
 * An egret warehouse catalog is excluded: its `config` is system-managed
 * (`{ metastore: { type: "egret", warehouse_id } }`), which no create form
 * offers and `submitProblem` has no arm for.
 */
export function storedConfigProblem(catalog: RegistryCatalogDto): string | null {
  if (isEgretWarehouse(catalog)) return null;
  if (!REGISTRY_KINDS.includes(catalog.kind)) return null;
  // Deliberately the RAW stored value, not `configObject(...)`: `submitProblem`
  // owns its own input domain (it is called from the create page too), so
  // narrowing here would hide whether its guard is really there.
  return submitProblem(
    catalog.kind,
    catalog.config as Record<string, unknown>,
    asMaps(advancedMaps(catalog.advanced)),
  );
}

/** Does this build have a typed form for the kind? */
function hasTypedForm(kind: string): boolean {
  return REGISTRY_KINDS.includes(kind);
}

/** One engine's blast radius, in the words `CatalogImpactEngineDto` documents. */
function impactSentence(engineName: string, action: string): string {
  if (action === "restart") return `${engineName} will restart`;
  if (action === "in_place") return `${engineName} updates in place`;
  return `${engineName}: ${action}`;
}

/**
 * Control Plane → Catalogs → detail → Configuration.
 *
 * Read mode renders the kind's OWN form inside a `fieldset[disabled]` rather
 * than a second, read-only renderer — one renderer cannot drift from itself —
 * and always offers the stored JSON underneath, because a typed form can only
 * show the keys it has fields for. Edit mode swaps the same form live.
 */
export function ConfigurationTab({ catalog }: { catalog: RegistryCatalogDto }) {
  const { me } = useAuth();
  const queryClient = useQueryClient();

  const canUpdate =
    !!me &&
    (me.platform_roles.includes("platform_admin") ||
      me.permissions.includes("catalog_registry:update"));
  const systemManaged = isEgretWarehouse(catalog);
  const typedForm = hasTypedForm(catalog.kind);
  const problem = storedConfigProblem(catalog);

  const [editing, setEditing] = useState(false);
  // Every draft is seeded in `startEdit`, never from a `useState` initialiser:
  // this component is mounted long before the user presses Edit, and long
  // before a refetch may have replaced `catalog`. A `useState(catalog.config)`
  // would freeze whatever the first render happened to see (SP3c review I1).
  const [draft, setDraft] = useState<Record<string, unknown>>({});
  const [draftAdvanced, setDraftAdvanced] = useState<AdvancedValue>({});
  const [draftDescription, setDraftDescription] = useState("");
  const [jsonMode, setJsonMode] = useState(false);
  const [jsonText, setJsonText] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issues, setIssues] = useState<AdvancedIssue[]>([]);
  // `null` means "the user has not decided": the panel then follows the row,
  // opening itself for a configuration that needs looking at and re-evaluating
  // on every render rather than on the first one only.
  const [jsonOpen, setJsonOpen] = useState<boolean | null>(null);

  // Only while editing, and only when there is something to impact. `retry:
  // false` so a 403 or 404 surfaces once instead of three times.
  const impact = useControlPlaneCatalogRegistryCatalogImpact(catalog.id, {
    query: { enabled: editing && catalog.engines.length > 0, retry: false },
  });
  const impactEngines =
    impact.data?.status === 200 ? impact.data.data.engines : [];
  const impactCount = impactEngines.length > 0 ? impactEngines.length : catalog.engines.length;

  // `<ApiError>`: `apiFetch` throws on every non-2xx, so what react-query
  // hands the error path is an `ApiError` carrying `.message`. The generated
  // `TError = ErrorBody` would make `.error` type-check and be `undefined` at
  // runtime — a refused save with nothing on screen.
  const updateMut = useControlPlaneCatalogRegistryUpdateRegistryCatalog<ApiError>({
    mutation: {
      onSuccess: (res) => {
        if (res.status !== 200) return;
        const failed = res.data.outcomes.filter((o) => o.status === "failed");
        // A partial success is the NORMAL case and is not a failure: §4.5.3
        // flags the attachment that refused and moves on, the other engines
        // already hold the new configuration, and nothing reverts. So the
        // failures are named one by one and the save still closes.
        for (const o of failed) {
          toast.error(
            `${o.engineName} did not take the new configuration: ${o.error ?? "no reason given"}`,
          );
        }
        if (failed.length === 0) toast.success("Configuration saved");
        void queryClient.invalidateQueries({ queryKey: catalogKey(catalog.id) });
        void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
        setEditing(false);
        setError(null);
        setIssues([]);
      },
      onError: (e) => {
        const message = e.message || "Failed to save the configuration.";
        setError(message);
        const issue = parseAdvancedIssue(message);
        setIssues(issue ? [issue] : []);
        if (issue) setAdvancedOpen(true);
      },
    },
  });

  function startEdit() {
    setDraft({ ...configObject(catalog.config) });
    setDraftAdvanced(advancedMaps(catalog.advanced));
    setDraftDescription(catalog.description ?? "");
    setJsonText(JSON.stringify(catalog.config, null, 2));
    // A kind with no typed form can only be edited as JSON; so can a
    // configuration the form would silently reshape.
    setJsonMode(!typedForm);
    setAdvancedOpen(catalog.kind === "generic");
    setError(null);
    setIssues([]);
    setEditing(true);
  }

  /**
   * Move the edit between the two editors, carrying it BOTH ways.
   *
   * The form→JSON direction always did. The JSON→form direction used to just
   * flip the flag, so everything typed in the JSON editor was dropped on the
   * floor and the next save posted the PRE-JSON draft under a "Configuration
   * saved" toast — a save that repaired nothing while claiming success, on the
   * path this tab exists for (SP3c review F2). JSON that does not parse into
   * an object refuses the switch and says why, rather than being silently
   * thrown away: half-typed JSON is work in progress, not a mistake to punish.
   */
  function switchEditor() {
    if (!jsonMode) {
      setJsonText(JSON.stringify(draft, null, 2));
      setJsonMode(true);
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(jsonText);
    } catch (e) {
      setError(
        `That is not valid JSON, so the form cannot take it over: ${(e as Error).message}`,
      );
      return;
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      setError("The configuration must be a JSON object before the form can take it over.");
      return;
    }
    setDraft(parsed as Record<string, unknown>);
    setError(null);
    setJsonMode(false);
  }

  function save() {
    setError(null);
    setIssues([]);

    let config: Record<string, unknown>;
    if (jsonMode) {
      // The repair path posts EXACTLY what the operator typed. `cleanConfig`
      // strips blanks and reshapes discriminated arms, which is right for a
      // form it drove and wrong for bytes written by hand to fix a row the
      // form cannot represent. The server is the authority either way.
      let parsed: unknown;
      try {
        parsed = JSON.parse(jsonText);
      } catch (e) {
        setError(`That is not valid JSON: ${(e as Error).message}`);
        return;
      }
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        setError("The configuration must be a JSON object.");
        return;
      }
      config = parsed as Record<string, unknown>;
    } else {
      // Validated and posted must be the SAME value. `submitProblem` tests
      // trimmed strings and `cleanConfig` trims, so checking the draft and
      // sending the cleaned object agree — a trailing space is neither
      // accepted here and refused there, nor stored to fail later at engine
      // authentication with no message anywhere.
      const problemNow = submitProblem(catalog.kind, draft, asMaps(draftAdvanced));
      if (problemNow) {
        setError(problemNow);
        return;
      }
      config = cleanConfig(catalog.kind, draft);
    }

    const advanced = cleanAdvanced(catalog.kind, config, asMaps(draftAdvanced));
    const description = draftDescription.trim();
    updateMut.mutate({
      id: catalog.id,
      data: {
        config,
        ...(Object.keys(advanced).length > 0 ? { advanced } : {}),
        // Sent explicitly rather than omitted because it SAYS what it does.
        // Omitting it would clear the description just the same — the field is
        // `#[serde(default)] Option<String>` and `update_catalog` binds it to
        // `description = $4` unconditionally (`store.rs:421-429`) — so this is
        // a legibility choice, not a correctness one (SP3c review F5).
        description: description === "" ? null : description,
      },
    });
  }

  // Brief Step 2 asked for "the `advanced` map per engine" in the read pass and
  // the first round left it only as a second <pre> INSIDE the stored-JSON
  // disclosure — collapsed whenever the row has no problem, so a catalog whose
  // behaviour on Trino is driven entirely by `advanced` showed none of it until
  // someone clicked (review F7). Rendered here as its own section, at the same
  // altitude as the typed form it overrides.
  const storedAdvanced = advancedMaps(catalog.advanced);
  const advancedEngines = (["trino", "starrocks"] as const).filter(
    (e) => storedAdvanced[e] !== undefined,
  );
  const advancedRead =
    advancedEngines.length === 0 ? null : (
      <ConfigSection
        level={2}
        title="Advanced properties"
        description="Written after the properties the form above sets, per engine."
      >
        <div data-testid="catalog-advanced-read" className="space-y-3">
          {advancedEngines.map((engine) => (
            <div key={engine} className="space-y-1 rounded-md border p-3">
              <p className="text-sm font-medium">{engine}</p>
              <dl className="grid grid-cols-1 gap-x-4 gap-y-1 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
                {Object.entries(storedAdvanced[engine] ?? {}).map(([k, v]) => (
                  <div key={k} className="contents">
                    <dt className="truncate font-mono text-xs text-muted-foreground">{k}</dt>
                    <dd className="break-all font-mono text-xs">{v}</dd>
                  </div>
                ))}
              </dl>
            </div>
          ))}
        </div>
      </ConfigSection>
    );

  const storedJson = (
    <Collapsible
      open={jsonOpen ?? problem !== null}
      onOpenChange={setJsonOpen}
      className="rounded-lg border"
    >
      <CollapsibleTrigger className="flex w-full items-center justify-between p-4 text-sm font-medium">
        Stored configuration (JSON)
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-2 border-t p-4">
        <p className="text-xs text-muted-foreground">
          Exactly what is stored, as egret returns it. The form above can only show the keys it
          has fields for.
        </p>
        <pre
          data-testid="catalog-config-json"
          className="overflow-x-auto rounded-md bg-muted/40 p-3 font-mono text-xs"
        >
          {JSON.stringify(catalog.config, null, 2)}
        </pre>
        {Object.keys(catalog.advanced ?? {}).length > 0 && (
          <pre className="overflow-x-auto rounded-md bg-muted/40 p-3 font-mono text-xs">
            {JSON.stringify(catalog.advanced, null, 2)}
          </pre>
        )}
      </CollapsibleContent>
    </Collapsible>
  );

  if (!editing) {
    return (
      <div className="space-y-4">
        {problem !== null && (
          <Alert variant="destructive" data-testid="catalog-config-stale">
            <AlertTitle>egret would refuse this configuration today</AlertTitle>
            <AlertDescription className="space-y-1">
              <p>{problem}</p>
              <p>
                This row is returned exactly as stored, without being parsed, which is why it can
                still be opened here. Rendering it for an engine — attaching it, or creating or
                updating an engine that already has it — is refused until it is fixed. Edit it and
                save to repair it.
              </p>
            </AlertDescription>
          </Alert>
        )}

        {!typedForm && (
          <Alert data-testid="catalog-config-unknown-kind">
            <AlertDescription>
              This build has no typed form for the kind <code>{catalog.kind}</code>. Its
              configuration is shown and edited as JSON.
            </AlertDescription>
          </Alert>
        )}

        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-sm text-muted-foreground">
            {systemManaged
              ? "egret manages this catalog's configuration with its warehouse — the catalog is the warehouse, and its storage is the warehouse's."
              : "What every attached engine is told about this catalog. Saving re-applies it to each of them."}
          </p>
          {!systemManaged && canUpdate && (
            <Button variant="outline" onClick={startEdit}>
              Edit configuration
            </Button>
          )}
        </div>

        {!systemManaged && typedForm ? (
          // The kind's own form, disabled. One renderer, so a read pass and an
          // edit pass cannot say different things about the same stored row.
          //
          // `!systemManaged` IS LOAD-BEARING, and was missing (SP3c-1 Task 11,
          // found in a browser). An egret warehouse catalog's `kind` is
          // `iceberg`, so `hasTypedForm` says yes and `IcebergForm` rendered —
          // over a stored config of `{ metastore: { type: "egret",
          // warehouse_id } }`, which that form has no fields for. The result
          // was an EMPTY Metastore `<Select>` and a "Select a storage
          // connection" placeholder, directly under the sentence "egret
          // manages this catalog's configuration with its warehouse — the
          // catalog is the warehouse, and its storage is the warehouse's". Two
          // unset-looking controls are a claim that nothing is set; the
          // warehouse has both a metastore and a storage connection. The two
          // blocks below were already gated on `systemManaged` and the Edit
          // button above it too, so this was the one site that escaped.
          // `storedJson` still renders, which is where a warehouse catalog's
          // configuration is legible.
          <fieldset disabled className="space-y-4">
            <KindForm
              kind={catalog.kind}
              config={configObject(catalog.config)}
              onChange={() => {}}
            />
          </fieldset>
        ) : null}

        {!systemManaged && typedForm && (
          // The banner's SILENCE is reachable today, not only after some future
          // tightening: `submitProblem` is a strict subset of `validate_config`,
          // so a stored jdbc row with an unknown dialect and a non-UUID secret
          // id fails at the backend and shows nothing here. A reader takes a
          // quiet page as a healthy row. Naming the instrument is what makes
          // the absence of a banner mean something falsifiable (review F3); the
          // durable fix is the shared fixture Task 5 carried forward.
          <p data-testid="catalog-config-check-bound" className="text-xs text-muted-foreground">
            {problem === null
              ? "Nothing here fails the rules this build checks — but it checks only the rules it knows, and egret validates the whole configuration when you save."
              : "This page checks only the rules it knows, and egret validates the whole configuration when you save — so there may be more than the above."}
          </p>
        )}

        {advancedRead}

        <ConfigSection level={2} title="Description">
          <p className="text-sm">
            {catalog.description ? (
              catalog.description
            ) : (
              <span className="text-muted-foreground">No description.</span>
            )}
          </p>
        </ConfigSection>

        {storedJson}
      </div>
    );
  }

  const canSaveJson = jsonMode;
  return (
    <div className="space-y-4">
      {error && (
        <Alert variant="destructive" data-testid="catalog-config-error">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <Alert>
        <AlertDescription>
          <span className="font-medium">{catalog.name}</span> is a{" "}
          <span className="font-medium">{catalog.kind}</span> catalog. Neither the name nor the
          kind can change — the name is the SQL name on every engine this catalog is attached to,
          and each kind reads a different set of properties.
        </AlertDescription>
      </Alert>

      {catalog.engines.length > 0 && (
        <Alert data-testid="catalog-impact">
          {/* Counted from the impact answer when there is one, so the headline
              and the bullets below it describe the same set. Falling back to
              the attachment count only while the query has not answered. */}
          <AlertTitle>
            Saving re-applies this catalog to {impactCount} engine
            {impactCount === 1 ? "" : "s"}
          </AlertTitle>
          <AlertDescription>
            {impact.isError ? (
              <p>Couldn&apos;t work out what saving does to each engine.</p>
            ) : impactEngines.length === 0 ? (
              <p>Working out what saving does to each engine…</p>
            ) : (
              <ul className="list-disc space-y-0.5 pl-5">
                {impactEngines.map((e) => (
                  <li key={e.engineId}>{impactSentence(e.engineName, e.action)}</li>
                ))}
              </ul>
            )}
            <p className="mt-1">
              An engine that refuses is flagged and the others keep the new configuration; nothing
              is rolled back.
            </p>
          </AlertDescription>
        </Alert>
      )}

      {typedForm && (
        <div className="flex justify-end">
          <Button
            variant="ghost"
            size="sm"
            onClick={switchEditor}
          >
            {jsonMode ? "Edit as a form" : "Edit as JSON"}
          </Button>
        </div>
      )}

      {canSaveJson ? (
        <ConfigSection
          level={2}
          title="Configuration"
          description="Posted exactly as written. egret parses it on save and refuses it with its own message if it is wrong."
        >
          <div className="space-y-1.5">
            <Label htmlFor="cd-config-json">Configuration JSON</Label>
            <Textarea
              id="cd-config-json"
              value={jsonText}
              rows={16}
              spellCheck={false}
              className="font-mono text-xs"
              onChange={(e) => setJsonText(e.target.value)}
            />
          </div>
        </ConfigSection>
      ) : (
        <>
          <KindForm kind={catalog.kind} config={draft} onChange={setDraft} />
          <AdvancedEditor
            value={draftAdvanced}
            onChange={setDraftAdvanced}
            issues={issues}
            requiredKeys={catalog.kind === "generic" ? GENERIC_REQUIRED_KEY : undefined}
            open={advancedOpen}
            onOpenChange={setAdvancedOpen}
          />
        </>
      )}

      <ConfigSection level={2} title="Description">
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cd-description"
            label="Description"
            help="Optional. Shown wherever the catalog is listed. Emptying the box clears it."
          />
          <Textarea
            id="cd-description"
            value={draftDescription}
            rows={2}
            onChange={(e) => setDraftDescription(e.target.value)}
          />
        </div>
      </ConfigSection>

      <div className="flex flex-wrap items-center justify-end gap-3 border-t pt-4">
        <Button variant="outline" onClick={() => setEditing(false)} disabled={updateMut.isPending}>
          Cancel
        </Button>
        <Button onClick={save} disabled={updateMut.isPending}>
          {updateMut.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
          Save configuration
        </Button>
      </div>
    </div>
  );
}
