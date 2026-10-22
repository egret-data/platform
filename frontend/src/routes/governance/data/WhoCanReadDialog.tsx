import type { Scope } from "@/api/generated/model";
import { SubjectExplainForm } from "@/components/perm/SubjectExplain";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { fullPath } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/**
 * What the dialog says it answers. Also this module's bundle marker
 * (`scripts/assert-governance-data-lazy.mjs`).
 */
export const WHO_CAN_READ_INTRO =
  "Whether a user, or each member of a group or role, can read this, as each engine that serves its catalog would decide it.";

/** The explain target for `path`: exactly this object, and its column when the path names one. */
function targetOf(path: ObjectPath, catalogId: string): Scope {
  const scope: Scope = { catalog_id: catalogId };
  if (path.schema === undefined) return scope;
  scope.schema = path.schema;
  if (path.object === undefined) return scope;
  scope.table = path.object;
  if (path.column !== undefined) scope.column = path.column;
  return scope;
}

/**
 * "Who can read?" (spec §2.2; DG #2 P7): action `read` on exactly this object
 * (and column), for a user (`POST /perm/explain`, answered per policy profile
 * with `ExplainResultView`) or a group or role (`POST /perm/explain/principal`:
 * each member's own answer, `MembersExplainView`, never one verdict for the
 * group). F10: this is the only place the Data area says whether someone can
 * read something; it is explain's answer, never one worked out here. Each
 * answer shows only under the subject it was asked for (`SubjectExplainForm`).
 * Explain needs `policy:read`.
 */
export function WhoCanReadDialog({
  open,
  onOpenChange,
  path,
  catalogId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  path: ObjectPath;
  catalogId: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
        {open ? <WhoCanReadForm path={path} catalogId={catalogId} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function WhoCanReadForm({ path, catalogId }: { path: ObjectPath; catalogId: string }) {
  return (
    <>
      <DialogHeader>
        <DialogTitle>Who can read?</DialogTitle>
        <DialogDescription>
          {WHO_CAN_READ_INTRO} <span className="font-mono text-xs break-all">{fullPath(path)}</span>
        </DialogDescription>
      </DialogHeader>
      <SubjectExplainForm
        action="read"
        target={targetOf(path, catalogId)}
        layout={({ subject, submit }) => (
          <div className="space-y-3">
            {/* The kind and the subject side by side from `sm`; stacked at 390 px. */}
            <div className="grid gap-3 sm:grid-cols-[10rem_minmax(0,1fr)]">{subject}</div>
            <div className="flex flex-wrap items-center gap-2">{submit}</div>
          </div>
        )}
      />
    </>
  );
}
