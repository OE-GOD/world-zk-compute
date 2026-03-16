{{/*
Expand the name of the chart.
*/}}
{{- define "worldzk.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this
(by the DNS naming spec). If release name contains chart name it will be used
as a full name.
*/}}
{{- define "worldzk.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "worldzk.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common selector labels (used by enclave, prover, private-input)
*/}}
{{- define "worldzk.selectorLabels" -}}
app.kubernetes.io/name: {{ include "worldzk.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "worldzk.labels" -}}
helm.sh/chart: {{ include "worldzk.chart" . }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: world-zk-compute
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
{{- end }}

{{/*
Operator labels
*/}}
{{- define "worldzk.operator.labels" -}}
{{ include "worldzk.labels" . }}
app.kubernetes.io/name: operator
app.kubernetes.io/component: operator
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Operator selector labels
*/}}
{{- define "worldzk.operator.selectorLabels" -}}
app.kubernetes.io/name: operator
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Indexer labels
*/}}
{{- define "worldzk.indexer.labels" -}}
{{ include "worldzk.labels" . }}
app.kubernetes.io/name: indexer
app.kubernetes.io/component: indexer
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Indexer selector labels
*/}}
{{- define "worldzk.indexer.selectorLabels" -}}
app.kubernetes.io/name: indexer
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Operator fully qualified name
*/}}
{{- define "worldzk.operator.fullname" -}}
{{- printf "%s-operator" (include "worldzk.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Indexer fully qualified name
*/}}
{{- define "worldzk.indexer.fullname" -}}
{{- printf "%s-indexer" (include "worldzk.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Operator service account name
*/}}
{{- define "worldzk.operator.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- include "worldzk.operator.fullname" . }}
{{- else }}
{{- default "default" }}
{{- end }}
{{- end }}

{{/*
Indexer service account name
*/}}
{{- define "worldzk.indexer.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- include "worldzk.indexer.fullname" . }}
{{- else }}
{{- default "default" }}
{{- end }}
{{- end }}

{{/*
Prover fully qualified name
*/}}
{{- define "worldzk.prover.fullname" -}}
{{- printf "%s-prover" (include "worldzk.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Prover labels
*/}}
{{- define "worldzk.prover.labels" -}}
{{ include "worldzk.labels" . }}
app.kubernetes.io/name: prover
app.kubernetes.io/component: prover
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Prover selector labels
*/}}
{{- define "worldzk.prover.selectorLabels" -}}
app.kubernetes.io/name: {{ include "worldzk.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: prover
{{- end }}

{{/*
Check if operator canary is enabled.
Returns "true" if operator.canary.enabled is set, empty string otherwise.
*/}}
{{- define "worldzk.operator.canary.isEnabled" -}}
{{- if .Values.operator }}
{{- if .Values.operator.canary }}
{{- if .Values.operator.canary.enabled }}
{{- printf "true" }}
{{- end }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Check if Flagger canary analysis is enabled.
Returns "true" if operator.canary.flagger.enabled is set, empty string otherwise.
*/}}
{{- define "worldzk.operator.canary.flagger.isEnabled" -}}
{{- if .Values.operator }}
{{- if .Values.operator.canary }}
{{- if .Values.operator.canary.flagger }}
{{- if .Values.operator.canary.flagger.enabled }}
{{- printf "true" }}
{{- end }}
{{- end }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Operator canary fully qualified name
*/}}
{{- define "worldzk.operator.canary.fullname" -}}
{{- printf "%s-operator-canary" (include "worldzk.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Operator canary labels
*/}}
{{- define "worldzk.operator.canary.labels" -}}
{{ include "worldzk.labels" . }}
app.kubernetes.io/name: operator
app.kubernetes.io/component: operator
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/variant: canary
{{- end }}

{{/*
Operator canary selector labels
*/}}
{{- define "worldzk.operator.canary.selectorLabels" -}}
app.kubernetes.io/name: operator
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/variant: canary
{{- end }}

{{/*
Secret name
*/}}
{{- define "worldzk.secretName" -}}
{{- if .Values.secrets.nameOverride }}
{{- .Values.secrets.nameOverride }}
{{- else }}
{{- printf "%s-secrets" (include "worldzk.fullname" .) }}
{{- end }}
{{- end }}

{{/*
ConfigMap name
*/}}
{{- define "worldzk.configMapName" -}}
{{- printf "%s-config" (include "worldzk.fullname" .) }}
{{- end }}
