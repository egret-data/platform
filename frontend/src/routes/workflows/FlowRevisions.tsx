import { useId, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { History, Loader2, RotateCcw, RotateCw } from "lucide-react";
import { useWorkflowListFlowVersions } from "@/api/generated/workflow/workflow";
import type { FlowVersionDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import Editor from "@/components/monaco/MonacoEditor";
import { RevisionDiff, type DiffSide } from "@/components/workflows/RevisionDiff";
import { versionSourceOf, type Enablement } from "@/components/workflows/draftState";
import { useIsDark } from "@/hooks/useIsDark";
import { DisabledReasons, ReasonButton } from "@/routes/workflows/FlowEditPage";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** `?compare=` value naming the saved draft rather than a version. */
export const COMPARE_DRAFT = "draft";
/** The compare picker's "no comparison" item; never written to the URL. */
const COMPARE_NONE = "none";

const errorMessage = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
const parseVersion = (s: string | null): number | null => (s !== null && /^[1-9]\d*$/.test(s) ? Number(s) : null);

/**
 * The flow's Revisions tab (spec §3, §5.3): its versions, one version's YAML,
 * a diff of two versions or of a version against the saved draft, and
 * "Restore to draft".
 *
 * Kestra's `Revisions.vue` is the model for the interaction: a revision is
 * picked, a second one is picked to compare, the diff is Monaco's, the pair is
 * in the URL, and restoring asks first. The selected version (`?v=`) and the
 * comparison (`?compare=`, a version number or `draft`) are in the URL
 * (FE-PAGE-2), pushed so that Back walks them.
 *
 * **What "draft" means here.** The saved draft, `savedSource`: the text every
 * run, schedule, webhook and subflow call uses (the user's ruling,
 * 2026-10-10). The editor's unsaved edits are not part of a comparison; the
 * tab says so while there are some.
 *
 * Restore is the page's save path (`onRestore`), never a publish.
 */
export function FlowRevisions({
  ws,
  flowId,
  savedSource,
  dirty,
  stale,
  saving,
  manage,
  onRestore,
}: {
  ws: string;
  flowId: string;
  /** The saved draft's text, as this editor last loaded or saved it. */
  savedSource: string;
  /** The editor holds edits that are not saved. */
  dirty: boolean;
  /**
   * Someone else's save was read back: the server's saved draft is not
   * `savedSource`, so nothing here may be compared against `savedSource`.
   */
  stale: boolean;
  /** A save of the page is in flight; a restore would be dropped. */
  saving: boolean;
  manage: boolean;
  /** Save `source` as the draft and put it in the editor; resolves `true` when it was saved. */
  onRestore: (version: FlowVersionDto, source: string) => Promise<boolean>;
}) {
  const query = useWorkflowListFlowVersions(ws, flowId, { query: { queryKey: workflowsKeys.versions(flowId) } });
  const versions = query.data?.status === 200 ? query.data.data : undefined;

  // Error first (FE-STATE-1): a failed read is never shown as "no versions".
  if (query.isError) {
    return (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>The versions could not be loaded: {errorMessage(query.error, "the request failed")}. Try again.</span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  }
  if (!versions) return <ListSkeleton cols={4} rows={4} />;
  if (versions.length === 0) {
    return (
      <EmptyState
        variant="dashed"
        icon={History}
        title="No versions yet"
        description="A version is a snapshot of the saved draft to compare and restore. Create one with Create version on the Edit tab."
      />
    );
  }
  return (
    <RevisionsView
      flowId={flowId}
      versions={versions}
      savedSource={savedSource}
      dirty={dirty}
      stale={stale}
      saving={saving}
      manage={manage}
      onRestore={onRestore}
    />
  );
}

function RevisionsView({
  flowId,
  versions,
  savedSource,
  dirty,
  stale,
  saving,
  manage,
  onRestore,
}: {
  flowId: string;
  versions: FlowVersionDto[];
  savedSource: string;
  dirty: boolean;
  stale: boolean;
  saving: boolean;
  manage: boolean;
  onRestore: (version: FlowVersionDto, source: string) => Promise<boolean>;
}) {
  const [params, setParams] = useSearchParams();
  // Newest first, whatever order the server sent.
  const sorted = useMemo(() => [...versions].sort((a, b) => b.version - a.version), [versions]);
  const latest = sorted[0];
  const byNumber = (n: number | null) => (n === null ? undefined : sorted.find((v) => v.version === n));

  const vParam = params.get("v");
  const asked = byNumber(parseVersion(vParam));
  // No `?v=` selects the latest version; one that names no version says so and shows the latest.
  const selected = asked ?? latest;
  const missingV = vParam !== null && !asked ? vParam : null;

  const compareParam = params.get("compare");
  const compareVersion = compareParam === COMPARE_DRAFT ? undefined : byNumber(parseVersion(compareParam));
  const compareTarget: "draft" | FlowVersionDto | null =
    compareParam === COMPARE_DRAFT ? "draft" : compareVersion && compareVersion.version !== selected.version ? compareVersion : null;
  const missingCompare = compareParam !== null && compareTarget === null ? compareParam : null;

  /** The search string that selects version `n`, keeping a comparison that still makes sense. */
  const selectHref = (n: number) => {
    const next = new URLSearchParams(params);
    next.set("v", String(n));
    if (next.get("compare") === String(n)) next.delete("compare");
    return `?${next.toString()}`;
  };
  const setCompare = (value: string) => {
    const next = new URLSearchParams(params);
    next.set("v", String(selected.version));
    if (value === COMPARE_NONE) next.delete("compare");
    else next.set("compare", value);
    setParams(next);
  };

  // A version whose text equals the saved draft: content, never a timestamp (FE-STATE-4).
  // While stale, `savedSource` is not the server's draft: nothing is said about the draft.
  const matchesDraft = (v: FlowVersionDto) => !stale && versionSourceOf(v) === savedSource;
  const selectedSource = versionSourceOf(selected);
  const compareId = useId();
  const reasonsId = useId();

  const [confirming, setConfirming] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const confirmRestore = async () => {
    // The confirm closes before the save starts: a refused save opens the stale-draft dialog, and
    // two alert dialogs opening and closing in one commit leave the second hidden from assistive tech.
    setConfirming(false);
    setRestoring(true);
    try {
      await onRestore(selected, selectedSource!);
    } finally {
      setRestoring(false);
    }
  };

  let restore: Enablement = { ok: true };
  if (restoring) restore = { ok: false, reason: "Restoring…" };
  else if (!manage) restore = { ok: false, reason: "Restoring needs the workflow:manage permission" };
  else if (saving) restore = { ok: false, reason: "Wait for the save to finish" };
  else if (stale) restore = { ok: false, reason: "The flow changed on the server; reload to restore" };
  else if (selectedSource === undefined) restore = { ok: false, reason: "This version's text is unavailable" };
  else if (selectedSource === savedSource) restore = { ok: false, reason: "The saved draft already has this text" };

  let viewer;
  if (selectedSource === undefined) {
    viewer = (
      <Alert>
        <AlertDescription>v{selected.version}'s text is unavailable: its snapshot carries no source.</AlertDescription>
      </Alert>
    );
  } else if (compareTarget === "draft" && stale) {
    viewer = (
      <Alert>
        <AlertDescription>The current draft on the server is newer than this page's; reload the page to compare with it.</AlertDescription>
      </Alert>
    );
  } else if (compareTarget !== null) {
    const other: DiffSide | undefined =
      compareTarget === "draft"
        ? { label: "Current draft", source: savedSource }
        : versionSourceOf(compareTarget) !== undefined
          ? { label: `v${compareTarget.version}`, source: versionSourceOf(compareTarget)! }
          : undefined;
    const mine: DiffSide = { label: `v${selected.version}`, source: selectedSource };
    // The older text on the left; the draft is newer than every version.
    const [left, right] =
      other === undefined
        ? [mine, mine]
        : compareTarget === "draft" || (compareTarget as FlowVersionDto).version > selected.version
          ? [mine, other]
          : [other, mine];
    viewer =
      other === undefined ? (
        <Alert>
          <AlertDescription>v{(compareTarget as FlowVersionDto).version}'s text is unavailable: its snapshot carries no source.</AlertDescription>
        </Alert>
      ) : (
        <RevisionDiff left={left} right={right} />
      );
  } else {
    viewer = <VersionSource key={selected.version} flowId={flowId} version={selected.version} source={selectedSource} />;
  }

  // The list sits beside the pane from `xl`: from `lg`, a 1024px screen left the diff 222px wide (W7a Task 8).
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 xl:flex-row">
      <section aria-label="Versions" className="min-h-0 shrink-0 overflow-auto xl:w-[30rem]">
        <p className="mb-2 text-sm text-muted-foreground">
          Versions are snapshots of the draft. Runs use the saved draft, never a version.
        </p>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Version</TableHead>
              <TableHead>Note</TableHead>
              <TableHead>Author</TableHead>
              <TableHead>Created</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {sorted.map((v) => {
              const isSelected = v.version === selected.version;
              return (
                <TableRow key={v.id} data-state={isSelected ? "selected" : undefined} className={isSelected ? "bg-muted" : undefined}>
                  <TableCell className="whitespace-nowrap">
                    <Link
                      to={{ search: selectHref(v.version) }}
                      aria-current={isSelected ? "true" : undefined}
                      className="font-medium tabular-nums hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      v{v.version}
                    </Link>
                    <span className="ml-2 inline-flex gap-1">
                      {v.version === latest.version && <Badge variant="secondary">Latest</Badge>}
                      {matchesDraft(v) && <Badge variant="outline">Matches draft</Badge>}
                    </span>
                  </TableCell>
                  <TableCell className="max-w-48 truncate" title={v.note || undefined}>
                    {v.note || <span className="text-muted-foreground">—</span>}
                  </TableCell>
                  <TableCell className="whitespace-nowrap">{v.created_by_name ?? <span className="text-muted-foreground">—</span>}</TableCell>
                  <TableCell className="whitespace-nowrap tabular-nums">{new Date(v.created_at).toLocaleString()}</TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </section>

      <section aria-labelledby={`${reasonsId}-title`} className="flex min-h-[28rem] min-w-0 flex-1 flex-col gap-3">
        {stale && (
          <Alert data-testid="revisions-stale">
            <AlertDescription>
              The flow changed on the server; reload to compare with its draft or restore a version. Until then this tab marks no
              version as matching the draft.
            </AlertDescription>
          </Alert>
        )}
        {missingV && (
          <Alert>
            <AlertDescription>There is no version {missingV}; showing v{selected.version}, the latest.</AlertDescription>
          </Alert>
        )}
        {missingCompare && (
          <Alert>
            <AlertDescription>
              Cannot compare with {missingCompare === String(selected.version) ? "the same version" : `version ${missingCompare}`}; showing v
              {selected.version} alone.
            </AlertDescription>
          </Alert>
        )}
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div className="min-w-0">
            <h2 id={`${reasonsId}-title`} className="text-lg font-semibold tabular-nums">
              v{selected.version}
            </h2>
            <p className="text-sm text-muted-foreground">
              {selected.created_by_name ?? "—"} · <span className="tabular-nums">{new Date(selected.created_at).toLocaleString()}</span>
              {selected.note ? ` · ${selected.note}` : ""}
            </p>
          </div>
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor={compareId}>Compare with</Label>
              <Select
                value={compareTarget === null ? COMPARE_NONE : compareTarget === "draft" ? COMPARE_DRAFT : String(compareTarget.version)}
                onValueChange={setCompare}
              >
                <SelectTrigger id={compareId} className="w-48">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={COMPARE_NONE}>Nothing</SelectItem>
                  <SelectItem value={COMPARE_DRAFT}>Current draft</SelectItem>
                  {sorted
                    .filter((v) => v.version !== selected.version)
                    .map((v) => (
                      <SelectItem key={v.id} value={String(v.version)}>
                        v{v.version}
                      </SelectItem>
                    ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col items-end gap-1">
              <ReasonButton enablement={restore} reasonId={`${reasonsId}-restore`} variant="outline" onClick={() => setConfirming(true)}>
                {restoring ? <Loader2 aria-hidden className="animate-spin" /> : <RotateCcw aria-hidden />}
                Restore to draft
              </ReasonButton>
            </div>
          </div>
        </div>
        <DisabledReasons actions={[{ id: `${reasonsId}-restore`, label: "Restore to draft", enablement: restore }]} />
        {compareTarget === "draft" && dirty && (
          <p className="text-sm text-muted-foreground">
            The current draft is the saved one. Your unsaved edits in the editor are not part of this comparison.
          </p>
        )}
        <div className="min-h-0 flex-1">{viewer}</div>
      </section>

      {selectedSource !== undefined && (
        <RestoreDialog
          open={confirming}
          onOpenChange={setConfirming}
          version={selected.version}
          latestVersion={latest.version}
          // What restoring loses: the saved draft, when no version holds its text.
          draftLost={!stale && !sorted.some(matchesDraft)}
          dirty={dirty}
          onConfirm={() => void confirmRestore()}
        />
      )}
    </div>
  );
}

/** One version's YAML, read-only, in Monaco (outside the flow model glob, so no schema markers). */
function VersionSource({ flowId, version, source }: { flowId: string; version: number; source: string }) {
  const isDark = useIsDark();
  return (
    <div className="h-full min-h-[24rem] rounded border border-border">
      <Editor
        height="100%"
        path={`inmemory://egret/revision/${encodeURIComponent(flowId)}/v${version}.yaml`}
        defaultLanguage="yaml"
        defaultValue={source}
        theme={isDark ? "vs-dark" : "vs"}
        options={{
          readOnly: true,
          domReadOnly: true,
          minimap: { enabled: false },
          fontSize: 13,
          scrollBeyondLastLine: false,
          automaticLayout: true,
          ariaLabel: `v${version} source`,
        }}
      />
    </div>
  );
}

/**
 * The confirm before "Restore to draft" (FE-ACTION-1). It always asks,
 * because restoring changes what runs, and it names what is lost: the saved
 * draft when no version holds its text, and the editor's unsaved edits.
 */
function RestoreDialog({
  open,
  onOpenChange,
  version,
  latestVersion,
  draftLost,
  dirty,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  version: number;
  latestVersion: number;
  draftLost: boolean;
  dirty: boolean;
  /** Closes the dialog and restores; the outcome is said by a toast or the stale-draft dialog. */
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Restore v{version} to the draft?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="space-y-2">
              <p>
                v{version}'s text replaces the saved draft, and every run uses the saved draft: the next run, schedule, webhook or subflow
                call runs v{version}. No version is created or changed.
              </p>
              {draftLost && (
                <p data-testid="restore-loses-draft">
                  The saved draft has changes since v{latestVersion} that no version keeps; restoring discards them. To keep them, cancel and
                  use Create version first.
                </p>
              )}
              {dirty && <p data-testid="restore-loses-edits">Your unsaved edits in the editor are discarded too.</p>}
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button variant={draftLost || dirty ? "destructive" : "default"} onClick={onConfirm}>
            Restore v{version}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
