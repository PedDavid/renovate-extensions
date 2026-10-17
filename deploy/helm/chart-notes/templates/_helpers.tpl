{{- define "chart-notes.labels" -}}
app.kubernetes.io/name: chart-notes
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end }}

{{- define "chart-notes.selectorLabels" -}}
app.kubernetes.io/name: chart-notes
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}
