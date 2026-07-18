# Follow-up: data-plane Trino + deploy kind

> Phát hiện từ live smoke end-to-end 2026-07-18 của nhánh `feat/deploy-egret-kind` (egret deploy lên kind
> qua `deploy/helm/egret`, OIDC single-origin, provision Trino + query). Đã chứng minh trọn đường
> **OIDC → engine JWKS → JWT → TLS:8443 → impersonation → query** (`SELECT current_user` trả về người
> gọi `admin`, không phải principal kết nối `egret_delegator`). 3 mục dưới là follow-up, KHÔNG chặn phần deploy.
>
> Bản chuẩn tiếng Anh: `docs/en/plans/2026-07-18-trino-dataplane-deploy-followups.md`.

## 1. [BUG] Sai tên Service coordinator của Trino — Trino provision kiểu plain không kết nối được

**Triệu chứng:** mọi query SQL Lab lên Trino fail: `error sending request … {name}-coordinator.{ns}.svc:8443`.

**Nguyên nhân:** egret provision Trino bằng plain Deployments (`{name}-coordinator`, `{name}-worker`) +
1 Service tên **`{name}-trino`** (8080/8443), nhưng tầng kết nối lại **dial** host `{name}-coordinator.{ns}.svc`
— không có Service nào tên vậy. Tên Service (provisioner) và host được dial (`coordinator_host`) lệch nhau.
Trước đây nhiều khả năng bị che bởi Stackable trino-operator (tạo Service `{name}-coordinator`); cluster kind
mới trong smoke không cài operator này nên lộ ra.

**Fix (chọn 1, cho khớp):** đặt tên Service coordinator = `{name}-coordinator`, HOẶC dial `{name}-trino`.
Thêm test provisioning assert tên Service == host dial.

**Workaround trong smoke:** tạo Service alias `{name}-coordinator` (selector `app: {name}-coordinator`, port 8080/8443).

## 2. [GAP] Trino luôn dùng plugin Ranger access-control → delegated query bị deny-all khi không có Ranger server

**Triệu chứng:** sau khi auth OK, query impersonate fail: `Access Denied: Principal egret_delegator cannot become user admin`.

**Nguyên nhân:** egret ghi `access-control.name=ranger` vào config coordinator **bất kể `config.ranger.enabled`**.
Không có Ranger server → plugin fail-closed (deny-all) → `egret_delegator` không được phép impersonate người gọi.
Đây là bẫy "ranger-configured-but-no-server = delegated-denied" đã biết.

**Fix (chọn 1):** khi `ranger.enabled=false` (hoặc không reach được Ranger) → provision Trino với access-control
permissive/file cho phép `egret_delegator` impersonate; HOẶC bắt buộc có Ranger server + seed policy impersonation.

**Workaround trong smoke:** patch config sang `access-control.name=allow-all` + restart coordinator → query impersonate chạy (`current_user` = người gọi).

## 3. [DX] Vài điểm nhỏ về chart/image

- **Cache image backend:** `deploy/docker/backend.Dockerfile` `COPY . .` trước `cargo build` → mọi thay đổi file
  (kể cả sửa chart/docs) phá cache → build lại full 20–40 phút mỗi lần `just deploy-kind`. Fix bằng cargo-chef hoặc
  copy `Cargo.toml`/`Cargo.lock` + manifest trước.
- **`egret init` không seed workspace mặc định:** help ghi "and default workspace" nhưng deploy mới có 0 workspace.
  Hoặc seed 1 workspace trong `init`, hoặc sửa help.
- **Engine-JWT không boot khi OIDC tắt (Milestone B đã fix, nên guard):** OIDC off → JWKS engine rỗng → bật "Accept
  egret JWT" ghi `jwt.key-file` rỗng → coordinator crash. egret nên chặn bật engine-JWT khi chưa có JWKS, hoặc tự
  serve JWKS riêng. (Bật OIDC = Milestone B → JWKS có → hết crash.)
