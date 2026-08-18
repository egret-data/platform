import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
// Mutations come from the hand-written wrapper, never from the generated hooks:
// the wrapper is what owns the cache invalidation (see `src/api/dataApi.ts`).
// Wired to the generated hook directly, "Endpoint disabled" toasts over a row
// whose badge still reads Live for the next 30 seconds.
import { useDisableEndpoint, useEnableEndpoint } from "@/api/dataApi";
import type { DataApiEndpointDto } from "@/api/generated/model";

/**
 * Per-row actions for a Data API endpoint: Open, the state-dependent
 * Disable/Enable, then Delete.
 *
 * Same shape as `DatasetActionsMenu`/`ChartActionsMenu`: a `RowActionsMenu`
 * wrapper, the destructive item last behind a `DropdownMenuSeparator` and
 * marked `text-destructive`, and the row's own mutations owned here rather than
 * threaded down as callbacks — `isPending` is then scoped to the row that fired
 * it. Deletion stays a callback because the confirmation dialog belongs to the
 * page, not to one row's menu.
 */
export function EndpointActionsMenu({ ws, endpoint, onDelete }: {
  ws: string;
  endpoint: DataApiEndpointDto;
  onDelete: () => void;
}) {
  const nav = useNavigate();
  const disableMutation = useDisableEndpoint();
  const enableMutation = useEnableEndpoint();

  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/data-api/endpoints/${endpoint.id}`)}>Open</DropdownMenuItem>
      {endpoint.state === "live" && (
        // `disabled` while in flight, like every other mutation trigger in
        // this feature: the menu closes on click but the row's menu can be
        // reopened before the request settles, and a second Disable on an
        // already-disabled endpoint is a 409 the author never asked for.
        <DropdownMenuItem
          disabled={disableMutation.isPending}
          onClick={() =>
            disableMutation.mutateAsync({ workspace: ws, id: endpoint.id })
              .then(() => toast.success("Endpoint disabled"))
              .catch((err) => toast.error(String(err)))
          }
        >
          Disable
        </DropdownMenuItem>
      )}
      {endpoint.state === "disabled" && (
        // `useDataApiEnableEndpoint` 409s when `live_version_id` is null
        // (EndpointStore::enable, lifecycle.rs) — there is nothing to
        // re-enable. `disabled` + `title` communicate this up front rather
        // than let the user discover it through a failed-mutation toast;
        // the `live_version_id` check INSIDE the handler too, since
        // `data-[disabled]:pointer-events-none` is CSS — real browsers
        // honour it, but nothing stops a click from reaching `onClick`
        // in an environment that doesn't apply CSS at all (jsdom).
        <DropdownMenuItem
          disabled={endpoint.live_version_id == null || enableMutation.isPending}
          title={endpoint.live_version_id == null ? "This endpoint has never been published; publish it instead." : undefined}
          onClick={() => {
            if (endpoint.live_version_id == null || enableMutation.isPending) return;
            enableMutation.mutateAsync({ workspace: ws, id: endpoint.id })
              .then(() => toast.success("Endpoint enabled"))
              .catch((err) => toast.error(String(err)));
          }}
        >
          Enable
        </DropdownMenuItem>
      )}
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>
        Delete
      </DropdownMenuItem>
    </RowActionsMenu>
  );
}
