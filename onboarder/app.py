"""
homelab-onboarder — UI Flask pour creer un user via homelab-onboard-user.sh

Endpoint :
  GET  /            -> form HTML
  POST /onboard     -> JSON {username, email} -> exec script -> JSON resultat
  GET  /health      -> 200 OK
"""
import os
import re
import subprocess
import time
from collections import defaultdict
from threading import Lock

from flask import Flask, render_template, request, jsonify

app = Flask(__name__)

SCRIPT_PATH = os.environ.get("ONBOARD_SCRIPT", "/scripts/homelab-onboard-user.sh")
RATE_LIMIT_SECONDS = int(os.environ.get("RATE_LIMIT_SECONDS", "30"))

USERNAME_RE = re.compile(r"^[a-zA-Z0-9_-]{2,32}$")
EMAIL_RE = re.compile(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$")

_last_req = defaultdict(float)
_lock = Lock()


def _client_ip():
    # Trust X-Forwarded-For if set by NPM
    fwd = request.headers.get("X-Forwarded-For", "")
    if fwd:
        return fwd.split(",")[0].strip()
    return request.remote_addr or "unknown"


def _rate_limited(ip):
    now = time.time()
    with _lock:
        last = _last_req[ip]
        if now - last < RATE_LIMIT_SECONDS:
            return True
        _last_req[ip] = now
    return False


def _parse_script_output(stdout):
    """Extract creds from homelab-onboard-user.sh stdout block."""
    res = {
        "username": None,
        "email": None,
        "password": None,
        "jellyfin_id": None,
        "jellyseerr_id": None,
        "jellyfin_url": None,
        "jellyseerr_url": None,
    }
    for line in stdout.splitlines():
        line = line.strip()
        if line.startswith("Username"):
            res["username"] = line.split(":", 1)[1].strip()
        elif line.startswith("Email"):
            res["email"] = line.split(":", 1)[1].strip()
        elif line.startswith("Password"):
            res["password"] = line.split(":", 1)[1].strip()
        elif line.startswith("Jellyfin Id"):
            res["jellyfin_id"] = line.split(":", 1)[1].strip()
        elif line.startswith("Jellyseerr id"):
            res["jellyseerr_id"] = line.split(":", 1)[1].strip()
        elif line.startswith("Streaming"):
            res["jellyfin_url"] = line.split(":", 1)[1].strip()
        elif line.startswith("Requetes") or line.startswith("Requêtes"):
            res["jellyseerr_url"] = line.split(":", 1)[1].strip()
    return res


@app.get("/")
def index():
    return render_template("index.html")


@app.get("/health")
def health():
    return {"status": "ok"}, 200


@app.post("/onboard")
def onboard():
    ip = _client_ip()
    data = request.get_json(silent=True) or {}
    username = (data.get("username") or "").strip()
    email = (data.get("email") or "").strip().lower()

    if not USERNAME_RE.match(username):
        return jsonify({"success": False, "error": "Username invalide (2-32 chars, lettres/chiffres/_-)"}), 400
    if not EMAIL_RE.match(email):
        return jsonify({"success": False, "error": "Email invalide"}), 400

    if _rate_limited(ip):
        return jsonify({"success": False, "error": f"Rate limit ({RATE_LIMIT_SECONDS}s entre requetes)"}), 429

    if not os.path.isfile(SCRIPT_PATH):
        return jsonify({"success": False, "error": f"Script {SCRIPT_PATH} introuvable"}), 500

    # Exec script — env vars heritees du container (.env mounted)
    try:
        proc = subprocess.run(
            ["bash", SCRIPT_PATH, username, email],
            capture_output=True,
            text=True,
            timeout=120,
            env={**os.environ},
        )
    except subprocess.TimeoutExpired:
        return jsonify({"success": False, "error": "Script timeout (120s)"}), 504

    log = (proc.stdout or "") + "\n--- stderr ---\n" + (proc.stderr or "")

    if proc.returncode != 0:
        return jsonify({
            "success": False,
            "error": f"Script exit code {proc.returncode}",
            "log": log,
        }), 500

    parsed = _parse_script_output(proc.stdout)
    if not parsed["password"]:
        return jsonify({
            "success": False,
            "error": "Impossible de parser les credentials depuis stdout",
            "log": log,
        }), 500

    return jsonify({
        "success": True,
        **parsed,
        "log": log,
    })


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=8765)
