import { SubjectExplainForm } from "@/components/perm/SubjectExplain";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";

/** What the dialog answers. Also this module's bundle marker (`scripts/assert-governance-people-lazy.mjs`). */
export const CHECK_ACCESS_INTRO =
  "Whether a user, or each member of a group or role, can reach a target, per policy profile, as each engine would decide it.";

/**
 * The People index's Check access (spec §2): subject kind, subject, action
 * and target (`ScopePicker`), answered as a principal page's Check access tab
 * answers (`SubjectExplainForm`). Explain access, in a dialog.
 */
export function CheckAccessDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Check access</DialogTitle>
          <DialogDescription>{CHECK_ACCESS_INTRO}</DialogDescription>
        </DialogHeader>
        {open ? (
          <SubjectExplainForm
            layout={({ subject, action, target, submit }) => (
              <div className="grid gap-6 md:grid-cols-2">
                <div className="space-y-4">
                  {subject}
                  {action}
                </div>
                <div className="space-y-4">
                  {target}
                  <div className="flex flex-wrap items-center gap-2">{submit}</div>
                </div>
              </div>
            )}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
