{{/* The base URL a BROWSER uses to reach egret, scheme included.

     `https` when the ingress terminates TLS, `http` otherwise. Hardcoding
     `http://` was a real defect on three keys at once, and the OIDC ones bite
     hardest: an issuer that disagrees with the browser's scheme fails token
     validation, and a redirect_uri that does not match what the provider has
     registered fails the login outright. The notification link merely arrives
     wrong.

     Deliberately keyed on `tls.enabled` rather than on whether a cert is set —
     `secretName` (bring-your-own, e.g. cert-manager) and inline `cert`/`key`
     are two ways of doing the same thing, and either means TLS. */}}
{{- define "egret.externalUrl" -}}
{{- if .Values.ingress.tls.enabled -}}https://{{ .Values.ingress.host }}{{- else -}}http://{{ .Values.ingress.host }}{{- end -}}
{{- end -}}
{{- define "egret.fullname" -}}egret{{- end -}}
{{/* Reuse an existing egret-secrets value if present (stable across upgrades),
     else generate. master_key >=32 bytes b64; secret_key >=64 bytes b64. */}}
{{- define "egret.masterKey" -}}
{{- $s := (lookup "v1" "Secret" .Release.Namespace "egret-secrets") -}}
{{- if and $s (index $s.data "master-key") -}}{{ index $s.data "master-key" | b64dec }}{{- else -}}{{ randBytes 32 }}{{- end -}}
{{- end -}}
{{- define "egret.secretKey" -}}
{{- $s := (lookup "v1" "Secret" .Release.Namespace "egret-secrets") -}}
{{- if and $s (index $s.data "secret-key") -}}{{ index $s.data "secret-key" | b64dec }}{{- else -}}{{ randBytes 64 }}{{- end -}}
{{- end -}}
{{- define "egret.catalogAuthToken" -}}
{{- $s := (lookup "v1" "Secret" .Release.Namespace "egret-secrets") -}}
{{- if and $s (index $s.data "catalog-auth-token") -}}{{ index $s.data "catalog-auth-token" | b64dec }}{{- else -}}{{ randAlphaNum 40 }}{{- end -}}
{{- end -}}
{{/* An egret image published by .github/workflows/images.yml, at the chart's appVersion:
     `include "egret.image" (dict "name" "egret-apps" "root" $)`. */}}
{{- define "egret.image" -}}ghcr.io/egret-data/{{ .name }}:{{ .root.Chart.AppVersion }}{{- end -}}
