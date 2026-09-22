import { useState } from "react";
import { Trash2 } from "lucide-react";

import { ConfigSection } from "@/components/form/ConfigSection";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { FieldLabel } from "../../config/FieldLabel";
import { NONE, SecretSelect, StorageSelect } from "./pickers";

/**
 * The typed create forms, one per `CatalogKind` family.
 *
 * Every shape here is the serde shape of
 * `crates/egret-catalog-registry/src/config.rs`, which is
 * `#[serde(deny_unknown_fields)]` throughout: an extra key is a 400, not a
 * silently ignored field. Two consequences run through the whole file:
 *
 *  1. A tagged-union discriminator (`metastore.type`, `auth.type`, `ssl.mode`)
 *     REPLACES its object rather than merging into it. Spreading the old arm
 *     leaves, say, `warehouse` behind when the metastore becomes `hive`, and
 *     the body is refused with a message about a field the user cannot see.
 *  2. An optional field is absent, never `""`. `""` is not a UUID and not a
 *     host, so it fails the typed parse with a message about a field the user
 *     deliberately left blank. `cleanConfig` strips them at submit time.
 *
 * The forms also decline to offer what the backend refuses outright, rather
 * than letting the user meet it as a 400:
 *  - `ca_secret_id` for `verify_full` ("a custom CA certificate is not
 *    supported yet", `config.rs:524-531`) — no field exists for it;
 *  - Glue for `hudi` ("Hudi reads a Hive metastore only", `config.rs:494-496`)
 *    — the metastore choice is not offered;
 *  - `database` for MySQL/MariaDB ("… expose every database as a schema; leave
 *    database empty", `config.rs:511-516`) — the field disappears with the
 *    dialect.
 */
export type KindFormProps = {
  config: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
};

type Rec = Record<string, unknown>;

function rec(v: unknown): Rec {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Rec) : {};
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

/**
 * The minimal skeleton a freshly chosen kind starts from. No default arm: an
 * unknown kind is a bug in the caller's kind list, not an empty object that
 * would be posted and refused.
 */
export function emptyConfigFor(kind: string): Record<string, unknown> {
  switch (kind) {
    case "iceberg":
      // `storage_connection_id` is NOT optional in practice: `validate_config`
      // refuses a non-egret iceberg catalog without one (`config.rs:451-455`).
      // It is `Option` in the struct only because the `egret` metastore — the
      // one this form cannot create — must not carry it.
      return {
        metastore: { type: "rest", uri: "", warehouse: "", auth: { type: "none" } },
        storage_connection_id: "",
      };
    case "hive":
    case "delta_lake":
    case "hudi":
      return { metastore: { type: "hive", uri: "" }, storage_connection_id: "" };
    case "jdbc":
      return {
        dialect: "",
        host: "",
        port: 5432,
        user: "",
        password_secret_id: "",
        ssl: { mode: "require" },
      };
    case "elasticsearch":
      return { hosts: [], port: 9200, tls: true };
    case "tpch":
    case "tpcds":
      return {};
    case "generic":
      return { engines: [], secret_properties: { trino: {}, starrocks: {} } };
    default:
      throw new Error(`no config skeleton for kind ${kind}`);
  }
}

/** The dialects `JdbcDialect` accepts, in `config.rs`'s own order. */
const JDBC_DIALECTS = [
  "postgresql",
  "mysql",
  "mariadb",
  "oracle",
  "sqlserver",
  "clickhouse",
];

/** Dialects whose catalog needs a database: `config.rs:505-510`. */
const DATABASE_REQUIRED = ["postgresql", "oracle", "sqlserver"];
/** Dialects that refuse one: `config.rs:511-516`. */
const DATABASE_REFUSED = ["mysql", "mariadb"];

/** The `advanced.<engine>` key a `generic` catalog must set: `config.rs:592-604`. */
export const GENERIC_REQUIRED_KEY: Record<string, string> = {
  trino: "connector.name",
  starrocks: "type",
};

// ── metastore ────────────────────────────────────────────────────────────────

/**
 * `IcebergMetastore` / `HiveMetastore`. Both enums carry byte-identical `hive`
 * and `glue` arms, so one editor serves both; `types` is what the kind allows.
 */
