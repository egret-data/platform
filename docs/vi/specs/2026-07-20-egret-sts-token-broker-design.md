# Egret STS / Token Broker — Thiết kế

> **Trạng thái:** Đã duyệt thiết kế (brainstorm). Bước tiếp: viết plan.
> **Phạm vi:** Sub-project 1/2. Quản lý user local (CRUD user password + group +
> UI control-plane) là **sub-project 2**, spec riêng, phụ thuộc spec này.
>
> *(Bản tiếng Anh là bản chuẩn: `docs/en/specs/2026-07-20-egret-sts-token-broker-design.md`.)*

**Mục tiêu:** Biến egret thành **nơi phát duy nhất** token của data plane (engine).
Mọi user đã xác thực — local password **hoặc** SSO — đều nhận một JWT per-user,
ngắn hạn, **ký bởi machine-IdP của egret** để truy cập engine, đúng như catalog
token đang làm. SSO/Keycloak hạ xuống thành **nguồn xác thực** dùng một lần lúc
login. Nhờ đó gỡ bỏ phụ thuộc Keycloak-refresh-token đang chặn các user không có
refresh token (vd local admin) query engine.

**Kiến trúc một dòng:** tách *authentication* (bạn là ai, lúc login) khỏi *token
issuance* (cái token data plane nhìn thấy); machine-IdP của egret sở hữu việc phát
token; engine chỉ trust egret.

---

## 1. Bối cảnh & động lực

Hiện tại đường notebook/SQL-Lab/BI mint engine JWT per-user bằng **Keycloak refresh
grant** (`EngineTokenMinter` → `egret-bin/src/engine_broker.rs`, dùng refresh token
đã lưu của user). Hệ quả:

- User **không có refresh token** (local/break-glass admin, mọi user không-OIDC)
  không mint được → `BrokerError::NoRefreshToken` → 401 ở notebook. Đây là chặn
  cứng, đã thấy live.
- Vòng đời truy cập engine bị trói vào tuổi thọ refresh token Keycloak: hết hạn thì
  bắt login lại mới query được.
- Khi OIDC **tắt**, `engine_jwks_uri`/`engine_oidc_issuer` rỗng → security
  integration của engine jwt-delegation bị template với issuer/jwks rỗng →
  jwt-delegation coi như hỏng khi OIDC off.

Trong khi đó phía **catalog** đã làm đúng: `sign_claims` + `PgSigningKeyStore` (crate
`egret-machine-idp`) mint JWT `aud:"catalog"` **không cần refresh token**, và catalog
nhúng trust chính issuer của egret. Spec này hoàn tất sự đối xứng catalog↔engine:
engine token cũng do egret ký.

## 2. Các quyết định (đã chốt lúc brainstorm)

| Quyết định | Lựa chọn |
| --- | --- |
| Phạm vi unify | **Full broker** — mọi user (local + SSO) → engine token do egret ký. Keycloak = authN-only. |
| Cơ chế mint | **Approach A** — minter nội bộ ký bằng active key của machine-IdP (không thêm grant công khai; đối xứng catalog). |
| Vòng đời session | **Session egret độc lập** (sliding lifetime + idle timeout). Keycloak token/refresh hết hạn không ảnh hưởng truy cập engine. |
| Revoke tập trung | **Để dành.** User-disable phía egret vô hiệu hoá session ngay; revoke từ IdP + back-channel logout là việc sau. |
| Trust ở engine | Engine trust **duy nhất issuer/JWKS của egret** (đích đến), qua giai đoạn dual-trust. |
| Nguồn group | Bảng `user_groups` của egret (ghi khi login qua JIT), là nguồn sự thật cho claim. |
| CRUD/UI user local | **Ngoài phạm vi** — sub-project 2. User local hiện có tự động hưởng. |

## 3. Kiến trúc & mô hình trust

