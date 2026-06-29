import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, Plus } from "lucide-react";

import {
  type Cell,
  type CellType,
  type Engine,
  useAddCell,
  useDeleteCell,
  useNotebook,
  useReorderCells,
  useUpdateCell,
  useUpdateNotebook,
} from "@/api/notebooks";
import { CellEditor } from "@/components/notebook/CellEditor";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

// ── Cell with autosave ────────────────────────────────────────────────────────

interface ManagedCellProps {
  cell: Cell;
  onChangeType: (cid: string, type: CellType) => void;
  onDelete: (cid: string) => void;
  onMoveUp: (cid: string) => void;
  onMoveDown: (cid: string) => void;
  updateCellMutate: (args: { cid: string; source?: string; cell_type?: CellType }) => void;
}

function ManagedCell({
  cell,
  onChangeType,
  onDelete,
  onMoveUp,
  onMoveDown,
  updateCellMutate,
}: ManagedCellProps) {
  const [source, setSource] = useState(cell.source);
  const timerRef = useRef<number | undefined>(undefined);

  // Sync source if the server data changes (e.g. after refetch)
  const prevCellIdRef = useRef(cell.id);
  useEffect(() => {
    if (prevCellIdRef.current !== cell.id) {
      setSource(cell.source);
      prevCellIdRef.current = cell.id;
    }
  }, [cell.id, cell.source]);

  const handleChangeSource = useCallback(
    (newSource: string) => {
      setSource(newSource);
      window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => {
        updateCellMutate({ cid: cell.id, source: newSource });
      }, 600);
    },
    [cell.id, updateCellMutate],
  );

  useEffect(() => {
    return () => window.clearTimeout(timerRef.current);
  }, []);

  return (
    <CellEditor
      cell={{ ...cell, source }}
      onChangeSource={handleChangeSource}
      onChangeType={(type) => onChangeType(cell.id, type)}
      onDelete={() => onDelete(cell.id)}
      onMoveUp={() => onMoveUp(cell.id)}
      onMoveDown={() => onMoveDown(cell.id)}
      runDisabled
    />
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function NotebookPage() {
  const { workspace: ws, id } = useParams<{ workspace: string; id: string }>();

  const notebook = useNotebook(ws!, id ?? null);
  const updateNotebook = useUpdateNotebook(ws!, id!);
  const addCell = useAddCell(ws!, id!);
  const updateCell = useUpdateCell(ws!, id!);
  const deleteCell = useDeleteCell(ws!, id!);
  const reorderCells = useReorderCells(ws!, id!);

  // Local name state for rename-on-blur
  const [localName, setLocalName] = useState("");
  useEffect(() => {
    if (notebook.data?.name) {
      setLocalName(notebook.data.name);
    }
  }, [notebook.data?.name]);

  const handleNameBlur = () => {
    if (!notebook.data) return;
    const trimmed = localName.trim();
    if (trimmed && trimmed !== notebook.data.name) {
      updateNotebook.mutate({ name: trimmed });
    }
  };

  const handleEngineChange = (engine: Engine) => {
    updateNotebook.mutate({ engine });
  };

  const handlePublicToggle = (checked: boolean) => {
    updateNotebook.mutate({ is_public: checked });
  };

  const handleAddCell = (cellType: CellType = "code") => {
    const cells = notebook.data?.cells ?? [];
    const maxOrder = cells.reduce((m, c) => Math.max(m, c.cell_order), -1);
    addCell.mutate({ cell_type: cellType, cell_order: maxOrder + 1 });
  };

  const handleDeleteCell = (cid: string) => {
    deleteCell.mutate(cid);
  };

  const handleMoveUp = (cid: string) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const idx = cells.findIndex((c) => c.id === cid);
    if (idx <= 0) return;
    const newOrder = cells.map((c) => c.id);
    [newOrder[idx - 1], newOrder[idx]] = [newOrder[idx], newOrder[idx - 1]];
    reorderCells.mutate(newOrder);
  };

  const handleMoveDown = (cid: string) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const idx = cells.findIndex((c) => c.id === cid);
    if (idx < 0 || idx >= cells.length - 1) return;
    const newOrder = cells.map((c) => c.id);
    [newOrder[idx], newOrder[idx + 1]] = [newOrder[idx + 1], newOrder[idx]];
    reorderCells.mutate(newOrder);
  };

  const handleChangeType = (cid: string, cellType: CellType) => {
    updateCell.mutate({ cid, cell_type: cellType });
  };

  // ── Loading / error states ────────────────────────────────────────────────

  if (notebook.isLoading) {
    return (
      <section className="flex flex-col gap-4 p-4">
        <div className="h-8 w-48 animate-pulse rounded bg-muted" />
        <ListSkeleton rows={3} cols={1} />
      </section>
    );
  }

  if (!notebook.data) {
    return (
      <section className="p-8 text-center text-muted-foreground">
        <p>Notebook not found.</p>
        <Link
          to={`/explore/${ws}/notebooks`}
          className="mt-2 inline-flex items-center gap-1 text-sm text-primary hover:underline"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Back to notebooks
        </Link>
      </section>
    );
  }

  const nb = notebook.data;
  const sortedCells = [...nb.cells].sort((a, b) => a.cell_order - b.cell_order);

  return (
    <section className="flex flex-col gap-4 p-4">
      {/* Header */}
      <div className="flex flex-col gap-3">
        <Link
          to={`/explore/${ws}/notebooks`}
          className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Notebooks
        </Link>

        <div className="flex flex-wrap items-center gap-4">
          {/* Rename on blur */}
          <Input
            value={localName}
            onChange={(e) => setLocalName(e.target.value)}
            onBlur={handleNameBlur}
            className="max-w-xs text-base font-semibold"
            aria-label="Notebook name"
          />

          {/* Engine select */}
          <div className="flex items-center gap-2">
            <Label htmlFor="nb-engine" className="text-sm">
              Engine
            </Label>
            <Select value={nb.engine} onValueChange={handleEngineChange}>
              <SelectTrigger id="nb-engine" className="w-36" aria-label="Engine">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="daft">Daft</SelectItem>
                <SelectItem value="spark_connect">Spark Connect</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {/* Public toggle */}
          <div className="flex items-center gap-2">
            <Switch
              id="nb-public"
              checked={nb.is_public}
              onCheckedChange={handlePublicToggle}
              aria-label="Public"
            />
            <Label htmlFor="nb-public" className="text-sm">
              Public
            </Label>
          </div>
        </div>
      </div>

      {/* Cells */}
      <div className="flex flex-col gap-3">
        {sortedCells.map((cell) => (
          <ManagedCell
            key={cell.id}
            cell={cell}
            onChangeType={handleChangeType}
            onDelete={handleDeleteCell}
            onMoveUp={handleMoveUp}
            onMoveDown={handleMoveDown}
            updateCellMutate={updateCell.mutate}
          />
        ))}
      </div>

      {/* Add cell buttons */}
      <div className="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => handleAddCell("code")}
          aria-label="Add cell"
        >
          <Plus className="h-4 w-4" />
          Add cell
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => handleAddCell("markdown")}
          aria-label="Add markdown cell"
        >
          <Plus className="h-4 w-4" />
          Add markdown
        </Button>
      </div>
    </section>
  );
}
