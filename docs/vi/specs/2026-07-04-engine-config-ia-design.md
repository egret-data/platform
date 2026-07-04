# Quy hoạch lại IA Configuration của EngineDetail (2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-engine-config-ia-design.md`.

**Mục tiêu:** Tổ chức lại trang engine detail để config không còn là 1 cuộn "Cluster"
dài lê thê, và tách file `EngineDetail.tsx` (1885 dòng) thành component nhỏ. **Chỉ
frontend — KHÔNG đổi backend/API** (cùng PUT `updateEngine` với `trinoConfig` như cũ).

**Vấn đề:** Top-level hiện chỉ `Overview | Monitor`; Overview chứa card `Configuration`
mà sub-tab `Cluster` (~384 dòng JSX) nhồi 11 nhóm không liên quan → cuộn mệt, file quá to.

**Non-goals:** đổi backend; thêm config mới (1.5.3/1.5.4/1.5.5 sau); đổi Monitor; đổi
semantics save (vẫn 1 PUT cả config).

## IA mới
Top-level tabs: **Overview · Configuration · Security & Exposure · Monitor**.
- **Overview**: Status/pods, Catalogs, Danger zone (bỏ card Configuration khỏi đây).
- **Configuration** → sub-tabs: **General · Coordinator · Worker · Workload · Reliability · Advanced**.
  - General: query max memory, log level, config.properties (raw KV), env vars.
  - Coordinator: replicas, resources, heap, probes, jvm, heap-headroom.
  - Worker: như trên + graceful shutdown.
  - Workload: resource groups + session properties (tái dùng `EngineResourceGroups.tsx`).
  - Reliability: fault-tolerant execution.
  - Advanced: event-listener, additional node/log properties (raw).
- **Security & Exposure**: password users, JWT, ingress (tách khỏi Cluster).
- **Monitor**: giữ nguyên (1.6b).

## Kiến trúc component
- **`useEngineConfigForm(engine)`** (hook mới): sở hữu TOÀN BỘ state config + prefill từ
  spec (kể cả re-sync theo status poll, giữ guard không đè edit đang gõ) + builder payload
  `trinoConfig` + mutation save (dirty flag + toast). Trả `{state, setters, dirty, save, saving}`.
  EngineDetail tạo 1 instance, truyền xuống sub-tabs → 1 nguồn state dùng chung cho cả
  Configuration lẫn Security.
- Tách file: `EngineConfigGeneral.tsx`, `EngineRoleConfig.tsx` (dùng chung Coordinator/Worker
  qua `role` prop; worker thêm graceful shutdown), `EngineConfigReliability.tsx`,
  `EngineConfigAdvanced.tsx`, `EngineSecurity.tsx`; Workload tái dùng `EngineResourceGroups.tsx`.
- `EngineDetail.tsx` = orchestrator mỏng: top-level Tabs + hook dùng chung + Overview +
  phân phối slice state xuống sub-tabs. Mục tiêu: nhỏ hơn nửa kích thước hiện tại.
- Helper KV (`KeyValueEditor`, `recordToKv`, `kvToRecord`) đang ở `EngineResourceGroups.tsx` —
  giữ nguyên hoặc chuyển sang `configShared.ts` nếu import gọn hơn.

## Save
1 state dùng chung; nút **Save changes** hiện trên CẢ tab Configuration lẫn Security, cùng gọi
`save()` PUT full `trinoConfig` (kèm auth/ingress) y như hiện tại. Disable khi `!dirty` hoặc đang save.

## Giữ nguyên hành vi
Prefill mọi field từ `engine.spec.trinoConfig` (kể cả re-sync theo poll, không đè edit); PUT
payload y hệt (diff before/after save form không đổi = không rớt field); save vẫn roll engine.

## Test
tsc/eslint/vitest/build xanh + Playwright (4 top-tab + 6 config sub-tab render, prefill đúng,
đổi 1 field + Save → PUT 200 + roll; Security tab prefill + Save chạy). Sanity: payload của engine
chưa sửa == payload trước refactor (không rớt field).

Compat: không migration/API/route. Refactor frontend thuần. Chi tiết xem bản EN.
