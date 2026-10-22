# Phát hành và image đã publish

> Bản tham khảo tiếng Việt. Bản gốc (canonical) là [docs/en/guides/releasing.md](../../en/guides/releasing.md); khi hai bản khác nhau, bản tiếng Anh thắng.

Image container và Helm chart của egret được [`.github/workflows/images.yml`](../../../.github/workflows/images.yml) build và publish lên GitHub Container Registry (GHCR) dưới `ghcr.io/egret-data`.

## Publish những gì

| Image | Build từ | Ai dùng |
|---|---|---|
| `ghcr.io/egret-data/egret-backend` | `deploy/docker/backend.Dockerfile` (stage cuối) | backend và init job của Helm chart |
| `ghcr.io/egret-data/egret-backend-demo` | `deploy/docker/backend.Dockerfile`, stage `demo` (egret cùng payload Data Apps) | chỉ demo stack trên Docker |
| `ghcr.io/egret-data/egret-frontend` | `frontend/Dockerfile` | chart và demo stack |
| `ghcr.io/egret-data/egret-renderer` | `renderer/Dockerfile` | chart và demo stack |
| `ghcr.io/egret-data/egret-apps` | `apps-image/Dockerfile` | pod Data Apps (`apps.pod.launcher=kubernetes`) |
| `ghcr.io/egret-data/egret-plugin-host` | `deploy/docker/plugin-host.Dockerfile` | task plugin Kestra (`workflow.containerRunner=kubernetes`) |
| `ghcr.io/egret-data/egret-task-helper` | `deploy/docker/task-helper.Dockerfile` | container helper của pod task workflow |

Mỗi image là một manifest list multi-arch cho `linux/amd64` và `linux/arm64`, mỗi kiến trúc build native trên runner riêng. Chart là `oci://ghcr.io/egret-data/charts/egret`.

