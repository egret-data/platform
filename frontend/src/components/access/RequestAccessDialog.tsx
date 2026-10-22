/**
 * Request-access dialog (P2b).
 *
 * A self-serve flow: a signed-in user picks a workspace they're not yet a
 * member of, an optional role + reason, and submits an access request. Below
 * the form, "Your requests" lists the caller's own requests (pending ones get
 * a Cancel button).
 */
import { useState } from "react";
import { toast } from "sonner";

import {
  useCancelRequest,
  useCreateRequest,
  useJoinable,
  useMyRequests,
  type AccessRequest,
} from "@/api/accessRequests";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { errorStatus } from "@/lib/httpStatus";
import { StatusBadge } from "@/components/ui/status-badge";

const ROLES = ["ws_viewer", "ws_editor"];
const DEFAULT_ROLE = "ws_viewer";

function statusBadge(status: AccessRequest["status"]) {
  switch (status) {
    case "pending":
      return <StatusBadge tone="highlight">Pending</StatusBadge>;
    case "approved":
      return <StatusBadge tone="success">Approved</StatusBadge>;
    case "denied":
      return <StatusBadge tone="destructive">Denied</StatusBadge>;
    case "cancelled":
      return <StatusBadge tone="inactive">Cancelled</StatusBadge>;
  }
}

export interface RequestAccessDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function RequestAccessDialog({ open, onOpenChange }: RequestAccessDialogProps) {
  const joinableQ = useJoinable(open);
  const mineQ = useMyRequests(open);
  const createMut = useCreateRequest();
  const cancelMut = useCancelRequest();

  const [workspaceId, setWorkspaceId] = useState("");
  const [role, setRole] = useState(DEFAULT_ROLE);
  const [reason, setReason] = useState("");

  const joinable = joinableQ.data ?? [];
  const mine = mineQ.data ?? [];
  const canSubmit = workspaceId.trim().length > 0 && !createMut.isPending;

  function reset() {
    setWorkspaceId("");
    setRole(DEFAULT_ROLE);
    setReason("");
  }

  function submit() {
    if (!canSubmit) return;
    createMut.mutate(
      { workspace_id: workspaceId, role, reason: reason.trim() || undefined },
      {
        onSuccess: () => {
          toast.success("Access request sent");
          reset();
        },
        onError: (e) => {
          if (errorStatus(e) === 409) {
            toast.error("You already requested or are a member.");
          } else {
            toast.error(String(e));
          }
        },
      },
    );
  }

  function cancel(id: string) {
    cancelMut.mutate(id, {
      onSuccess: () => toast.success("Request cancelled"),
      onError: (e) => toast.error(String(e)),
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Request access</DialogTitle>
          <DialogDescription>
            Ask a workspace admin to add you. They&apos;ll review and approve or deny the request.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="request-access-workspace">Workspace</Label>
            {joinableQ.isLoading ? (
              <p className="text-sm text-muted-foreground">…</p>
            ) : joinable.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                You&apos;re a member of every workspace.
              </p>
            ) : (
              <Select value={workspaceId} onValueChange={setWorkspaceId}>
                <SelectTrigger id="request-access-workspace" className="w-full" aria-label="Workspace">
                  <SelectValue placeholder="Select a workspace…" />
                </SelectTrigger>
                <SelectContent>
                  {joinable.map((w) => (
                    <SelectItem key={w.id} value={w.id}>
                      {w.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="request-access-role">Role</Label>
            <Select value={role} onValueChange={setRole}>
              <SelectTrigger id="request-access-role" className="w-full" aria-label="Role">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ROLES.map((r) => (
                  <SelectItem key={r} value={r}>
                    {r}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="request-access-reason">Reason</Label>
            <Textarea
              id="request-access-reason"
              aria-label="Reason"
              placeholder="Why do you need access? (optional)"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
            />
          </div>
        </div>

        <DialogFooter>
          <Button type="button" onClick={submit} disabled={!canSubmit}>
            {createMut.isPending ? "Submitting…" : "Submit request"}
          </Button>
        </DialogFooter>

        <div className="space-y-2 border-t pt-4">
          <h3 className="text-sm font-semibold">Your requests</h3>
          {mineQ.isLoading ? (
            <p className="text-sm text-muted-foreground">…</p>
          ) : mine.length === 0 ? (
            <p className="text-sm text-muted-foreground">No requests yet.</p>
          ) : (
            <ul className="space-y-2">
              {mine.map((r) => (
                <li
                  key={r.id}
                  className="flex items-center justify-between gap-2 rounded-md border px-3 py-2 text-sm"
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-medium">{r.workspace_name ?? r.workspace_id}</span>
                    {statusBadge(r.status)}
                  </span>
                  {r.status === "pending" && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={() => cancel(r.id)}
                      disabled={cancelMut.isPending}
                    >
                      Cancel
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
