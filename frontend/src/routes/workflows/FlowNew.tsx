import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Lock, Save } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowCreateFlow } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { canSave } from "@/components/workflows/draftState";
import { useFlowEditor } from "@/components/workflows/editor/EditorContext";
import { disposeFlowModel } from "@/components/workflows/editor/flowModel";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import type { CarriedDraft } from "@/routes/workflows/FlowDetail";
import {
  FlowEditPage,
  storeFlowLayout,
  useFlowLayout,
  useSaveShortcut,
} from "@/routes/workflows/FlowEditPage";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The create page's starting text (spec §5.4), verbatim. */
export const NEW_FLOW_TEMPLATE = `id: hello
namespace: company.team
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: Hello from egret
`;

/** The Monaco model of the flow being created; `flowModelPath("new")`. */
const NEW_MODEL_KEY = "new";

/**
 * `flows/new` (spec §5.4, FE-PAGE-3): the editor on the template, validated
 * through the id-less route. The first Save creates the flow and replaces the
 * URL with its editor, carrying the text and the layout over; a refusal (a
 * 409 when the namespace and id are taken) is shown above the editor in the
 * server's words, and the text stays.
 *
 * There is no flow to ask `can_manage` of, so the page learns the caller's
 * permission from the server: its own validate call (and Save) make the same
 * `workflow:manage` check, and a 403 from either turns the page read-only,
 * with a banner and no Save.
 */
export default function FlowNew() {
  const ws = useParams().workspace!;
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const create = useWorkflowCreateFlow();

  const saveRef = useRef<() => Promise<void>>(async () => {});
  const save = useCallback(() => void saveRef.current(), []);
  const flowLayout = useFlowLayout("");
  // Latched: once refused, the page stays read-only, and read-only stops the
  // validation that told it so.
  const [noPermission, setNoPermission] = useState(false);
  const editor = useFlowEditor({
    workspace: ws,
    initialSource: NEW_FLOW_TEMPLATE,
    readOnly: noPermission,
    save: noPermission ? undefined : save,
    onRevealCode: flowLayout.revealCode,
  });
  if (editor.validation.forbidden && !noPermission) setNoPermission(true);
  // The latest text, for what is typed while the POST is in flight.
  const latestSource = useRef(editor.source);
  useEffect(() => { latestSource.current = editor.source; });
  useEffect(() => () => disposeFlowModel(NEW_MODEL_KEY), []);

  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const inFlight = useRef(false);
  // Nothing is saved yet, so any text is unsaved; the template untouched is not worth a warning.
  const dirty = editor.source !== NEW_FLOW_TEMPLATE;
  useUnsavedChangesWarning(dirty && !noPermission);

  const saveFlow = async () => {
    if (inFlight.current || noPermission) return;
    inFlight.current = true;
    setSaving(true);
    setError(null);
    const source = editor.source;
    try {
      const res = await create.mutateAsync({ workspace: ws, data: { source } });
      if (res.status !== 201) throw new Error("The flow could not be created.");
      const flow = res.data;
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.flows(ws) });
      queryClient.setQueryData(workflowsKeys.flow(flow.id), { status: 200, data: flow, headers: new Headers() });
      storeFlowLayout(flow.id, flowLayout.layout);
      // The flow's editor is the result, so no toast (FE-ACTION-2). `replace`: Back
      // should not return to a create page for a flow that now exists. Text typed
      // while the POST was in flight goes with it, unsaved (FE-ACTION-3's intent).
      const typed = latestSource.current;
      const state: CarriedDraft | null = typed !== source ? { unsavedSource: typed } : null;
      navigate(`/explore/${ws}/workflows/flows/${encodeURIComponent(flow.id)}/edit`, { replace: true, state });
    } catch (e) {
      if (e instanceof ApiError && e.status === 403) {
        // The banner says it; the text stays, read-only.
        setNoPermission(true);
        return;
      }
      // Nothing is cleared (FE-ACTION-3): the text stays in the editor.
      setError(e instanceof Error && e.message ? e.message : "The flow could not be created. Try again.");
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  };
  useEffect(() => { saveRef.current = saveFlow; });
  useSaveShortcut(save, !noPermission);

  const saveState = canSave({ dirty: true, saving });
  const reasonId = useId();

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex flex-wrap items-start justify-between gap-3 px-4 pt-4">
        <div className="min-w-0">
          <Link className="text-sm text-muted-foreground hover:underline" to={`/explore/${ws}/workflows/flows`}>
            ← Flows
          </Link>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <h1 className="text-xl font-semibold">New flow</h1>
            {dirty && <Badge variant="secondary">Unsaved changes</Badge>}
          </div>
          <p className="text-sm text-muted-foreground">
            The flow&apos;s namespace and id come from its <code>namespace:</code> and <code>id:</code>.
          </p>
        </div>
        {!noPermission && (
          <div className="flex flex-col items-end gap-1">
            <ReasonButton enablement={saveState} reasonId={reasonId} onClick={save}>
              <Save aria-hidden />
              Save
            </ReasonButton>
            <DisabledReasons actions={[{ id: reasonId, label: "Save", enablement: saveState }]} />
          </div>
        )}
      </header>
      {noPermission && (
        <Alert className="mx-4 mt-3 w-auto">
          <Lock aria-hidden />
          <AlertTitle>You need permission to manage flows</AlertTitle>
          <AlertDescription>
            Creating a flow needs the workflow:manage permission. Ask a workspace admin to grant it.
          </AlertDescription>
        </Alert>
      )}
      {error && (
        <Alert variant="destructive" className="mx-4 mt-3 w-auto">
          <AlertTitle>The flow was not created</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <div className="mt-3 min-h-0 flex-1">
        <FlowEditPage ctx={editor} flowLayout={flowLayout} />
      </div>
    </div>
  );
}
