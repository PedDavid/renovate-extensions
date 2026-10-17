FROM python:3.14-slim@sha256:cad9a2c871761c413caa6fdd6441c783451e740a48aaeba60ae62a8b53525ef6
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
WORKDIR /app
# Readable by the non-root user whatever the checkout's file modes.
COPY --chmod=0444 chart_notes.py charts.json ./
USER 65534:65534
EXPOSE 8080
ENTRYPOINT ["python", "/app/chart_notes.py"]
