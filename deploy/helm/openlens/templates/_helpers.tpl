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

{{/* The minor channel of the chart's release: appVersion v0.5.0 -> 0.5. */}}
{{- define "openlens.channel" -}}
{{- $v := semver (trimPrefix "v" .Chart.AppVersion) -}}
{{- printf "%d.%d" $v.Major $v.Minor -}}
{{- end -}}

{{/* With autoUpdate, an empty tag follows the minor channel instead of the
    pinned release, so a restart can bring its patches (#198). */}}
{{- define "openlens.tag" -}}
{{- if .Values.image.tag -}}{{ .Values.image.tag }}
{{- else if .Values.autoUpdate.enabled -}}{{ include "openlens.channel" . }}
{{- else -}}{{ .Chart.AppVersion }}{{- end -}}
{{- end -}}

{{- define "openlens.image" -}}
{{ .Values.image.repository }}:{{ include "openlens.tag" . }}
{{- end -}}

{{/* A channel tag only moves if the node pulls it again. */}}
{{- define "openlens.pullPolicy" -}}
{{- if .Values.autoUpdate.enabled -}}Always{{- else -}}{{ .Values.image.pullPolicy }}{{- end -}}
{{- end -}}

{{/* kubectl for the auto-update Job: Kubernetes' own image (no shell). */}}
{{- define "openlens.kubectlImage" -}}
registry.k8s.io/kubectl:v1.33.0
{{- end -}}

{{/* `/readyz` only exists from v0.3.0 (#399, NOPS-01): pinning an older
    tag must keep the pre-0.3.0 probe, not one the image never answers. A
    tag that is not a plain semver (`latest`, a digest, a custom build) is
    assumed current. */}}
{{- define "openlens.readinessPath" -}}
{{- $v := include "openlens.tag" . -}}
{{- if regexMatch "^v?[0-9]+\\.[0-9]+\\.[0-9]+" $v -}}
{{- if semverCompare ">=0.3.0" $v -}}/readyz{{- else -}}/healthz{{- end -}}
{{- else -}}/readyz{{- end -}}
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
