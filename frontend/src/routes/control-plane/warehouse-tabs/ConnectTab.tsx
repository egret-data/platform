import { useQuery } from "@tanstack/react-query";
import { Cable } from "lucide-react";

import { type Warehouse, listStorage } from "@/api/controlPlane";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CopyButton } from "./shared";

export function ConnectTab({ warehouse }: { warehouse: Warehouse | undefined }) {
  const storage = useQuery({
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const sc = storage.data?.find((s) => s.id === warehouse?.storageConnId);
  const server = window.location.origin;
  const s3 = sc?.endpoint ?? "http://localhost:9000";
  // Iceberg REST clients append `/v1/...` themselves, so the catalog base URI is
  // `/catalog` (egret serves the endpoints under `/catalog/v1`).
  const catalogUri = `${server}/catalog`;
  const tokenUri = `${server}/oauth/token`;

  if (!warehouse?.lakekeeperWarehouseId) {
    return (
      <Card>
        <CardContent className="py-10 text-center text-sm text-muted-foreground">
          This warehouse isn&apos;t provisioned yet — no catalog endpoint to connect to.
        </CardContent>
      </Card>
    );
  }
  // Clients pass the warehouse NAME (the token is scoped to it); egret resolves it
  // to the lakekeeper prefix internally.
  const wh = warehouse.name;

  const pyiceberg = `from pyiceberg.catalog.rest import RestCatalog

catalog = RestCatalog(
    "egret",
    uri="${catalogUri}",
    warehouse="${wh}",
    credential="<CLIENT_ID>:<CLIENT_SECRET>",
    **{
        "oauth2-server-uri": "${tokenUri}",
        # ask egret to vend short-lived, table-scoped S3 creds:
        "header.X-Iceberg-Access-Delegation": "vended-credentials",
        "s3.endpoint": "${s3}",
    },
)

print(catalog.list_namespaces())
table = catalog.load_table("demo_ns.events")
print(table.scan().to_arrow())`;

  const trino = `# etc/catalog/egret.properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=${catalogUri}
iceberg.rest-catalog.warehouse=${wh}
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<CLIENT_ID>:<CLIENT_SECRET>
iceberg.rest-catalog.oauth2.server-uri=${tokenUri}
# use egret's vended (short-lived, table-scoped) credentials:
iceberg.rest-catalog.vended-credentials-enabled=true
fs.native-s3.enabled=true
s3.endpoint=${s3}
s3.region=us-east-1
s3.path-style-access=true

-- then, in the Trino CLI:
SELECT * FROM egret.demo_ns.events;`;

  const starrocks = `-- StarRocks 4.0+
CREATE EXTERNAL CATALOG egret
PROPERTIES (
    "type" = "iceberg",
    "iceberg.catalog.type" = "rest",
    "iceberg.catalog.uri" = "${catalogUri}",
    "iceberg.catalog.warehouse" = "${wh}",
    "iceberg.catalog.security" = "OAUTH2",
    "iceberg.catalog.oauth2-server-uri" = "${tokenUri}",
    "iceberg.catalog.credential" = "<CLIENT_ID>:<CLIENT_SECRET>",
    "iceberg.catalog.vended-credentials-enabled" = "true",
    "aws.s3.endpoint" = "${s3}",
    "aws.s3.region" = "us-east-1",
    "aws.s3.enable_path_style_access" = "true"
);

SET CATALOG egret;
SELECT * FROM demo_ns.events;`;

  const engines = [
    { key: "pyiceberg", label: "PyIceberg", code: pyiceberg },
    { key: "trino", label: "Trino", code: trino },
    { key: "starrocks", label: "StarRocks", code: starrocks },
  ];

  return (
    <Card>
      <CardHeader>
        <CardTitle aria-level={2} className="flex items-center gap-2 text-lg">
          <Cable className="h-5 w-5 text-muted-foreground" />
          Connect an engine
        </CardTitle>
        <CardDescription>
          egret exposes this warehouse as an Iceberg REST catalog. Point any Iceberg-compatible
          engine at it using a Catalog Client&apos;s credentials.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        <div className="grid gap-3 sm:grid-cols-3">
          <Coord label="Catalog URI" value={catalogUri} />
          <Coord label="Warehouse" value={wh} />
          <Coord label="OAuth token URL" value={tokenUri} />
        </div>

        <Alert>
          <AlertDescription>
            The <code className="font-mono text-xs">credential</code> is{" "}
            <code className="font-mono text-xs">client_id:client_secret</code> from a{" "}
            <strong>Catalog Client</strong> (create one in the <em>Catalog Clients</em> tab); its
            role grants decide which namespaces/tables it may read or write. On a{" "}
            <code className="font-mono text-xs">vended-credentials</code> load egret hands back{" "}
            <strong>short-lived S3 credentials scoped to just that table</strong> — the configs below
            opt into them, so no static object-store keys are needed.
          </AlertDescription>
        </Alert>

        <Tabs defaultValue="pyiceberg">
          <TabsList variant="underline">
            {engines.map((e) => (
              <TabsTrigger key={e.key} value={e.key}>
                {e.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {engines.map((e) => (
            <TabsContent key={e.key} value={e.key} className="mt-4 space-y-2">
              <CodeBlock code={e.code} />
              <p className="text-xs text-muted-foreground">
                Replace the <code className="font-mono">&lt;…&gt;</code> placeholders with a Catalog
                Client&apos;s id/secret. If the engine runs in Docker, use the egret host reachable
                from that container instead of <code className="font-mono">{server}</code>.
              </p>
            </TabsContent>
          ))}
        </Tabs>
      </CardContent>
    </Card>
  );
}

function Coord({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border bg-muted/20 p-3">
      <div className="text-xs uppercase tracking-wide text-muted-foreground">{label}</div>
      <div className="mt-1 flex items-center gap-1.5">
        <code className="min-w-0 flex-1 truncate font-mono text-xs">{value}</code>
        <CopyButton value={value} />
      </div>
    </div>
  );
}

function CodeBlock({ code }: { code: string }) {
  return (
    <div className="relative">
      <div className="absolute right-2 top-2 z-10">
        <CopyButton value={code} />
      </div>
      <pre className="overflow-x-auto rounded-lg border bg-muted/40 p-4 pr-10 text-xs leading-relaxed">
        <code className="font-mono">{code}</code>
      </pre>
    </div>
  );
}

