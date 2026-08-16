import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, NavLink, Outlet, useOutletContext, useParams } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { useDataApiGetEndpoint, useDataApiUpdateEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiEndpointDto, UpdateEndpointInput } from "@/api/generated/model";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

/**
 * The subset of `DataApiEndpointDto` an author edits. Deliberately every
 * config field (`output` included, even though only the Response section —
 * Task 4 — puts a control on it), not just what Definition renders: this is
 * ONE form backing ALL five detail sections, so a field a later section owns
 * still needs to survive a Save made from a different section.
 *
 * Excludes id/folder_id/source_kind/source_ref/live_version_id/state/
 * created_by/created_at/updated_at — those are read-only here; state and
 * live_version_id in particular are the publish lifecycle (Versions, Task 7)
 * and must never move through a draft-edit PATCH.
 */
export interface EndpointFormState {
  name: string;
  description: string;
  method: string;
  path: string;
  database_id: string;
  sql: string;
  params: DataApiEndpointDto["params"];
  output: DataApiEndpointDto["output"];
  cache: DataApiEndpointDto["cache"];
  limits: DataApiEndpointDto["limits"];
  rate_limit: DataApiEndpointDto["rate_limit"];
}

function toForm(e: DataApiEndpointDto): EndpointFormState {
  return {
    name: e.name,
    description: e.description,
    method: e.method,
    path: e.path,
    database_id: e.database_id,
    sql: e.sql,
    params: e.params,
    output: e.output,
    cache: e.cache,
    limits: e.limits,
    rate_limit: e.rate_limit,
  };
}

function toUpdateInput(f: EndpointFormState): UpdateEndpointInput {
  return {
    name: f.name,
    description: f.description,
    method: f.method,
    path: f.path,
    database_id: f.database_id,
    sql: f.sql,
    params: f.params,
    output: f.output,
    cache: f.cache,
    limits: f.limits,
    rate_limit: f.rate_limit,
  };
}

/** What every nested detail section (`detail/*.tsx`) reads via `useEndpointDetail()`. */
export interface EndpointDetailContext {
  ws: string;
  id: string;
  /** The last-loaded/-saved record, for read-only bits (state, live_version_id, timestamps…). */
  endpoint: DataApiEndpointDto;
  form: EndpointFormState;
  setForm: (updater: EndpointFormState | ((f: EndpointFormState) => EndpointFormState)) => void;
  dirty: boolean;
  saving: boolean;
  save: () => Promise<void>;
}

/** Nested sections read/write the shared form through this, not through `useOutletContext` directly. */
export function useEndpointDetail(): EndpointDetailContext {
  return useOutletContext<EndpointDetailContext>();
}

const SECTIONS: { to: string; label: string; end?: boolean }[] = [
  { to: "", label: "Definition", end: true },
  { to: "response", label: "Response" },
  { to: "access", label: "Access" },
  { to: "try", label: "Try it" },
  { to: "versions", label: "Versions" },
];

/**
 * The endpoint detail shell: a header (name + Save), a horizontal section nav,
 * and the active section in the Outlet. Owns the one piece of state every
 * section shares — the draft form and its dirtiness — so Save and the
 * unsaved-changes guard live in exactly one place no matter which section is
 * open.
 *
 * **Dirtiness is a comparison against the loaded value, never an `onChange`
 * flag.** An `onChange` boolean latches on Radix's hydration echo and raises a
 * phantom "Leave site?" on a page nobody edited — the defect Alerts #486 hit.
 * `baseline` is a serialized snapshot of the loaded (or just-saved) form;
 * `dirty` is `current !== baseline`, nothing else.
 *
 * Hydration is guarded to run once (`hydrated` ref): re-seeding the form on
 * every render that sees a `data` reference — including an unchanged refetch
 * handing back a new object identity — would clobber a mid-edit form. This
 * codebase has shipped that bug twice (ChartExplore, NotebookSettings).
 */
