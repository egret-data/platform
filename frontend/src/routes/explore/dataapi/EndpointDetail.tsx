import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, NavLink, Outlet, useOutletContext, useParams } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { useDataApiGetEndpoint } from "@/api/generated/data-api/data-api";
// Save's invalidation (this record + the list its Name column feeds) lives in
// the wrapper, next to every other Data API mutation — see `src/api/dataApi.ts`.
import { useUpdateEndpoint } from "@/api/dataApi";
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

/**
 * A PATCH carrying only the fields that changed since `baseline` — never the
 * whole form. `UpdateEndpointInput`'s own doc says an absent field means
 * "leave it alone"; a whole-document PATCH built from a form hydrated once at
 * mount silently reverts any change made elsewhere since — another tab, a
 * colleague, a server-side normalization during a publish — the instant this
 * page's Save is clicked, even though the author only touched one field.
 */
function diffToUpdateInput(baseline: EndpointFormState, form: EndpointFormState): UpdateEndpointInput {
  const out: UpdateEndpointInput = {};
  if (form.name !== baseline.name) out.name = form.name;
  if (form.description !== baseline.description) out.description = form.description;
  if (form.method !== baseline.method) out.method = form.method;
  if (form.path !== baseline.path) out.path = form.path;
  if (form.database_id !== baseline.database_id) out.database_id = form.database_id;
  if (form.sql !== baseline.sql) out.sql = form.sql;
  if (JSON.stringify(form.params) !== JSON.stringify(baseline.params)) out.params = form.params;
  if (JSON.stringify(form.output) !== JSON.stringify(baseline.output)) out.output = form.output;
  if (JSON.stringify(form.cache) !== JSON.stringify(baseline.cache)) out.cache = form.cache;
  if (JSON.stringify(form.limits) !== JSON.stringify(baseline.limits)) out.limits = form.limits;
  if (JSON.stringify(form.rate_limit) !== JSON.stringify(baseline.rate_limit)) out.rate_limit = form.rate_limit;
  return out;
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
  /** Re-fetch the endpoint record — for a section (Versions, Task 7) that just
   *  changed `state`/`live_version_id` server-side (publish/rollback) and needs
   *  the read-only half of this context to catch up. Does not touch `form`. */
  refetch: () => void;
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
 * Thin wrapper: reads `:id` from the route and keys `EndpointDetailInner` by
 * it.
 *
 * **This is the fix for a real defect, not a stylistic choice.** React Router
 * reconciles the SAME component instance across `/endpoints/A` →
 * `/endpoints/B` — only the `:id` param changes — so any state owned by a
 * bare `EndpointDetail` (the draft form, the hydration guard) would survive
 * that navigation unless something explicitly resets it. The `key={id}` here
 * is that reset: React fully unmounts the old instance and mounts a brand new
 * one whenever `id` changes, so `EndpointDetailInner`'s `useState`/`useRef`
 * start fresh for every endpoint, with no imperative "clear the form" step
 * anywhere for a stale record to leak through before it runs. This is React's
 * own documented pattern for resetting state on a prop change (see
 * "Resetting a form with a key" in the React docs), and it also means every
 * nested section (the `Outlet`'s children) remounts fresh on a cross-endpoint
 * navigation, rather than carrying its own local state across records either.
 */
export default function EndpointDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  return <EndpointDetailInner key={id} ws={ws} id={id} />;
}

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
 * `baselineForm` is a snapshot of the loaded (or just-saved) form; `dirty` is
 * `current !== baseline` (compared as JSON), nothing else.
 *
 * Hydration is guarded to run once (`hydrated` ref) — safe to write as a bare
 * "ran once" flag here, unlike a component that lives across an id change,
 * because `EndpointDetail` above remounts a fresh instance of this component
 * per id (see its own doc comment).
 */
function EndpointDetailInner({ ws, id }: { ws: string; id: string }) {
  const { data: envelope, isLoading, isError, error, refetch } = useDataApiGetEndpoint(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const endpoint = envelope?.status === 200 ? envelope.data : undefined;

  const updateMutation = useUpdateEndpoint();

  const [form, setFormState] = useState<EndpointFormState | null>(null);
  const [baselineForm, setBaselineForm] = useState<EndpointFormState | null>(null);
  const hydrated = useRef(false);

  useEffect(() => {
    if (hydrated.current || !endpoint) return;
    hydrated.current = true;
    const seeded = toForm(endpoint);
    setFormState(seeded);
    setBaselineForm(seeded);
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

  const currentJson = useMemo(() => (form ? JSON.stringify(form) : null), [form]);
  const baselineJson = useMemo(() => (baselineForm ? JSON.stringify(baselineForm) : null), [baselineForm]);
  const dirty = baselineJson !== null && currentJson !== null && currentJson !== baselineJson;
  useUnsavedChangesWarning(dirty);

  const save = useCallback(async () => {
    if (!form || !baselineForm) return;
    try {
      const res = await updateMutation.mutateAsync({
        workspace: ws,
        id,
        data: diffToUpdateInput(baselineForm, form),
      });
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
      setBaselineForm(seeded);
      toast.success("Saved");
    } catch (e) {
      toast.error(String(e));
    }
  }, [form, baselineForm, updateMutation, ws, id]);

  // The page's one document-outline `h1`, on every branch below — this route has no
  // shell to supply it (unlike the three list pages, it has no room for a *visible*
  // `h1`: the endpoint name is already the page's title as a breadcrumb-style `h2`,
  // and doubling it visibly would read as two titles stacked on top of each other).
  // `sr-only`, same idiom as `SqlLab.tsx`/`ChartExplore.tsx` for a page whose design
  // has no visible slot for one. Falls back to "Endpoint" before the name is known
  // (loading/error), exactly as `ChartExplore.tsx`'s own `h1` falls back to "New chart".
  const headingName = endpoint?.name ?? "Endpoint";

  if (isLoading) {
    return (
      <div className="p-4">
        <h1 className="sr-only">{headingName}</h1>
        <ListSkeleton cols={2} />
      </div>
    );
  }
  if (isError || !endpoint) {
    return (
      <div className="p-4">
        <h1 className="sr-only">{headingName}</h1>
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
  if (!form || !baselineForm) {
    // Either the hydration effect hasn't flushed yet, or `id` just changed
    // and the new endpoint hasn't arrived — one render, at most, either way.
    return (
      <div className="p-4">
        <h1 className="sr-only">{headingName}</h1>
        <ListSkeleton cols={2} />
      </div>
    );
  }

  const ctx: EndpointDetailContext = {
    ws,
    id,
    endpoint,
    form,
    setForm,
    dirty,
    saving: updateMutation.isPending,
    save,
    refetch: () => {
      void refetch();
    },
  };

  return (
    // `p-4`, matching every other top-level Explore detail route (e.g.
    // DatasetDetail.tsx) — this used to come for free from `DataApiLayout`'s
    // `mx-auto max-w-6xl p-6`, which wrapped its `<Outlet/>`. That shell is
    // gone (Data API is now three flat sidebar items, not a shell + section
    // nav), so this route supplies its own padding like its siblings.
    // `max-w-3xl` stays scoped to the Outlet below, not the header/nav above
    // it — the shell never capped those either, only the section content.
    <div className="p-4">
      <h1 className="sr-only">{headingName}</h1>
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

      {/* `max-w-3xl` lives here, not in each section — one structural width
          rule every section inherits for free, rather than something Tasks
          4-7 each have to remember to add to their own root element. `key={id}`
          is redundant with `EndpointDetail`'s own remount-per-id above, kept
          anyway as a second, independent guarantee that a section can never
          carry its own local state across two different endpoints. */}
      <div className="max-w-3xl">
        <Outlet key={id} context={ctx} />
      </div>
    </div>
  );
}
