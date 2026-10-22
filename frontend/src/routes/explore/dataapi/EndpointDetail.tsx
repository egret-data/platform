import type { ComponentProps, MouseEvent as ReactMouseEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, NavLink, Outlet, useNavigate, useOutletContext, useParams } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import {
  useDataApiGetEndpoint,
  useDataApiListEndpointVersions,
} from "@/api/generated/data-api/data-api";
import {
  EndpointStateBadge,
  draftVsServed,
  endpointHeaderStateLabel,
  liveVersionLookup,
} from "./endpointState";
// Save's invalidation (this record + the list its Name column feeds) lives in
// the wrapper, next to every other Data API mutation — see `src/api/dataApi.ts`.
import { useRestoreEndpoint, useUpdateEndpoint } from "@/api/dataApi";
import type { DataApiEndpointDto, UpdateEndpointInput } from "@/api/generated/model";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { BackLink } from "@/components/layout/DetailHeader";
import { tabsListClass, tabsTriggerClass } from "@/components/ui/tabs";

/**
 * The subset of `DataApiEndpointDto` an author edits. Deliberately every
 * config field (`output` included, even though only the Response section —
 * Task 4 — puts a control on it), not just what Query renders: this is
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
  /**
   * The endpoint's SQL definitions, in execution order — a LIST since M2, and a
   * one-statement endpoint is the same shape as a ten-statement one. There is no
   * "simple" spelling of this to keep in step with the general one, which is the
   * whole reason the draft row dropped `sql text` for `statements jsonb`
   * (migration 0129) rather than carrying both.
   */
  statements: DataApiEndpointDto["statements"];
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
    statements: e.statements,
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
  // Compared as JSON like the other structured fields, and replaced WHOLE:
  // `UpdateEndpointInput.statements` has no per-statement patch, because the
  // ORDER is part of the definition and "edit statement 2" would need an
  // addressing scheme the list itself already is.
  if (JSON.stringify(form.statements) !== JSON.stringify(baseline.statements)) {
    out.statements = form.statements;
  }
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
  /**
   * This endpoint is in the trash (`deleted_at` is set).
   *
   * Every state transition the server offers — update, publish, rollback,
   * enable, disable — answers a trashed endpoint with a 409 naming restore.
   * Sections read this so they do not present a control whose only possible
   * outcome is that refusal; the shell additionally wraps the whole `Outlet` in
   * a disabled `fieldset`, so a control that forgets to check is inert rather
   * than merely misleading.
   */
  trashed: boolean;
  /** Re-fetch the endpoint record — for a section (Versions, Task 7) that just
   *  changed `state`/`live_version_id` server-side (publish/rollback) and needs
   *  the read-only half of this context to catch up. Does not touch `form`. */
  refetch: () => void;
  /**
   * Intercept a click that would LEAVE this editor while the form is dirty.
   *
   * Sections do not call this directly — they render {@link LeaveGuardLink}
   * instead, which is the same guard with the plumbing already attached. It is
   * on the context because the dirty form and the confirmation dialog both
   * live in the shell, and there is exactly one of each.
   *
   * A no-op when the form is clean, so a guarded link is a plain link on every
   * page where there is nothing to lose.
   */
  guardLeave: (e: ReactMouseEvent<HTMLAnchorElement>, to: string) => void;
}

/** Nested sections read/write the shared form through this, not through `useOutletContext` directly. */
export function useEndpointDetail(): EndpointDetailContext {
  return useOutletContext<EndpointDetailContext>();
}

/**
 * A `<Link>` that LEAVES the editor, and therefore asks first when the form is
 * dirty.
 *
 * # Which links this is for, and which it is not
 *
 * Only the ones that navigate OUT of `/endpoints/:id`. The six tabs and the
 * two cross-tab links (`Access` → `Try it`, `Try it` → `Access`) stay ordinary
 * `<Link>`s on purpose: the shell owns the form and keys the `Outlet` by id,
 * not by pathname, so moving between tabs loses nothing at all. A confirmation
 * on those would be a false alarm on the one navigation that is always safe —
 * and a guard that cries wolf is a guard people learn to click through.
 *
 * # Why this exists at all, rather than a router-level block
 *
 * `beforeunload` (see `useUnsavedChangesWarning`) covers reload, tab-close and
 * external navigation, and it was doing so correctly — measured back to back
 * against a control, a browser reload warned and clicking `← Endpoints`
 * discarded the edit in silence. The app knew: the badge rendered and the
 * handler was installed. The unguarded door was the one an author actually
 * clicks.
 *
 * react-router's `useBlocker` would cover every in-app exit at once, but it
 * exists only under a data router and `App.tsx` mounts `<BrowserRouter>`;
 * migrating the whole app's router to close this is a change with a blast
 * radius far past one editor. So this guards the CONTROLS, one by one, and the
 * ones it does not reach are named in the report rather than implied to be
 * covered: the sidebar, the workspace switcher and the browser Back button
 * still discard silently.
 */
