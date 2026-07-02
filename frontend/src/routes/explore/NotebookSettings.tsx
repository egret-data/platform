import { useState, useEffect } from "react";
import { useParams } from "react-router-dom";
import { Loader2, BookOpen } from "lucide-react";
import { toast } from "sonner";

import {
  type EngineOption,
  useNotebookSettings,
  useSaveNotebookSettings,
} from "@/api/notebookSettings";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

export default function NotebookSettings() {
  const ws = useParams().workspace!;
  const { data, isLoading, isError, error } = useNotebookSettings(ws);
  const saveMutation = useSaveNotebookSettings(ws);

  const [engineIds, setEngineIds] = useState<Set<string>>(new Set());
  const [saveError, setSaveError] = useState<string | null>(null);

  // Seed checked state from server data whenever it loads/changes.
  useEffect(() => {
    if (data) {
      setEngineIds(new Set(data.engines.filter((e) => e.enabled).map((e) => e.id)));
    }
  }, [data]);

  function toggleEngine(id: string) {
    setEngineIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function handleSave() {
    setSaveError(null);
    try {
      await saveMutation.mutateAsync({
        engine_ids: Array.from(engineIds),
      });
      toast.success("Notebook settings saved.");
    } catch (err) {
      const apiErr = err as ApiError;
      if (apiErr?.status === 403) {
        setSaveError("Requires workspace-admin to change notebook settings.");
      } else {
        setSaveError(apiErr?.message ?? "Failed to save notebook settings.");
      }
    }
  }

  if (isLoading) {
    return (
      <section className="p-4">
        <PageHeader />
        <div className="mt-6">
          <ListSkeleton rows={3} cols={2} />
        </div>
      </section>
    );
  }

  if (isError) {
    return (
      <section className="p-4">
        <PageHeader />
        <Alert variant="destructive" className="mt-4">
          <AlertTitle>Failed to load notebook settings</AlertTitle>
          <AlertDescription>
            {(error as ApiError | null)?.message ?? "Something went wrong. Please try again."}
          </AlertDescription>
        </Alert>
      </section>
    );
  }

  const engines = data?.engines ?? [];

  return (
    <section className="p-4">
      <PageHeader />

      {/* Engines section */}
      <div className="mt-6">
        <h2 className="text-sm font-semibold">Engines</h2>
        <p className="mb-3 text-sm text-muted-foreground">
          Query engines notebooks in this workspace can use.
        </p>
        {engines.length === 0 ? (
          <EmptyState message="No engines configured for this workspace yet." />
        ) : (
          <OptionList>
            {engines.map((engine) => (
              <EngineRow
                key={engine.id}
                engine={engine}
                checked={engineIds.has(engine.id)}
                onToggle={() => toggleEngine(engine.id)}
              />
            ))}
          </OptionList>
        )}
      </div>

      {saveError && (
        <Alert variant="destructive" className="mt-4">
          <BookOpen className="h-4 w-4" />
          <AlertTitle>Could not save</AlertTitle>
          <AlertDescription>{saveError}</AlertDescription>
        </Alert>
      )}

      <div className="mt-6">
        <Button onClick={handleSave} disabled={saveMutation.isPending}>
          {saveMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
          Save
        </Button>
      </div>
    </section>
  );
}

function PageHeader() {
  return (
    <div>
      <h1 className="text-lg font-semibold">Notebook settings</h1>
      <p className="text-sm text-muted-foreground">
        Choose which engines notebooks in this workspace can use.
      </p>
    </div>
  );
}

function OptionList({ children }: { children: React.ReactNode }) {
  return (
    <div className="divide-y divide-border rounded border border-border">
      {children}
    </div>
  );
}

function EngineRow({
  engine,
  checked,
  onToggle,
}: {
  engine: EngineOption;
  checked: boolean;
  onToggle: () => void;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-3 px-4 py-3 hover:bg-accent">
      <Checkbox
        checked={checked}
        onCheckedChange={onToggle}
        aria-label={`Enable engine ${engine.name}`}
      />
      <span className="flex-1 text-sm font-medium">{engine.name}</span>
      <Badge variant="secondary">{engine.engine}</Badge>
    </label>
  );
}

function EmptyState({ message }: { message: string }) {
  return (
    <div className="rounded border border-dashed p-6 text-center text-sm text-muted-foreground">
      {message}
    </div>
  );
}
