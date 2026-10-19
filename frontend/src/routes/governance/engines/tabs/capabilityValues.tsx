import { RotateCw } from "lucide-react";
import type { CapabilityResource, CapabilityView, GovernanceCatalog, Reach } from "@/api/generated/model";
import { SubjectUserField } from "@/components/perm/ExplainResult";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { failedText } from "@/routes/governance/tabParts";

/**
 * The value controls of an engine capability, one per `resources[]` entry in
 * the catalogue's order (spec §6.1): `username` → a user search (the value is
 * the username, spec premise 2); `catalog` → a select of the catalogs attached
 * to an engine the grant reaches (Task 1 M3: the server accepts exactly
 * these); `identifier` and `function_signature` → inputs with their rule;
 * `fixed` → no control, the compiler supplies `*`. Shared by the grant dialog
 * and Check.
 */

/** A label per resource key the catalogue names (F11 §3); an unknown key shows itself. */
const KEY_LABEL: Record<string, string> = {
  trinouser: "Target user",
  user: "Target user",
  catalog: "Catalog",
  schema: "Schema",
  sessionproperty: "Session property",
  systemproperty: "System session property",
  procedure: "Procedure",
  schemafunction: "Function",
  global_function: "Function signature",
};
export const keyLabel = (key: string) => KEY_LABEL[key] ?? key;

export const IDENTIFIER_HELP = "A name: letters, digits and _, then also . $ -.";
export const SIGNATURE_HELP = "e.g. my_udf(INT,VARCHAR)";
export const FIXED_TEXT = "Applies to all (*)";

/** The resources a grant supplies a value for: every one but `fixed`. */
export function valueResources(cap: CapabilityView | undefined): CapabilityResource[] {
  return (cap?.resources ?? []).filter((r) => r.valueKind !== "fixed");
}

/**
 * The server's shape rules (F11 §2), checked before sending; the server stays
 * the authority. `null` when the value passes.
 */
export function valueProblem(value: string): string | null {
  if (value === "") return "Enter a value.";
  if (value.trim() !== value) return "No leading or trailing spaces.";
  if (/[*?{}]/.test(value)) return "No *, ?, { or }: values are concrete names.";
  return null;
}

/** The catalogs attached to an engine of `reach`, by name. */
export function reachedCatalogs(catalogs: GovernanceCatalog[], reach: Reach | null): string[] {
  const names = new Set((reach?.engines ?? []).map((e) => e.name));
  return catalogs
    .filter((c) => c.engines.some((e) => names.has(e)))
    .map((c) => c.name)
    .sort((a, b) => a.localeCompare(b));
}

/**
 * What a `catalog` control can offer: the list loads and fails on its own
 * (FE-STATE-1), and only a list that loaded may say no catalog is attached.
 */
export type CatalogChoices =
  | { state: "loading" }
  | { state: "error"; retry: () => void }
  | { state: "ready"; names: string[] };

/** The `catalog` control's choices from the governance catalog list query and the grant's reach. */
export function catalogChoices(
  q: { data?: { status: number; data: unknown }; isError: boolean; refetch: () => unknown },
  reach: Reach | null,
): CatalogChoices {
  if (q.data?.status === 200) return { state: "ready", names: reachedCatalogs(q.data.data as GovernanceCatalog[], reach) };
  if (q.isError || q.data) return { state: "error", retry: () => void q.refetch() };
  return { state: "loading" };
}

const CATALOGS_LOADING = "Loading the catalogs…";
const NO_CATALOG = "No catalog is attached to an engine this grant reaches.";
const SOME_CATALOGS = "The catalogs attached to an engine this grant reaches.";

export function CapabilityValueFields({
  idPrefix,
  capability,
  values,
  onChange,
  errors,
  catalogs,
}: {
  idPrefix: string;
  capability: CapabilityView;
  /** By resource key. */
  values: Record<string, string>;
  onChange: (key: string, value: string) => void;
  /** By resource key; shown beside the field (FE-FORM-4). */
  errors: Record<string, string | undefined>;
  /** What a `catalog` control offers, or that its list is loading or failed. */
  catalogs: CatalogChoices;
}) {
  return (
    <div className="space-y-3">
      {capability.resources.map((r) => {
        const id = `${idPrefix}-${r.key}`;
        const error = errors[r.key];
        const errorId = `${id}-error`;
        const helpId = `${id}-help`;
        const label = keyLabel(r.key);
        const value = values[r.key] ?? "";
        const errorLine = error ? (
          <p id={errorId} className="text-xs text-destructive">
            {error}
          </p>
        ) : null;
        if (r.valueKind === "fixed") {
          return (
            <p key={r.key} className="text-sm">
              <span className="text-muted-foreground">{`${label}: `}</span>
              {FIXED_TEXT}
            </p>
          );
        }
        if (r.valueKind === "username") {
          return (
            <div key={r.key} className="space-y-1">
              <SubjectUserField
                id={id}
                label={label}
                value={value}
                onChange={(v) => onChange(r.key, v)}
                invalid={Boolean(error)}
                describedBy={error ? errorId : undefined}
              />
              {errorLine}
            </div>
          );
        }
        if (r.valueKind === "catalog") {
          return (
            <div key={r.key} className="space-y-1.5">
              <Label htmlFor={id}>{label}</Label>
              <Select value={value} onValueChange={(v) => onChange(r.key, v)}>
                <SelectTrigger
                  id={id}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? `${errorId} ${helpId}` : helpId}
                >
                  <SelectValue placeholder="Choose a catalog" />
                </SelectTrigger>
                <SelectContent>
                  {(catalogs.state === "ready" ? catalogs.names : []).map((c) => (
                    <SelectItem key={c} value={c}>
                      {c}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {errorLine}
              {catalogs.state === "error" ? (
                <div className="flex flex-wrap items-center gap-2">
                  <p id={helpId} className="text-xs text-destructive">
                    {failedText("list of catalogs")}
                  </p>
                  <Button type="button" variant="outline" size="sm" onClick={catalogs.retry}>
                    <RotateCw aria-hidden />
                    Retry
                  </Button>
                </div>
              ) : (
                <p id={helpId} className="text-xs text-muted-foreground">
                  {catalogs.state === "loading"
                    ? CATALOGS_LOADING
                    : catalogs.names.length === 0
                      ? NO_CATALOG
                      : SOME_CATALOGS}
                </p>
              )}
            </div>
          );
        }
        const help = r.valueKind === "function_signature" ? SIGNATURE_HELP : IDENTIFIER_HELP;
        return (
          <div key={r.key} className="space-y-1.5">
            <Label htmlFor={id}>{label}</Label>
            <Input
              id={id}
              value={value}
              onChange={(e) => onChange(r.key, e.target.value)}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? `${errorId} ${helpId}` : helpId}
              className={r.valueKind === "function_signature" ? "font-mono" : undefined}
            />
            {errorLine}
            <p id={helpId} className="text-xs text-muted-foreground">
              {help}
            </p>
          </div>
        );
      })}
    </div>
  );
}
