# StarRocks Config UX — Tổ chức lại (editor có cấu trúc, gộp section) — Thiết kế

**Ngày:** 2026-07-17 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-config-ux-reorg` (off `main`)
**Skill:** ui-ux-pro-max (Forms). Feedback user: gộp Compute + Resources; thay textarea raw-JSON ở **Pod & Containers** (Env / Tolerations / Affinity / Init containers / Sidecars) bằng editor "thêm property" có cấu trúc.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-17-starrocks-config-ux-reorg-design.md`.

## Vấn đề

Tab Settings StarRocks (#266/#267) có 9 section + bắt nhập **JSON thô** cho pod fields (admin gõ tay `[{"name":"TZ","value":"UTC"}]`…). Dễ sai, vi phạm Forms best-practice. Ngoài ra Compute (replicas) và Resources (CPU/mem) tách rời — đều là sizing, nên gộp; Pod & Containers gộp lộn scheduling+containers+env+identity thành 1 khối JSON.

egret đã có primitive cấu trúc (editor config Trino): `KeyValueEditor` (`Kv[]`), toleration-rows + affinity trong `PlacementSection`, mini-container trong `ContainersMountsSection`. Reorg tái dùng/tách chúng cho StarRocks + bridge JSON↔structured.

## Mục tiêu

Sắp lại tab Settings: sizing thành 1 section; Pod & Containers dùng **editor có cấu trúc** thay JSON thô, chỉ giữ 1 escape "Advanced (JSON)" ở chỗ shape k8s thật sự quá lồng (affinity). Hành vi + save/dirty không đổi — PUT body vẫn mang `starrocksConfig.pod` JSON như cũ.

**Section sau (8, từ 9):** `Image · Compute & Resources · FE config · CN config · Autoscaling · Pod & Containers · Storage · Security & Delegation`.

## Ngoài phạm vi

- **Không đổi backend** — wire shape (`pod.{env,tolerations,affinity,initContainers,sidecars,nodeSelector}` JSON) giữ nguyên; chỉ đổi widget nhập + convert.
- **Không đụng engine khác** — tab config Trino giữ nguyên (đã có editor).
- **Không thêm field k8s mới.**

## Bối cảnh — primitive tái dùng (đã kiểm chứng)

- `KeyValueEditor` (`../EngineResourceGroups`, `Kv[]`) → dùng cho **env** + **nodeSelector** + **podLabels/annotations**.
- `PlacementSection` có toleration-rows (key/operator/value/effect) + affinity (nodeMatch + RawJsonField), nhưng bound `RoleForm` Trino → **tách `TolerationsEditor({rows,onChange})`** standalone.
- `ContainersMountsSection` có mini-container editor (name/image, command/args, env KV, mounts + raw) bound `ContainerForm` → **tách `ContainerListEditor({rows,onChange})`**.
- `RawJsonField` + `ValidityProvider` → giữ cho affinity "Advanced" (JSON sai chặn Save).

## Kiến trúc

**A. Gộp Compute + Resources:** thay 2 nav item bằng 1 `{key:"compute", label:"Compute & Resources"}`; render FE/CN replicas + CPU/mem chung; badge = union non-default 2 section; giữ deep-link `?section=compute`, alias `?section=resources`→compute.

**B. Pod & Containers có cấu trúc:** dựng lại `PodContainersSection` chia 4 nhóm có nhãn, mỗi field 1 editor cấu trúc bind vào `pod` block qua **converter** (parse JSON→rows lúc seed; serialize rows→JSON lúc đổi):
- **Environment** — env: `KeyValueEditor` ↔ `[{name,value}]`.
- **Scheduling** — nodeSelector: `KeyValueEditor` (`{k:v}`); tolerations: `TolerationsEditor` (tách) ↔ `[{key,operator,value,effect}]`; affinity: giữ `RawJsonField` nhãn **"Affinity (advanced JSON)"** (shape lồng, không có form gọn — field raw duy nhất chính đáng).
- **Containers** — initContainers + sidecars: `ContainerListEditor` (tách; name/image/command/env + Advanced-JSON mỗi container) ↔ `[Container]`.
- **Identity** — serviceAccount (input), imagePullSecrets (comma list giữ), podLabels + annotations (`KeyValueEditor`).
`ValidityProvider` vẫn bọc các escape raw-JSON (affinity + per-container advanced); `SaveBar disabled` giữ `autoscalingInvalid || podRawInvalid`.

**C. Converters (`starrocksPodShape.ts`):** hàm thuần bridge `pod` JSON ↔ shape editor: env/nodeSelector/tolerations/containers, **giữ lại key lạ/malformed** (raw fallback). `normPod` (dirty/save) không đổi — vẫn ghi cùng `pod` JSON.

## Luồng dữ liệu

Seed: `engine.spec.starrocksConfig.pod` (đã surface bởi fix DTO #267) → converter → rows. Sửa row → converter → `pod` JSON → `normPod` → dirty/save (không đổi). JSON-advanced sai → ValidityProvider → Save disabled. PUT body y như hiện tại.

## Kiểm thử

- **Unit (Vitest):** converter round-trip (env/nodeSelector/tolerations/containers ↔ structured, rỗng + malformed-preserved); thêm env row → PUT `pod.env`=`[{name,value}]`; thêm toleration → `[{key,operator,value,effect}]`; thêm init container → `[{name,image}]`; affinity JSON sai chặn Save; Compute&Resources 1 section sửa cả replicas+resources; `?section=resources`→compute. Test config StarRocks cũ vẫn xanh (PUT shape không đổi).
- **Live (browser):** Pod & Containers hiện editor add-row (không textarea trừ Advanced có nhãn); thêm env/nodeSelector/toleration/init-container qua form → Save → PUT body đúng JSON; Compute&Resources gộp. Screenshot.

## Rủi ro

1. Fidelity converter — key lạ/unknown trong `pod` phải GIỮ, không drop: mỗi editor cặp với escape raw "Advanced" round-trip key lạ.
2. Tách widget không làm regression Trino (Trino vẫn dùng widget đã tách).
3. Dirty ổn định — converter phải deterministic (sorted/fixed order) để seed→no-edit không spurious-dirty.

## Phân rã (cho plan)

1. Gộp Compute+Resources (1 nav + badge + alias `?section=`). 2. Tách `TolerationsEditor` + `ContainerListEditor` (không regression Trino). 3. Converters (`starrocksPodShape.ts`) malformed-preserving + unit test. 4. Dựng lại `PodContainersSection` cấu trúc (4 nhóm, affinity Advanced-JSON, ValidityProvider). 5. Live smoke + screenshot.
