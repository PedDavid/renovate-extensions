{{- define "chart-notes.name" -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "chart-notes.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "chart-notes.labels" -}}
app.kubernetes.io/name: {{ include "chart-notes.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{- define "chart-notes.selectorLabels" -}}
app.kubernetes.io/name: {{ include "chart-notes.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "chart-notes.publicUrl" -}}
{{- if .Values.publicUrl -}}
{{- .Values.publicUrl -}}
{{- else if .Values.ingress.enabled -}}
{{- printf "https://%s" .Values.ingress.host -}}
{{- end -}}
{{- end -}}