```
                         ┌──────────────── egret (lõi tin cậy) ──────────────────┐
 login (nguồn authN)     │                                                       │
  ├─ local password ─────┼─▶ session egret ─▶ EngineTokenMinter (machine-IdP) ───┼─▶ engine JWT
  └─ SSO / OIDC ──────────┼─▶ (+ JIT provision)     sign_claims(active key)       │    do egret ký
        (validate 1 lần   │        │                aud:"engine"                  │   (per-user,
         lúc login)       │        └─ groups → user_groups (ghi bền)             │    ngắn hạn)
                          └───────────────────────────────────────────────────────┘
                                                    │  chỉ trust JWKS/issuer egret
                                                    ▼
                                    StarRocks / Trino  (+ catalog nhúng, đã trust egret)
```

Ba mảnh trust:

1. **machine-IdP egret là issuer data-plane duy nhất.** Đã ký `aud:"catalog"`; thêm
   `aud:"engine"`. Engine và catalog cùng validate theo một JWKS (`/jwks`) + issuer
   — của egret.
2. **Keycloak/SSO chỉ là nguồn authN.** Validate một lần lúc login để biết danh tính
   + group; sau đó không đụng nữa cho truy cập engine; không lưu/dùng refresh token.
3. **Session egret là chủ vòng đời.** Sliding lifetime + idle timeout, do egret sở
   hữu. Keycloak token hết hạn không liên quan.

**Chi phí thật duy nhất:** key ký của egret thành trust-root cho *mọi* truy cập engine
(đã đúng cho catalog). `PgSigningKeyStore` rotate được nên vẫn quản lý được (§7).

## 4. Thành phần

**Mới**

1. **`MachineIdpEngineMinter`** (`egret-bin`, thay `engine_broker.rs`) — impl mới của
   `EngineTokenMinter`. `mint(user_id)` load danh tính, dựng claim set, ký bằng
   `active` key của machine-IdP qua `sign_claims`. Không Keycloak, không refresh
   token. Đây là **seam duy nhất bị swap**: kernel notebook, SQL Lab, BI đều đi qua
   trait `EngineTokenMinter`, nên swap impl là đổi hết consumer cùng lúc.
2. **`engine_claims(user, issuer, ttl)`** — hàm thuần, unit-test được, trả
   `{iss, sub, preferred_username, groups, aud:"engine", iat, exp}`.
   `preferred_username` == slug mà `UserRepo::ensure_username` sinh ở mọi nơi → khớp
   `principal_field=preferred_username` của Ranger.
3. **Identity loader** — load `(username, groups)` theo `user_id` từ `users` +
   `user_groups`. DB egret là nguồn sự thật runtime cho claim authz.

**Sửa**

4. **OIDC login → JIT provisioning** (`egret-auth/oidc_routes.rs`) — lúc SSO login:
   validate OIDC token một lần, **create-or-link** user egret, **ghi bền** group
   claim vào `user_groups` (write-through, vì DB nay nuôi việc phát token). Ngừng
   lưu/dùng refresh token cho truy cập engine.
5. **Engine trust wiring** (`egret-bin/main.rs:~1514` + provisioning) —
   `engine_jwks_uri`/`engine_oidc_issuer` trỏ vào JWKS + issuer của **machine-IdP**
   (URL pod-reachable), **không phụ thuộc `config.oidc.enabled`**. Audience thống
   nhất thành `aud:"engine"` (StarRocks `required_audience="engine"` trong template
   `CREATE SECURITY INTEGRATION`; Trino JWT config khớp theo) — một thay đổi nhỏ ở
   template DDL, đồng nhất mọi engine.
6. **Session lifetime policy** (`sessions` + auth middleware) — sliding lifetime +
   idle timeout; **user-disable vô hiệu hoá session** (đường revoke giữ lại lúc này).

**Bỏ**

7. Đường Keycloak refresh-grant cho engine (`engine_broker.rs`) và phụ thuộc
   `oidc_refresh_tokens` **cho truy cập engine**. Bảng có thể giữ cho mục đích
   khác hoặc drop ở follow-up dọn dẹp (không làm ở đây).

## 5. Luồng dữ liệu

**Login (2 nguồn authN → 1 session):**

