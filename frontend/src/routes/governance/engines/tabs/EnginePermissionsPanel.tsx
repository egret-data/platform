import { useRef, useState } from "react";
import { Plus, SearchCheck, TriangleAlert } from "lucide-react";
import type { EngineGrantView } from "@/api/generated/model";
import { usePermListEngineCapabilities } from "@/api/generated/perm/perm";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import type { TabContext } from "@/routes/governance/engines/PageFrame";
import { CheckCapabilityDialog } from "@/routes/governance/engines/tabs/CheckCapabilityDialog";
import { EngineGrantDialog } from "@/routes/governance/engines/tabs/EngineGrantDialog";
import { EnginePermissionsTab } from "@/routes/governance/engines/tabs/EnginePermissionsTab";

/** Q4's "saved but not compiled" sentence. */
export function notCompiledSentence(reason: string): string {
  return `Saved, but not compiled: ${reason}. It is not on the engine.`;
}

/**
 * The Engine permissions tab with its writes (spec §6.1): Grant and Edit
 * (`policy:update`) open the grant dialog; Check (`policy:read`) asks
 * explain. A grant the compile skipped is reported here, on the tab (Q4).
 */
export function EnginePermissionsPanel({ ctx }: { ctx: TabContext }) {
  const can = usePolicyPermissions();
  const canWrite = can("policy:update");
  const capsQ = usePermListEngineCapabilities();
  const [dialog, setDialog] = useState<{ mode: "grant" } | { mode: "edit"; grant: EngineGrantView } | null>(null);
  const [checking, setChecking] = useState(false);
  const [skipped, setSkipped] = useState<string | null>(null);
  // The dialogs open from state, not from a Radix trigger, so Radix has no
  // element to return focus to on close: remember what held focus when one
  // opened (the Grant, Check or a row's Edit button) and focus it again.
  const opener = useRef<HTMLElement | null>(null);
  const rememberOpener = () => {
    const active = document.activeElement;
    opener.current = active instanceof HTMLElement && active !== document.body ? active : null;
  };
  const returnFocus = (event: Event) => {
    const element = opener.current?.isConnected ? opener.current : null;
    opener.current = null;
    if (element) {
      event.preventDefault();
      element.focus();
    }
  };

  const engine = ctx.engine;
  const grants = ctx.grants;
  const profile = grants?.profile?.name ?? null;
  const catalogue =
    capsQ.data?.status === 200 ? capsQ.data.data.engines.find((e) => e.engineType === engine?.type) : undefined;
  // The dialogs build their controls from the catalogue, so they open once it answered.
  const ready = Boolean(engine && grants && profile && catalogue);

  const toolbar = ready ? (
    <div className="flex flex-wrap items-center gap-2">
      {canWrite ? (
        <Button size="sm" onClick={() => {
            rememberOpener();
            setDialog({ mode: "grant" });
          }}>
          <Plus className="mr-1 h-4 w-4" aria-hidden />
          Grant
        </Button>
      ) : null}
      <Button size="sm" variant="outline" onClick={() => {
          rememberOpener();
          setChecking(true);
        }}>
        <SearchCheck className="mr-1 h-4 w-4" aria-hidden />
        Check
      </Button>
    </div>
  ) : null;

  return (
    <div className="space-y-4">
      {skipped ? (
        <Alert variant="destructive">
          <TriangleAlert className="h-4 w-4" aria-hidden />
          <AlertDescription>{notCompiledSentence(skipped)}</AlertDescription>
        </Alert>
      ) : null}
      <EnginePermissionsTab
        ctx={ctx}
        toolbar={toolbar}
        onEdit={
          canWrite && ready
            ? (grant) => {
                rememberOpener();
                setDialog({ mode: "edit", grant });
              }
            : undefined
        }
      />
      {ready && engine && grants && profile && catalogue ? (
        <>
          <EngineGrantDialog
            open={dialog !== null}
            onOpenChange={(open) => {
              if (!open) setDialog(null);
            }}
            engine={engine}
            profile={profile}
            reach={ctx.reach}
            capabilities={catalogue.capabilities}
            existing={dialog?.mode === "edit" ? dialog.grant : null}
            onSkipped={setSkipped}
            onCloseAutoFocus={returnFocus}
          />
          <CheckCapabilityDialog
            open={checking}
            onOpenChange={setChecking}
            engine={engine}
            reach={ctx.reach}
            grants={grants}
            capabilities={catalogue.capabilities}
            onCloseAutoFocus={returnFocus}
          />
        </>
      ) : null}
    </div>
  );
}
