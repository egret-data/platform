import { useState } from "react";
import { useParams } from "react-router-dom";
import { toast } from "sonner";

import {
  useApproveRequest,
  useDenyRequest,
  useWorkspaceRequests,
  type AccessRequest,
} from "@/api/accessRequests";
import { useWorkspaceRoles } from "@/api/members";
import { type WsRole } from "@/api/workspaceAccess";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
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
import { Forbidden } from "@/routes/Forbidden";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

/** Small dot colour for a role, using semantic design tokens (mirrors Members). */
function roleDotColor(role: string): string {
  if (role.includes("admin")) return "var(--success)";
  if (role.includes("editor")) return "var(--primary)";
  return "var(--muted-foreground)";
}

/** A coloured swatch preceding a role name. */
function RoleDot({ role }: { role: string }) {
  return (
    <span
      className="inline-block h-2 w-2 shrink-0 rounded-full"
      style={{ background: roleDotColor(role) }}
      aria-hidden
    />
  );
}

/** Convert a `yyyy-mm-dd` date-input value to an end-of-local-day RFC3339 timestamp. */
function toExpiresAt(dateValue: string): string | undefined {
  if (!dateValue) return undefined;
  return new Date(`${dateValue}T23:59:59`).toISOString();
}

/**
 * Workspace admin → Settings → Requests.
 *
 * The pending access-request queue for the workspace: approve (adding the
 * requester with a role + optional expiry) or deny.
 */
