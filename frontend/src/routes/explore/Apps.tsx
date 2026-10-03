import { useState } from "react";
import { Loader2, PencilLine, Play, Plus } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";

import { type AppSummary, useApps, useCreateApp } from "@/api/apps";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

function NewAppDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const create = useCreateApp(ws);
  const [name, setName] = useState("");

  function close(next: boolean) {
    if (!next) setName("");
    onOpenChange(next);
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    try {
      const created = await create.mutateAsync({ name: name.trim() });
      close(false);
      navigate(`/explore/${ws}/apps/${created.id}/edit`);
    } catch (err) {
      toast.error(String((err as Error)?.message ?? err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent>
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Create app</DialogTitle>
            <DialogDescription>
              A data app is a marimo notebook that runs SQL as whoever is looking at it. You edit a draft;
              publishing it makes it the version people open.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-1.5">
            <Label htmlFor="app-name">Name</Label>
            <Input
              id="app-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Revenue explorer"
              autoFocus
            />
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || name.trim() === ""}>
              {create.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function StatusBadge({ app }: { app: AppSummary }) {
  return app.latest_version_no == null ? (
    <Badge variant="outline">Draft only</Badge>
  ) : (
    <Badge variant="secondary">Published v{app.latest_version_no}</Badge>
  );
}

export default function Apps() {
  const { workspace } = useParams();
  const ws = workspace ?? "";
  const { data, isLoading, error } = useApps(ws);
  const [creating, setCreating] = useState(false);

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Apps</h1>
        <Button onClick={() => setCreating(true)}>
          <Plus className="h-4 w-4" />
          New app
        </Button>
      </div>

      {error && (
        <Alert variant="destructive" className="mb-4">
          <AlertDescription>{error.message}</AlertDescription>
        </Alert>
      )}

      {isLoading ? (
        <ListSkeleton />
      ) : !data || data.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No apps yet. Create one to build a dashboard in code that queries your data as each viewer.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left text-muted-foreground">
              <th className="py-1 font-medium">Name</th>
              <th className="font-medium">Status</th>
              <th className="font-medium">Updated</th>
              <th className="w-px" />
            </tr>
          </thead>
          <tbody>
            {data.map((app) => (
              <tr key={app.id} className="border-t hover:bg-accent">
                <td className="py-1">
                  <Link className="font-medium hover:underline" to={`/explore/${ws}/apps/${app.id}/edit`}>
                    {app.name}
                  </Link>
                  {app.description && (
                    <div className="text-xs text-muted-foreground">{app.description}</div>
                  )}
                </td>
                <td>
                  <StatusBadge app={app} />
                </td>
                <td className="text-muted-foreground">
                  <RelativeTime iso={app.updated_at} />
                </td>
                <td className="whitespace-nowrap py-1 text-right">
                  {app.latest_version_no == null ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        {/* aria-disabled, not `disabled`: a disabled button swallows the hover that shows why. */}
                        <span>
                          <Button size="sm" variant="ghost" aria-disabled="true" tabIndex={-1} className="pointer-events-none opacity-50">
                            <Play className="h-4 w-4" />
                            Open
                          </Button>
                        </span>
                      </TooltipTrigger>
                      <TooltipContent side="bottom">Publish a draft before it can be opened.</TooltipContent>
                    </Tooltip>
                  ) : (
                    <Button size="sm" variant="ghost" asChild>
                      <Link to={`/explore/${ws}/apps/${app.id}`}>
                        <Play className="h-4 w-4" />
                        Open
                      </Link>
                    </Button>
                  )}
                  <Button size="sm" variant="outline" asChild>
                    <Link to={`/explore/${ws}/apps/${app.id}/edit`}>
                      <PencilLine className="h-4 w-4" />
                      Edit
                    </Link>
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <NewAppDialog ws={ws} open={creating} onOpenChange={setCreating} />
    </div>
  );
}
