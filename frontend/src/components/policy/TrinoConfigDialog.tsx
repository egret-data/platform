import { useQuery } from "@tanstack/react-query";
import { type TrinoConfig, getTrinoConfig } from "@/api/policies";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { downloadTextFile } from "@/lib/downloadTextFile";

interface Props {
  open: boolean;
  service: string;
  onOpenChange: (o: boolean) => void;
}

export function TrinoConfigDialog({ open, service, onOpenChange }: Props) {
  const cfg = useQuery<TrinoConfig>({
    queryKey: ["trino-config", service],
    queryFn: () => getTrinoConfig(service),
    enabled: open,
    retry: false,
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* The `max-h-[85vh] overflow-y-auto` that used to be here is now in the primitive,
          and better: scrolling the CONTENT took the close button with it, since the close
          button is absolutely positioned inside. The primitive scrolls an inner body and
          leaves the close button pinned. */}
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Trino config — {service}</DialogTitle>
          <DialogDescription>
            Drop these into the Trino coordinator&apos;s <code>etc/</code>. The native ranger plugin then enforces
            this profile. (Assumes anonymous <code>/service</code>; a Ranger auth token needs extra proxy setup.)
          </DialogDescription>
        </DialogHeader>
        {(cfg.data?.warnings ?? []).map((w) => (
          <Alert key={w}>
            <AlertDescription>{w}</AlertDescription>
          </Alert>
        ))}
        {(cfg.data?.files ?? []).map((file) => (
          <div key={file.name} className="space-y-1">
            <div className="flex items-center justify-between">
              <span className="font-mono text-sm">{file.name}</span>
              <Button size="sm" variant="outline" onClick={() => downloadTextFile(file.name, file.content)}>
                Download
              </Button>
            </div>
            <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">{file.content}</pre>
          </div>
        ))}
      </DialogContent>
    </Dialog>
  );
}
