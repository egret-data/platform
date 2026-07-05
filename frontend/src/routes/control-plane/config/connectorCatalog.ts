/**
 * Curated catalog of common Trino connectors and their high-value properties,
 * for the engine "Add catalog" wizard's generic-connector path. Each entry's
 * `props` is a `TrinoProp[]` scoped to that connector — same shape as
 * `trinoCatalog.ts`, with `file` set to the dummy value `"config"` since
 * `KeyedPropertyList` ignores it (these keys live in a catalog `.properties`
 * file, not `config.properties`). Nothing here is invented — every key
 * appears on the connector's Trino docs page; obscure/advanced knobs are
 * intentionally omitted in favor of the 5-15 most commonly set properties.
 */

import { type TrinoProp } from "./trinoCatalog";

export interface Connector {
  /** The Trino `connector.name` value. */
  name: string;
  /** Human label for the picker. */
  label: string;
  props: TrinoProp[];
}

const MYSQL_DOCS = "https://trino.io/docs/current/connector/mysql.html";
const POSTGRESQL_DOCS =
  "https://trino.io/docs/current/connector/postgresql.html";
const SQLSERVER_DOCS = "https://trino.io/docs/current/connector/sqlserver.html";
const CLICKHOUSE_DOCS =
  "https://trino.io/docs/current/connector/clickhouse.html";
const ICEBERG_DOCS = "https://trino.io/docs/current/connector/iceberg.html";
const HIVE_DOCS = "https://trino.io/docs/current/connector/hive.html";
const DELTA_LAKE_DOCS =
  "https://trino.io/docs/current/connector/delta-lake.html";
const KAFKA_DOCS = "https://trino.io/docs/current/connector/kafka.html";
const MONGODB_DOCS = "https://trino.io/docs/current/connector/mongodb.html";
const ORACLE_DOCS = "https://trino.io/docs/current/connector/oracle.html";
const REDSHIFT_DOCS = "https://trino.io/docs/current/connector/redshift.html";
const MARIADB_DOCS = "https://trino.io/docs/current/connector/mariadb.html";
const SINGLESTORE_DOCS =
  "https://trino.io/docs/current/connector/singlestore.html";
const SNOWFLAKE_DOCS = "https://trino.io/docs/current/connector/snowflake.html";
const DRUID_DOCS = "https://trino.io/docs/current/connector/druid.html";
const IGNITE_DOCS = "https://trino.io/docs/current/connector/ignite.html";
const PHOENIX5_DOCS = "https://trino.io/docs/current/connector/phoenix5.html";
const CASSANDRA_DOCS =
  "https://trino.io/docs/current/connector/cassandra.html";
const ELASTICSEARCH_DOCS =
  "https://trino.io/docs/current/connector/elasticsearch.html";
const REDIS_DOCS = "https://trino.io/docs/current/connector/redis.html";
const PROMETHEUS_DOCS =
  "https://trino.io/docs/current/connector/prometheus.html";
const MEMORY_DOCS = "https://trino.io/docs/current/connector/memory.html";
const BIGQUERY_DOCS = "https://trino.io/docs/current/connector/bigquery.html";
const GOOGLESHEETS_DOCS =
  "https://trino.io/docs/current/connector/googlesheets.html";