| Sự kiện | Tag image | Chart |
|---|---|---|
| Push tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest` | version `X.Y.Z`, `appVersion` `X.Y.Z` |
| Push tag pre-release như `v1.3.0-rc.1` | chỉ `1.3.0-rc.1` (không `X.Y`, không `latest`) | version `1.3.0-rc.1` |
| Push lên `main` | `edge`, `sha-<7 hex>` | không publish |
| Pull request đổi một Dockerfile, một `.dockerignore` hoặc workflow | không gì cả: build mọi image cho cả hai kiến trúc và quét, chart được lint và package | không publish |
| Chạy tay (Actions, "images", Run workflow) | trên `main` hoặc một tag: như push ở trên; nhánh khác: chỉ build | như trên |

Chart trong cây có `appVersion: edge`, nên `helm install` từ checkout của `main` triển khai image `edge`. Chart đã phát hành triển khai image của đúng bản phát hành đó. Xem [README của chart](../../../deploy/helm/egret/README.md#images) để override image và dùng bản build local trên kind.

Mỗi image đã publish còn mang:

- Label OCI và annotation trên index: `org.opencontainers.image.source` (repository này, để liên kết package với nó), `revision` (commit), `version`, `licenses` (`Apache-2.0`), `created`, `title`.
- Một attestation build provenance (`actions/attest-build-provenance`, cũng được push lên registry) cùng attestation SBOM và provenance của BuildKit trong manifest list.

Kiểm tra một image:

```bash
gh attestation verify oci://ghcr.io/egret-data/egret-backend:1.2.0 -R egret-data/platform
docker buildx imagetools inspect ghcr.io/egret-data/egret-backend:1.2.0 --format '{{ json .SBOM }}'
```

## Cắt một bản phát hành

1. Bảo đảm `main` xanh và lần chạy `images` gần nhất trên `main` thành công (nó build đúng thứ tag sẽ build).
2. Gắn tag commit trên `main` và push tag:

   ```bash
   git fetch origin
   git tag -a v1.2.0 origin/main -m "egret 1.2.0"
   git push origin v1.2.0
   ```

3. Theo dõi lần chạy `images` của tag. Nó build song song 14 nhánh (7 image, 2 kiến trúc); image Rust lâu nhất. Sau đó `merge` gắn tag và attest từng image, và `chart` push chart sau cùng, chỉ khi mọi image của bản phát hành đã publish.
4. Viết release notes (GitHub, Releases, Draft a new release, chọn tag). Workflow không tạo GitHub release.

`version` và `appVersion` của chart lấy từ tag; không sửa `Chart.yaml` trong cây khi phát hành.

Phát hành hỏng: sửa trên `main`, rồi gắn tag bản patch kế tiếp. Không dời một tag đã push; nếu buộc phải chạy lại cùng tag, chạy lại các job hỏng của lần chạy `images` đó.

## Đặt package thành public (một lần, do admin của org)

GHCR tạo package của organisation ở chế độ **private** ở lần push đầu, kể cả từ repository public. Sau lần publish đầu, một owner của `egret-data`:

1. Mở từng package (trang organisation, Packages): bảy image và `charts/egret`.
2. Package settings, Danger Zone, Change visibility, **Public**.
3. Kiểm tra trong "Manage Actions access" rằng `egret-data/platform` có vai trò **Admin** (lần push đầu từ workflow cấp quyền này; cleanup hằng tuần cần nó để xoá version).

Cũng một lần, trong cài đặt organisation: Packages phải cho phép package public (Settings, Packages, Package creation), và nếu organisation giới hạn repository nào được publish package từ workflow, phải cho phép `egret-data/platform`. Quyền mặc định của workflow trong repository có thể giữ read-only: workflow chỉ xin `packages: write` ở các job push. Code scanning phải được bật trên repository để báo cáo Trivy hiện ra.

## Ngưỡng quét lỗ hổng

Trivy quét từng image (bản build `amd64`) trong job `build`:

- **Báo cáo, không bao giờ chặn:** mọi phát hiện `CRITICAL` và `HIGH`, có hay không có bản sửa, được upload lên code scanning (Security, Code scanning, category `trivy-<image>`).
- **Chặn:** một phát hiện `CRITICAL` CÓ bản sửa làm job hỏng. Khi push, điều này chặn `merge`, nên không gì của lần build đó được gắn tag (mọi image của một phiên bản được publish cùng nhau hoặc không). Sửa bằng cách nâng base image hoặc dependency.

Phát hiện chưa có bản sửa không chặn, vì không có gì để nâng lên; chúng vẫn hiện trong code scanning.

## Cache khi build

Image Rust biên dịch toàn bộ workspace trong stage builder. Cache mount của BuildKit (cargo registry và `target/`) không sống qua các lần chạy GitHub, nên mỗi lần push lên `main` biên dịch từ đầu (cargo-chef không chạy được với các crate vendored trong `third_party/` của workspace này, xem đầu Dockerfile). Cache của GitHub Actions (`type=gha`, một scope cho mỗi image và kiến trúc, chỉ các lần chạy publish mới ghi) lưu layer image: `mode=min` cho backend, frontend và renderer (không export các stage build lớn, để nằm trong 10 GB cache của repository, mà cache Rust của `ci.yml` cũng dùng) và `mode=max` cho các image nhỏ. Nó biến việc chạy lại một commit không đổi, như một tag trên commit mà `main` đã build, thành cache hit.

## Giữ lại bao lâu

[`.github/workflows/ghcr-cleanup.yml`](../../../.github/workflows/ghcr-cleanup.yml) chạy mỗi thứ Hai và, cho từng package image, xoá các version không có tag và các image `sha-*` ngoài 20 cái mới nhất. Nó không bao giờ xoá `latest`, `edge` hay một tag phiên bản (mọi tag có dấu chấm), không bao giờ xoá thứ gì mới hơn hai ngày (một lần publish đang chạy), và không bao giờ đụng chart. Manifest theo nền tảng và attestation thuộc về image của chúng và chỉ bị xoá cùng image. Chạy tay ("ghcr-cleanup", Run workflow) là dry run trừ khi bỏ chọn "dry-run".