export function LeaveGuardLink({
  to,
  onClick,
  ...rest
}: Omit<ComponentProps<typeof Link>, "to"> & { to: string }) {
  const { guardLeave } = useEndpointDetail();
  return (
    <Link
      to={to}
      onClick={(e) => {
        onClick?.(e);
        guardLeave(e, to);
      }}
      {...rest}
    />
  );
}

const SECTIONS: { to: string; label: string; end?: boolean }[] = [
  { to: "", label: "Query", end: true },
  { to: "config", label: "Config" },
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
  const restoreMutation = useRestoreEndpoint();

  /**
   * The version history, in the SHELL — because the header has to say what is
   * serving on all six tabs, not only on the one tab that lists versions.
   *
   * Same query key as `Versions` and `Response`, so react-query serves all
   * three from one fetch rather than three; this adds a request to a page that
   * did not make one only when the author never opens either of those tabs.
   * `is_live` is read as the server computed it against `live_version_id` and
   * is never re-derived here.
   *
   * The response carries every version's full frozen snapshot, and that is now
   * the point rather than the waste it used to be: the live version's snapshot
   * IS the served definition, so the header can compare the draft against it
   * directly instead of inferring from `updated_at`. See `draftVsServed`.
   */
  const versionsQuery = useDataApiListEndpointVersions(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  /**
   * A VALUE THAT SAYS WHETHER IT KNOWS, not a list that might be empty for two
   * different reasons.
   *
   * This read used to be `envelope?.status === 200 ? envelope.data : []`, which
   * turned a failed request — and the first paint before one lands — into an
   * empty list, then into an absent live version, then into a confident "not
   * diverged". A diverged endpoint read as in step whenever this request was
   * the one that failed. `liveVersionLookup` keeps the two apart, and the pill
   * says so out loud.
   */
  const lookup = liveVersionLookup(versionsQuery.data);
  const liveVersion = lookup.known ? lookup.version : undefined;

  /**
   * The one fact this page had no idea about until now.
   *
   * The Trash links straight here by name, so this route is reachable for an
   * endpoint every mutating route will refuse. Read from the record rather than
   * from where the reader came from: a bookmark, a pasted URL and the Trash all
   * have to arrive at the same page.
   */
  const trashed = !!endpoint?.deleted_at;

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
      // A trashed endpoint is read-only, and this is the floor under that: the
      // `fieldset` below and every section's own `trashed` check are what the
      // author SEES, but neither is what makes the form unable to move. With
      // the form pinned, `dirty` can never go true either, so the unsaved-
      // changes warning cannot fire on a page that has nothing to save.
      if (trashed) return;
      setFormState((prev) => {
        if (!prev) return prev;
        return typeof updater === "function"
          ? (updater as (f: EndpointFormState) => EndpointFormState)(prev)
          : updater;
      });
    },
    [trashed],
  );

  const currentJson = useMemo(() => (form ? JSON.stringify(form) : null), [form]);
  const baselineJson = useMemo(() => (baselineForm ? JSON.stringify(baselineForm) : null), [baselineForm]);
  const dirty = baselineJson !== null && currentJson !== null && currentJson !== baselineJson;
  useUnsavedChangesWarning(dirty);

  /**
   * Where an intercepted click was heading, or `null` when nothing is pending.
   *
   * The destination is held rather than a bare `open` boolean so the dialog's
   * "leave" can go where the author was actually going — one guard, any number
   * of guarded links, and no second copy of any href for the two to disagree
   * about.
   */
  const [pendingExit, setPendingExit] = useState<string | null>(null);
  const navigate = useNavigate();

  /**
   * A plain function, not a `useCallback`. It is handed out on `ctx`, which is
   * a fresh object literal every render anyway, so there is no identity for a
   * memo to preserve — and wrapping it made React Compiler skip optimising this
   * whole component ("existing memoization could not be preserved").
   */
  const guardLeave = (e: ReactMouseEvent<HTMLAnchorElement>, to: string) => {
    // Everything a modified click means (open in a new tab, a middle click)
    // leaves this page standing, so there is nothing to guard and swallowing it
    // would break the link. `defaultPrevented` because a caller's own `onClick`
    // runs first and may already have handled it.
    if (!dirty || e.defaultPrevented || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) {
      return;
    }
    e.preventDefault();
    setPendingExit(to);
  };

  const save = useCallback(async () => {
    // `trashed` here as well as on the button: Save is also reachable from a
    // section that renders its own submit, and the server's answer would be a
    // 409 read out as a red toast.
    if (!form || !baselineForm || trashed) return;
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
  }, [form, baselineForm, trashed, updateMutation, ws, id]);

  /**
   * Come back from the trash. The one action this page offers a trashed
   * endpoint, and the only reason the page is worth rendering at all in that
   * state.
   *
   * The form is re-seeded from what the server returned rather than left as it
   * was: restore lands the endpoint in `disabled`, never back in `live`, and
   * the read-only half of this page (the state badge sections render) has to
   * agree with that immediately. The wrapper's invalidation refreshes the
   * record and both lists on its own — see `src/api/dataApi.ts`.
   */
  const restore = useCallback(async () => {
    try {
      const res = await restoreMutation.mutateAsync({ workspace: ws, id });
      if (res.status !== 200) {
        toast.error("Could not restore.");
        return;
      }
      const seeded = toForm(res.data);
      setFormState(seeded);
      setBaselineForm(seeded);
      toast.success("Endpoint restored as disabled");
    } catch (e) {
      toast.error(String(e));
    }
  }, [restoreMutation, ws, id]);

  // The page's one document-outline `h1`, on every branch below. Once the endpoint
  // has loaded it is the VISIBLE compact tool title (FE-PAGE-7, 16px); before
  // that (loading/error) there is no title to show, so it is `sr-only` and falls
  // back to "Endpoint", as `ChartExplore.tsx`'s own `h1` falls back to "New chart".
  const headingName = endpoint?.name ?? "Endpoint";

  if (isLoading) {
    return (
      <PageContainer>
        <h1 className="sr-only">{headingName}</h1>
        <ListSkeleton cols={2} />
      </PageContainer>
    );
  }
  if (isError || !endpoint) {
    return (
      <PageContainer>
        <h1 className="sr-only">{headingName}</h1>
        <Alert variant="destructive">
          <AlertDescription>
            {(error as unknown as Error)?.message ?? "Could not load this endpoint."}
          </AlertDescription>
        </Alert>
        <div className="mt-3">
          <BackLink to={`/explore/${ws}/data-api/endpoints`}>Endpoints</BackLink>
        </div>
      </PageContainer>
    );
  }
  if (!form || !baselineForm) {
    // Either the hydration effect hasn't flushed yet, or `id` just changed
    // and the new endpoint hasn't arrived — one render, at most, either way.
    return (
      <PageContainer>
        <h1 className="sr-only">{headingName}</h1>
        <ListSkeleton cols={2} />
      </PageContainer>
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
    trashed,
    refetch: () => {
      void refetch();
    },
    guardLeave,
  };

  const endpointsHref = `/explore/${ws}/data-api/endpoints`;
  /**
   * Is the draft on screen the definition a consumer is receiving?
   *
   * The one state this header exists for. Before it, a Save and a Publish left
   * the same trace on every surface egret has — proven end to end: mid-
   * divergence the router answered `{"one":1}` at `version:1` while the draft
   * on screen read `SELECT 42`.
   *
   * Answered by comparing the draft against the live version's frozen snapshot
   * — see `draftVsServed`. Its four values include `unknown`, and the pill
   * renders that word rather than falling back to the confident one.
   */
  const served = draftVsServed(endpoint, lookup);
  const stateLabel = endpointHeaderStateLabel(endpoint, {
    liveVersion: liveVersion?.version,
    served,
  });

  return (
    // `p-4`, matching every other top-level Explore detail route (e.g.
    // DatasetDetail.tsx) — this used to come for free from `DataApiLayout`'s
    // `mx-auto max-w-6xl p-6`, which wrapped its `<Outlet/>`. That shell is
    // gone (Data API is now three flat sidebar items, not a shell + section
    // nav), so this route supplies its own padding like its siblings.
    // The header and nav were never capped by the shell either — and as of the
    // width pass below, neither is the section content.
    //
    // **`flex h-full flex-col` — a bounded column, not a growing block.** One
    // section (Query) is a full-height editing surface: an SQL editor above a
    // preview, split by a drag handle, which can only work if it is handed a
    // height rather than growing to whatever its content needs. The height is
    // already there and needs no viewport constant: `AppShell` renders
    // `<main className="min-w-0 flex-1 overflow-auto p-6">` inside an
    // `h-screen` flex column, so `h-full` here resolves against a definite
    // height — the same chain `routes/explore/SqlLab.tsx` uses.
    //
    // The five other sections are unchanged in what they render; what changed
    // is WHICH element scrolls. Before, the page grew and `main` scrolled;
    // now the `fieldset` below scrolls and `main` has nothing to scroll. The
    // header, the trash banner and the section nav are `shrink-0` so a long
    // section cannot squeeze them (flex items shrink by default), which is
    // also what keeps them on screen while a section scrolls under them.
    <PageContainer className="flex h-full flex-col">
      <div className="mb-4 flex shrink-0 flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          {/* Guarded, and it is the whole of Finding 3: measured back to back
              against a control on the same dirty form, a browser reload raised
              `beforeunload` and this link discarded the edit without a word. */}
          {/* The shell is OUTSIDE its own `Outlet`, so it cannot use
              `LeaveGuardLink` (which reads the context the Outlet provides) —
              it holds `guardLeave` directly, and this is the same call the
              component makes for the sections. */}
          <BackLink to={endpointsHref} onClick={(e) => guardLeave(e, endpointsHref)}>
            Endpoints
          </BackLink>
          <PageTitle size="compact" className="truncate">
            {headingName}
          </PageTitle>
        </div>
        {/* No Save at all for a trashed endpoint — not a disabled one either.
            A greyed-out Save still says "this page edits things, just not right
            now"; the only thing this page can do to a trashed endpoint is
            restore it, and that control lives in the banner below with the
            sentence that explains it. */}
        <div className="flex items-center gap-2">
          {/* WHAT THIS ENDPOINT IS, BESIDE SAVE, ON ALL SIX TABS.
              Rendered for a trashed endpoint too — Save is not, but the state
              is still a fact, and the banner below says what the trash means
              rather than what the endpoint is.

              A LINK, and to `versions` deliberately: this pill is about the
              publish lifecycle, and `Versions` is the page for it. That also
              answers "Publish lives on one tab of six" without a second
              publish control anywhere — the header ROUTES to the one publish
              path rather than growing its own. A second Publish button here
              would also rebuild, permanently and on every tab, the exact
              geometry `Versions` deliberately dismantled: two primary-looking
              buttons in the page's top-right corner doing unrelated things. */}
          <Link
            to="versions"
            /* The visible word is the state; the accessible name is that word
               plus where the link goes, for a reader who cannot see that it
               sits beside a tab strip. WCAG 2.5.3 Label in Name is satisfied
               by construction — the name STARTS with the visible string,
               because both come from one call to `endpointHeaderStateLabel`
               and there is no second spelling to drift. (An `sr-only` suffix
               was tried first: name computation trims each text node, so the
               two ran together as `Live · v2(open the…)`.) */
            aria-label={`${stateLabel} — open the Versions tab`}
            /* No bespoke focus style: `index.css`'s base layer already gives
               every focusable element a solid `outline-ring`, which is the
               product's one focus indicator and what the two sibling links in
               this header use. `inline-flex` so the outline hugs the pill
               rather than the anchor's inline box. */
            className="inline-flex rounded-md"
          >
            <EndpointStateBadge
              endpoint={endpoint}
              liveVersion={liveVersion?.version}
              served={served}
            />
          </Link>
          {/* No Save at all for a trashed endpoint — not a disabled one either.
              A greyed-out Save still says "this page edits things, just not right
              now"; the only thing this page can do to a trashed endpoint is
              restore it, and that control lives in the banner below with the
              sentence that explains it. */}
          {!trashed && (
            <>
              {dirty && <span className="text-xs text-muted-foreground">Unsaved changes</span>}
              <Button onClick={save} disabled={!dirty || updateMutation.isPending}>
                {updateMutation.isPending ? "Saving…" : "Save"}
              </Button>
            </>
          )}
        </div>
      </div>

      {/* Finding 3's other half: the question the silent link never asked.
          `AlertDialog`, not `Dialog` — this interrupts an action and wants a
          decision, which is the role difference (`alertdialog`) rather than a
          styling one. */}
      <AlertDialog
        open={pendingExit !== null}
        onOpenChange={(open) => {
          if (!open) setPendingExit(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Leave without saving?</AlertDialogTitle>
            <AlertDialogDescription>
              This endpoint has changes you have not saved. Leaving discards them — the saved draft
              and the live version both stay exactly as they are.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Stay on this page</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                const to = pendingExit;
                setPendingExit(null);
                if (to) navigate(to);
              }}
            >
              Discard changes and leave
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* The page saying plainly what it is. Before this, the Trash linked by
          name straight into a six-section editor with an enabled Save, no
          banner and no way back: every edit, publish and rollback made from
          here came back as `toast.error("Error: endpoint is in the trash;
          restore it before editing it")`. The row menu in the Trash had already
          dropped its Open item for exactly that reason — the link beside it had
          not. */}
      {trashed && (
        <Alert className="mb-5 shrink-0 border-destructive/50">
          <AlertTitle>This endpoint is in the trash</AlertTitle>
          <AlertDescription className="mt-1 flex flex-wrap items-center justify-between gap-3">
            <span className="max-w-prose">
              It is not serving, and it cannot be edited, published, rolled back, enabled or
              disabled while it is here — every one of those is refused. Everything below is
              read-only. Restoring brings it back <strong>disabled</strong>, never straight back to
              serving.
            </span>
            <Button size="sm" onClick={() => void restore()} disabled={restoreMutation.isPending}>
              {restoreMutation.isPending ? "Restoring…" : "Restore"}
            </Button>
          </AlertDescription>
        </Alert>
      )}

      <nav
        className={`-mx-1 mb-5 shrink-0 px-1 ${tabsListClass("underline")}`}
        aria-label="Endpoint sections"
      >
        {SECTIONS.map((s) => (
          <NavLink
            key={s.to}
            to={s.to}
            end={s.end}
            className={tabsTriggerClass("underline")}
          >
            {s.label}
          </NavLink>
        ))}
      </nav>

      {/* No width cap here. This wrapper used to carry `max-w-3xl`, which is
          what left the right third of the page empty while Config's Details
          column wrapped to three lines. A single page-level cap turned out to
          be the wrong instrument: the six sections do not want the same width.
          Config and Query should use the page — Config holds the two widest
          controls of any tab, Query a SQL editor and a result grid. Access,
          Try it, Versions and Response read worse stretched to a wide monitor
          (prose, short-column lists, and in Response's case a three-column
          table whose two Selects ended up ~1260px right of their labels), so
          each of those four carries its own cap on its own root element —
          visible next to the layout it constrains, rather than imposed on
          siblings that did not ask for it. `key={id}` is redundant with `EndpointDetail`'s
          own remount-per-id above, kept anyway as a second, independent
          guarantee that a section can never carry its own local state across
          two different endpoints. */}
      {/* `fieldset[disabled]` is the floor, not the explanation — a section
          also reads `ctx.trashed` and says WHY its own actions are dead, which
          is the half the shell cannot supply: the banner above is over-broad
          for any section that keeps something deliberately live (Access's
          `Try it` link), and only the section knows which of its controls
          those are.

          Measured rather than asserted, because the sentence here used to
          claim "every section" does this and that was never true (§9):
          `Query`, `TryIt`, `Versions`, `Access` and `Response` read the flag;
          `Versions` and `Access` put a reason on screen. `Config` needs
          neither — its only mutation is Save, which the header removes
          entirely rather than greying.

          The floor is what makes forgetting safe: the failure mode of a
          section added later would otherwise be a control that looks live and
          answers 409. HTML
          disables every descendant input, button, select and textarea of a
          disabled fieldset, so the default for anything new here is inert.
          `min-w-0` because a `fieldset`'s UA `min-inline-size: min-content` is
          not part of Tailwind's preflight reset and would otherwise refuse to
          shrink below its widest child. */}
      {/* The flexible remainder of the column above, and the page's ONE scroll
          region.

          `flex-1` takes every pixel the fixed-size header/banner/nav leave, and
          `min-h-0` is what lets it also give pixels BACK: a flex item's default
          `min-height: auto` refuses to shrink below its content, so without it
          a tall section pushes this box past the bottom of `main` and the whole
          page scrolls again — exactly the growing block this replaced, plus a
          Query tab whose `h-full` now resolves against a box taller than the
          viewport. `SqlLab.tsx`'s body carries the same pair for the same
          reason.

          `overflow-auto` rather than `overflow-hidden`: SQL Lab's body hosts
          one full-height child and nothing else, but five of the six sections
          here are ordinary documents that are legitimately taller than the
          viewport, and hiding their overflow would clip them with no way to
          reach the rest. `auto` scrolls those five and leaves the sixth — which
          fits itself to `h-full` — with nothing to scroll. */}
      <fieldset disabled={trashed} className="min-h-0 min-w-0 flex-1 overflow-auto">
        <Outlet key={id} context={ctx} />
      </fieldset>
    </PageContainer>
  );
}
