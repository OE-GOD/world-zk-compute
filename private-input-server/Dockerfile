# Private Input Server — minimal Python stdlib HTTP server
FROM python:3.14-slim

# Security: create non-root user
RUN groupadd --gid 1000 appuser && \
    useradd --uid 1000 --gid 1000 --create-home appuser

WORKDIR /app

COPY server.py .

# No requirements.txt needed — uses only Python stdlib

USER appuser

EXPOSE 3001

HEALTHCHECK --interval=10s --timeout=3s --retries=5 \
    CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:3001/health')" || exit 1

ENTRYPOINT ["python", "-u", "server.py"]