function MetastoreEditor({
  idPrefix,
  types,
  value,
  onChange,
}: {
  idPrefix: string;
  types: string[];
  value: Rec;
  onChange: (next: Rec) => void;
}) {
  const type = str(value.type);
  const auth = rec(value.auth);
  return (
    <div className="space-y-4">
      <div className="space-y-1.5">
        <FieldLabel
          htmlFor={`${idPrefix}-type`}
          label="Metastore"
          help="Where the table metadata lives. Changing this replaces the fields below — the engines read a different set of properties for each."
        />
        <Select
          value={type}
          // Wholesale replacement, never a spread: `deny_unknown_fields`
          // refuses a `warehouse` left over from the REST arm.
          onValueChange={(t) => onChange(emptyMetastore(t))}
        >
          <SelectTrigger id={`${idPrefix}-type`} className="w-full">
            <SelectValue placeholder="Select a metastore" />
          </SelectTrigger>
          <SelectContent>
            {types.map((t) => (
              <SelectItem key={t} value={t}>
                {METASTORE_LABEL[t] ?? t}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {type === "rest" && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-uri`}
              label="REST URI"
              help="The Iceberg REST catalog endpoint, http:// or https:// with a host."
            />
            <Input
              id={`${idPrefix}-uri`}
              value={str(value.uri)}
              placeholder="https://catalog.example:8181"
              onChange={(e) => onChange({ ...value, uri: e.target.value })}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-warehouse`}
              label="Warehouse"
              help="The warehouse name the REST catalog serves, as that catalog spells it."
            />
            <Input
              id={`${idPrefix}-warehouse`}
              value={str(value.warehouse)}
              onChange={(e) => onChange({ ...value, warehouse: e.target.value })}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-auth`}
              label="Authentication"
              help="How the engines authenticate to the REST catalog."
            />
            <Select
              value={str(auth.type)}
              onValueChange={(t) =>
                onChange({
                  ...value,
                  // Replaced, not merged: an OAuth2 `credential_secret_id`
                  // surviving a switch to `none` is a refused body.
                  auth: t === "oauth2" ? { type: "oauth2", credential_secret_id: "" } : { type: t },
                })
              }
            >
              <SelectTrigger id={`${idPrefix}-auth`} className="w-full">
                <SelectValue placeholder="Select authentication" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">None</SelectItem>
                <SelectItem value="oauth2">OAuth 2</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {str(auth.type) === "oauth2" && (
            <>
              <div className="space-y-1.5">
                <FieldLabel
                  htmlFor={`${idPrefix}-credential`}
                  label="Credential secret"
                  help="A platform secret holding the OAuth2 client credential. The catalog never stores it in plaintext."
                />
                <SecretSelect
                  id={`${idPrefix}-credential`}
                  value={str(auth.credential_secret_id)}
                  onChange={(v) =>
                    onChange({ ...value, auth: { ...auth, credential_secret_id: v } })
                  }
                />
              </div>
              <div className="space-y-1.5">
                <FieldLabel
                  htmlFor={`${idPrefix}-scope`}
                  label="Scope"
                  help="Optional OAuth2 scope. Left blank, the catalog's own default applies."
                />
                <Input
                  id={`${idPrefix}-scope`}
                  value={str(auth.scope)}
                  onChange={(e) => onChange({ ...value, auth: { ...auth, scope: e.target.value } })}
                />
              </div>
              <div className="space-y-1.5">
                <FieldLabel
                  htmlFor={`${idPrefix}-token-uri`}
                  label="Token URI"
                  help="Optional token endpoint, http:// or https://. Left blank, the REST catalog's own token endpoint is used."
                />
                <Input
                  id={`${idPrefix}-token-uri`}
                  value={str(auth.token_uri)}
                  placeholder="https://idp.example/oauth/token"
                  onChange={(e) =>
                    onChange({ ...value, auth: { ...auth, token_uri: e.target.value } })
                  }
                />
              </div>
            </>
          )}
        </div>
      )}

      {type === "hive" && (
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor={`${idPrefix}-uri`}
            label="Metastore URI"
            help="thrift://host:port. Several may be given, comma-separated."
          />
          <Input
            id={`${idPrefix}-uri`}
            value={str(value.uri)}
            placeholder="thrift://hms.example:9083"
            onChange={(e) => onChange({ ...value, uri: e.target.value })}
          />
        </div>
      )}

      {type === "glue" && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-region`}
              label="Region"
              help="The AWS region the Glue Data Catalog lives in."
            />
            <Input
              id={`${idPrefix}-region`}
              value={str(value.region)}
              placeholder="eu-west-1"
              onChange={(e) => onChange({ ...value, region: e.target.value })}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-access-key`}
              label="Access key id"
              help="Optional. Left blank, the engine's own AWS credentials are used — but an access key and its secret go together."
            />
            <Input
              id={`${idPrefix}-access-key`}
              value={str(value.access_key)}
              onChange={(e) => onChange({ ...value, access_key: e.target.value })}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-secret-key`}
              label="Secret key secret"
              help="The platform secret holding the AWS secret access key."
            />
            <SecretSelect
              id={`${idPrefix}-secret-key`}
              value={str(value.secret_key_secret_id)}
              onChange={(v) => onChange({ ...value, secret_key_secret_id: v })}
            />
          </div>
        </div>
      )}
    </div>
  );
}