- **SSO:** browser → egret OIDC login → Keycloak → callback. egret validate OIDC
  token một lần, rút danh tính + group, JIT create-or-link user, ghi group →
  `user_groups`, mint session egret. Vứt token/refresh Keycloak.
- **Local (kể cả local admin):** browser → egret local login → validate với user
  store của egret → session egret. Không đụng IdP.

**Query engine (notebook / SQL Lab / BI — cùng seam):**

```
kernel/backend ──kernel-token hoặc session──▶ EngineTokenMinter::mint(user_id)
                                               ├─ load (username, groups) từ users + user_groups
                                               ├─ engine_claims{ sub, preferred_username, groups, aud:"engine", exp }
                                               └─ sign_claims(active machine-IdP key) ─▶ JWT do egret ký
kernel ──JWT (cleartext-pw / JWTAuth) qua pinned TLS──▶ StarRocks / Trino
engine ──validate theo JWKS + issuer egret + aud:"engine"──▶ principal = preferred_username
Ranger ──enforce per-user theo preferred_username──▶ (mask / row-filter / allow — y nguyên)
```

Enforcement **giống hệt từng byte** so với hiện tại: engine key theo
`preferred_username`; policy Ranger khớp cùng username. Chỉ đổi *người ký* token và
*issuer được trust*.

**Refresh token (kernel sống lâu):** engine JWT ngắn hạn (~15 phút). Kernel cache tới
gần hết hạn rồi callback (`/api/kernel/engine-token` / `notebook-runtime/token`) →
egret re-mint từ session. Không Keycloak. Chạy được chừng nào session egret còn hiệu
lực.

**Trigger login lại duy nhất:** khi session egret hết hạn, callback trả 401 → user
login lại. Keycloak token/refresh hết hạn không liên quan việc query.

**Case động lực, đã giải:** local admin → local login → session → `mint` chạy (không
refresh token). `NoRefreshToken` biến mất.

## 6. Migration & rollout

**Bất biến thứ tự:** engine phải *trust* egret **trước khi** token của user thành
egret-signed, nếu không nó reject. Rollout là trust trước, mint sau.

- **Engine đang chạy** (trust issuer Keycloak): re-wire qua đường engine **update**
  sẵn có (update đã re-run bootstrap DDL của FE, idempotent). **Dual-trust:** thêm
  `egret_machine` (JWKS egret) song song `egret_oidc` (Keycloak) trong
  `authentication_chain` → cả token cũ lẫn mới đều auth được — zero downtime. Khi mọi
  mint đã egret-signed, follow-up drop integration Keycloak → **end state egret-only**.
  Với dev/kind, đơn giản hơn là delete + re-provision.
- **OIDC on/off — cải thiện tuyệt đối:** issuer machine-IdP egret tồn tại bất kể
  `config.oidc.enabled`, nên `engine_jwks_uri`/`issuer` trỏ vào nó vô điều kiện →
  jwt-delegation nay chạy cả khi OIDC **off**. Đường empty-issuer hỏng (và warning)
  biến mất.
- **Consumer không đổi:** kernel + SQL Lab + BI đi qua trait `EngineTokenMinter`,
  không soi claim/aud — không đổi kernel-image hay consumer.
- **Backfill dữ liệu:** `user_groups` thành nguồn group của token; SSO user được ghi
  group ở lần login kế (JIT), user cũ lười theo cùng cách. Không migration chặn —
  tệ nhất token đầu sau cutover thiếu group tới khi login lại (Ranger fallback về
  policy mức user).
- **An toàn rollback (tuỳ chọn):** giữ minter Keycloak sau một config flag chuyển
  tiếp (`engine_token_issuer: egret | keycloak`, mặc định `egret`) để fallback không
  cần redeploy; bỏ flag + đường chết ở cleanup. Cắt nếu muốn cutover sạch.

## 7. Xử lý lỗi & bảo mật

**Bất biến giữ nguyên:**
- **Per-user scoping giữ.** Mỗi engine JWT scoped 1 user + ngắn hạn; kernel untrusted
  chỉ giữ token của chính user đó, không impersonate ai. (Đây là lý do mint bằng
  machine-IdP thay vì nhét service credential — impersonate mode — vào kernel.)
