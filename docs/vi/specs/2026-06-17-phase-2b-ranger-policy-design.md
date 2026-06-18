# Phase 2b — Apache Ranger Policy & REST — Spec thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-06-17 · **Phiên bản:** 0.1
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-06-17-phase-2b-ranger-policy-design.md`](../../en/specs/2026-06-17-phase-2b-ranger-policy-design.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Hiện thực nửa **Apache Ranger** của [Kiến trúc gốc](../architecture/2026-06-17-egret-architecture-design.md) §8.3 / Phase 2 (nửa catalog data-plane đã ship ở Phase 2a). Đã đối chiếu reference `ranger` (`data-portal/ranger`).

---

## 1. Mục tiêu & phạm vi

Phơi một bề mặt **tương thích REST của Ranger Admin** để plugin Apache Ranger gốc trong Trino (StarRocks sau) tải policy từ Egret và enforce khi chạy query — kiểm soát access, row-level filter, và column masking. Không build/chạy Ranger Admin; engine dùng plugin chuẩn trỏ vào Egret.

**Trong phạm vi**
- Hai crate mới: `egret-authz` (lưu policy + versioning, sở hữu model và service-def seed) và `egret-ranger` (bề mặt HTTP tương thích Ranger + DTO JSON Ranger).
- **Năng lực policy đầy đủ:** access (allow/deny theo resource cho user/group/role với access types), **row-level filter**, và **column masking** — tức `policyItems`, `rowFilterPolicyItems`, `dataMaskPolicyItems` của Ranger.
- REST tương thích Ranger: đọc service-def, CRUD service, CRUD policy, và **download policy theo version** (`lastKnownVersion` → 304/200) mà plugin poll. Tag download là stub rỗng hợp lệ.
- Engine **Trino**: seed service-def Trino chính thức để resource hierarchy + access types khớp đúng cái plugin/policy-engine mong đợi.
- Test với **Ranger policy engine thật** (`RangerPolicyEngineImpl` từ `ranger-plugins-common`) nạp `ServicePolicies` JSON của Egret, kèm Rust integration (gate DB) và unit.

**Ngoài phạm vi (hoãn)**
- Service-def StarRocks (khung engine-agnostic; thêm service-def sau).
- Tag-based policies và `GET /service/tags/download/{service}` thật (trả payload rỗng hợp lệ phase này để plugin không lỗi).
- `egret-events` / event `policy.changed` — versioning là counter DB plugin poll.
- Admin UI authoring policy — chỉ backend; UI ở phase frontend governance.
- REST sync user/group/role của Ranger, search policy, grant/revoke — không cần cho download+enforce.
- Enforcement nội bộ của Egret cho các policy này (query engine của Egret là Phase 3); phase này chỉ phục vụ policy để engine enforce.

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Hình spec | Một spec: `egret-authz` + `egret-ranger` (gắn chặt — ranger serialize policy authz lưu) | Khác chẻ catalog/ranger ở 2a, đây là một deliverable |
| 2 | Năng lực policy | Access + row-filter + column-mask (cả 3 loại policy Ranger) | Governance đầy đủ |
| 3 | Mức test | Ranger policy engine thật (`RangerPolicyEngineImpl`) đánh giá `ServicePolicies` của Egret | Đúng code enforcement plugin chạy |
| 4 | Engine | Trino trước (seed service-def Trino chính thức); StarRocks sau | Tài liệu nhiều nhất; một engine giữ scope gọn |
| A | Hình lưu trữ | **Lưu native `RangerPolicy` JSON** (jsonb); download gom `ServicePolicies`. Không dịch internal model. | Toàn bộ mục đích là tương thích Ranger; YAGNI so với model translate ở §9.3 |
| B | Versioning | **Counter DB** `authz_services.policy_version`, tăng khi policy đổi; download theo `lastKnownVersion` → 304. Không `egret-events`. | Event bus chưa có; polling + counter là đủ |
| C | Service-def | **Seed service-def Trino chính thức** từ `ranger/agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json` | Resource hierarchy + access types phải khớp đúng plugin |

## 3. Kiến trúc / cấu trúc crate

```text
crates/
├── egret-authz/
│   └── src/
│       ├── lib.rs
│       ├── model.rs          # wrapper typed mỏng trên phần RangerPolicy dùng nội bộ
│       ├── store.rs          # AuthzStore (Postgres): service_defs, services, policies CRUD, bump version
│       ├── service_def.rs    # service-def Trino nhúng (include_str!) + seed-on-init
│       ├── version.rs        # helper đọc/tăng policy_version
│       └── error.rs          # AuthzError
│
└── egret-ranger/
    └── src/
        ├── lib.rs
        ├── dto.rs            # RangerServiceDef, RangerService, RangerPolicy, ServicePolicies, ...
        │                     #   serde khớp JSON agents-common của ranger byte-for-byte
        ├── api/
        │   ├── service_defs.rs  # GET definitions, GET definitions/name/{name}
        │   ├── services.rs      # CRUD service
        │   ├── policies.rs      # CRUD policy
        │   ├── download.rs      # policies/download/{service} (304/200), tags/download stub
        ├── error.rs          # RangerError → body lỗi kiểu Ranger
        └── routes.rs         # ranger_router() mount tại /service
