import { useQuery } from "@tanstack/react-query";

import { listSecrets, listStorage } from "@/api/controlPlane";
import { CATALOG_KIND_LABELS } from "@/lib/catalogKinds";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/**
 * shadcn's `Select` cannot render a `SelectItem` whose value is `""` (Radix
 * uses the empty string internally to mean "nothing selected"), so an empty
 * list still needs a non-empty value for its disabled placeholder row.
 * (`CatalogForm.tsx` documented the same constraint for its own default-client
 * sentinel; SP3c-1 Task 9 deleted that file with the inline catalog editor.)
 */
export const NONE = "__none__";

/**
 * The kind whose catalog egret creates together with its warehouse.
 *
 * It is NOT a `registry.catalogs.kind`: it is `iceberg` with an `egret`
 * metastore, and `create_registry_catalog` refuses to create one
 * ("an egret warehouse catalog is created with its warehouse",
 * `catalogs.rs:318-322`). The create page offers it as a kind anyway, because
 * to an operator it is one of the things they can make here — and routes that
 * one branch to the warehouse endpoint instead.
 */
export const EGRET_WAREHOUSE_KIND = "egret_warehouse";

/**
 * The kinds the create page offers, egret's own warehouse first.
 *
 * Everything below `egret_warehouse` is a real `registry.catalogs.kind`; the
 * set is checked against the generated client's own list of kinds by
 * `CatalogCreate.test.tsx`, so a kind added (or renamed) in
 * `CatalogKind::parse` and carried into the OpenAPI description cannot leave
 * this form silently behind. There is no machine-readable enum to derive from:
 * `CreateRegistryCatalogBody.kind` is a plain `string` whose permitted values
 * live only in its description.
 */
export const KIND_OPTIONS: { kind: string; label: string; blurb: string }[] = [
  {
    kind: EGRET_WAREHOUSE_KIND,
    label: "egret warehouse",
    blurb:
      "An Iceberg warehouse egret manages itself, over a storage connection. Creates the warehouse as well as the catalog.",
  },
  {
    kind: "iceberg",
    label: CATALOG_KIND_LABELS.iceberg,
    blurb: "An existing Iceberg catalog — REST, Hive Metastore or AWS Glue.",
  },
  { kind: "hive", label: CATALOG_KIND_LABELS.hive, blurb: "Hive tables through a Hive Metastore or Glue." },
  {
    kind: "delta_lake",
    label: CATALOG_KIND_LABELS.delta_lake,
    blurb: "Delta tables through a Hive Metastore or Glue.",
  },
  { kind: "hudi", label: CATALOG_KIND_LABELS.hudi, blurb: "Hudi tables through a Hive Metastore." },
  {
    kind: "jdbc",
    label: CATALOG_KIND_LABELS.jdbc,
    blurb:
      "A relational database: PostgreSQL, MySQL, MariaDB, Oracle, SQL Server or ClickHouse.",
  },
  { kind: "elasticsearch", label: CATALOG_KIND_LABELS.elasticsearch, blurb: "Elasticsearch indices as tables." },
  { kind: "tpch", label: CATALOG_KIND_LABELS.tpch, blurb: "Generated benchmark data. No external system." },
  { kind: "tpcds", label: CATALOG_KIND_LABELS.tpcds, blurb: "Generated benchmark data. No external system." },
  {
    kind: "generic",
    label: CATALOG_KIND_LABELS.generic,
    blurb: "Engine properties written by hand, for a connector egret has no typed form for.",
  },
];

/** The kinds `create_registry_catalog` accepts — every option but egret's own. */
export const REGISTRY_KINDS: string[] = KIND_OPTIONS.filter(
  (o) => o.kind !== EGRET_WAREHOUSE_KIND,
).map((o) => o.kind);

/**
 * A platform storage-connection chooser.
 *
 * NOT `components/SecretPicker.tsx`: that one is workspace-scoped (`ws: string`)
 * and offers inline secret creation, both wrong for a platform catalog, and six
 * other screens depend on its current shape. These are plain controlled
 * `<Select>`s over the platform lists, the same pattern `EngineCreate.tsx`
 * already inlines.
 */
export function StorageSelect({
  id,
  value,
  onChange,
}: {
  id: string;
  value: string;
  onChange: (v: string) => void;
}) {
  const q = useQuery({
    // The literal `ControlPlaneStorage.tsx` exports as `storageQueryKey`, so
    // this page reads the storage list the Storage page has already cached
    // (30s staleTime). Pinned by `CatalogCreate.test.tsx`.
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const items = q.data ?? [];
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger id={id} className="w-full">
        <SelectValue placeholder="Select a storage connection" />
      </SelectTrigger>
      <SelectContent>
        {items.length === 0 ? (
          <SelectItem value={NONE} disabled>
            No storage connections
          </SelectItem>
        ) : (
          items.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.name}
            </SelectItem>
          ))
        )}
      </SelectContent>
    </Select>
  );
}

/**
 * A platform secret chooser. Every credential a catalog references is a secret
 * id — `config.rs` has no plaintext-password field anywhere — so this is the
 * only control that may stand next to the word "password" on this page.
 */
export function SecretSelect({
  id,
  value,
  onChange,
  placeholder,
}: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const q = useQuery({
    queryKey: ["control-plane", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });
  const items = q.data ?? [];
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger id={id} className="w-full">
        <SelectValue placeholder={placeholder ?? "Select a secret"} />
      </SelectTrigger>
      <SelectContent>
        {items.length === 0 ? (
          <SelectItem value={NONE} disabled>
            No secrets
          </SelectItem>
        ) : (
          items.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.name}
            </SelectItem>
          ))
        )}
      </SelectContent>
    </Select>
  );
}
