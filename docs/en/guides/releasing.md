# Releasing and published images

egret's container images and its Helm chart are built by [`.github/workflows/images.yml`](../../../.github/workflows/images.yml) and published to the GitHub Container Registry (GHCR) under `ghcr.io/egret-data`.

## What gets published

| Image | Built from | Used by |
|---|---|---|
| `ghcr.io/egret-data/egret-backend` | `deploy/docker/backend.Dockerfile` (last stage) | the Helm chart's backend and init job |
| `ghcr.io/egret-data/egret-backend-demo` | `deploy/docker/backend.Dockerfile`, stage `demo` (egret plus the Data Apps payload) | the Docker demo stack only |
| `ghcr.io/egret-data/egret-frontend` | `frontend/Dockerfile` | the chart and the demo stack |
| `ghcr.io/egret-data/egret-renderer` | `renderer/Dockerfile` | the chart and the demo stack |
| `ghcr.io/egret-data/egret-apps` | `apps-image/Dockerfile` | Data Apps pods (`apps.pod.launcher=kubernetes`) |
| `ghcr.io/egret-data/egret-plugin-host` | `deploy/docker/plugin-host.Dockerfile` | Kestra plugin tasks (`workflow.containerRunner=kubernetes`) |
| `ghcr.io/egret-data/egret-task-helper` | `deploy/docker/task-helper.Dockerfile` | the helper containers of workflow task pods |

Every image is a multi-arch manifest list for `linux/amd64` and `linux/arm64`, each built natively on its own runner. The chart is `oci://ghcr.io/egret-data/charts/egret`.

| Event | Image tags | Chart |
|---|---|---|
| Push of tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest` | version `X.Y.Z`, `appVersion` `X.Y.Z` |
| Push of a pre-release tag such as `v1.3.0-rc.1` | `1.3.0-rc.1` only (no `X.Y`, no `latest`) | version `1.3.0-rc.1` |
| Push to `main` | `edge`, `sha-<7 hex>` | not published |
| Pull request that changes a Dockerfile, a `.dockerignore` or the workflow | nothing: every image is built for both architectures and scanned, the chart is linted and packaged | not published |
| Manual run (Actions, "images", Run workflow) | on `main` or a tag as the push above; on any other branch build only | as above |

The chart in the tree has `appVersion: edge`, so `helm install` from a checkout of `main` deploys the `edge` images. A released chart deploys the images of its own release. See [the chart's README](../../../deploy/helm/egret/README.md#images) for overriding images and for the local kind builds.

Each published image also carries:

- OCI labels and index annotations: `org.opencontainers.image.source` (this repository, which links the package to it), `revision` (the commit), `version`, `licenses` (`Apache-2.0`), `created`, `title`.
- A build provenance attestation (`actions/attest-build-provenance`, also pushed to the registry) and BuildKit's own SBOM and provenance attestations in the manifest list.

Verify an image:

```bash
gh attestation verify oci://ghcr.io/egret-data/egret-backend:1.2.0 -R egret-data/platform
docker buildx imagetools inspect ghcr.io/egret-data/egret-backend:1.2.0 --format '{{ json .SBOM }}'
```

## Cut a release

1. Make sure `main` is green, and that the last `images` run on `main` succeeded (it builds what the tag will build).
2. Tag the commit on `main` and push the tag:

   ```bash
   git fetch origin
   git tag -a v1.2.0 origin/main -m "egret 1.2.0"
   git push origin v1.2.0
   ```

3. Watch the `images` run for the tag. It builds 14 image legs (7 images, 2 architectures) in parallel; the Rust image takes the longest. `merge` then tags each image and attests it, and `chart` pushes the chart last, only when every image of the release is published.
4. Write the release notes (GitHub, Releases, Draft a new release, choose the tag). The workflow does not create a GitHub release.

The chart's `version` and `appVersion` come from the tag; `Chart.yaml` in the tree is not edited for a release.

A failed release: fix on `main`, then tag the next patch version. Do not move a pushed tag; if you must re-run the same tag, re-run the failed jobs of its `images` run.

## Make the packages public (once, by an org admin)

GHCR creates an organisation's packages **private** on their first push, even from a public repository. After the first publish, an owner of `egret-data`:

1. Opens each package (organisation page, Packages): the seven images and `charts/egret`.
2. Package settings, Danger Zone, Change visibility, **Public**.
3. Checks under "Manage Actions access" that `egret-data/platform` has the **Admin** role (the first push from the workflow grants it; the weekly cleanup needs it to delete versions).

Also once, in the organisation settings: Packages must allow public packages (Settings, Packages, Package creation), and if the organisation restricts which repositories' workflows may publish packages, `egret-data/platform` must be allowed. The repository's default workflow permissions can stay read-only: the workflow asks for `packages: write` only on the jobs that push. Code scanning must be enabled on the repository for the Trivy reports to appear.

## Vulnerability scan threshold

Trivy scans each image (the `amd64` build) in the `build` job:

- **Reported, never blocking:** every `CRITICAL` and `HIGH` finding, with or without a fixed version, is uploaded to code scanning (Security, Code scanning, category `trivy-<image>`).
- **Blocking:** a `CRITICAL` finding that has a fixed version fails the job. On a push this stops `merge`, so nothing of that build is tagged (all images of a version are published together or not at all). Fix it by bumping the base image or the dependency.

Unfixed findings do not block, because there is nothing to upgrade to; they stay visible in code scanning.

## Build cache

The Rust image compiles the whole workspace in the builder stage. Its BuildKit cache mounts (cargo registry and `target/`) do not survive between GitHub runs, so each push to `main` compiles from scratch (cargo-chef does not work with this workspace's vendored `third_party/` crates, see the Dockerfile's header). The GitHub Actions cache (`type=gha`, one scope per image and architecture, written only by publishing runs) stores image layers: with `mode=min` for the backend, frontend and renderer (the large build stages are not exported, to stay within the repository's 10 GB cache, which `ci.yml`'s Rust cache also uses) and `mode=max` for the small images. It turns a re-run of an unchanged commit, such as a tag on a commit `main` already built, into a cache hit.

## Retention

[`.github/workflows/ghcr-cleanup.yml`](../../../.github/workflows/ghcr-cleanup.yml) runs every Monday and, per image package, deletes untagged versions and `sha-*` images beyond the newest 20. It never deletes `latest`, `edge` or a version tag (any tag with a dot), never anything younger than two days (a publish in progress), and never the chart. Platform manifests and attestations belong to their image and go only with it. A manual run ("ghcr-cleanup", Run workflow) is a dry run unless you untick "dry-run".
