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
 * Control Plane → Resources
 *
 * A structured, no-backend frame for the future engine-resource registry
 * (Trino / StarRocks / Spark Connect). It renders the eventual layout — type
 * tabs, the resource table header, an empty state per tab, and a stub
 * "Add resource" button — but makes no API calls. The real CRUD + assignment
 * + live monitoring arrive in a later epic.
 */

type ResourceType = {
  value: string;
  label: string;
};

const RESOURCE_TYPES: ResourceType[] = [
  { value: "trino", label: "Trino" },
  { value: "starrocks", label: "StarRocks" },
  { value: "spark-connect", label: "Spark Connect" },
];

const COLUMNS = ["Name", "Type", "Endpoint", "Status", "Workspaces", "Actions"];

function ResourceTable({ label }: { label: string }) {
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
            No {label} resources yet — connect one in a later phase.
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  );
}

export default function ControlPlaneResources() {
  return (
    <Card data-testid="resources-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Server className="h-5 w-5 text-muted-foreground" />
              Resources
            </CardTitle>
            <CardDescription>
              Query engines available to workspaces (Trino, StarRocks, Spark
              Connect). Provisioning &amp; assignment arrive in a later phase.
            </CardDescription>
          </div>
          <Button disabled title="Coming soon">
            <Plus className="h-4 w-4" />
            Add resource
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        <Tabs defaultValue={RESOURCE_TYPES[0].value}>
          <TabsList>
            {RESOURCE_TYPES.map((type) => (
              <TabsTrigger key={type.value} value={type.value}>
                {type.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {RESOURCE_TYPES.map((type) => (
            <TabsContent key={type.value} value={type.value}>
              <ResourceTable label={type.label} />
            </TabsContent>
          ))}
        </Tabs>
      </CardContent>
    </Card>
  );
}
