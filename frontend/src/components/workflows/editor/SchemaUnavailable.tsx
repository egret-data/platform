import { Button } from "@/components/ui/button";

/**
 * Spec §10: when the schema cannot be fetched the editor still works, without
 * completion, and says so with a retry. Server validation does not depend on
 * the schema and keeps running.
 */
export function SchemaUnavailable({ error, retry }: { error: string | null; retry: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-2 border-b border-border bg-muted/40 px-3 py-2 text-sm">
      <span className="font-medium text-foreground">Autocomplete unavailable.</span>
      <span className="text-muted-foreground">
        The flow schema could not be loaded{error ? ` (${error})` : ""}. You can keep editing; retry to get completion and documentation back.
      </span>
      <Button type="button" variant="outline" size="sm" onClick={retry}>
        Retry loading the schema
      </Button>
    </div>
  );
}
