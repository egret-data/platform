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
  const [open, setOpen] = useState(false);
  // PNG availability is read from the DOM when the menu opens (reflects the currently-rendered viz).
  const [hasSvg, setHasSvg] = useState(false);

  const handleOpenChange = (next: boolean) => {
    if (next) setHasSvg(!!containerRef.current?.querySelector("svg"));
    setOpen(next);
  };

  return (
    <DropdownMenu open={open} onOpenChange={handleOpenChange}>
      <DropdownMenuTrigger asChild>
        <Button
          size="sm"
          variant="ghost"
          aria-label="export chart"
          disabled={disabled}
          // Radix's Trigger opens via pointer events, which jsdom (fireEvent.click) doesn't emit.
          // Force-open on click so the menu is openable via plain click everywhere; Radix still
          // handles closing (outside-click / Escape / item-select).
          onClick={() => handleOpenChange(true)}
        >
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
                downloadPng(title, svg as SVGElement).catch((e) => {
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