const METASTORE_LABEL: Record<string, string> = {
  rest: "Iceberg REST",
  hive: "Hive Metastore",
  glue: "AWS Glue",
};

function emptyMetastore(type: string): Rec {
  switch (type) {
    case "rest":
      return { type, uri: "", warehouse: "", auth: { type: "none" } };
    case "hive":
      return { type, uri: "" };
    case "glue":
      return { type, region: "" };
    default:
      throw new Error(`no metastore skeleton for ${type}`);
  }
}

/** The storage field shared by iceberg and the hive family — required in both. */
function StorageField({
  idPrefix,
  value,
  onChange,
}: {
  idPrefix: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="space-y-1.5">
      <FieldLabel
        htmlFor={`${idPrefix}-storage`}
        label="Storage connection"
        help="The object storage the table data lives on. egret renders its endpoint and credentials into every engine this catalog is attached to."
      />
      <StorageSelect id={`${idPrefix}-storage`} value={value} onChange={onChange} />
    </div>
  );
}

// ── the kind forms ───────────────────────────────────────────────────────────

export function IcebergForm({ config, onChange }: KindFormProps) {
  const c = config;
  return (
    <ConfigSection
      level={2}
      title="Iceberg catalog"
      description="An Iceberg catalog egret does not manage. Its metastore is the source of truth for table metadata; egret only tells the engines how to reach it."
    >
      <MetastoreEditor
        idPrefix="cc-iceberg"
        types={["rest", "hive", "glue"]}
        value={rec(c.metastore)}
        onChange={(m) => onChange({ ...c, metastore: m })}
      />
      <StorageField
        idPrefix="cc-iceberg"
        value={str(c.storage_connection_id)}
        onChange={(v) => onChange({ ...c, storage_connection_id: v })}
      />
    </ConfigSection>
  );
}

export function HiveFamilyForm({
  kind,
  config,
  onChange,
}: KindFormProps & { kind: string }) {
  const c = config;
  // `HiveMetastore` has a Glue arm, but `validate_config` refuses it for Hudi.
  const types = kind === "hudi" ? ["hive"] : ["hive", "glue"];
  return (
    <ConfigSection
      level={2}
      title="Metastore and storage"
      description="Both are required for this kind: the metastore holds the table definitions, the storage connection holds the files."
    >
      <MetastoreEditor
        idPrefix="cc-hive"
        types={types}
        value={rec(c.metastore)}
        onChange={(m) => onChange({ ...c, metastore: m })}
      />
      <StorageField
        idPrefix="cc-hive"
        value={str(c.storage_connection_id)}
        onChange={(v) => onChange({ ...c, storage_connection_id: v })}
      />
    </ConfigSection>
  );
}

