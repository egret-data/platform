# Engine JWT Delegation Bật Mặc Định — Thiết kế

> **Trạng thái:** Đã duyệt thiết kế. Bước tiếp: viết plan.
> **Xây trên:** STS (#304–308). Machine-IdP của egret đã là single issuer của
> engine token per-user; việc này khiến engine *dùng* nó theo mặc định.
> *(Bản tiếng Anh `docs/en/...` là chuẩn; bản này để tham chiếu.)*

**Mục tiêu:** Khi admin provision Trino/StarRocks **không** chỉ định delegation
mode, egret mặc định dùng **per-user JWT delegation** (engine tin machine-IdP của
egret) thay vì engine "open" service-user như hiện tại. Khi không thể dùng
per-user JWT trên platform, egret **tự hạ xuống `impersonate`** thay vì fail hay
tạo engine hỏng — nên provision mặc định luôn được governed per-user và không bao
giờ hard-fail.

## 1. Quyết định (đã chốt)

1. **Nơi đặt default:** backend **default-when-omitted** — `delegationMode` bỏ
   trống/blank được resolve thành mode cụ thể ở server (§3). FE wizard cũng phản
   ánh (pre-select JWT, thêm opt-out mới). Đây là default-on thật, không chỉ UI.
2. **Degrade nhẹ nhàng, không hard-fail ở default path:** default *ưu tiên* `jwt`
   nhưng tự hạ xuống `impersonate` khi không đảm bảo được `jwt` (không có JWKS
   platform, hoặc image StarRocks rõ ràng cũ hơn `authentication_jwt`).
   `impersonate` không cần JWKS, không cần version tối thiểu, chạy cho mọi user
   (local + OIDC) → là fallback an toàn phổ quát. Vì vậy default path **không bao
   giờ hard-fail** và không tạo engine hỏng.
3. **Chỉ hard-fail khi yêu cầu *tường minh* bất khả thi:** nếu admin truyền tường
   minh `delegationMode:"jwt"` mà platform không có machine-IdP JWKS
   (`catalog.public_url` chưa set) → trả **400** — tôn trọng ý định tường minh,
   không âm thầm hạ cấp. (Trino `resolve_auth` đã có; StarRocks bổ sung guard này.)
4. **Opt-out tường minh:** `delegationMode:"none"` (giá trị mới được chấp nhận)
   tạo engine **open** — hành vi trước thay đổi — để deploy muốn engine service-user,
   hoặc chưa có `public_url`, vẫn có đường thoát.
5. **StarRocks TLS:** mode resolved = `jwt` tự bật FE TLS trên :9030 + sinh cert
   per-engine (StarRocks gửi JWT như cleartext password → bắt buộc TLS, connector
   đã fail-closed). Mode resolved = `impersonate` giữ nguyên hành vi hiện tại
   (không có cleartext JWT).

## 2. Seam sẵn có (tái dùng)

Cả hai engine đã key delegation theo chuỗi `delegationMode` và derive toạ độ OIDC
server-side (không lấy từ request):

- **`st.engine_jwks_uri` / `st.engine_oidc_issuer`** (`egret-bin/main.rs:1518`) —
  JWKS + issuer của machine-IdP egret, set bất kể `oidc.enabled`, từ
  `catalog.public_url`. **Rỗng ⟺ `public_url` chưa set** — đây là tín hiệu tin cậy
  DUY NHẤT cho câu hỏi "có làm được JWT không?" lúc provision.
- **StarRocks:** `StarRocksConfigBody.delegation_mode` (`engines.rs:198`);
  `validate_sr_delegation_mode` (`engines.rs:2478`); create path
  `create_starrocks_engine` (`engines.rs:2714`) persist block `delegation {mode,
  tlsEnabled}`; provision-req builder (`engines.rs:~2314`) đọc lại thành
  `StarRocksDelegation` + `StarRocksDelegationBootstrap`;
  `starrocks_jwt_integration_sql` (`engines.rs:3099`) render DDL
  `CREATE SECURITY INTEGRATION ... authentication_jwt`.
- **Trino:** `trinoConfig.delegationMode` (`"jwt"|"impersonate"`) + block
  `trinoConfig.auth.jwt` do `resolve_auth` xử lý (`engines.rs:1341`, đã default
  `jwksUri` rỗng về `st.engine_jwks_uri`, audience về `"engine"`, và đã 400 khi JWT
  được yêu cầu mà không có JWKS); `inject_delegator_user` (`engines.rs:1456`) seed
  service user impersonate; `build_provision_req` (`engines.rs:544`) lắp request.
- **Connector fail-closed:** `StarRocksConnector` từ chối bearer trên socket
  non-TLS (`egret-query/.../connector/starrocks.rs`) → engine jwt không TLS đã
  không dùng được; §1.5 khiến provision bật TLS nên case này không xảy ra.

## 3. Quy tắc resolve default

Một helper duy nhất quyết định mode cụ thể lúc **create** và kết quả được
**persist** vào block delegation của stored spec. Vì mode resolved được lưu,
provision-req builder và đường "UPDATE giữ giá trị đã lưu" không cần đổi.

```
resolve_delegation(explicit: Option<&str>, has_jwks: bool, sr_image_tag: Option<&str>) -> Resolved
  match explicit {
    Some("none")                    => Open,                    // opt-out
    Some("impersonate")             => Impersonate,             // honored
    Some("jwt") if has_jwks         => Jwt,                     // honored
    Some("jwt")                     => Err(400 no-JWKS),        // tường minh + bất khả thi
    None | Some("")  =>                                          // DEFAULT (bỏ trống)
      if has_jwks && !starrocks_image_is_pre_3_5(sr_image_tag)  => Jwt
      else                                                      => Impersonate
  }
```

- **`has_jwks`** = `!engine_jwks_uri.is_empty() && !engine_oidc_issuer.is_empty()`.
- **`starrocks_image_is_pre_3_5`** = best-effort parse tag `fe_image`
  (`starrocks/fe-ubuntu:<tag>`): nếu tag parse rõ ra `major.minor < 3.5` → true;
  tag không parse được/vắng → **false** (giả định capable — default image 4.1). Chỉ
  **lái CHỌN default sang `impersonate`**, không bao giờ hard-fail. Với Trino,
  `sr_image_tag` luôn `None` (JWT authenticator Trino không có version floor), nên
  default Trino thuần là `has_jwks ? jwt : impersonate`.

**Áp dụng mỗi engine:**
- **StarRocks** (`create_starrocks_engine`): resolve, persist `mode` resolved vào
  block `delegation`; set `tlsEnabled=true` khi resolved `jwt`; khi resolved
  `Open` không persist block delegation (engine open, như hiện tại).
- **Trino** (`build_provision_req` / `resolve_auth`): resolve; resolved `jwt`
  synthesize block `auth.jwt` (JWKS nền + aud `"engine"`) và set
  `delegationMode="jwt"`; resolved `impersonate` set `delegationMode="impersonate"`
  (→ `inject_delegator_user`); `Open` để trống auth/delegation.

## 4. Bảng kết quả (admin **không** gửi field delegation)

| Platform | Trino | StarRocks |
|---|---|---|
| `public_url` set, SR image ≥3.5 (default 4.1) | **jwt** | **jwt + FE TLS** |
| `public_url` set, SR image rõ ràng <3.5 | **jwt** | **impersonate** |
| `public_url` chưa set | **impersonate** | **impersonate** |

`delegationMode` tường minh: `"jwt"` honored (400 nếu không JWKS), `"impersonate"`
/ `"none"` honored nguyên văn.

## 5. Xử lý lỗi

- Tường minh `delegationMode:"jwt"` + không JWKS → **400 BadRequest**, thông điệp
  nêu cả hai cách sửa: set `catalog.public_url`, hoặc truyền
  `delegationMode:"none"`/`"impersonate"`. (Trino đã có; StarRocks bổ sung.)
- Default path (bỏ trống) → không bao giờ 400 vì capability; degrade sang
  `impersonate`.
- StarRocks resolved `impersonate` do image <3.5 → `tracing::info` ghi nhận degrade
  (observability; không phải lỗi).
- Image StarRocks thật sự cũ truyền kèm **tường minh** `"jwt"` vẫn không detect
  chắc được; DDL `authentication_jwt` sẽ fail lúc bootstrap và hiện thành engine
  error (như hiện tại, xem #303).

## 6. Backward-compat

- **Engine đã tồn tại không đổi** cho tới khi re-provision/update; block delegation
  đã lưu vốn giữ mode tường minh (hoặc none), resolver truyền thẳng qua.
- **Default resolver CHỈ được gọi bởi create path; UPDATE không bao giờ chạy nó.**
  Update với `delegationMode` bỏ trống giữ nguyên block delegation đã lưu — engine
  open cũ không bị âm thầm lật sang jwt khi sửa config không liên quan.
- **`"none"` tái hiện hành vi engine open cũ** cho caller vốn dựa vào "omitted = open".
- OpenAPI: doc default mới + giá trị `"none"`; regenerate `openapi.json` (drift test
  bắt buộc).

## 7. Frontend (phản ánh default)

Selector delegation thêm mục tường minh **"Auto (JWT, else impersonate)"** làm
**lựa chọn mặc định** và **gửi `delegationMode` bỏ trống** — để resolution nhẹ
nhàng của backend (§3) áp dụng. **Không** được gửi `"jwt"` tường minh để biểu diễn
default: `"jwt"` tường minh trên platform không JWKS là 400 theo thiết kế (§5), sẽ
biến degrade nhẹ nhàng thành hard-fail. Selector cũng expose tường minh **"JWT"**,
**"Impersonate"**, và **"None (open engine)"** (`delegationMode:"none"`) cho admin
muốn pin cứng mode. Không hành vi gì ngoài gửi giá trị; backend là authoritative.
(`useEngineConfigForm.ts`, Security tab trong `EngineDetail.tsx`.)

## 8. Kiểm thử

- **Unit (`engines.rs` tests):** StarRocks bỏ trống+JWKS+4.1 ⇒ jwt & `tlsEnabled`;
  bỏ trống+JWKS+tag 3.4 ⇒ impersonate; bỏ trống+không JWKS ⇒ impersonate; tường
  minh `"jwt"`+không JWKS ⇒ 400; tường minh `"none"` ⇒ open. Trino bỏ trống+JWKS ⇒
  jwt (auth.jwt synthesize, aud `"engine"`); bỏ trống+không JWKS ⇒ impersonate;
  tường minh `"jwt"`+không JWKS ⇒ 400. UPDATE bỏ trống giữ stored mode.
  `starrocks_image_is_pre_3_5`: `3.4`/`3.4.10`→true, `4.1`/`latest`/`""`→false.
- **Live e2e (kind, opt-in):** provision một Trino **và** một StarRocks với **không**
  `delegationMode` trên kind deploy có `public_url` ⇒ cả hai lên jwt-delegated; query
  per-user chạy (admin-full vs member-masked), tái dùng harness `sts-broker-kind`.
  Recipe `just e2e-kind-delegation-default`.

## 9. Ngoài phạm vi

- Không retro-migrate engine đang chạy (re-provision để nhận default).
- Không thêm surface override `jwksUri`/`issuer` per-engine cho StarRocks (giữ nguyên).
- Không đổi code path `impersonate` hay `"none"` ngoài việc khiến chúng reachable
  như default/opt-out.
- Engine khác Trino/StarRocks.
