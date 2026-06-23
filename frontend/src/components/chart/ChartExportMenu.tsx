import { useState } from "react";
import { Download } from "lucide-react";
import { toast } from "sonner";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import type { ResultColumn } from "@/components/chart/ChartTable";
import { downloadCsv, downloadPng } from "./export-chart";

interface Props {
  title: string | undefined;
  columns: ResultColumn[];
  rows: unknown[][];
  containerRef: React.RefObject<HTMLElement>;
  disabled?: boolean;
}

export function ChartExportMenu({ title, columns, rows, containerRef, disabled }: Props) {
  // PNG availability is read from the DOM when the menu opens (reflects the currently-rendered viz).
  const [hasSvg, setHasSvg] = useState(false);

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (open) setHasSvg(!!containerRef.current?.querySelector("svg"));
      }}
    >
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="ghost" aria-label="Export chart" disabled={disabled}>
          <Download className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => downloadCsv(title, columns, rows)}>
          Download CSV
        </DropdownMenuItem>
        {hasSvg && (
          <DropdownMenuItem
            onSelect={() => {
              const svg = containerRef.current?.querySelector("svg");
              if (svg)
                downloadPng(title, svg).catch((e) => {
                  console.error(e);
                  toast.error("Failed to export PNG");
                });
            }}
          >
            Download PNG
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
