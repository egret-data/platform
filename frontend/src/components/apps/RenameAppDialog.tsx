import { useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import { type AppSummary, useAppMutations } from "@/api/apps";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/**
 * Rename an app and edit its description (`PATCH`).
 *
 * The fields are seeded from `app` ONCE, at mount. Mount it only while there is an app to rename
 * (`{target && <RenameAppDialog … />}`), as the list does — a dialog kept mounted across targets
 * would open on the previous app's text.
 */
export function RenameAppDialog({ ws, app, open, onOpenChange }: {
  ws: string;
  app: Pick<AppSummary, "id" | "name" | "description">;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { update } = useAppMutations(ws);
  const [name, setName] = useState(app.name);
  const [description, setDescription] = useState(app.description);
  const trimmed = name.trim();

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    // The button is disabled on a blank name, but Enter in the field submits the form anyway.
    if (!trimmed) return;
    try {
      // The description is sent even when blank, unlike on create: this is an edit, and "" is
      // how an existing description is cleared. Omitting the key would leave the old one.
      await update.mutateAsync({ id: app.id, name: trimmed, description: description.trim() });
      onOpenChange(false);
    } catch (err) {
      toast.error(String((err as Error)?.message ?? err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Rename app</DialogTitle>
          </DialogHeader>
          <div className="my-4 space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="rename-app-name">Name</Label>
              <Input id="rename-app-name" value={name} onChange={(e) => setName(e.target.value)} autoFocus />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="rename-app-description">Description</Label>
              <textarea
                id="rename-app-description"
                className="h-20 w-full resize-none rounded-md border bg-background px-3 py-2 text-sm text-foreground"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={update.isPending || trimmed === ""}>
              {update.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
