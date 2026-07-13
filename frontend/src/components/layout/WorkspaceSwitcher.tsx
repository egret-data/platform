import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { UserPlus } from "lucide-react";
import { useMyWorkspaces } from "@/api/hooks";
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

export function WorkspaceSwitcher() {
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
        <SelectTrigger className="w-56" aria-label="Workspace">
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
