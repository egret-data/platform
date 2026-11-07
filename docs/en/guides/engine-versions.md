# Engine versions: what egret pins and how to move an engine

egret reads each provisioned engine's Ranger plugin version from the engine's exact image, never from its type. The same StarRocks tag can ship different plugins on different days, so a type-only guess would evaluate access with a plugin nobody measured.

The control plane shows the result for every engine: the image it runs, the Ranger plugin version that image ships, and whether egret's evaluator is modelled for that (type, version) pair. When it is not, the engine page shows a warning that says how to fix it.

## What egret pins

These four images are the ones egret has measured: the plugin jar was read out of each image. Defaults for new engines are pinned by digest.

| Engine | Image | Digest | Ranger plugin |
|---|---|---|---|
| Trino | `trinodb/trino:483` | `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.4` | `sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.6` | `sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` | 2.9.0 |
| StarRocks | `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.1` (egret's patched 4.1.7, patches 0001-0002; superseded) | `sha256:da948dad19e98a5a43b0073ec48f87c9e7429246ce4581a000c0955c742af250` | 2.9.0 |
| StarRocks | `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.3` (egret's patched 4.1.7, patches 0001-0007; superseded) | `sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0` | 2.9.0 |
| StarRocks | `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4` (egret's patched 4.1.7, patches 0001-0009; the default) | `sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5` | 2.9.0 |

A new engine defaults to Trino 483 and StarRocks 4.1.7: the FE is egret's patched image, the CN the official `starrocks/cn-ubuntu:4.1.7` (each pinned by digest). See [egret's patched StarRocks image](#egrets-patched-starrocks-image). The modelled (type, plugin version) pairs are Trino 2.8.0, StarRocks 2.8.0 and StarRocks 2.9.0. `GET /api/control-plane/engines/modelled-ranger-plugins` lists them, and the register form for an external engine offers only the pairs for the chosen type, plus "Other…".

A tag with a digest matches only if the digest is the measured one. A tag without a digest matches the measured tag.

## egret's patched StarRocks image

The official StarRocks images have holes that no policy egret compiles can close. Each was measured on 4.1.7 with a Ranger-enforced user under a column mask and a row filter, and one with no grant:

- **Iceberg metadata tables are not checked.** StarRocks serves `t$files`, `t$partitions`, `t$manifests`, `t$snapshots`, `t$history`, `t$refs`, `t$metadata_log_entries` and the other metadata tables of an Iceberg table without asking Ranger. Anyone who can log in reads, for every table, row counts, partition values, data file paths and each column's lowest and highest value, whatever the grants, masks, row filters and classifications say.
- **Prepared statements skip masks and row filters.** A statement run through `PREPARE`/`EXECUTE` or the MySQL binary protocol (`COM_STMT_PREPARE`/`COM_STMT_EXECUTE`) gets no mask and no row filter. egret's StarRocks connector runs Data API endpoints with parameters this way, so on an official image such an endpoint returns every row and column in the clear (upstream issue [#78176](https://github.com/StarRocks/starrocks/issues/78176)). `PIVOT` skips them the same way.
- **The HTTP SQL API and Arrow Flight SQL skip masks and row filters.** A query sent to the FE's HTTP port (`/api/v1/catalogs/<catalog>/databases/<db>/sql`, port 8030, which the StarRocks Ingress exposes) or to Arrow Flight SQL returns every row and column in the clear (upstream [#78175](https://github.com/StarRocks/starrocks/issues/78175)).
- **`FILES()` has no privilege.** Any user reads and writes object storage through it, past every grant, mask and row filter, and with `aws.s3.use_instance_profile` through the BE's own cloud identity (upstream [#80233](https://github.com/StarRocks/starrocks/issues/80233)).
- **A materialized view stores data past its creator's policy.** Its refresh reads the base tables without the creator's masks and row filters, so a masked and filtered user who may create one reads the whole table through it.
- **`EXPLAIN COSTS` shows the statistics of hidden rows.** It prints each column's lowest and highest value and distinct count over the whole table, so a user under a row filter learns the range of the rows the filter hides.
- **Loads and exports run with the cluster's identity.** `BROKER LOAD`, `EXPORT`, `SELECT ... INTO OUTFILE` and `ENGINE = file` tables that name no keys of their own fall back to the node's identity (an instance profile, the AWS default credential chain, a managed identity). Any user, even one with no grant, can write query results to any bucket that identity reaches.
- **`SELECT *` under a mask or row filter returns `_file` and `_pos`** on an Iceberg table (KI-337), and **`SHOW CREATE CATALOG` needs no privilege**: any user reads every catalog's endpoints and the ends of its keys.

`ghcr.io/egret-data/egret-starrocks-fe` closes all of them from `4.1.7-egret.4` (`4.1.7-egret.3` left `EXPLAIN COSTS` and loads with the cluster's identity open, `4.1.7-egret.1` closed the first two holes only). It is the official `starrocks/fe-ubuntu` image of the same release with two FE jars rebuilt from the release tag plus egret's patches; nothing else differs, and it ships the official Ranger plugin jars. On it, `FILES()` needs the OPERATE privilege on SYSTEM, and so do `BROKER LOAD`, `EXPORT`, `SELECT ... INTO OUTFILE` and `ENGINE = file` tables unless they carry keys of their own for an object store (error 5214). Creating a materialized view over a table where a mask or row filter applies to the creator is refused, and so is `EXPLAIN COSTS` over such a table (error 5213; the other `EXPLAIN` levels still work). The image also ships StarRocks' AuditLoader plugin, unused until egret installs it. The source, the patches, the limits it does not change and the security end-to-end test that every image passes before it is published are in [github.com/egret-data/egret-starrocks](https://github.com/egret-data/egret-starrocks). Compute nodes use the official `starrocks/cn-ubuntu` image of the same release: no patch touches them.

The engine page shows the warning about these holes for a StarRocks engine bound to a profile unless its FE image is `egret-starrocks-fe` `4.1.7-egret.4` or later, so engines on `4.1.7-egret.3` show it again. An external engine always shows it, because egret does not know which image it runs.

To move an existing StarRocks engine onto the patched image, update it as in [How to pin](#how-to-pin) with `feImage` `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4@sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5` and `cnImage` `starrocks/cn-ubuntu:4.1.7@sha256:39f7320127c9924abb1f1ce0ba0583ab11989cbe5bd1bb6274ada6f89ba5740d`. From 4.1.4 or 4.1.6 this is an upgrade (to 4.1.7), never a downgrade; an engine already on `4.1.7-egret.1` or `4.1.7-egret.3` moves the same way.

## Ranger plugin 2.8.0 is the floor (Trino 470 is dropped)

egret supports Ranger plugin 2.8.0 and later. Plugin 2.5.0, which `trinodb/trino:470` ships, cannot send the per-engine credential egret's Ranger surface requires, so it is no longer supported. Measured with the real jars: with `policy.rest.client.username` and `.password` set, 2.8.0 and 2.9.0 send `Authorization: Basic …` to the secure download path, while 2.5.0 ignores both properties and sends no header ([measurements](../plans/2026-10-20-ranger-surface-auth-measurements.md), "Floor"). The credential and the surface are described in [ranger-surface.md](ranger-surface.md).

- Creating an engine on `trinodb/trino:470`, or updating one onto it, is refused with a 400.
- Registering an external engine that declares a plugin older than 2.8.0 is refused with a 400 ("Ranger plugin 2.5.0 is not supported: egret requires 2.8.0 or later").
- An engine that already runs Trino 470 stays readable but is reported as not modelled and unsupported; it is not migrated. Its plugin cannot authenticate, so it gets 401 on every poll: it keeps enforcing the policies it has cached, receives no changes, and denies everything after a restart without a cache. Upgrade it with `PUT /api/control-plane/engines/{id}` and `imageVersion: "483"`, or delete it.

The supported (engine, plugin) pairs are therefore Trino 2.8.0, StarRocks 2.8.0 and StarRocks 2.9.0, and the Ranger gates (`just test-ranger-gates`) run the real 2.8.0 and 2.9.0 jars only.

A StarRocks engine name is limited to 48 characters (400 at create and on every apply of a bound StarRocks engine), because the operator names each FE volume after the Secret or ConfigMap it mounts plus a 4-character hash, and a volume name is at most 63 characters.

## What StarRocks 4.1.6 changes

StarRocks 4.1.6 ships Ranger plugin 2.9.0 and enforces view privileges that 4.1.4 did not. The measurement ([`docs/en/plans/2026-10-13-view-clearance-measurement.md`](../plans/2026-10-13-view-clearance-measurement.md), round 2, Question B) found, on the same objects and the same users:

> F-1 and F-2 are fixed in StarRocks 4.1.6 (`ranger-plugins-common` 2.9.0), by upstream commit `e0d97ad2659` / backport `2a36c2d0715`, absent from 4.1.4.

In practice, on 4.1.6:

- A user with no grant can no longer read a plain view. On 4.1.4 that user read real rows (F-1); on 4.1.6 the read is refused with `ERROR 5204 ... SELECT ... VIEW <name>`.
- `SECURITY INVOKER` is now enforced. On 4.1.4 the clause was accepted, stored and printed but not enforced under Ranger (F-2); on 4.1.6 the base-table check runs as the reader.

**Expect denials where views were read without grants.** After moving a StarRocks engine from 4.1.4 to 4.1.6, a reader who relied on a view without holding a grant on it will start to get `ERROR 5204`. Grant the view, or the base columns for an `INVOKER` view, to the readers who should have it.

Not changed by 4.1.6, per the same measurement: a plain view is still an access boundary and does not consult a deny on its base table, and a materialized view built by the reader still carries data past a mask or deny.

## Engines on a floating tag

An engine whose image is a floating tag (`starrocks/fe-ubuntu:4.1-latest`, `latest`, a tag egret has not measured) is reported as **not modelled**. egret cannot tell which plugin it runs: `starrocks/fe-ubuntu:4.1-latest` shipped plugin 2.8.0 on 2026-09-28 and 2.9.0 from 2026-10-01.

For such an engine:

- `explain` refuses to answer for its profile, and a migration into its profile is refused, because neither can be proved on a plugin version nobody measured.
- Enforcement is unchanged. Policies keep compiling and downloading to the engine's plugin, and it keeps enforcing them.
- The engine page shows the reason and the fix in a warning, and the API returns it as `rangerPluginVersionNote`.

An external engine is judged by the version its operator declared, and is not modelled when that pair is not in the list above.

## How to pin

Send `PUT /api/control-plane/engines/{id}` with the full image. The body is the engine's create body: `name`, `coordinatorReplicas`, `workerReplicas`, `cpuMin`, `cpuMax` and `memory` are required, and everything else may be omitted. An omitted image keeps the one the engine already runs, so a resize never moves an engine to a newer default.

Trino, `imageVersion` is the bare tag and optionally the digest:

```json
{
  "name": "analytics-trino",
  "coordinatorReplicas": 1,
  "workerReplicas": 2,
  "cpuMin": "1",
  "cpuMax": "2",
  "memory": "4Gi",
  "imageVersion": "483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
}
```

egret stores this as `spec.imageVersion` and shows it as `trinodb/trino:483@sha256:...`.

**A Trino image override wins over `imageVersion`.** When the engine's per-role config sets `trinoConfig.image` (the "Image" field on the engine's config page), the provisioner runs that image and ignores `imageVersion`. egret reads the same field: the engine page, the API's `image` and the plugin resolution all use the override as written, and the exact-image table above decides what it means. So:

- `trinoConfig.image: "trinodb/trino:483"` (or with the measured digest) resolves to plugin 2.8.0;
- `trinoConfig.image: "trinodb/trino:483@sha256:<any other digest>"` is not modelled;
- a custom or mirrored image (`registry.corp/trino-custom:x`) is not modelled, and the note names it, even when `imageVersion` is 483. egret cannot tell which plugin a rebuilt image carries.

A blank override counts as no override, in both the provisioner and the report. To pin an engine that has an override, either set the override to a measured image or clear it and pin `imageVersion`.

StarRocks, the body's `starrocks` object takes `feImage` and `cnImage`; egret stores them as `spec.starrocksConfig.feImage` and `spec.starrocksConfig.cnImage`. Plugin resolution reads `feImage`:

```json
{
  "name": "analytics-sr",
  "coordinatorReplicas": 1,
  "workerReplicas": 1,
  "cpuMin": "1",
  "cpuMax": "2",
  "memory": "4Gi",
  "starrocks": {
    "feImage": "starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31",
    "cnImage": "starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b"
  }
}
```

**An image change restarts the engine's pods.** There is no separate restart step. The update route stores the new spec, rebuilds the provisioning request from it and applies it to the cluster through the provisioner (the same call as any update). Trino has no hot reload, so the provisioner rolls the pods onto the new image; for StarRocks the operator reconciles the FE and CN workloads onto the new images. Expect the engine to be unavailable while its pods roll. If the cluster refuses the apply, egret puts the previous spec back and the request fails with the cluster's reason. After the apply, every assigned workspace's connection is re-materialised.

The engine page and `GET /api/control-plane/engines/{id}` then report the new `image`, the plugin version it ships and `rangerPluginVersionModelled: true`.

An external engine is never updated this way: egret does not run its cluster. Declare the plugin version you installed with `PATCH /api/control-plane/engines/{id}/external`.

### Pinning a StarRocks engine that ran `4.1-latest`

Migration 0185 wrote `starrocks/fe-ubuntu:4.1-latest` and `starrocks/cn-ubuntu:4.1-latest` into every provisioned StarRocks engine that named no image, because that floating tag is what those engines ran. egret reports such an engine as not modelled: **`explain` and migrations for its profile refuse until you pin it.** Enforcement is unchanged in the meantime.

Do not pick the pin from the table by habit. `4.1-latest` has resolved to 4.1.6 since 2026-10-01, so an engine whose pods pulled the image on or after that date already runs 4.1.6, and pinning it to 4.1.4 would be a **downgrade**. StarRocks does not support downgrading FE metadata: an FE that has written 4.1.6 metadata may fail to start on 4.1.4, or start on metadata it does not fully understand.

Read what the FE pod actually runs, then pin to the matching known image:

```bash
kubectl -n <namespace> get pod <engine-name>-fe-0 -o jsonpath='{..imageID}'
```

The output ends in the running digest (`...@sha256:<digest>`). Compare it with the table above:

- `sha256:d07aa750...` is 4.1.6: pin `starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31`, and the CN image to `starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b`.
- `sha256:0b909948...` is 4.1.4: pin the 4.1.4 FE image. Moving it to 4.1.6 afterwards is an upgrade, with the view-privilege changes described above.
- Any other digest is an image egret has not measured. Pin it to the known image of the same StarRocks version only if you can confirm that version; otherwise move it forward to 4.1.6, never back.

Check the CN pods the same way (`<engine-name>-cn-0`) and pin the CN image to the same version as the FE.
