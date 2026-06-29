import { Plus, Server } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

/**
 * Control Plane → Engines
 *
 * A structured, no-backend frame for the future platform engine registry
 * (Trino / StarRocks / Spark Connect). It renders the eventual layout — type
 * tabs, the engine table header (incl. live Status/health, folded in here),
 * an empty state per tab, and a stub "Add engine" button — but makes no API
 * calls. Real provisioning + per-workspace assignment + live monitoring arrive
 * in a later epic.
 */

type EngineType = {
  value: string;
  label: string;
};

const ENGINE_TYPES: EngineType[] = [
  { value: "trino", label: "Trino" },
  { value: "starrocks", label: "StarRocks" },
  { value: "spark-connect", label: "Spark Connect" },
];

const COLUMNS = ["Name", "Type", "Endpoint", "Status", "Workspaces", "Actions"];

function EngineTable({ label }: { label: string }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          {COLUMNS.map((col) => (
            <TableHead
              key={col}
              className={col === "Actions" ? "text-right" : undefined}
            >
              {col}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow className="hover:bg-transparent">
          <TableCell
            colSpan={COLUMNS.length}
            className="py-10 text-center text-muted-foreground"
          >
            No {label} engines yet — provision one in a later phase.
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  );
}

export default function ControlPlaneEngines() {
  return (
    <Card data-testid="engines-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Server className="h-5 w-5 text-muted-foreground" />
              Engines
            </CardTitle>
            <CardDescription>
              Platform query engines (Trino, StarRocks, Spark Connect). Provision
              new engines and assign them to workspaces — arrives in a later
              phase.
            </CardDescription>
          </div>
          <Button disabled title="Coming soon">
            <Plus className="h-4 w-4" />
            Add engine
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        <Tabs defaultValue={ENGINE_TYPES[0].value}>
          <TabsList>
            {ENGINE_TYPES.map((type) => (
              <TabsTrigger key={type.value} value={type.value}>
                {type.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {ENGINE_TYPES.map((type) => (
            <TabsContent key={type.value} value={type.value}>
              <EngineTable label={type.label} />
            </TabsContent>
          ))}
        </Tabs>
      </CardContent>
    </Card>
  );
}
