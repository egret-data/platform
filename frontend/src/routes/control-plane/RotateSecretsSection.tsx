import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { rotateEngineSecrets } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { ConfigSection } from "@/components/form/ConfigSection";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

/**
 * Rotate a Trino engine's internal secrets: the shared secret its coordinator and
 * workers authenticate each other with, and the password egret's monitor signs in
 * with. Rotation always mints a new pair and records it on the engine row, then
 * re-applies the engine so every pod restarts — hence the confirmation. If the
 * cluster apply then fails, the new pair is still on record (the cluster may
 * already hold it) and the previous pair is kept, not deleted, so nothing here
 * claims the old secrets are still in force on a failure. Saves on its own; it
 * is not part of the Settings form's Save.
 */
export function RotateSecretsSection({ engineId }: { engineId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const mutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => rotateEngineSecrets(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      setOpen(false);
      toast.success("Secrets rotated. The engine's pods are restarting.");
    },
    onError: (e) =>
      toast.error(e.message ?? "Failed to rotate the engine's secrets."),
  });

  return (
    <ConfigSection
      title="Internal secrets"
      description="The shared secret Trino's coordinator and workers use to trust each other, and the password egret's monitor signs in with. Rotate them if they may have been exposed."
    >
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => setOpen(true)}
          disabled={mutation.isPending}
        >
          {mutation.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <KeyRound className="h-4 w-4" />
          )}
          Rotate secrets
        </Button>
      </div>

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rotate internal secrets?</AlertDialogTitle>
            <AlertDialogDescription>
              New secrets are generated and applied, so all pods will restart and
              running queries are interrupted.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                mutation.mutate(engineId);
              }}
              disabled={mutation.isPending}
            >
              {mutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Rotate
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </ConfigSection>
  );
}
