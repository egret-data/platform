# Publish egret's images and Helm chart to GHCR (2026-10-22)

Before this change nothing was published: every deployment built `egret-*:dev` locally.

## What changes

- `.github/workflows/images.yml`: builds seven images (`egret-backend`, `egret-backend-demo`,
  `egret-frontend`, `egret-renderer`, `egret-apps`, `egret-plugin-host`, `egret-task-helper`, one per
  Dockerfile plus the backend's `demo` stage) for amd64 and arm64 on native runners, pushes by digest,
  merges a manifest list per image, attests provenance, scans with Trivy, and on a `v*` tag pushes the
  Helm chart to `oci://ghcr.io/egret-data/charts/egret`. Pull requests build and scan only.
- `.github/workflows/ghcr-cleanup.yml`: weekly retention (untagged, `sha-*` beyond 20).
- Chart: images default to `ghcr.io/egret-data/<image>` at the chart's `appVersion` (`edge` in the
  tree, the release version when published). `deploy/k8s/values-local-images.yaml` restores the local
  `:dev` images and the justfile's kind recipes pass it, so their rendered manifests are unchanged.
- Demo: `deploy/docker-compose.demo.yml` is unchanged (still builds; `up --build --no-deps egret`
  works as before). `deploy/docker-compose.demo.ghcr.yml` + `just demo-up-ghcr [tag]` pull instead.
- Docs: `docs/en/guides/releasing.md` (+ `vi`), demo-stack guide, chart README.

## Decisions

- Image names keep the existing local names with the `ghcr.io/egret-data/` prefix (`egret-backend`,
  not `backend`), so a package name says what it is on the org page and the chart's values keep
  their shape.
- `egret-backend-demo` is published too: the demo stack runs the `demo` stage, and pulling needs it.
  It is built in the backend's job, so the Rust builder stage is compiled once per architecture.
- Rust caching: cargo-chef does not work with this workspace (Dockerfile header); the existing cache
  mounts do not persist on GitHub runners. `type=gha` layer cache with `mode=min` for the three large
  images (the shared 10 GB cache also holds `ci.yml`'s Rust cache), `mode=max` for the small ones;
  only publishing runs write it.
- Trivy: CRITICAL and HIGH (fixed or not) go to code scanning and never block. A CRITICAL with a
  fixed version fails the build leg on a version tag (stopping that release); on main and PRs the
  step may fail without failing the job, so `edge` keeps flowing. Measured on the first PR run:
  three images already carry one each (renderer `tar` npm, apps `certifi`, plugin-host
  `netty-handler`), so blocking main would have stopped every publish from day one.
- Retention uses `dataaxiom/ghcr-cleanup-action`: deleting "untagged versions" through the API
  would delete the platform manifests of every multi-arch image.
- The chart's string-valued images (`apps.pod.image`, `workflow.pluginHost.image`,
  `workflow.tasks.helperImage`) keep their shape; empty means the GHCR image at `appVersion`.
