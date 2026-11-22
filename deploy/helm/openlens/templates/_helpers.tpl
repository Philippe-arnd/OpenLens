{{- define "openlens.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "openlens.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{- end -}}

{{- define "openlens.selector" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "openlens.tokenSecret" -}}
{{- default (printf "%s-admin" (include "openlens.fullname" .)) .Values.adminToken.existingSecret -}}
{{- end -}}

{{- define "openlens.image" -}}
{{ .Values.image.repository }}:{{ default .Chart.AppVersion .Values.image.tag }}
{{- end -}}

{{- define "openlens.publicUrl" -}}
{{- if .Values.publicUrl -}}{{ .Values.publicUrl }}{{- else if and .Values.ingress.enabled .Values.ingress.host -}}https://{{ .Values.ingress.host }}{{- end -}}
{{- end -}}

{{/* Shared hardening for every OpenLens container. */}}
{{- define "openlens.securityContext" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
runAsNonRoot: true
runAsUser: 65532
runAsGroup: 65532
capabilities:
  drop: [ALL]
{{- end -}}
