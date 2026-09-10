FROM python:3.11-slim

# Outils requis par homelab-onboard-user.sh
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl jq openssl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app.py .
COPY templates ./templates
COPY static ./static

ENV PYTHONUNBUFFERED=1
EXPOSE 8765

CMD ["gunicorn", "--bind", "0.0.0.0:8765", "--workers", "2", "--timeout", "60", "--access-logfile", "-", "app:app"]
