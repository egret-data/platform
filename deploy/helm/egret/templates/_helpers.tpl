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
