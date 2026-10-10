import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Link, Navigate, useLocation, useNavigate, useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Milestone, Play, RotateCw, Save, Trash2 } from "lucide-react";
import { ApiError } from "@/api/client";
import {
  useWorkflowDeleteFlow,
  useWorkflowGetFlow,
  useWorkflowListFlowVersions,
  useWorkflowPublishFlow,
  useWorkflowUpdateFlow,
} from "@/api/generated/workflow/workflow";
import type { FlowDto, FlowVersionDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { StateChip } from "@/components/workflows/StateChip";
import { PublishDialog } from "@/components/workflows/PublishDialog";
import { RunFlow } from "@/components/workflows/run/RunFlow";
import { StaleSaveDialog } from "@/components/workflows/StaleSaveDialog";
import {
  canCreateVersion,
  canRun,
  canSave,
  changedSinceVersion,
  isFlowDisabled,
  latestVersion,
  versionSourceOf,
} from "@/components/workflows/draftState";
import { useFlowEditor } from "@/components/workflows/editor/EditorContext";
import { disposeFlowModel } from "@/components/workflows/editor/flowModel";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { Forbidden } from "@/routes/Forbidden";
import {
  DisabledReasons,
  FlowEditPage,
  ReasonButton,
  useFlowLayout,
  useSaveShortcut,
} from "@/routes/workflows/FlowEditPage";
import { FlowRevisions } from "@/routes/workflows/FlowRevisions";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The flow detail's tabs in W7a (spec §3); W7b and W7e add theirs here. */
export const FLOW_TABS = ["edit", "revisions"] as const;
type FlowTab = (typeof FLOW_TABS)[number];
const isFlowTab = (t: string | undefined): t is FlowTab => (FLOW_TABS as readonly string[]).includes(t ?? "");

/**
 * `ErrorBody.code` of the 409 `PUT …/flows/{id}` answers when `if_updated_at`
 * no longer matches (`DRAFT_CHANGED_CODE` in
 * `crates/egret-workflow/src/routes.rs`). The same status also refuses a
 * namespace and id another flow holds; that one carries no code.
 */
export const DRAFT_CHANGED_CODE = "draft_changed";

/**
 * Where the create page hands over text typed while its POST was in flight
 * (`FlowNew.tsx`): the flow's editor opens on it, unsaved.
 */
export type CarriedDraft = { unsavedSource: string };

const errorMessage = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

/**
 * `flows/:id/:tab` (spec §3, §5.3): the flow's header, its actions, and its
 * tabs. An unknown tab goes to `edit`. Keyed by the flow, so another flow
 * starts from its own state.
 */
export default function FlowDetail() {
  const { workspace = "", id = "", tab } = useParams();
  if (!isFlowTab(tab)) {
    return <Navigate to={`/explore/${workspace}/workflows/flows/${encodeURIComponent(id)}/edit`} replace />;
  }
  return <FlowDetailLoader key={id} ws={workspace} id={id} tab={tab} />;
}

function FlowDetailLoader({ ws, id, tab }: { ws: string; id: string; tab: FlowTab }) {
  const query = useWorkflowGetFlow(ws, id, { query: { queryKey: workflowsKeys.flow(id) } });
  const flow = query.data?.status === 200 ? query.data.data : undefined;
  const listHref = `/explore/${ws}/workflows/flows`;

  // Error first (FE-STATE-1), but only while nothing has loaded: a refetch that
  // fails after the editor opened must not unmount it and drop unsaved edits
  // (FE-ACTION-3); the editor stays on the last answer.
  if (query.isError && !flow) {
    // Spec §10: a 403 (the module or `workflow:read`) is the forbidden page; a retry cannot help.
    if (query.error instanceof ApiError && query.error.status === 403) return <Forbidden />;
    const notFound = query.error instanceof ApiError && query.error.status === 404;
    return (
      <div className="p-4">
        <h1 className="sr-only">{notFound ? "Flow not found" : "Flow"}</h1>
        {notFound ? (
          <EmptyState
            variant="dashed"
            title="Flow not found"
            description="It may have been deleted, or the link points to another workspace."
            action={
              <Button asChild variant="outline" size="sm">
                <Link to={listHref}>Back to flows</Link>
              </Button>
            }
          />
        ) : (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>The flow could not be loaded: {errorMessage(query.error, "the request failed")}. Try again.</span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        )}
      </div>
    );
  }
  if (!flow) {
    return (
      <div className="p-4">
        <h1 className="sr-only">Flow</h1>
        <ListSkeleton cols={2} />
      </div>
    );
  }
  return <FlowWorkspace ws={ws} id={id} tab={tab} flow={flow} refetchFlow={query.refetch} />;
}

/**
 * One save of the draft. A plain Save sends the editor's text at the moment it
 * runs; "Restore to draft" sends a version's text and, once it is saved, puts
 * it in the editor (one undoable edit, `CodeTab`), so a refused or failed
 * restore leaves the editor's text as it was (FE-ACTION-3).
 */
type SaveRequest = {
  /** The text to save; the editor's current text when absent. */
  source?: string;
  /** Write `source` into the editor once it is saved (a restore). */
  replaceEditor?: boolean;
  successToast?: string;
  /** What this save writes, as the stale-save dialog names it ("v1's text"); the editor's text when absent. */
  label?: string;
};

const SAVED_TOAST = "Saved. New runs use this version of the flow.";

/** What the editor's text was last loaded or saved as: the precondition of the next save. */
type Baseline = {
  source: string;
  /**
   * `FlowDto.updated_at` exactly as the server sent it. It carries
   * microseconds; a JS `Date` keeps milliseconds, so parsing it would make
   * every conditional save a 409. Never parsed, never re-formatted.
   */
  updatedAt: string;
};

function FlowWorkspace({
  ws,
  id,
  tab,
  flow,
  refetchFlow,
}: {
  ws: string;
  id: string;
  tab: FlowTab;
  /** The latest answer of `GET …/flows/{id}`: state, source and permission come from here. */
  flow: FlowDto;
  refetchFlow: () => Promise<{ isError: boolean; data?: { status: number; data: unknown } }>;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const base = `/explore/${ws}/workflows`;
  const manage = flow.can_manage;

  const [baseline, setBaseline] = useState<Baseline>(() => ({ source: flow.source, updatedAt: flow.updated_at }));
  // Text the create page carried over (typed while its POST was in flight). Read
  // once, then cleared from the history entry so a reload opens the saved draft.
  const location = useLocation();
  const [carried] = useState(() => (location.state as CarriedDraft | null)?.unsavedSource);
  useEffect(() => {
    if (carried !== undefined) navigate(location.pathname, { replace: true, state: null });
    // Once, on mount: `carried` never changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const versionsQuery = useWorkflowListFlowVersions(ws, id, { query: { queryKey: workflowsKeys.versions(id) } });
  const versions: FlowVersionDto[] | undefined = versionsQuery.data?.status === 200 ? versionsQuery.data.data : undefined;

  const update = useWorkflowUpdateFlow();
  const publish = useWorkflowPublishFlow();
  const remove = useWorkflowDeleteFlow();

  // Save is bound once, into Monaco's Ctrl/Cmd+S and the context, so it calls
  // through a ref that every render points at that render's `saveFlow`.
  const saveRef = useRef<(request?: SaveRequest, overwrite?: boolean) => Promise<boolean>>(async () => false);
  const save = useCallback(() => void saveRef.current(), []);
  const flowLayout = useFlowLayout(id);
  const editor = useFlowEditor({
    workspace: ws,
    flowId: id,
    initialSource: carried ?? flow.source,
    readOnly: !manage,
    save: manage ? save : undefined,
    onRevealCode: flowLayout.revealCode,
  });

  // The Code tab keeps its Monaco model on unmount; the page drops it when it leaves the flow.
  useEffect(() => () => disposeFlowModel(id), [id]);

  const dirty = editor.source !== baseline.source;
  useUnsavedChangesWarning(dirty);

  /** The save the server refused with `draft_changed`, which the stale dialog may overwrite; open while set. */
  const [staleSave, setStaleSave] = useState<SaveRequest | null>(null);
  const [publishOpen, setPublishOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [runOpen, setRunOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const inFlight = useRef(false);
  /**
   * Set when the read after a publish failed, so `baseline.updatedAt` may be
   * the pre-publish value: the next save reads the draft first rather than
   * collect a false "draft changed".
   */
  const needsReread = useRef(false);

  /** `GET …/flows/{id}` again; `undefined` when that read failed (react-query keeps the old data then). */
  const readFresh = async (): Promise<FlowDto | undefined> => {
    const res = await refetchFlow();
    if (res.isError || res.data?.status !== 200) return undefined;
    return res.data.data as FlowDto;
  };

  const adopt = useCallback((saved: FlowDto) => {
    setBaseline({ source: saved.source, updatedAt: saved.updated_at });
    queryClient.setQueryData(workflowsKeys.flow(id), { status: 200, data: saved, headers: new Headers() });
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.flow(id) });
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.flows(ws) });
  }, [queryClient, id, ws]);

  /** Resolves `true` when the draft was saved; a refusal or failure has been said (a toast, the stale dialog). */
  const saveFlow = async (request: SaveRequest = {}, overwrite = false): Promise<boolean> => {
    // One save at a time: Ctrl/Cmd+S reaches both Monaco's binding and the page's.
    if (!manage || inFlight.current) return false;
    const source = request.source ?? editor.source;
    if (!overwrite && source === baseline.source) return false;
    inFlight.current = true;
    setSaving(true);
    let precondition = baseline.updatedAt;
    try {
      if (needsReread.current && !overwrite) {
        const fresh = await readFresh();
        if (!fresh) {
          toast.error("The draft could not be re-read after publishing, so it was not saved. Reload the page, then save again.");
          return false;
        }
        needsReread.current = false;
        // Still the draft this editor saved: publishing moved only `updated_at`. If
        // someone else saved meanwhile, the old value stands and the save is refused.
        if (fresh.source === baseline.source) {
          precondition = fresh.updated_at;
          setBaseline({ source: baseline.source, updatedAt: fresh.updated_at });
        }
      }
      const res = await update.mutateAsync({
        workspace: ws,
        id,
        // `folder_id` is sent as it is: the route sets whatever it is given, and the
        // workflow UI never moves a flow between folders.
        data: overwrite
          ? { source, folder_id: flow.folder_id ?? null }
          : { source, folder_id: flow.folder_id ?? null, if_updated_at: precondition },
      });
      if (res.status !== 200) throw new Error("The flow could not be saved.");
      adopt(res.data);
      if (request.replaceEditor) editor.setSource(res.data.source);
      setStaleSave(null);
      // Every execution runs the saved draft (the user's ruling, 2026-10-10), so a save changes what runs.
      toast.success(request.successToast ?? SAVED_TOAST);
      return true;
    } catch (e) {
      if (!overwrite && e instanceof ApiError && e.status === 409 && e.code === DRAFT_CHANGED_CODE) {
        setStaleSave(request);
      } else {
        toast.error(errorMessage(e, "The flow could not be saved. Try again."));
        if (overwrite) throw e;
      }
      return false;
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  };
  useEffect(() => { saveRef.current = saveFlow; });
  useSaveShortcut(save, manage);

  /**
   * "Restore to draft" (Revisions): a Save of the version's text, with the same
   * precondition and the same stale-draft dialog. It never publishes.
   */
  const restoreVersion = async (version: FlowVersionDto, source: string) => {
    const saved = await saveFlow({
      source,
      replaceEditor: true,
      successToast: `Restored v${version.version} to the draft. New runs use it.`,
      label: `v${version.version}'s text`,
    });
    if (saved) void queryClient.invalidateQueries({ queryKey: workflowsKeys.versions(id) });
    return saved;
  };

  /** Reload after a 409: the saved draft replaces the editor's text (the dialog has confirmed). */
  const reload = async () => {
    const fresh = await readFresh();
    if (!fresh) {
      toast.error("The saved draft could not be loaded. Try again.");
      throw new Error("reload failed");
    }
    needsReread.current = false;
    setBaseline({ source: fresh.source, updatedAt: fresh.updated_at });
    editor.setSource(fresh.source);
    toast.success("Reloaded the saved draft");
  };

  const runPublish = async (note: string) => {
    const res = await publish.mutateAsync({ workspace: ws, id, data: note ? { note } : {} });
    if (res.status !== 200) throw new Error("The version could not be created.");
    const version = res.data;
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.versions(id) });
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.flows(ws) });
    // Creating a version moves the draft's `updated_at` too (`set_live`), so the next
    // conditional save needs the new one. It is taken only while the saved
    // draft is still the text this editor holds; if someone saved in between,
    // the next save is refused, which is the point of the precondition.
    const fresh = await readFresh();
    const ours = baseline;
    // A failed read leaves the pre-publish `updated_at`; the next save reads first.
    if (!fresh) needsReread.current = true;
    else if (fresh.source === ours.source) setBaseline({ source: ours.source, updatedAt: fresh.updated_at });
    if (versionSourceOf(version) !== undefined && versionSourceOf(version) !== ours.source) {
      toast.warning(`Created v${version.version} from the saved draft, which someone changed since you opened it.`);
    } else {
      toast.success(`Created v${version.version}`);
    }
  };

  const runDelete = async () => {
    try {
      await remove.mutateAsync({ workspace: ws, id });
      queryClient.removeQueries({ queryKey: workflowsKeys.flow(id) });
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.flows(ws) });
      // The list is the result; no toast (FE-ACTION-2).
      navigate(`${base}/flows`);
    } catch (e) {
      toast.error(errorMessage(e, "The flow could not be deleted. Try again."));
    }
  };

  // The changes indicator (spec §5.3, FE-STATE-4): the SAVED draft against the
  // latest version's frozen source, byte for byte. No version, no pill; a
  // version list that could not be read is said, never shown as "no changes".
  const latest = versions ? latestVersion(versions) : undefined;
  const latestSource = versionSourceOf(latest);
  let changesPill: string | null = null;
  if (versionsQuery.isError) changesPill = "Versions unknown";
  else if (latest && latestSource === undefined) changesPill = "Versions unknown";
  else if (latest && latestSource !== undefined && changedSinceVersion(baseline.source, latestSource)) {
    changesPill = `Changes since v${latest.version}`;
  }

  const errors = editor.problems.filter((p) => p.severity === "error").length;
  const validationStatus = editor.validation.status;
  const saveState = canSave({ dirty, saving });
  const versionState = canCreateVersion({ dirty, errors, validationStatus });
  // Run starts the saved draft (spec §6.1); with no unsaved edits, the editor's
  // text, its validation and the form's inputs are all that draft.
  const disabled = isFlowDisabled(flow.state, flow.source);
  // Someone else's save, read back (after Create version, for one): the server would run text
  // this editor neither shows nor validated.
  const stale = !dirty && flow.source !== baseline.source;
  const runState = canRun({ manage, dirty, errors, validationStatus, disabled, stale });
  const nextVersion = versions ? Math.max(0, ...versions.map((v) => v.version)) + 1 : null;
  const flowName = `${flow.namespace}.${flow.flow_id}`;
  const reasonsId = useId();

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex flex-wrap items-start justify-between gap-3 px-4 pt-4">
        <div className="min-w-0">
          <Link className="text-sm text-muted-foreground hover:underline" to={`${base}/flows`}>
            ← Flows
          </Link>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <h1 className="truncate text-xl font-semibold">{flow.flow_id}</h1>
            <StateChip state={flow.state} kind="flow" />
            {/* The latest version, shown apart from the chip (the user's ruling, 2026-10-10):
                a snapshot to compare and restore, never what runs. */}
            <span className="text-sm text-muted-foreground tabular-nums" data-testid="flow-latest-version">
              Version {flow.latest_version != null ? `v${flow.latest_version}` : "—"}
            </span>
            {changesPill && <Badge variant="outline">{changesPill}</Badge>}
            {dirty && <Badge variant="secondary">Unsaved changes</Badge>}
          </div>
          <p className="text-sm text-muted-foreground">{flow.namespace}</p>
        </div>
        {manage && (
          <div className="flex flex-col items-end gap-1">
            <div className="flex flex-wrap items-center justify-end gap-2">
              <ReasonButton enablement={saveState} reasonId={`${reasonsId}-save`} variant="outline" onClick={save}>
                <Save aria-hidden />
                Save
              </ReasonButton>
              <ReasonButton enablement={versionState} reasonId={`${reasonsId}-version`} onClick={() => setPublishOpen(true)}>
                <Milestone aria-hidden />
                Create version
              </ReasonButton>
              <ReasonButton enablement={runState} reasonId={`${reasonsId}-run`} variant="outline" onClick={() => setRunOpen(true)}>
                <Play aria-hidden />
                Run
              </ReasonButton>
              <Button type="button" size="sm" variant="ghost" onClick={() => setDeleteOpen(true)}>
                <Trash2 aria-hidden />
                Delete
              </Button>
            </div>
            <DisabledReasons
              actions={[
                { id: `${reasonsId}-save`, label: "Save", enablement: saveState },
                { id: `${reasonsId}-version`, label: "Create version", enablement: versionState },
                { id: `${reasonsId}-run`, label: "Run", enablement: runState },
              ]}
            />
          </div>
        )}
      </header>

      <Tabs
        value={tab}
        onValueChange={(t) => navigate(`${base}/flows/${encodeURIComponent(id)}/${t}`)}
        className="mt-3 flex min-h-0 flex-1 flex-col"
      >
        <TabsList className="mx-4 self-start">
          <TabsTrigger value="edit">Edit</TabsTrigger>
          <TabsTrigger value="revisions">Revisions</TabsTrigger>
        </TabsList>
        <TabsContent value="edit" className="min-h-0 flex-1">
          <FlowEditPage ctx={editor} flowLayout={flowLayout} />
        </TabsContent>
        <TabsContent value="revisions" className="min-h-0 flex-1 overflow-auto p-4">
          <FlowRevisions
            ws={ws}
            flowId={id}
            savedSource={baseline.source}
            dirty={dirty}
            stale={stale}
            saving={saving}
            manage={manage}
            onRestore={restoreVersion}
          />
        </TabsContent>
      </Tabs>

      <StaleSaveDialog
        open={staleSave !== null}
        onOpenChange={(o) => { if (!o) setStaleSave(null); }}
        onReload={reload}
        onOverwrite={async () => { await saveFlow(staleSave ?? {}, true); }}
        overwriteWith={staleSave?.label}
      />
      <PublishDialog
        open={publishOpen}
        onOpenChange={setPublishOpen}
        flowName={flowName}
        nextVersion={nextVersion}
        onPublish={runPublish}
      />
      {manage ? (
        // The run form reads the SAVED draft's inputs (`FlowDto.source`), the text the server
        // starts. Keyed by that text: a new save starts a new form; reopening keeps what was typed.
        <RunFlow
          key={flow.source}
          ws={ws}
          flowId={id}
          flowName={flowName}
          source={flow.source}
          open={runOpen}
          onOpenChange={setRunOpen}
        />
      ) : null}
      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete ${flowName}?`}
        description="The flow leaves the flow list and can no longer be run or scheduled. Executions already started are not stopped."
        onConfirm={() => void runDelete()}
      />
    </div>
  );
}
