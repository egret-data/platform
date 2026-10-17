import { useState } from "react";
import { Loader2, Search } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { ExplainResponse, Scope } from "@/api/generated/model";
import { usePermExplain } from "@/api/generated/perm/perm";
import { ExplainResultView, SubjectUserField } from "@/components/perm/ExplainResult";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { fullPath } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/**
 * What the dialog says it answers. Also this module's bundle marker
 * (`scripts/assert-governance-data-lazy.mjs`).
 */
export const WHO_CAN_READ_INTRO = "Whether one user can read this, per policy profile, as each engine would decide it.";

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
 * "Who can read?" (spec §2.2): `POST /perm/explain` for one user, action
 * `read`, on exactly this object (and column), answered per policy profile
 * with Explain access's own result rendering (`ExplainResultView`). F10: this
 * is the only place the Data area says whether someone can read something; it
 * is explain's answer, never one worked out here.
 *
 * The subject is a user and nothing else, because that is all explain takes:
 * groups and roles left `ExplainSubject` (see `ExplainAccess`), so the field
 * is Explain access's user search (`SubjectUserField`), not `PrincipalPicker`,
 * which has no single-subject mode. Explain needs `policy:read`.
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
      <DialogContent className="sm:max-w-3xl">
        {open ? <WhoCanReadForm path={path} catalogId={catalogId} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function WhoCanReadForm({ path, catalogId }: { path: ObjectPath; catalogId: string }) {
  const [user, setUser] = useState("");
  const [result, setResult] = useState<ExplainResponse | null>(null);
  const explain = usePermExplain<ApiError>({
    mutation: {
      onSuccess: (res) => setResult(res.status === 200 ? res.data : null),
      onError: () => setResult(null),
    },
  });
  const ready = user.trim().length > 0;

  return (
    <>
      <DialogHeader>
        <DialogTitle>Who can read?</DialogTitle>
        <DialogDescription>
          {WHO_CAN_READ_INTRO} <span className="font-mono text-xs break-all">{fullPath(path)}</span>
        </DialogDescription>
      </DialogHeader>
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ready) return;
          explain.mutate({ data: { subject: { user: user.trim() }, action: "read", target: targetOf(path, catalogId) } });
        }}
      >
        <div className="min-w-0 flex-1 basis-56">
          <SubjectUserField
            value={user}
            onChange={(next) => {
              setUser(next);
              // An answer is about the user it was asked for.
              setResult(null);
            }}
          />
        </div>
        <Button type="submit" disabled={!ready || explain.isPending} aria-describedby={ready ? undefined : "who-can-read-needs"}>
          {explain.isPending ? <Loader2 aria-hidden className="animate-spin" /> : <Search aria-hidden />}
          Explain
        </Button>
        {ready ? null : (
          <p id="who-can-read-needs" className="w-full text-xs text-muted-foreground">
            Explain needs a user.
          </p>
        )}
      </form>
      {explain.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {explain.error.status === 403
              ? "Explaining access needs the policy:read permission."
              : explain.error.message || "Access could not be explained. Try again."}
          </AlertDescription>
        </Alert>
      ) : null}
      {result ? <ExplainResultView result={result} /> : null}
    </>
  );
}
