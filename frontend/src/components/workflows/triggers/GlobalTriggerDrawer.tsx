import { ApiError } from "@/api/client";
import { useWorkflowGetFlow, useWorkflowListFlowTriggers } from "@/api/generated/workflow/workflow";
import { TriggerDetailsDrawer } from "@/components/workflows/triggers/TriggerDetailsDrawer";
import { triggerBlock } from "@/components/workflows/triggers/triggerYaml";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** What the drawer says when the flow's triggers cannot be read (FE-COPY-2). Also this module's bundle marker. */
export const DETAILS_UNREADABLE = "The trigger's details could not be loaded: ";

const readError = (e: unknown) =>
  e instanceof ApiError && e.message ? `${e.message.replace(/\.+$/, "")}.` : "the request failed. Check your connection and try again.";

/**
 * The details drawer of the workspace's Triggers page (W7e spec §4.4, §4.2),
 * for `?trigger=<flow uuid>/<trigger id>`.
 *
 * A list row has no webhook path and no conditions (§3.5), so the drawer reads
 * the flow's own triggers, and the flow for the YAML its Definition cuts out.
 * The page reaches this module through `lazy()` only: it brings
 * `TriggerDetailsDrawer` and `triggerYaml.ts` (the `yaml` library), which the
 * list does not need (`scripts/assert-workflows-lazy.mjs`).
 */
export default function GlobalTriggerDrawer({
  ws,
  flowUuid,
  triggerId,
  onClose,
}: {
  ws: string;
  flowUuid: string;
  triggerId: string;
  onClose: () => void;
}) {
  const triggers = useWorkflowListFlowTriggers(ws, flowUuid, { query: { queryKey: workflowsKeys.flowTriggers(flowUuid) } });
  const flow = useWorkflowGetFlow(ws, flowUuid, { query: { queryKey: workflowsKeys.flow(flowUuid) } });
  const page = triggers.data?.status === 200 ? triggers.data.data : undefined;
  const source = flow.data?.status === 200 ? flow.data.data.source : undefined;
  // The Definition waits for the flow too, so the YAML does not appear after the rest; a flow
  // that cannot be read leaves the Definition saying so, and the rest still shows.
  const pending = !page || (source === undefined && !flow.isError);

  return (
    <TriggerDetailsDrawer
      open
      onClose={onClose}
      triggerId={triggerId}
      entry={page?.triggers.find((t) => t.trigger_id === triggerId)}
      ctx={{ flowDisabled: page?.flow_disabled ?? false, flowDeleted: page?.flow_deleted ?? false }}
      webhooksEnabled={page?.webhooks_enabled ?? true}
      block={source === undefined ? null : triggerBlock(source, triggerId)}
      base={`/explore/${ws}/workflows`}
      pending={pending && !triggers.isError}
      failure={triggers.isError && !page ? `${DETAILS_UNREADABLE}${readError(triggers.error)}` : null}
    />
  );
}
