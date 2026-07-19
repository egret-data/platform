# egret own-ingress Helm TLS — Thiết kế

**Ngày:** 2026-07-19 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/egret-own-ingress-tls` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-19-egret-own-ingress-tls-design.md`.

## Vấn đề

Ingress của chính egret (`egret.localtest.me`, `deploy/helm/egret/templates/ingress.yaml`) chỉ HTTP — không có block `tls:`. P2/P3/StarRocks đã đưa typed-TLS lên các bề mặt coordinator/ingress của engine, nhưng endpoint public của chính egret vẫn chưa phục vụ HTTPS được. Theo quyết định scope "cert everywhere", capstone global-cert gộp vào thay đổi deploy-time cụ thể này (không làm cơ chế platform-default).

## Mục tiêu — slice này

Cho Ingress của chính egret phục vụ HTTPS bằng cert, cấu hình qua Helm chart. Hai chế độ: **bring-your-own** k8s TLS Secret có sẵn (vd cert-manager) qua `ingress.tls.secretName`, hoặc **inline** cert/key mà chart tự tạo thành Secret `egret-ingress-tls`. Backward-compatible hoàn toàn: `ingress.tls.enabled=false` (mặc định) giữ Ingress HTTP-only như hiện tại.

## Ngoài phạm vi

- Cơ chế platform-default / chọn cert runtime (đã bỏ ở quyết định scope — engine ingress đã tham chiếu cert riêng qua P3/SR).
- Resource `Certificate`/`Issuer` của cert-manager (chart tham chiếu 1 Secret; tạo nó qua cert-manager là lựa chọn của operator qua `secretName`).

## Bối cảnh — bề mặt đã grounding (verify)

- **`deploy/helm/egret/templates/ingress.yaml`**: `{{- if .Values.ingress.enabled }}` … `spec: { ingressClassName, rules: [{ host, http: { paths: [/auth→keycloak:8080, /→egret-frontend:80] } }] }`. Không có `tls:`.
- **`deploy/helm/egret/values.yaml`**: `ingress: { enabled: false, host: egret.localtest.me, className: nginx }`.

## Kiến trúc

### A. `values.yaml` — mở rộng `ingress`

```yaml
ingress:
  enabled: false
  host: egret.localtest.me
  className: nginx
  tls:
    enabled: false
    # Bring-your-own k8s TLS Secret có sẵn (vd cert-manager). Thắng khi set.
    secretName: ""
    # HOẶC inline PEM cert+key — chart tạo `egret-ingress-tls` từ chúng.
    cert: ""
    key: ""
```

### B. `templates/ingress.yaml` — thêm block `tls:`

Trong `spec:` hiện có (vẫn gate bởi `ingress.enabled`), thêm khi `ingress.tls.enabled`:

```yaml
{{- if .Values.ingress.tls.enabled }}
  tls:
    - hosts:
        - {{ .Values.ingress.host }}
      secretName: {{ .Values.ingress.tls.secretName | default "egret-ingress-tls" }}
{{- end }}
```

### C. `templates/ingress-tls-secret.yaml` — mới (chỉ chế độ inline)

Tạo Secret `kubernetes.io/tls` chỉ khi có inline cert/key và KHÔNG tham chiếu `secretName` ngoài:

```yaml
{{- if and .Values.ingress.enabled .Values.ingress.tls.enabled (not .Values.ingress.tls.secretName) .Values.ingress.tls.cert .Values.ingress.tls.key }}
apiVersion: v1
kind: Secret
type: kubernetes.io/tls
metadata:
  name: egret-ingress-tls
  namespace: {{ .Release.Namespace }}
data:
  tls.crt: {{ .Values.ingress.tls.cert | b64enc }}
  tls.key: {{ .Values.ingress.tls.key | b64enc }}
{{- end }}
```

Ưu tiên: `secretName` tường minh luôn thắng (không tạo Secret chart-managed); inline cert/key chỉ dùng khi `secretName` rỗng.

## Kiểm thử / acceptance

- **`helm template`** (assert manifest render):
  - `ingress.tls.enabled=false` (mặc định) → Ingress không có `tls:` và không có Secret `egret-ingress-tls` (backward-compatible).
  - inline mode (`tls.enabled=true`, `tls.cert`+`tls.key` set, `secretName` rỗng) → Ingress `spec.tls[0].secretName == egret-ingress-tls` và render Secret `egret-ingress-tls` `kubernetes.io/tls` với b64 cert/key.
  - byo mode (`tls.enabled=true`, `secretName: my-tls`) → Ingress tham chiếu `my-tls` và KHÔNG render Secret của chart.
- **Live (opt-in, kind):** `helm upgrade` release egret với `ingress.tls.enabled=true` + inline cert (SAN=`egret.localtest.me`) → `curl https://egret.localtest.me/api/auth/config` qua nginx serve cert cung cấp (fingerprint khớp) và trả 200.

## Rủi ro

1. **SAN cert** phải khớp `ingress.host` (`egret.localtest.me`), nếu không client từ chối (nginx vẫn serve).
2. **Secret inline trong values** — `tls.cert`/`tls.key` trong values nhạy cảm; ghi rõ production nên ưu tiên `secretName` (cert-manager), inline cho dev. Không bao giờ commit cert thật vào values của repo.
3. **Deploy hiện có** — mặc định `tls.enabled=false` giữ mọi deploy hiện tại HTTP-only; không migration.

## Tiếp theo

Consumer P3 còn lại (OIDC/notebook/cron) và các type typed-secret `dockerconfigjson`/`ssh-auth` — mỗi cái spec → plan riêng.