```

**Layering:** `ranger_router` (HTTP, schema JSON Ranger) → `egret-authz` (`AuthzStore` + versioning, Postgres). `egret-ranger` phụ thuộc `egret-authz`, `egret-metastore`, `egret-config`, `egret-core`. Mount tại **`/service`** (anh em `/api` và `/catalog`) — engine gọi trực tiếp, ngoài stack session/CSRF của trình duyệt.

## 4. REST API surface (mount tại `/service`)

Tập con Ranger Admin REST (base `/service/plugins`). Route cần bearer token catalog/engine (§7).

| Method + path | Mục đích |
|---|---|
| `GET /service/plugins/definitions` | Tất cả service-def (seed: trino). Trả `{ serviceDefs: [...] }`. |
| `GET /service/plugins/definitions/name/{name}` | Một service-def theo tên. |
| `GET /service/plugins/services` · `POST` · `PUT /{id}` · `DELETE /{id}` | CRUD service instance (một "service" = một cụm Trino). |
| `GET /service/plugins/policies` · `POST` · `PUT /{id}` · `DELETE /{id}` | CRUD policy. Body là `RangerPolicy`. Mọi ghi đều bump `policy_version` của service. |
| `GET /service/plugins/policies/download/{serviceName}?lastKnownVersion=N&pluginId=...` | **Plugin poll.** Nếu `N == policy_version` → **304**; ngược lại **200** `ServicePolicies`. |
| `GET /service/tags/download/{serviceName}?lastKnownVersion=N` | Tag policies — trả `ServiceTags` rỗng hợp lệ (hoặc 304) phase này. |

Lỗi theo HTTP semantics của Ranger (400 bad request, 404 not found, 409 trùng tên) kèm body JSON; hình `RESTResponse`/`VXResponse` chính xác của Ranger không bắt buộc cho đường download (plugin chỉ parse body success + status code), nên body tối giản `{ msg, statusCode }` là đủ.

## 5. Mô hình & lưu trữ policy

**Lưu native (Quyết định A).** `RangerPolicy` lưu nguyên làm cột `jsonb`; endpoint CRUD nhận/trả `RangerPolicy` JSON. Endpoint download gom `ServicePolicies` từ: row service (`serviceName`, `serviceId`, `policyVersion`), `serviceDef` seed, và các policy enabled của service.

Một `RangerPolicy` mang cả 3 loại năng lực (khớp `ranger/.../model/RangerPolicy.java`):
- **`resources`**: map resource-type → `{ values, isExcludes, isRecursive }` (vd `database`/`table`/`column` cho Trino — key đúng từ service-def seed).
- **`policyItems[]`** (access): `{ accesses: [{type, isAllowed}], users, groups, roles, delegateAdmin, conditions }`.
- **`rowFilterPolicyItems[]`**: `policyItem` + `rowFilterInfo: { filterExpr }`.
- **`dataMaskPolicyItems[]`**: `policyItem` + `dataMaskInfo: { dataMaskType, valueExpr? }`.

`egret-authz::model` chỉ cung cấp helper typed mỏng dùng nội bộ (vd trích service name/id để bump version, kiểm policy tham chiếu resource hợp lệ trong service-def). KHÔNG remodel toàn schema Ranger — jsonb là nguồn sự thật; `egret-ranger::dto` cung cấp type serde cho wire.

## 6. Download protocol & versioning (Quyết định B)

- `authz_services.policy_version` (bigint) khởi đầu 1. Mọi create/update/delete policy thành công trên service chạy `policy_version = policy_version + 1` trong cùng transaction với việc ghi policy.
- `GET .../policies/download/{serviceName}?lastKnownVersion=N`:
  1. Resolve service theo tên → 404 nếu không có.
  2. Nếu `N == policy_version` → **304 Not Modified** (body rỗng).
  3. Ngược lại load policy enabled của service, dựng `ServicePolicies { serviceName, serviceId, policyVersion, policyUpdateTime, serviceDef, policies }`, trả **200**.
- Plugin cache `policyVersion` trả về và gửi làm `lastKnownVersion` lần poll sau — polling chuẩn Ranger. Không event bus; chu kỳ poll plugin (mặc định ~30s) chặn độ trễ lan truyền.

## 7. Auth & config

**Auth.** `ranger_router` gate bằng cùng pattern middleware bearer-token tĩnh như catalog (`[ranger].auth_token`, hoặc dùng lại token catalog). Plugin Ranger gốc hỗ trợ credential Bearer/basic trong config `policy.rest.url`; ta nhận bearer token. (Auth plugin Ranger thường Kerberos/basic; cho Egret MVP, bearer token cấu hình phía plugin là hợp đồng tích hợp.)

**Thêm config (`egret-config`).**
```toml
[ranger]
enabled    = false
auth_token = "..."   # bearer tĩnh (env/secret); có thể bằng catalog.auth_token
```
`validate()` từ chối `ranger.enabled && auth_token rỗng`.

**Wiring server (`egret-server`).** `AppState` thêm `ranger: Option<RangerState>`; mount `ranger_router()` tại `/service` khi `ranger.enabled`. Service-def Trino seed vào `authz_service_defs` lúc khởi động (upsert idempotent theo tên) khi ranger bật.

## 8. Data model (`migrations/0007_authz.sql`)

```sql
CREATE TABLE authz_service_defs (
    id         bigint PRIMARY KEY,          -- id service-def Ranger (trino seed id cố định)
    name       text NOT NULL UNIQUE,        -- 'trino'
    def_jsonb  jsonb NOT NULL,              -- full RangerServiceDef
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_services (
    id             bigserial PRIMARY KEY,    -- id service Ranger
    name           text NOT NULL UNIQUE,     -- vd 'trino_prod'
    type           text NOT NULL REFERENCES authz_service_defs(name),  -- 'trino'
    config_jsonb   jsonb NOT NULL DEFAULT '{}',
    policy_version bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_policies (
    id           bigserial PRIMARY KEY,      -- id policy Ranger
    service_id   bigint NOT NULL REFERENCES authz_services(id) ON DELETE CASCADE,
    name         text NOT NULL,
    policy_jsonb jsonb NOT NULL,             -- RangerPolicy native
    enabled      boolean NOT NULL DEFAULT true,
    version      bigint NOT NULL DEFAULT 1,  -- version từng policy (Ranger cũng theo dõi)
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (service_id, name)
);

CREATE TABLE policy_audit (
    id        bigserial PRIMARY KEY,
    policy_id bigint,                         -- nullable (action mức service)
    action    text NOT NULL,                  -- create|update|delete
    actor     uuid REFERENCES users(id),
    at        timestamptz NOT NULL DEFAULT now()
);
```
Id Ranger là số nguyên (không phải uuid) vì model JSON Ranger và plugin mong đợi `id`/`serviceId`/`policyId` dạng số. `bigserial` cấp; service-def seed dùng id cố định.

## 9. Test

- **Gate Ranger policy-engine thật** (JVM, gate giống pyiceberg/Keycloak): harness Java nhỏ (Maven/Gradle pull `org.apache.ranger:ranger-plugins-common` từ Maven Central) (1) deserialize `ServicePolicies` JSON do endpoint download Egret trả vào `ServicePolicies` của Ranger, (2) dựng `RangerPolicyEngineImpl` với service-def Trino, (3) gửi các `RangerAccessRequest` và assert: policy access allow/deny đúng `{catalog,schema,table,column}` + user/access-type; policy row-filter ra `filterExpr` đúng; policy data-mask ra `maskType` đúng. Test Rust gate shell ra harness, self-skip trừ khi `EGRET_TEST_RANGER_E2E=1` (cần JVM + mạng cho jar). Runbook ở `tests/ranger/README.md`.
- **Rust integration (gate DB):** seed/đọc service-def; CRUD service + policy round-trip (POST RangerPolicy → GET lại y nguyên); `download` trả 200 với `ServicePolicies` hợp lệ rồi 304 khi `lastKnownVersion == policy_version`; ghi policy bump `policy_version`; `download` service lạ → 404.
- **Unit:** gom `ServicePolicies` (service + def + policies); logic bump version; service-def Trino parse vào `RangerServiceDef`; serde round-trip `RangerPolicy` đủ 3 loại item.

## 10. Phân rã cho plan

Task gợi ý: (1) scaffold crate `egret-authz` + `egret-ranger` + workspace deps; (2) `egret-ranger::dto` (serde RangerServiceDef/Service/Policy/ServicePolicies, validate với def Trino seed + policy mẫu); (3) `migrations/0007_authz.sql` + `AuthzStore` seed + đọc service-def (gate DB); (4) `AuthzStore` CRUD services + CRUD policies + bump `policy_version` (gate DB); (5) `egret-authz::service_def` nhúng + seed-on-init idempotent; (6) endpoint download `egret-ranger` (304/200 + gom ServicePolicies) + stub tags; (7) handler CRUD service/policy `egret-ranger` + map lỗi + bearer auth + `ranger_router`; (8) `egret-config` `[ranger]` + `egret-server` mount `/service` + seed-on-startup; (9) Rust integration test (CRUD, download 200/304/404, bump version); (10) gate JVM Ranger policy-engine thật + runbook; (11) verify cuối.

## 11. Tham khảo

- **ranger** (`data-portal/ranger`): `agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json` (seed), `agents-common/.../model/{RangerPolicy,RangerServiceDef,RangerService}.java` + `util/ServicePolicies.java` (hình wire), `agents-common/.../policyengine/RangerPolicyEngineImpl.java` (test harness), `plugin-trino/` (plugin engine poll). Artifact Maven `org.apache.ranger:ranger-plugins-common` cho test.
- Kiến trúc: [Bản gốc](../architecture/2026-06-17-egret-architecture-design.md) §8 (authz hai tầng), §8.3 (Ranger REST), §9.3 (data model authz).
- Anh em: spec Phase 2a Iceberg catalog (cùng pattern mount-ngoài-`/api`, cùng cách bearer-auth).