export default function EndpointDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  const { data: envelope, isLoading, isError, error } = useDataApiGetEndpoint(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const endpoint = envelope?.status === 200 ? envelope.data : undefined;

  const updateMutation = useDataApiUpdateEndpoint();

  const [form, setFormState] = useState<EndpointFormState | null>(null);
  const [baseline, setBaseline] = useState<string | null>(null);
  const hydrated = useRef(false);

  useEffect(() => {
    if (hydrated.current || !endpoint) return;
    hydrated.current = true;
    const seeded = toForm(endpoint);
    setFormState(seeded);
    setBaseline(JSON.stringify(seeded));
  }, [endpoint]);

  const setForm = useCallback(
    (updater: EndpointFormState | ((f: EndpointFormState) => EndpointFormState)) => {
      setFormState((prev) => {
        if (!prev) return prev;
        return typeof updater === "function"
          ? (updater as (f: EndpointFormState) => EndpointFormState)(prev)
          : updater;
      });
    },
    [],
  );

  const current = useMemo(() => (form ? JSON.stringify(form) : null), [form]);
  const dirty = baseline !== null && current !== null && current !== baseline;
  useUnsavedChangesWarning(dirty);

  const save = useCallback(async () => {
    if (!form) return;
    try {
      const res = await updateMutation.mutateAsync({ workspace: ws, id, data: toUpdateInput(form) });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — same trick as `Endpoints.tsx`'s
      // create dialog, just narrowing TS's view of the response union.
      if (res.status !== 200) {
        toast.error("Could not save.");
        return;
      }
      // Re-seed from what the server actually persisted (it may fill in
      // defaults for the optional cache/limits/rate_limit sub-fields) rather
      // than from the just-sent form, so the baseline can never drift from
      // what a reload would show.
      const seeded = toForm(res.data);
      setFormState(seeded);
      setBaseline(JSON.stringify(seeded));
      toast.success("Saved");
    } catch (e) {
      toast.error(String(e));
    }
  }, [form, updateMutation, ws, id]);

  if (isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton cols={2} />
      </div>
    );
  }
  if (isError || !endpoint) {
    return (
      <div className="p-4">
        <Alert variant="destructive">
          <AlertDescription>
            {(error as unknown as Error)?.message ?? "Could not load this endpoint."}
          </AlertDescription>
        </Alert>
        <Link className="mt-3 inline-block text-sm underline" to={`/explore/${ws}/data-api/endpoints`}>
          ← Back to endpoints
        </Link>
      </div>
    );
  }
  if (!form) {
    // Endpoint has loaded but the hydration effect hasn't flushed yet — one render, at most.
    return (
      <div className="p-4">
        <ListSkeleton cols={2} />
      </div>
    );
  }

  const ctx: EndpointDetailContext = { ws, id, endpoint, form, setForm, dirty, saving: updateMutation.isPending, save };

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <Link
            className="text-sm text-muted-foreground hover:underline"
            to={`/explore/${ws}/data-api/endpoints`}
          >
            ← Endpoints
          </Link>
          <h2 className="truncate text-lg font-semibold leading-tight">{endpoint.name}</h2>
        </div>
        <div className="flex items-center gap-2">
          {dirty && <span className="text-xs text-muted-foreground">Unsaved changes</span>}
          <Button onClick={save} disabled={!dirty || updateMutation.isPending}>
            {updateMutation.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </div>

      <nav
        className="-mx-1 mb-5 flex gap-1 overflow-x-auto border-b border-border px-1"
        aria-label="Endpoint sections"
      >
        {SECTIONS.map((s) => (
          <NavLink
            key={s.to}
            to={s.to}
            end={s.end}
            className={({ isActive }) =>
              `-mb-px shrink-0 border-b-2 px-3 py-2 text-sm transition-colors ${
                isActive
                  ? "border-primary font-semibold text-foreground"
                  : "border-transparent font-medium text-muted-foreground hover:border-border hover:text-foreground"
              }`
            }
          >
            {s.label}
          </NavLink>
        ))}
      </nav>

      <Outlet context={ctx} />
    </div>
  );
}
