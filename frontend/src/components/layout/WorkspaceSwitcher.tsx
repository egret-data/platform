import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { UserPlus } from "lucide-react";
import { useMyWorkspaces } from "@/api/hooks";
import { cn } from "@/lib/utils";
import RequestAccessDialog from "@/components/access/RequestAccessDialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const REQUEST_ACCESS_VALUE = "__request-access__";

/** `className` overrides the trigger's default fixed width. It exists for the narrow-viewport
 *  header, where a `w-56` trigger cannot shrink and drives the whole row past the viewport;
 *  `cn` is tailwind-merge, so a `w-*` passed here replaces `w-56` rather than fighting it. */
export function WorkspaceSwitcher({ className }: { className?: string } = {}) {
  const navigate = useNavigate();
  const { workspace } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);
  const [requestOpen, setRequestOpen] = useState(false);

  return (
    <>
      <Select
        value={workspace ?? ""}
        onValueChange={(v) => {
          if (v === REQUEST_ACCESS_VALUE) {
            setRequestOpen(true);
            return;
          }
          navigate(`/explore/${v}`);
        }}
      >
        <SelectTrigger className={cn("w-56", className)} aria-label="Workspace">
          <SelectValue placeholder="Select a workspace" />
        </SelectTrigger>
        <SelectContent>
          {(workspaces ?? []).map((w) => (
            <SelectItem key={w.id} value={w.slug}>{w.name}</SelectItem>
          ))}
          <SelectSeparator />
          <SelectItem value={REQUEST_ACCESS_VALUE}>
            <span className="flex items-center gap-2 text-muted-foreground">
              <UserPlus className="h-4 w-4" /> Request access…
            </span>
          </SelectItem>
        </SelectContent>
      </Select>
      <RequestAccessDialog open={requestOpen} onOpenChange={setRequestOpen} />
    </>
  );
}
