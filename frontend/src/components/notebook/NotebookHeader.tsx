import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowLeft, Eraser, Play, RotateCcw, Square } from "lucide-react";
import type { KernelStatus } from "@/lib/kernel/KernelConnection";
import { KernelStatusBadge } from "./KernelStatusBadge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

export interface NotebookHeaderProps {
  ws: string;
  name: string;
  isPublic: boolean;
  status: KernelStatus;
  runningAll: boolean;
  onRename: (name: string) => void;
  onTogglePublic: (v: boolean) => void;
  onRunAll: () => void;
  onInterrupt: () => void;
  onRestart: () => void;
  onClear: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

export function NotebookHeader(props: NotebookHeaderProps) {
  const [localName, setLocalName] = useState(props.name);
  useEffect(() => setLocalName(props.name), [props.name]);

  useEffect(() => {
    const dirty = localName.trim() !== "" && localName.trim() !== props.name;
    props.onDirtyChange?.(dirty);
  }, [localName, props.name]);

  const commitName = () => {
    const trimmed = localName.trim();
    if (trimmed && trimmed !== props.name) props.onRename(trimmed);
  };

  return (
    <div className="sticky top-0 z-10 flex flex-col gap-3 border-b bg-background/95 px-4 py-3 backdrop-blur">
      <Link
        to={`/explore/${props.ws}/notebooks`}
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-3.5 w-3.5" />
        Notebooks
      </Link>

      <div className="flex flex-wrap items-center gap-4">
        <Input
          value={localName}
          onChange={(e) => setLocalName(e.target.value)}
          onBlur={commitName}
          className="max-w-xs text-base font-semibold"
          aria-label="Notebook name"
        />

        <div className="flex items-center gap-2">
          <Switch
            id="nb-public"
            checked={props.isPublic}
            onCheckedChange={props.onTogglePublic}
            aria-label="Public"
          />
          <Label htmlFor="nb-public" className="text-sm">
            Public
          </Label>
        </div>

        <KernelStatusBadge status={props.status} />

        <div className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="outline" onClick={props.onRunAll} disabled={props.runningAll}>
            <Play className="h-3.5 w-3.5" />
            Run all
          </Button>
          <Button size="sm" variant="outline" onClick={props.onInterrupt}>
            <Square className="h-3.5 w-3.5" />
            Interrupt
          </Button>
          <Button size="sm" variant="outline" onClick={props.onRestart}>
            <RotateCcw className="h-3.5 w-3.5" />
            Restart
          </Button>
          <Button size="sm" variant="ghost" onClick={props.onClear}>
            <Eraser className="h-3.5 w-3.5" />
            Clear outputs
          </Button>
        </div>
      </div>
    </div>
  );
}