export const connectors: Connector[] = [
  {
    name: "mysql",
    label: "MySQL",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the MySQL server.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to MySQL with.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to MySQL with.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "decimal-mapping",
        group: "Mapping",
        file: "config",
        type: "enum",
        enumOptions: ["STRICT", "ALLOW_OVERFLOW"],
        default: "STRICT",
        help: "How MySQL DECIMAL/NUMERIC columns with unbounded precision are mapped.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "decimal-rounding-mode",
        group: "Mapping",
        file: "config",
        type: "enum",
        enumOptions: [
          "UNNECESSARY",
          "UP",
          "DOWN",
          "CEILING",
          "FLOOR",
          "HALF_UP",
          "HALF_DOWN",
          "HALF_EVEN",
        ],
        default: "UNNECESSARY",
        help: "Rounding mode used when decimal-mapping is ALLOW_OVERFLOW.",
        docsUrl: MYSQL_DOCS,
      },
      {
        key: "metadata.cache-ttl",
        group: "Cache",
        file: "config",
        type: "duration",
        default: "0s",
        help: "How long to cache table/schema metadata before refreshing.",
        docsUrl: MYSQL_DOCS,
      },
    ],
  },
  {
    name: "postgresql",
    label: "PostgreSQL",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the PostgreSQL server.",
        docsUrl: POSTGRESQL_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to PostgreSQL with.",
        docsUrl: POSTGRESQL_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to PostgreSQL with.",
        docsUrl: POSTGRESQL_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: POSTGRESQL_DOCS,
      },
      {
        key: "postgresql.array-mapping",
        group: "Mapping",
        file: "config",
        type: "enum",
        enumOptions: ["DISABLED", "AS_ARRAY", "AS_JSON"],
        default: "DISABLED",
        help: "How PostgreSQL array columns are exposed in Trino.",
        docsUrl: POSTGRESQL_DOCS,
      },
      {
        key: "metadata.cache-ttl",
        group: "Cache",
        file: "config",
        type: "duration",
        default: "0s",
        help: "How long to cache table/schema metadata before refreshing.",
        docsUrl: POSTGRESQL_DOCS,
      },
    ],
  },
  {
    name: "sqlserver",
    label: "SQL Server",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the SQL Server instance.",
        docsUrl: SQLSERVER_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to SQL Server with.",
        docsUrl: SQLSERVER_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to SQL Server with.",
        docsUrl: SQLSERVER_DOCS,
      },
      {
        key: "sqlserver.snapshot-isolation.disabled",
        group: "Behavior",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Disable use of SNAPSHOT transaction isolation for queries.",
        docsUrl: SQLSERVER_DOCS,
      },
    ],
  },
  {
    name: "clickhouse",
    label: "ClickHouse",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the ClickHouse server.",
        docsUrl: CLICKHOUSE_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to ClickHouse with.",
        docsUrl: CLICKHOUSE_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to ClickHouse with.",
        docsUrl: CLICKHOUSE_DOCS,
      },
      {
        key: "clickhouse.map-string-as-varchar",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Map ClickHouse String columns to unbounded VARCHAR instead of VARBINARY.",
        docsUrl: CLICKHOUSE_DOCS,
      },
    ],
  },
  {
    name: "iceberg",
    label: "Iceberg",
    props: [
      {
        key: "iceberg.catalog.type",
        group: "Catalog",
        file: "config",
        type: "enum",
        enumOptions: ["hive_metastore", "glue", "jdbc", "rest", "nessie"],
        default: "hive_metastore",
        help: "Metastore implementation backing this Iceberg catalog.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "hive.metastore.uri",
        group: "Catalog",
        file: "config",
        type: "string",
        help: "Thrift URI of the Hive metastore (when catalog.type is hive_metastore).",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "iceberg.file-format",
        group: "Storage",
        file: "config",
        type: "enum",
        enumOptions: ["ORC", "PARQUET", "AVRO"],
        default: "PARQUET",
        help: "Default file format used when writing new Iceberg tables.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "iceberg.compression-codec",
        group: "Storage",
        file: "config",
        type: "string",
        default: "ZSTD",
        help: "Compression codec used when writing table data.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "fs.native-s3.enabled",
        group: "S3",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Enable the native S3 file system implementation.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "s3.endpoint",
        group: "S3",
        file: "config",
        type: "string",
        help: "S3-compatible endpoint URL, for non-AWS object stores.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "s3.region",
        group: "S3",
        file: "config",
        type: "string",
        help: "AWS region of the S3 bucket backing this catalog.",
        docsUrl: ICEBERG_DOCS,
      },
      {
        key: "s3.path-style-access",
        group: "S3",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Use path-style S3 URLs instead of virtual-hosted-style.",
        docsUrl: ICEBERG_DOCS,
      },
    ],
  },
  {
    name: "hive",
    label: "Hive",
    props: [
      {
        key: "hive.metastore.uri",
        group: "Metastore",
        file: "config",
        type: "string",
        help: "Thrift URI of the Hive metastore.",
        docsUrl: HIVE_DOCS,
      },
      {
        key: "hive.metastore",
        group: "Metastore",
        file: "config",
        type: "enum",
        enumOptions: ["thrift", "glue"],
        default: "thrift",
        help: "Metastore implementation to use.",
        docsUrl: HIVE_DOCS,
      },
      {
        key: "fs.native-s3.enabled",
        group: "S3",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Enable the native S3 file system implementation.",
        docsUrl: HIVE_DOCS,
      },
      {
        key: "s3.endpoint",
        group: "S3",
        file: "config",
        type: "string",
        help: "S3-compatible endpoint URL, for non-AWS object stores.",
        docsUrl: HIVE_DOCS,
      },
      {
        key: "s3.region",
        group: "S3",
        file: "config",
        type: "string",
        help: "AWS region of the S3 bucket backing this catalog.",
        docsUrl: HIVE_DOCS,
      },
      {
        key: "hive.non-managed-table-writes-enabled",
        group: "Behavior",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Allow writes to external (non-managed) Hive tables.",
        docsUrl: HIVE_DOCS,
      },
    ],
  },
  {
    name: "delta_lake",
    label: "Delta Lake",
    props: [
      {
        key: "hive.metastore.uri",
        group: "Metastore",
        file: "config",
        type: "string",
        help: "Thrift URI of the Hive metastore.",
        docsUrl: DELTA_LAKE_DOCS,
      },
      {
        key: "fs.native-s3.enabled",
        group: "S3",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Enable the native S3 file system implementation.",
        docsUrl: DELTA_LAKE_DOCS,
      },
      {
        key: "s3.endpoint",
        group: "S3",
        file: "config",
        type: "string",
        help: "S3-compatible endpoint URL, for non-AWS object stores.",
        docsUrl: DELTA_LAKE_DOCS,
      },
      {
        key: "s3.region",
        group: "S3",
        file: "config",
        type: "string",
        help: "AWS region of the S3 bucket backing this catalog.",
        docsUrl: DELTA_LAKE_DOCS,
      },
      {
        key: "delta.enable-non-concurrent-writes",
        group: "Behavior",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Allow writes from Trino when other engines may write concurrently.",
        docsUrl: DELTA_LAKE_DOCS,
      },
    ],
  },
  {
    name: "kafka",
    label: "Kafka",
    props: [
      {
        key: "kafka.nodes",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Comma-separated list of Kafka broker hostname:port pairs.",
        docsUrl: KAFKA_DOCS,
      },
      {
        key: "kafka.table-names",
        group: "Tables",
        file: "config",
        type: "string",
        help: "Comma-separated list of Kafka topics exposed as tables.",
        docsUrl: KAFKA_DOCS,
      },
      {
        key: "kafka.default-schema",
        group: "Tables",
        file: "config",
        type: "string",
        help: "Schema name used for tables that don't specify one.",
        docsUrl: KAFKA_DOCS,
      },
      {
        key: "kafka.hide-internal-columns",
        group: "Tables",
        file: "config",
        type: "boolean",
        default: "true",
        help: "Hide internal Kafka columns (partition, offset, etc.) from DESCRIBE.",
        docsUrl: KAFKA_DOCS,
      },
    ],
  },
  {
    name: "mongodb",
    label: "MongoDB",
    props: [
      {
        key: "mongodb.connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "MongoDB connection URL, e.g. mongodb://user:pass@host:27017.",
        docsUrl: MONGODB_DOCS,
      },
      {
        key: "mongodb.case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match database and collection names case-insensitively.",
        docsUrl: MONGODB_DOCS,
      },
      {
        key: "mongodb.schema-collection",
        group: "Mapping",
        file: "config",
        type: "string",
        default: "_schema",
        help: "Name of the collection storing explicit table schema definitions.",
        docsUrl: MONGODB_DOCS,
      },
    ],
  },
  {
    name: "hudi",
    label: "Hudi",
    props: [],
  },
  {
    name: "oracle",
    label: "Oracle",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Oracle server.",
        docsUrl: ORACLE_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to Oracle with.",
        docsUrl: ORACLE_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to Oracle with.",
        docsUrl: ORACLE_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: ORACLE_DOCS,
      },
    ],
  },
  {
    name: "redshift",
    label: "Redshift",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Redshift cluster.",
        docsUrl: REDSHIFT_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to Redshift with.",
        docsUrl: REDSHIFT_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to Redshift with.",
        docsUrl: REDSHIFT_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: REDSHIFT_DOCS,
      },
    ],
  },
  {
    name: "mariadb",
    label: "MariaDB",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the MariaDB server.",
        docsUrl: MARIADB_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to MariaDB with.",
        docsUrl: MARIADB_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to MariaDB with.",
        docsUrl: MARIADB_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: MARIADB_DOCS,
      },
    ],
  },
  {
    name: "singlestore",
    label: "SingleStore",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the SingleStore server.",
        docsUrl: SINGLESTORE_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to SingleStore with.",
        docsUrl: SINGLESTORE_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to SingleStore with.",
        docsUrl: SINGLESTORE_DOCS,
      },
      {
        key: "case-insensitive-name-matching",
        group: "Mapping",
        file: "config",
        type: "boolean",
        default: "false",
        help: "Match schema and table names case-insensitively.",
        docsUrl: SINGLESTORE_DOCS,
      },
    ],
  },
  {
    name: "snowflake",
    label: "Snowflake",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Snowflake account.",
        docsUrl: SNOWFLAKE_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to Snowflake with.",
        docsUrl: SNOWFLAKE_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to Snowflake with.",
        docsUrl: SNOWFLAKE_DOCS,
      },
    ],
  },
  {
    name: "druid",
    label: "Druid",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Druid broker.",
        docsUrl: DRUID_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to Druid with.",
        docsUrl: DRUID_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to Druid with.",
        docsUrl: DRUID_DOCS,
      },
    ],
  },
  {
    name: "ignite",
    label: "Ignite",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Ignite cluster.",
        docsUrl: IGNITE_DOCS,
      },
      {
        key: "connection-user",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Username to connect to Ignite with.",
        docsUrl: IGNITE_DOCS,
      },
      {
        key: "connection-password",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Password to connect to Ignite with.",
        docsUrl: IGNITE_DOCS,
      },
    ],
  },
  {
    name: "phoenix5",
    label: "Phoenix",
    props: [
      {
        key: "connection-url",
        group: "Connection",
        file: "config",
        type: "string",
        help: "JDBC connection URL for the Phoenix cluster (zookeeper quorum).",
        docsUrl: PHOENIX5_DOCS,
      },
    ],
  },
  {
    name: "cassandra",
    label: "Cassandra",
    props: [
      {
        key: "cassandra.contact-points",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Comma-separated list of Cassandra contact-point hostnames.",
        docsUrl: CASSANDRA_DOCS,
      },
      {
        key: "cassandra.load-policy.dc-aware.local-dc",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Name of the local Cassandra datacenter to prefer for query routing.",
        docsUrl: CASSANDRA_DOCS,
      },
    ],
  },
  {
    name: "elasticsearch",
    label: "Elasticsearch",
    props: [
      {
        key: "elasticsearch.host",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Hostname of the Elasticsearch cluster to connect to.",
        docsUrl: ELASTICSEARCH_DOCS,
      },
      {
        key: "elasticsearch.port",
        group: "Connection",
        file: "config",
        type: "number",
        default: "9200",
        help: "Port of the Elasticsearch cluster to connect to.",
        docsUrl: ELASTICSEARCH_DOCS,
      },
      {
        key: "elasticsearch.default-schema-name",
        group: "Mapping",
        file: "config",
        type: "string",
        default: "default",
        help: "Schema name under which Elasticsearch indexes are exposed as tables.",
        docsUrl: ELASTICSEARCH_DOCS,
      },
    ],
  },
  {
    name: "opensearch",
    label: "OpenSearch",
    props: [],
  },
  {
    name: "redis",
    label: "Redis",
    props: [
      {
        key: "redis.table-names",
        group: "Tables",
        file: "config",
        type: "string",
        help: "Comma-separated list of Redis key groups exposed as tables.",
        docsUrl: REDIS_DOCS,
      },
      {
        key: "redis.nodes",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Comma-separated list of Redis node hostname:port pairs.",
        docsUrl: REDIS_DOCS,
      },
    ],
  },
  {
    name: "kinesis",
    label: "Kinesis",
    props: [],
  },
  {
    name: "prometheus",
    label: "Prometheus",
    props: [
      {
        key: "prometheus.uri",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Base URI of the Prometheus server to query.",
        docsUrl: PROMETHEUS_DOCS,
      },
      {
        key: "prometheus.query.chunk.size.duration",
        group: "Query",
        file: "config",
        type: "duration",
        default: "1d",
        help: "Time-range chunk size used when splitting up Prometheus queries.",
        docsUrl: PROMETHEUS_DOCS,
      },
      {
        key: "prometheus.max.query.range.duration",
        group: "Query",
        file: "config",
        type: "duration",
        default: "21d",
        help: "Maximum time range permitted for a single Prometheus query.",
        docsUrl: PROMETHEUS_DOCS,
      },
    ],
  },
  {
    name: "tpch",
    label: "TPC-H",
    props: [],
  },
  {
    name: "tpcds",
    label: "TPC-DS",
    props: [],
  },
  {
    name: "blackhole",
    label: "Blackhole",
    props: [],
  },
  {
    name: "memory",
    label: "Memory",
    props: [
      {
        key: "memory.max-data-per-node",
        group: "Storage",
        file: "config",
        type: "size",
        default: "128MB",
        help: "Maximum amount of data the memory connector may hold per worker node.",
        docsUrl: MEMORY_DOCS,
      },
    ],
  },
  {
    name: "jmx",
    label: "JMX",
    props: [],
  },
  {
    name: "bigquery",
    label: "BigQuery",
    props: [
      {
        key: "bigquery.project-id",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Google Cloud project ID that hosts the BigQuery data to query.",
        docsUrl: BIGQUERY_DOCS,
      },
      {
        key: "bigquery.credentials-key",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Base64-encoded Google Cloud service account credentials.",
        docsUrl: BIGQUERY_DOCS,
      },
    ],
  },
  {
    name: "googlesheets",
    label: "Google Sheets",
    props: [
      {
        key: "gsheets.credentials-path",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Path to the Google API service account JSON key file.",
        docsUrl: GOOGLESHEETS_DOCS,
      },
      {
        key: "gsheets.metadata-sheet-id",
        group: "Connection",
        file: "config",
        type: "string",
        help: "Sheet ID of the metadata sheet mapping table names to sheet URLs.",
        docsUrl: GOOGLESHEETS_DOCS,
      },
    ],
  },
];

export const connectorByName: Record<string, Connector> = Object.fromEntries(
  connectors.map((c) => [c.name, c]),
);
