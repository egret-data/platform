# StarRocks Root Password (bịt root FE không mật khẩu) — Thiết kế

**Ngày:** 2026-07-17 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-root-password` (off `main`)
**Chương trình:** StarRocks hardening. Full live smoke phát hiện tài khoản `root` của FE StarRocks **không có mật khẩu trên :9030** — egret chưa từng set root password (chỉ user `egret_delegator` có). OK cho kind-dev, lỗ hổng thật ở prod. Slice này set root password (mint) qua cơ chế init in-cluster của operator.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-17-starrocks-root-password-design.md`.

## Vấn đề

FE StarRocks mới provision có `root@%` **không mật khẩu** (`SHOW ALL AUTHENTICATION` → `root | No | ... | NULL`). Ai reach được query port `:9030` là login root — toàn quyền cluster. Provisioning của egret không bịt (comment `engines.rs:2621`: "root với no password (dev-cluster default)").

## Mục tiêu

Mọi FE StarRocks egret provision được set **root password (mint)**, set **in-cluster** (chạy được ở dev VÀ prod — khác delegation bootstrap bị kẹt DNS egret-host→FE), lưu như egret secret. Kết nối bootstrap-FE của egret dùng password đã lưu.

Chứng minh live (in-cluster, verify được ở DEV): sau provision, `mysql -uroot` (không pw) BỊ TỪ CHỐI, `mysql -uroot -p<minted>` thành công.

## Ngoài phạm vi

- **Không UI** — hardening tự động (secure-by-default), không phải toggle. (Rotate password = sau.)
- **Không đụng data-path query/delegation** — query delegated dùng JWT / impersonate `egret_delegator`, không phải root.
- **Không sửa hồi tố engine cũ** — engine provision trước slice này không có root secret → vẫn passwordless đến khi re-provision (thay đổi additive: vắng root secret = hành vi legacy).

## Bối cảnh — cơ chế init in-cluster của operator (đã kiểm chứng)

Helm chart kube-starrocks làm `initPassword` bằng 3 resource (KHÔNG phải field CR), egret replicate bằng cách apply trực tiếp:
- **Secret** `{name}-initpwd-secret` — `data.password = <b64 minted>`.
- **ConfigMap** `{name}-initpwd-shell` — `fe_initpwd.sh`: bash chạy `SET PASSWORD = PASSWORD('$INIT_PWD')` qua `mysql -h $1 -P $2 -u root`, coi lỗi **1045**/**1064** là đã-đổi = success (idempotent, chạy lại an toàn).
- **Job** `{name}-initpwd` — FE image + `fe_initpwd.sh <fe-host> 9030`, `INIT_PWD` env từ Secret. Chạy **in-cluster** → DNS FE resolve (blocker DNS delegation KHÔNG áp).

`KubeClient::apply(&Value)` derive ApiResource từ apiVersion+kind → apply Job `batch/v1` / Secret / ConfigMap không cần đổi client.

## Kiến trúc

**A. Provisioning (`orchestrator/starrocks.rs`):** thêm builder gate theo `root_password` trên spec: `starrocks_root_password_secret` (`{name}-initpwd-secret`), `starrocks_initpwd_configmap` (`fe_initpwd.sh` verbatim), `starrocks_initpwd_job` (`{name}-initpwd` batch/v1: FE image, args `[.../fe_initpwd.sh, {name}-fe-service.{ns}, "9030"]`, `INIT_PWD` từ secretKeyRef, script mount từ ConfigMap, `restartPolicy: OnFailure`, `backoffLimit`). `StarRocksProvisioner::provision` (+update idempotent) apply cả 3 khi có root password; `delete` cleanup best-effort.

**B. Kết nối bootstrap FE (`engine-mgmt/engines.rs`):** `spawn_starrocks_catalog_wire` kết nối FE **as root**; root có pw thì phải present. Thread root password vào kết nối bootstrap (`mysql_async OptsBuilder.pass(...)`) khi engine có root secret; giữ đường passwordless cho engine legacy. (Đường này dev-DNS-blocked, nhưng phải đúng cho prod.)

**C. Contract + mint (`engine-mgmt-api` + `engines.rs`):** thêm `root_password: Option<String>` vào `StarRocksConfig` (redact trong Debug). `create_starrocks_engine`: mint root password + lưu egret secret (tái dùng pattern `ensure_delegator_secret`) + `root_secret_id` (mirror `delegator_secret_id`); secure-by-default (luôn mint cho engine mới). `build_starrocks_provision_req` resolve root password từ secret đã lưu (update re-apply cùng pw).

## Luồng dữ liệu

Create → mint root pw → egret secret (`root_secret_id`) → provisioner apply CR + FE ConfigMap + **initpwd Secret/ConfigMap/Job** → Job (in-cluster) set pw cho `root` khi FE up (idempotent) → kết nối root sau này của egret dùng pw đã lưu. Data-path không đổi.

## Kiểm thử

- **Unit:** shape `starrocks_initpwd_job/_configmap/_secret`; provisioner apply cả 3 khi có root pw, không khi vắng (legacy); `build_starrocks_provision_req` thread `root_password` từ secret; kết nối bootstrap dùng pw khi có.
- **Live (verify được ở DEV — Job in-cluster):** provision → Job `{name}-initpwd` completes → `mysql -uroot` (no pw) TỪ CHỐI, `mysql -uroot -p<minted>` OK. Idempotent (re-run update exit 0).

## Rủi ro chỉ-live

1. Timing Job (FE query port chưa up) — `--connect-timeout` script + Job `backoffLimit`/`OnFailure` retry đến khi Ready.
2. `SET PASSWORD` vs `ALTER USER` — operator dùng `SET PASSWORD = PASSWORD(...)`; xác nhận chạy trên image 3.4 (dùng verbatim).
3. Idempotent re-provision — script coi 1045 là success.

## Phân rã (cho plan)

1. initpwd builders (secret/configmap/job) + tests. 2. Provisioner apply/cleanup + tests. 3. Contract + mint secret + `root_secret_id` + resolve + tests. 4. Bootstrap connection thread root pw + tests. 5. Live smoke (Job set pw in-cluster; passwordless từ chối, minted OK; idempotent).
