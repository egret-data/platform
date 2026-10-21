# Engine versions: what egret pins and how to move an engine

egret reads each provisioned engine's Ranger plugin version from the engine's exact image, never from its type. The same StarRocks tag can ship different plugins on different days, so a type-only guess would evaluate access with a plugin nobody measured.

The control plane shows the result for every engine: the image it runs, the Ranger plugin version that image ships, and whether egret's evaluator is modelled for that (type, version) pair. When it is not, the engine page shows a warning that says how to fix it.

## What egret pins

These three images are the ones egret has measured: the plugin jar was read out of each image. Defaults for new engines are pinned by digest.

| Engine | Image | Digest | Ranger plugin |
|---|---|---|---|
| Trino | `trinodb/trino:483` | `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.4` | `sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.6` | `sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` | 2.9.0 |

A new engine defaults to Trino 483 and StarRocks 4.1.6 (FE and CN, each pinned by digest). The modelled (type, plugin version) pairs are Trino 2.8.0, StarRocks 2.8.0 and StarRocks 2.9.0. `GET /api/control-plane/engines/modelled-ranger-plugins` lists them, and the register form for an external engine offers only the pairs for the chosen type, plus "Other…".

A tag with a digest matches only if the digest is the measured one. A tag without a digest matches the measured tag.

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