export function JdbcForm({ config, onChange }: KindFormProps) {
  const c = config;
  const set = (patch: Rec) => onChange({ ...c, ...patch });
  const dialect = str(c.dialect);
  const ssl = rec(c.ssl);
  const showDatabase = dialect !== "" && !DATABASE_REFUSED.includes(dialect);
  return (
    <ConfigSection
      level={2}
      title="Database"
      description="egret connects with these credentials from every engine this catalog is attached to."
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-jdbc-dialect"
            label="Dialect"
            help="Selects the driver and the URL vocabulary. StarRocks treats mysql and mariadb identically."
          />
          <Select
            value={dialect}
            onValueChange={(v) =>
              // The database field's very existence depends on the dialect, so
              // a dialect change drops whatever was typed for the previous one
              // — MySQL refuses a database outright.
              set({ dialect: v, ...(DATABASE_REFUSED.includes(v) ? { database: undefined } : {}) })
            }
          >
            <SelectTrigger id="cc-jdbc-dialect" className="w-full">
              <SelectValue placeholder="Select a dialect" />
            </SelectTrigger>
            <SelectContent>
              {JDBC_DIALECTS.map((d) => (
                <SelectItem key={d} value={d}>
                  {d}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-jdbc-host"
            label="Host"
            help="A host name or address, reachable from the engines — not from your browser."
          />
          <Input
            id="cc-jdbc-host"
            value={str(c.host)}
            placeholder="db.example"
            onChange={(e) => set({ host: e.target.value })}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel htmlFor="cc-jdbc-port" label="Port" help="1-65535." />
          <Input
            id="cc-jdbc-port"
            type="number"
            min={1}
            max={65535}
            value={typeof c.port === "number" ? c.port : ""}
            onChange={(e) => set({ port: Number(e.target.value) })}
          />
        </div>
        {showDatabase && (
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor="cc-jdbc-database"
              label="Database"
              help={
                DATABASE_REQUIRED.includes(dialect)
                  ? "Required for this dialect: the catalog exposes one database."
                  : "Optional for this dialect."
              }
            />
            <Input
              id="cc-jdbc-database"
              value={str(c.database)}
              onChange={(e) => set({ database: e.target.value })}
            />
          </div>
        )}
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-jdbc-user"
            label="User"
            help="The database user every engine connects as."
          />
          <Input
            id="cc-jdbc-user"
            value={str(c.user)}
            onChange={(e) => set({ user: e.target.value })}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-jdbc-password"
            label="Password secret"
            help="A platform secret id. The catalog never stores a plaintext password."
          />
          <SecretSelect
            id="cc-jdbc-password"
            value={str(c.password_secret_id)}
            onChange={(v) => set({ password_secret_id: v })}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-jdbc-ssl"
            label="TLS"
            help="verify_full validates the server certificate against the engine's own trust store. A custom CA is not supported yet, so there is nothing else to configure here."
          />
          <Select
            value={str(ssl.mode)}
            // `{ mode }` and nothing else, in every arm: `ssl` is a tagged
            // union and only `verify_full` has a second field — one this form
            // must never set, because validation refuses it outright.
            onValueChange={(mode) => set({ ssl: { mode } })}
          >
            <SelectTrigger id="cc-jdbc-ssl" className="w-full">
              <SelectValue placeholder="Select a TLS mode" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="disable">disable</SelectItem>
              <SelectItem value="require">require</SelectItem>
              <SelectItem value="verify_full">verify_full</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
    </ConfigSection>
  );
}

export function ElasticsearchForm({ config, onChange }: KindFormProps) {
  const c = config;
  const set = (patch: Rec) => onChange({ ...c, ...patch });
  const hosts = Array.isArray(c.hosts) ? (c.hosts as string[]) : [];
  // The wire shape is a list; the control is one comma-separated field, so the
  // raw text (including a half-typed entry) lives here rather than in `config`.
  const [hostsText, setHostsText] = useState(hosts.join(", "));
  const hasUser = str(c.user) !== "" || str(c.password_secret_id) !== "";
  return (
    <ConfigSection
      level={2}
      title="Elasticsearch"
      description="Indices are exposed as tables. The engines connect directly to these hosts."
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5 sm:col-span-2">
          <FieldLabel
            htmlFor="cc-es-hosts"
            label="Hosts"
            help="One or more host names or addresses, comma-separated. They all share the port below."
          />
          <Input
            id="cc-es-hosts"
            value={hostsText}
            placeholder="es-1.example, es-2.example"
            onChange={(e) => {
              setHostsText(e.target.value);
              set({
                hosts: e.target.value
                  .split(",")
                  .map((h) => h.trim())
                  .filter((h) => h !== ""),
              });
            }}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel htmlFor="cc-es-port" label="Port" help="1-65535, shared by every host." />
          <Input
            id="cc-es-port"
            type="number"
            min={1}
            max={65535}
            value={typeof c.port === "number" ? c.port : ""}
            onChange={(e) => set({ port: Number(e.target.value) })}
          />
        </div>
        <div className="flex items-center gap-2 pt-6">
          <Switch
            id="cc-es-tls"
            checked={c.tls === true}
            onCheckedChange={(v) => set({ tls: v })}
          />
          <Label htmlFor="cc-es-tls">TLS</Label>
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-es-user"
            label="User"
            help="Optional. A user and its password secret go together — one without the other is refused."
          />
          <Input
            id="cc-es-user"
            value={str(c.user)}
            onChange={(e) => set({ user: e.target.value })}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-es-password"
            label="Password secret"
            help="The platform secret holding that user's password."
          />
          <SecretSelect
            id="cc-es-password"
            value={str(c.password_secret_id)}
            onChange={(v) => set({ password_secret_id: v })}
          />
        </div>
      </div>
      {hasUser && (str(c.user) === "" || str(c.password_secret_id) === "") && (
        <p className="text-xs text-destructive">
          A user and its password secret go together.
        </p>
      )}
    </ConfigSection>
  );
}

export function SampleForm({ config, onChange }: KindFormProps) {
  const c = config;
  return (
    <ConfigSection
      level={2}
      title="Generated data"
      description="The connector generates the data; there is nothing to connect to and no credential to hold."
    >
      <div className="space-y-1.5 sm:max-w-xs">
        <FieldLabel
          htmlFor="cc-sample-scale"
          label="Scale"
          help="Optional, and Trino ignores it — its schema names carry the scale. StarRocks reads it."
        />
        <Input
          id="cc-sample-scale"
          type="number"
          min={1}
          value={typeof c.scale === "number" ? c.scale : ""}
          placeholder="default"
          onChange={(e) => {
            const next = { ...c };
            if (e.target.value === "") delete next.scale;
            else next.scale = Number(e.target.value);
            onChange(next);
          }}
        />
      </div>
    </ConfigSection>
  );
}

export function GenericForm({ config, onChange }: KindFormProps) {
  const c = config;
  const engines = Array.isArray(c.engines) ? (c.engines as string[]) : [];
  const secrets = rec(c.secret_properties);
  const toggle = (engine: string, on: boolean) => {
    const nextEngines = on
      ? [...engines.filter((e) => e !== engine), engine]
      : engines.filter((e) => e !== engine);
    // An engine that is not selected may carry neither secret properties nor
    // advanced properties (`config.rs:606-611`), so unselecting it clears its
    // secrets here; `cleanAdvanced` does the same for `advanced`.
    onChange({
      ...c,
      engines: nextEngines,
      secret_properties: { ...secrets, [engine]: on ? rec(secrets[engine]) : {} },
    });
  };
  return (
    <ConfigSection
      level={2}
      title="Engines and secret properties"
      description="A generic catalog is whatever properties you write in Advanced below. Name the engines it is valid for, and reference any credential as a secret instead of typing it."
    >
      <div className="space-y-2">
        {["trino", "starrocks"].map((e) => (
          <div key={e} className="flex items-center gap-2">
            <Checkbox
              id={`cc-generic-${e}`}
              checked={engines.includes(e)}
              onCheckedChange={(v) => toggle(e, v === true)}
            />
            <Label htmlFor={`cc-generic-${e}`}>{e}</Label>
            <span className="text-xs text-muted-foreground">
              needs <code>advanced.{e}.{GENERIC_REQUIRED_KEY[e]}</code>
            </span>
          </div>
        ))}
      </div>
      {engines.map((e) => (
        <SecretPropertyList
          key={e}
          engine={e}
          value={Object.fromEntries(
            Object.entries(rec(secrets[e])).map(([k, v]) => [k, str(v)]),
          )}
          onChange={(next) =>
            onChange({ ...c, secret_properties: { ...secrets, [e]: next } })
          }
        />
      ))}
    </ConfigSection>
  );
}

/** `secret_properties.<engine>`: property name → platform secret id. */
function SecretPropertyList({
  engine,
  value,
  onChange,
}: {
  engine: string;
  value: Record<string, string>;
  onChange: (next: Record<string, string>) => void;
}) {
  const [newKey, setNewKey] = useState("");
  const entries = Object.entries(value);
  return (
    <div className="space-y-2 rounded-md border p-3">
      <p className="text-sm font-medium">Secret properties — {engine}</p>
      <p className="text-xs text-muted-foreground">
        Each property's value is read from a secret at render time. A key here may not also
        appear in Advanced below.
      </p>
      {entries.map(([k, v]) => (
        <div key={k} className="flex items-end gap-2">
          <div className="min-w-0 flex-1 space-y-1.5">
            <Label htmlFor={`cc-generic-${engine}-secret-${k}`}>{k}</Label>
            <SecretSelect
              id={`cc-generic-${engine}-secret-${k}`}
              value={v}
              onChange={(id) => onChange({ ...value, [k]: id })}
            />
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={`Remove ${engine} secret property ${k}`}
            onClick={() => {
              const next = { ...value };
              delete next[k];
              onChange(next);
            }}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <div className="flex items-end gap-2">
        <div className="min-w-0 flex-1 space-y-1.5">
          <Label htmlFor={`cc-generic-${engine}-newkey`}>{engine} secret property name</Label>
          <Input
            id={`cc-generic-${engine}-newkey`}
            value={newKey}
            placeholder="e.g. password"
            onChange={(e) => setNewKey(e.target.value)}
          />
        </div>
        <Button
          type="button"
          variant="outline"
          aria-label={`Add ${engine} secret property`}
          disabled={newKey.trim() === "" || newKey.trim() in value}
          onClick={() => {
            onChange({ ...value, [newKey.trim()]: "" });
            setNewKey("");
          }}
        >
          Add
        </Button>
      </div>
    </div>
  );
}

/**
 * The egret-warehouse branch is not a registry form at all: the registry
 * refuses to create one, so these two fields feed `createWarehouse` instead
 * (see `CatalogCreate.handleCreate`).
 */
export function EgretWarehouseForm({
  workspaces,
  workspaceId,
  onWorkspaceId,
  storageId,
  onStorageId,
}: {
  workspaces: { id: string; name: string }[];
  workspaceId: string;
  onWorkspaceId: (v: string) => void;
  storageId: string;
  onStorageId: (v: string) => void;
}) {
  return (
    <ConfigSection
      level={2}
      title="Warehouse"
      description="egret provisions a lakekeeper warehouse over this storage connection and records its catalog. The workspace owns it."
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="cc-wh-workspace"
            label="Workspace"
            help="The workspace the warehouse belongs to. Its catalog is assigned to that workspace on creation."
          />
          <Select value={workspaceId} onValueChange={onWorkspaceId}>
            <SelectTrigger id="cc-wh-workspace" className="w-full">
              <SelectValue placeholder="Select a workspace" />
            </SelectTrigger>
            <SelectContent>
              {workspaces.length === 0 ? (
                <SelectItem value={NONE} disabled>
                  No workspaces
                </SelectItem>
              ) : (
                workspaces.map((w) => (
                  <SelectItem key={w.id} value={w.id}>
                    {w.name}
                  </SelectItem>
                ))
              )}
            </SelectContent>
          </Select>
        </div>
        <StorageField idPrefix="cc-wh" value={storageId} onChange={onStorageId} />
      </div>
    </ConfigSection>
  );
}

/** Dispatches to the kind's form. `egret_warehouse` is the page's own branch. */
export function KindForm({
  kind,
  config,
  onChange,
}: KindFormProps & { kind: string }) {
  switch (kind) {
    case "iceberg":
      return <IcebergForm config={config} onChange={onChange} />;
    case "hive":
    case "delta_lake":
    case "hudi":
      return <HiveFamilyForm kind={kind} config={config} onChange={onChange} />;
    case "jdbc":
      return <JdbcForm config={config} onChange={onChange} />;
    case "elasticsearch":
      return <ElasticsearchForm config={config} onChange={onChange} />;
    case "tpch":
    case "tpcds":
      return <SampleForm config={config} onChange={onChange} />;
    case "generic":
      return <GenericForm config={config} onChange={onChange} />;
    default:
      return null;
  }
}

// ── submit-time shaping ──────────────────────────────────────────────────────

/**
 * Trim every string in the configuration, in place of trimming nowhere.
 *
 * `submitProblem` validates `host.trim()`, `user.trim()`, `warehouse.trim()`
 * and `region.trim()`; posting the untrimmed value means validating one string
 * and storing another. `host` at least fails readably at the backend's own
 * `host()` regex — `user`, `warehouse` and `region` with a trailing space are
 * ACCEPTED, stored, and then fail at engine authentication with no message
 * anywhere (SP3c review, fix round 1). Secret and storage ids come from
 * `<Select>`s and cannot carry spaces, so a blanket trim cannot lose anything.
 * `advanced` is deliberately NOT trimmed: those values are engine property
 * values, where a space may be what the operator means.
 */
function trimDeep<T>(v: T): T {
  if (typeof v === "string") return v.trim() as T;
  if (Array.isArray(v)) return v.map(trimDeep) as T;
  if (v && typeof v === "object") {
    return Object.fromEntries(
      Object.entries(v as Rec).map(([k, x]) => [k, trimDeep(x)]),
    ) as T;
  }
  return v;
}

/** Drop `k` when its value is an empty string (an absent optional, not `""`). */
function withoutBlanks(o: Rec, keys: string[]): Rec {
  const next = { ...o };
  for (const k of keys) {
    if (next[k] === "" || next[k] === undefined) delete next[k];
  }
  return next;
}

/**
 * The body's `config`: the form state with every blank optional removed.
 *
 * `""` is never a valid value for any of these — not a UUID, not a host, not a
 * database name — so sending one turns "I left this blank" into a parse error
 * naming a field the user never filled in.
 */
export function cleanConfig(kind: string, config: Record<string, unknown>): Record<string, unknown> {
  const c = trimDeep({ ...config });
  switch (kind) {
    case "iceberg":
    case "hive":
    case "delta_lake":
    case "hudi": {
      let m = { ...rec(c.metastore) };
      if (str(m.type) === "rest") {
        m.auth = withoutBlanks(rec(m.auth), ["scope", "token_uri", "credential_secret_id"]);
      }
      if (str(m.type) === "glue") {
        m = withoutBlanks(m, ["access_key", "secret_key_secret_id"]);
      }
      c.metastore = m;
      return withoutBlanks(c, ["storage_connection_id"]);
    }
    case "jdbc": {
      const dialect = str(c.dialect);
      const out = withoutBlanks(c, ["database", "password_secret_id"]);
      // MySQL and MariaDB refuse a database outright, whatever is in the
      // form's memory from a previous dialect.
      if (DATABASE_REFUSED.includes(dialect)) delete out.database;
      return out;
    }
    case "elasticsearch":
      return withoutBlanks(c, ["user", "password_secret_id"]);
    case "generic": {
      const engines = Array.isArray(c.engines) ? (c.engines as string[]) : [];
      const secrets = rec(c.secret_properties);
      const kept: Rec = {};
      for (const e of ["trino", "starrocks"]) {
        if (engines.includes(e)) kept[e] = rec(secrets[e]);
      }
      return { ...c, engines, secret_properties: kept };
    }
    default:
      return c;
  }
}

/**
 * `advanced`, pruned to the engines a `generic` catalog names.
 *
 * `validate_advanced` refuses `advanced.<engine>` for an engine the generic
 * config does not include, so unselecting an engine after writing properties
 * for it has to drop them — otherwise the save is refused for a section the
 * user can no longer see.
 */
export function cleanAdvanced(
  kind: string,
  config: Record<string, unknown>,
  advanced: Record<string, Record<string, string>>,
): Record<string, Record<string, string>> {
  const out: Record<string, Record<string, string>> = {};
  const engines = Array.isArray(config.engines) ? (config.engines as string[]) : [];
  for (const e of ["trino", "starrocks"]) {
    const map = advanced[e] ?? {};
    if (Object.keys(map).length === 0) continue;
    if (kind === "generic" && !engines.includes(e)) continue;
    out[e] = map;
  }
  return out;
}

/**
 * Why this form cannot be submitted yet, in the backend's own words where it
 * has them — so the sentence the user reads before they press Create is the
 * sentence they would have read after.
 *
 * Returns `null` when the configuration is complete. This is a completeness
 * check, not a re-implementation of `validate_config`: the server is still the
 * authority, and anything it refuses that is not listed here surfaces as its
 * own message on the page.
 */
export function submitProblem(
  kind: string,
  config: Record<string, unknown>,
  advanced: Record<string, Record<string, string>>,
): string | null {
  const c = config;
  const whole = (v: unknown, max: number) =>
    typeof v === "number" && Number.isInteger(v) && v >= 1 && v <= max;
  const port = (v: unknown) => (whole(v, 65535) ? null : "port must be 1-65535");

  switch (kind) {
    case "iceberg":
    case "hive":
    case "delta_lake":
    case "hudi": {
      const m = rec(c.metastore);
      const type = str(m.type);
      if (type === "rest") {
        if (!/^https?:\/\//.test(str(m.uri))) return "uri must be an http(s) URL";
        if (str(m.warehouse).trim() === "") return "warehouse is required";
        const auth = rec(m.auth);
        if (str(auth.type) === "oauth2" && str(auth.credential_secret_id) === "") {
          return "an OAuth2 REST catalog needs a credential secret";
        }
      } else if (type === "hive") {
        if (
          str(m.uri).trim() === "" ||
          str(m.uri)
            .split(",")
            .some((u) => !u.trim().startsWith("thrift://"))
        ) {
          return "uri must be thrift://host:port (comma-separated)";
        }
      } else if (type === "glue") {
        if (str(m.region).trim() === "") return "region is required";
        if ((str(m.access_key) === "") !== (str(m.secret_key_secret_id) === "")) {
          return "access_key and secret_key_secret_id go together";
        }
      } else {
        return "select a metastore";
      }
      if (str(c.storage_connection_id) === "") return "storage_connection_id is required";
      return null;
    }
    case "jdbc": {
      const dialect = str(c.dialect);
      if (dialect === "") return "select a dialect";
      if (str(c.host).trim() === "") return "host is required";
      const p = port(c.port);
      if (p) return p;
      if (str(c.user).trim() === "") return "user is required";
      if (str(c.password_secret_id) === "") return "a password secret is required";
      if (DATABASE_REQUIRED.includes(dialect) && str(c.database).trim() === "") {
        return `database is required for ${dialect}`;
      }
      if (str(rec(c.ssl).mode) === "") return "select a TLS mode";
      return null;
    }
    case "elasticsearch": {
      const hosts = Array.isArray(c.hosts) ? (c.hosts as string[]) : [];
      if (hosts.length === 0) return "hosts must list at least one host";
      const p = port(c.port);
      if (p) return p;
      if ((str(c.user) === "") !== (str(c.password_secret_id) === "")) {
        return "user and password_secret_id go together";
      }
      return null;
    }
    case "tpch":
    case "tpcds":
      // `scale` is `Option<u32>`: `2.5` reaches serde as "invalid type:
      // floating point `2.5`", which names no field at all.
      if (c.scale !== undefined && !whole(c.scale, 4294967295)) {
        return "scale must be a positive whole number";
      }
      return null;
    case "generic": {
      const engines = Array.isArray(c.engines) ? (c.engines as string[]) : [];
      if (engines.length === 0) return "engines must name trino, starrocks or both";
      for (const e of engines) {
        const required = GENERIC_REQUIRED_KEY[e];
        if ((advanced[e]?.[required] ?? "").trim() === "") {
          return `advanced.${e}.${required} is required`;
        }
      }
      const secrets = rec(c.secret_properties);
      for (const e of engines) {
        for (const [k, v] of Object.entries(rec(secrets[e]))) {
          if (str(v) === "") return `secret_properties.${e}.${k} needs a secret`;
          if (advanced[e]?.[k] !== undefined) {
            return `advanced.${e}.${k} collides with a property the generic form sets`;
          }
        }
      }
      return null;
    }
    default:
      return null;
  }
}
