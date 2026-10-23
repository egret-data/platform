import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { DashboardRevision } from "@/api/dashboards";
import { attributionLine } from "./attribution";
import { DashboardHistoryTab } from "./DashboardHistoryTab";

interface Props {
  name: string;
  description: string;
  /** Folder name, or null for Uncategorized. */
  folderName: string | null;
  /** Where "Saved in" points. There is no folder route in this app — this is the dashboards list
   *  scoped with ?folder=<id>, which the list uses to expand that section. */
  folderHref: string | null;
  /** Display name for the creator, already resolved by the API (`created_by_name`); falls back
   *  to the raw id, exactly as ShareDialog.tsx does for a grant's principal. */
  ownerLabel: string | null;
  createdAt: string | null;
  editorLabel: string | null;
  updatedAt: string;
  revisions: DashboardRevision[];
  revisionsLoading: boolean;
  onRevert: (revisionId: string) => void;
  reverting: boolean;
  /** Fires when the History tab is first selected, so the route can enable its query. */
  onOpenHistory: () => void;
  onSaveDescription: (next: string) => void;
  onClose: () => void;
  /** Certification (0108). `canCertify` reflects the PLATFORM-scoped `dashboard:certify`, which
   *  is deliberately not implied by being able to edit this dashboard — see rbac.rs. */
  certified: boolean;
  certifiedBy: string;
  certificationDetails: string;
  canCertify: boolean;
  certifying: boolean;
  onSetCertification: (certified: boolean, details: string) => void;
}

export function DashboardInfoSidebar({
  name, description, folderName, folderHref, ownerLabel, createdAt, editorLabel, updatedAt,
  revisions, revisionsLoading, onRevert, reverting, onOpenHistory,
  onSaveDescription, onClose,
  certified, certifiedBy, certificationDetails, canCertify, certifying, onSetCertification,
}: Props) {
  const [note, setNote] = useState("");
  const [draft, setDraft] = useState(description);
  // Re-sync if the description changes underneath us (e.g. a concurrent edit refetch);
  // [description] compares by value, so this only fires on a genuine external change.
  useEffect(() => { setDraft(description); }, [description]);
  const added = attributionLine(createdAt, ownerLabel);
  // `editorLabel ?? ownerLabel` is deliberate: updated_by is NULL for every dashboard that has
  // not been saved since migration 0106, and in that state the last person known to have touched
  // it is its creator. Metabase's last-edit-info falls back to the creator the same way.
  const edited = attributionLine(updatedAt, editorLabel ?? ownerLabel);
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[300px] shrink-0 self-start overflow-auto rounded-lg border bg-card p-3 text-sm">
      <div className="mb-2 flex items-center justify-between">
        <div className="font-medium">{name}</div>
        <button type="button" aria-label="close info" className="text-muted-foreground hover:text-foreground" onClick={onClose}>✕</button>
      </div>
      <Tabs defaultValue="overview" onValueChange={(v) => { if (v === "history") onOpenHistory(); }}>
        <TabsList variant="underline" className="mb-2">
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <label className="mb-3 flex flex-col gap-1 text-xs text-muted-foreground">Description
            <textarea aria-label="description" className="h-24 w-full rounded border bg-background px-2 py-1 text-sm"
              value={draft} onChange={(e) => setDraft(e.target.value)}
              onBlur={() => { if (draft !== description) onSaveDescription(draft); }} />
          </label>
          <dl className="flex flex-col gap-1 text-xs">
            <div className="flex justify-between gap-2">
              <dt className="shrink-0 text-muted-foreground">Saved in</dt>
              <dd className="truncate text-right">
                {folderName && folderHref
                  ? <Link className="hover:underline" to={folderHref}>{folderName}</Link>
                  : "Uncategorized"}
              </dd>
            </div>
            {added && (
              <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Added</dt><dd className="text-right">{added}</dd></div>
            )}
            {edited && (
              <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Last edited</dt><dd className="text-right">{edited}</dd></div>
            )}
            {certified && (
              <div className="flex justify-between gap-2">
                <dt className="shrink-0 text-muted-foreground">Certified</dt>
                <dd className="truncate text-right" title={certificationDetails || undefined}>
                  {certifiedBy || "—"}
                </dd>
              </div>
            )}
          </dl>
          {/* The control is withheld — not disabled — from anyone without `dashboard:certify`.
              A greyed-out button would advertise a capability the reader does not have and
              invite them to ask why; showing nothing states the same thing more quietly. */}
          {canCertify && (
            <div className="mt-3 flex flex-col gap-2 border-t pt-3">
              {!certified && (
                <input aria-label="certification note" className="h-8 w-full rounded border bg-background px-2 text-xs"
                  placeholder="Why is this trusted? (optional)"
                  value={note} onChange={(e) => setNote(e.target.value)} />
              )}
              <Button size="sm" variant={certified ? "outline" : "default"} disabled={certifying}
                onClick={() => onSetCertification(!certified, note)}>
                {certified ? "Withdraw certification" : "Certify this dashboard"}
              </Button>
            </div>
          )}
        </TabsContent>
        <TabsContent value="history">
          <DashboardHistoryTab revisions={revisions} isLoading={revisionsLoading}
            onRevert={onRevert} reverting={reverting} />
        </TabsContent>
      </Tabs>
    </aside>
  );
}