export default function AccessRequests() {
  const ws = useParams().workspace!;
  const q = useWorkspaceRequests(ws);
  const rolesQ = useWorkspaceRoles(ws);
  const approveMut = useApproveRequest(ws);
  const denyMut = useDenyRequest(ws);

  // Roles come from the same catalog Members uses. While loading (or if the
  // fetch fails/returns nothing) we fall back to an empty option list rather
  // than a hardcoded set that could drift from the real roles.
  const roles = rolesQ.data ?? [];

  const [approveTarget, setApproveTarget] = useState<AccessRequest | null>(null);
  const [denyTarget, setDenyTarget] = useState<AccessRequest | null>(null);

  const requests = q.data ?? [];
  const forbidden = errorStatus(q.error) === 403;

  const confirmApprove = (role: string, expiresDate: string) => {
    if (!approveTarget) return;
    const id = approveTarget.id;
    approveMut.mutate(
      { id, role, expires_at: toExpiresAt(expiresDate) },
      {
        onSuccess: () => toast.success("Request approved"),
        onError: (e) => toast.error(String(e)),
      },
    );
    setApproveTarget(null);
  };

  const confirmDeny = (note: string) => {
    if (!denyTarget) return;
    const id = denyTarget.id;
    denyMut.mutate(
      { id, note: note.trim() || undefined },
      {
        onSuccess: () => toast.success("Request denied"),
        onError: (e) => toast.error(String(e)),
      },
    );
    setDenyTarget(null);
  };

  return (
    <div className="space-y-5">
      <header>
        <h2 className="text-lg font-semibold">Access requests</h2>
        <p className="text-sm text-muted-foreground">
          People asking to join <span className="font-medium">{ws}</span>. Approve to add them
          (optionally time-boxed) or deny.
        </p>
      </header>

      {q.isLoading ? (
        <ListSkeleton rows={4} cols={5} />
      ) : forbidden ? (
        <Forbidden level="section" reason="Only a workspace admin can review access requests." />
      ) : q.isError ? (
        <p className="py-6 text-sm text-destructive" role="alert">
          Couldn&apos;t load requests. {String(q.error)}
        </p>
      ) : requests.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No pending requests.
        </p>
      ) : (
        <ul className="divide-y rounded-md border">
          {requests.map((r) => (
            <li key={r.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1 space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{r.requester_name ?? r.requester_id}</span>
                  <Badge variant="outline">{r.requested_role}</Badge>
                  <span className="text-xs text-muted-foreground" title={fullTimestamp(r.created_at)}>
                    requested {timeAgo(r.created_at)}
                  </span>
                </div>
                {r.reason && (
                  <p className="truncate text-sm text-muted-foreground" title={r.reason}>
                    {r.reason}
                  </p>
                )}
              </div>
              <div className="flex shrink-0 gap-2">
                <Button size="sm" onClick={() => setApproveTarget(r)}>
                  Approve
                </Button>
                <Button size="sm" variant="outline" onClick={() => setDenyTarget(r)}>
                  Deny
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}

      <ApproveDialog
        request={approveTarget}
        roles={roles}
        pending={approveMut.isPending}
        onOpenChange={(o) => !o && setApproveTarget(null)}
        onConfirm={confirmApprove}
      />

      <DenyDialog
        request={denyTarget}
        pending={denyMut.isPending}
        onOpenChange={(o) => !o && setDenyTarget(null)}
        onConfirm={confirmDeny}
      />
    </div>
  );
}

/** Approve confirmation: pick a workspace role (defaults to the requested one) + optional expiry. */
function ApproveDialog({
  request,
  roles,
  pending,
  onOpenChange,
  onConfirm,
}: {
  request: AccessRequest | null;
  roles: WsRole[];
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (role: string, expiresDate: string) => void;
}) {
  const [role, setRole] = useState("");
  const [expiresDate, setExpiresDate] = useState("");

  // A past expiry makes no sense — floor the date picker at today (local).
  const today = new Date().toISOString().slice(0, 10);

  // Reset local state whenever a new request is targeted.
  const key = request?.id ?? "";
  const [lastKey, setLastKey] = useState(key);
  if (key !== lastKey) {
    setLastKey(key);
    setRole(request?.requested_role ?? "");
    setExpiresDate("");
  }

  return (
    <Dialog open={!!request} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Approve request</DialogTitle>
          <DialogDescription>
            Add {request?.requester_name ?? request?.requester_id} to the workspace.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="approve-role">Role</Label>
            <Select value={role} onValueChange={setRole}>
              <SelectTrigger id="approve-role" aria-label="Role" className="w-full">
                <span className="flex items-center gap-2">
                  {role && <RoleDot role={role} />}
                  <SelectValue placeholder="Select a role…" />
                </span>
              </SelectTrigger>
              <SelectContent>
                {roles.map((r) => (
                  <SelectItem key={r.name} value={r.name}>
                    <span className="flex items-center gap-2">
                      <RoleDot role={r.name} />
                      {r.name}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="approve-expires">Access expires</Label>
            <Input
              id="approve-expires"
              type="date"
              min={today}
              value={expiresDate}
              onChange={(e) => setExpiresDate(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">Optional — leave blank for no expiry.</p>
          </div>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            disabled={!role || pending}
            onClick={() => onConfirm(role, expiresDate)}
          >
            Confirm
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Deny confirmation with an optional note. */
function DenyDialog({
  request,
  pending,
  onOpenChange,
  onConfirm,
}: {
  request: AccessRequest | null;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (note: string) => void;
}) {
  const [note, setNote] = useState("");

  const key = request?.id ?? "";
  const [lastKey, setLastKey] = useState(key);
  if (key !== lastKey) {
    setLastKey(key);
    setNote("");
  }

  return (
    <Dialog open={!!request} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Deny request</DialogTitle>
          <DialogDescription>
            {request?.requester_name ?? request?.requester_id} will not be added to the
            workspace.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-1.5">
          <Label htmlFor="deny-note">Note (optional)</Label>
          <Textarea
            id="deny-note"
            aria-label="Note (optional)"
            placeholder="Let them know why…"
            value={note}
            onChange={(e) => setNote(e.target.value)}
          />
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="destructive"
            disabled={pending}
            onClick={() => onConfirm(note)}
          >
            Confirm
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