- **Transport không đổi.** Guard fail-closed bearer-requires-TLS (#302) vẫn áp;
  token egret-signed vẫn đi qua pinned TLS.

**Trust root & quản key:** key ký trong `oidc_signing_keys` (đã có cho catalog).
`PgSigningKeyStore` rotate qua `active` key trong khi `/jwks` publish key cũ+mới →
rotate zero-downtime. Khuyến nghị rotate định kỳ + TTL ngắn. Bán kính thiệt hại khi
lộ key = mint-mọi-user; giảm thiểu bằng access control DB + rotate + TTL ngắn (KMS/HSM
là note tương lai).

**Tư thế revoke (đánh đổi đã chấp nhận):** user-disable phía egret vô hiệu hoá session
→ mint/refresh kế fail; token đã phát chỉ sống tới khi hết hạn. **TTL token là độ mịn
của revoke** (~15 phút chặn thời gian dư của user bị disable). Để dành: revoke từ IdP
+ back-channel logout.

**Các chế độ lỗi:**
1. **Mint fail** (DB chết / không có active key) → `mint` báo lỗi → consumer hiện
   "không lấy được engine token, thử lại". Không fallback mất an toàn.
2. **Session hết hạn** → callback 401 → nhắc login lại rõ ràng.
3. **Engine không tới được JWKS egret** → reject mọi JWT. URL JWKS phải pod-reachable
   (đúng yêu cầu như JWKS Keycloak — `egret.localtest.me` resolve toàn cluster qua
   CoreDNS rewrite); engine cache keyset nên egret chớp nháy không làm hỏng auth.
4. **Lệch đồng hồ** → `exp`/`iat` fail. Cho leeway nhỏ + giả định NTP.
5. **Thiếu group** (user_groups chưa backfill) → Ranger fallback policy mức user.
   Suy giảm chứ không hỏng.

## 8. Kiểm thử

**Unit (thuần):** `engine_claims` table-driven (username == slug `ensure_username`,
có group, `aud:"engine"`, `exp == iat + ttl`, có `iss`); sign→verify round-trip với
test JWKS; biên hết hạn + leeway đồng hồ.

**Integration (DB, `sqlx::PgPool`):**
- `MachineIdpEngineMinter::mint(user_id)`: seed `users` + `user_groups` → mint →
  decode → assert claim. Không Keycloak.
- JIT provisioning: giả lập callback OIDC → assert create-or-link user + ghi group.
- **Regression local-admin:** user local không refresh token mint thành công — guard
  để `NoRefreshToken` không bao giờ tái diễn.
- Session policy: session hết hạn / bị disable → mint & refresh fail rõ ràng.

**Contract:** swap `EngineTokenMinter` — test notebook-runtime / SQL Lab / BI hiện có
pass với impl mới inject vào.

**Live e2e (kind, opt-in):**
- **Local admin → notebook `starrocks("SELECT 1")` → ra row** (đúng case fail cả
  chuỗi này; nay xanh, không Keycloak/refresh token). Bằng chứng đinh.
- **SSO parity:** admin-sso (OIDC → JIT) full vs member-sso masked/row-filtered trên
  cùng query — chứng minh enforcement parity. Tái dùng harness `e2e-kind-per-user`,
  nay chạy bằng token egret-minted.
- **Deployment OIDC-off:** provision jwt-delegation với OIDC tắt → query vẫn chạy
  (đường empty-issuer từng hỏng).
- **Migration:** engine dual-trust nhận cả token Keycloak-minted lẫn egret-minted;
  egret-only sau khi drop integration Keycloak.

## 9. Ngoài phạm vi (spec này)

- CRUD user local + UI control-plane (sub-project 2, phụ thuộc spec này).
- Revoke từ IdP + back-channel (SSO single-logout) — tương lai.
- KMS/HSM cho key ký — tương lai.
- Drop bảng `oidc_refresh_tokens` / dọn Keycloak khác — follow-up.
