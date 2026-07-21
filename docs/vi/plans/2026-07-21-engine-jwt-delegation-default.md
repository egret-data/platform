# Engine JWT Delegation Bật Mặc Định — Kế hoạch (tóm tắt)

> **Bản chi tiết (chuẩn, code từng bước):** `docs/en/plans/2026-07-21-engine-jwt-delegation-default.md`
> **Spec:** `docs/vi/specs/2026-07-21-engine-jwt-delegation-default-design.md`
> **Thực thi:** subagent-driven-development.

**Mục tiêu:** Provision Trino/StarRocks không truyền `delegationMode` ⇒ mặc định per-user JWT (tin machine-IdP egret), tự hạ xuống `impersonate` khi không đảm bảo JWT được; `"none"` = opt-out engine open.

**Kiến trúc:** Một resolver thuần `resolve_delegation(explicit, has_jwks, sr_image_tag) → {Jwt|Impersonate|Open}`. Hai create path gọi nó và **persist mode đã resolve** vào stored spec ⇒ provision-req builder và đường update-preserve không đổi. FE thêm lựa chọn "Auto" (gửi mode bỏ trống).

## Task
1. **Resolver thuần** `crates/egret-engine-mgmt/src/delegation_default.rs` — `resolve_delegation` + `has_platform_jwks` + `starrocks_image_is_pre_3_5` (best-effort parse tag; `<3.5`→true, không parse được→false) + `ResolvedDelegation`. Unit test đầy đủ (honored/hard-fail/default/degrade/parse). Đăng ký module ở `lib.rs`.
2. **StarRocks create** (`engines.rs` `create_starrocks_engine` ~2714): sau `validate_sr_delegation_mode` (nới nhận `"none"`), gọi resolver, **ghi lại `sr_body.delegation_mode`** (Jwt⇒"jwt"+`tls_enabled=Some(true)`; Impersonate⇒"impersonate"; Open⇒None), rồi `build_starrocks_spec` persist. Explicit jwt + không JWKS ⇒ 400. Unit test qua `store::Engine::delegation_mode()`/`supports_jwt_delegation()` (mẫu test ~5973).
3. **Trino create** (`engines.rs` nhánh Trino trong `create_engine` ~1902–2060): resolve từ `trinoConfig.delegationMode`, ghi lại JSON `trinoConfig` (Jwt⇒`delegationMode="jwt"`+ đảm bảo `auth.jwt` object; Impersonate⇒`"impersonate"`; Open⇒bỏ `delegationMode`). `sr_image=None`. Nới validator Trino nhận `"none"`. Hard-fail explicit-jwt-no-JWKS đã sẵn ở `resolve_auth` (surface qua provision inline ~2077). Unit test qua `is_jwt_delegation()`/`is_impersonate_delegation()` (mẫu ~5120).
4. **Frontend** (`useEngineConfigForm.ts` + Security tab `EngineDetail.tsx`): `DelegationMode = "" | "jwt" | "impersonate" | "none"`; default engine mới = `""` (Auto) ⇒ **omit** `delegationMode` khi serialize. Selector: Auto (khuyến nghị) / JWT / Impersonate / None. Regenerate `openapi.json` (drift test xanh). tsc + lint sạch.
5. **Live e2e** `frontend/tests/e2e/delegation-default-kind.spec.ts` (`just e2e-kind-delegation-default`, opt-in `EGRET_E2E_KIND=1`): provision Trino + StarRocks KHÔNG truyền `delegationMode` trên deploy có `public_url` ⇒ cả hai lên `delegationMode="jwt"`, query per-user chạy (admin-full vs member-masked), tái dùng harness `sts-broker-kind`.

## Verify cuối
Holistic review (resolver ↔ 2 create path ↔ persisted-spec ↔ provision; không hồi quy explicit/update-preserve) → rebuild+redeploy+`just e2e-kind-delegation-default` xanh → full gate (fmt+clippy workspace + `cargo test -p egret-engine-mgmt` + FE tsc/lint + openapi drift) → finish branch → PR → squash-merge.
