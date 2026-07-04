# Khung UX cho Config UI (2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-config-ux-framework-design.md`.

**Mục tiêu:** Làm Configuration UI dễ dùng hơn HẲN + dựng **khung tái dùng** để phần
completeness sau (Trino tuning, security/TLS, observability) cắm vào mà không làm UI rối
thêm. **Chỉ frontend — không đổi backend/API; payload `trinoConfig` giữ nguyên.**

Đây là sub-project #1 của nỗ lực "config đầy đủ + dễ dùng" (4 phần). Làm trước vì sửa pain
nặng nhất + tạo primitive cho #2-#4.

**Non-goals:** field config Trino mới (#2-#4); search box + unsaved-nav-guard (defer);
đổi backend.

## Vấn đề fix (từ audit UX)
Help text thiếu/không đồng nhất (nhất là Coordinator/Worker); PodScheduling 1481 dòng cuộn 1
mạch; 3 biến thể raw-JSON khác nhau; không chỉ báo "field nào đã set"; size input không đơn
vị/validate; style section ad-hoc; prefill 5s đè edit đang gõ; strip 8 tab overflow.

## Framework (primitives tái dùng)
1. **`ConfigSection`** — card dùng chung: title + description + docsUrl? (link docs Trino). Mọi group refactor về nó → hết style lộn xộn.
2. **`fieldHelp` map + `FieldLabel`** — `fieldHelp.ts` (`fieldKey → {help, docsUrl?, unit?}`); FieldLabel render label + icon ℹ️ tooltip (+ link docs). Author high-stakes trước (Coordinator/Worker/Reliability/Autoscaling/Security).
3. **`RawJsonField`** — 1 editor raw-JSON thay 3 biến thể; text-in-form + debounce validate + **validity registry** chung → raw sai ở đâu cũng chặn Save (bỏ threading onValidityChange thủ công).
4. **`QuantityInput`** — input CPU/mem/heap/size validate dạng k8s-quantity (`4Gi`/`500m`/`2GB`) + hint đơn vị + lỗi inline; number input thêm min/max.

## Đổi cấu trúc
5. **Layout Configuration → vertical left-nav** (kiểu trang Settings): tách PodScheduling đẩy số sub-tab lên ~10, horizontal overflow → chuyển nav dọc trái (danh sách section trái, field phải), mỗi item mang badge non-default. Top-level tabs giữ nguyên.
6. **Tách `PodSchedulingSection`** thành 3 section/nav item: **Pod** (serviceAccount, imagePull, securityContext, deployment strategy/rollout/annotations, shareProcessNamespace) · **Placement** (per-role: nodeSelector/tolerations/topologySpread/affinity/grace/ports) · **Containers & Mounts** (init/sidecar, envFrom, config/secret mounts). Thứ tự nav: General · Coordinator · Worker · Workload · Reliability · Advanced · Pod · Placement · Containers & Mounts · Autoscaling.
7. **Badge non-default** trên mỗi nav item = số field đã set trong section (dùng lại logic omit-empty). 0 ⇒ không badge.
8. **Diff-before-save (modal)** — nút **Review changes** cạnh Save mở modal diff `trinoConfig` build vs `spec.trinoConfig` đã lưu (path thêm/xoá/đổi old→new + note "save sẽ roll pod"). Read-only, dùng payload builder sẵn.

## Fix đúng đắn
9. **Prefill không đè edit** — effect `[engine.spec]` KHÔNG reset khi form đang `dirty` (guard) → poll 5s không vứt input đang gõ. Vẫn prefill lần đầu + khi không dirty.

## Phạm vi refactor
Mọi section component hiện có refactor về `ConfigSection`+`FieldLabel`+`RawJsonField`+
`QuantityInput`. `useEngineConfigForm` thêm selector "non-default count" per-section + prefill
guard-dirty; state/payload giữ nguyên (payload parity).

## Test
tsc/eslint/vitest/build + unit (QuantityInput validate; non-default count; diff builder) +
Playwright (vertical nav ~10 section; badge; help tooltip; RawJsonField sai chặn Save; Review
changes mở modal diff; edit rồi poll 5s KHÔNG mất edit; Save vẫn PUT 200 payload y hệt cho engine chưa sửa).

Compat: refactor frontend thuần, không migration/API/route; payload không đổi. Chi tiết xem bản EN.
