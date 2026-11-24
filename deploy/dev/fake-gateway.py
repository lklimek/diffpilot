#!/usr/bin/python3
"""Loopback-only, token-free plan gateway for the isolated dev/e2e stack."""

import hmac
import json
import os
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import unquote


class Admin(BaseHTTPRequestHandler):
    """Implement the gateway's credentials scope without any upstream requests."""

    states = {}

    def log_message(self, *_args):
        """Keep auth headers, device codes and session metadata out of logs."""

    def reply(self, status, body=None):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        if body is not None:
            self.wfile.write(json.dumps(body).encode())

    def handle_admin(self):
        if not hmac.compare_digest(
            self.headers.get("Authorization", ""),
            "Bearer " + os.environ["SR_GATEWAY_CREDENTIALS_TOKEN"],
        ):
            self.reply(403)
            return
        parts = self.path.split("/")
        if len(parts) not in (4, 5) or parts[1:3] != ["admin", "credentials"]:
            self.reply(404)
            return
        user = unquote(parts[3])
        linking = len(parts) == 5 and parts[4] == "link"
        if len(parts) == 5 and not linking:
            self.reply(404)
            return
        status = self.states.setdefault(user, {
            "mode": "chatgpt_plan", "state": "relink_needed", "relink_reason": "terminal",
            "label": None, "plan": None, "paused_until": None, "last_call": None,
        })
        length = int(self.headers.get("Content-Length", "0"))
        if length > 16384:
            self.reply(413)
            return
        body = json.loads(self.rfile.read(length)) if length else None
        if self.command == "POST" and linking:
            if body != {"mode": "chatgpt_plan"}:
                self.reply(400)
                return
            status.update(state="linked" if status["state"] == "linked" else "linking", relink_reason=None)
            self.reply(200, {"user_code": "FAKE-PLAN", "verification_url": "http://127.0.0.1/fake-device",
                             "expires_at": (datetime.now(timezone.utc) + timedelta(minutes=5)).isoformat()})
        elif self.command == "GET":
            if linking and status["state"] == "linking":
                status.update(state="linked", label="Fake payer", plan="fake")
            self.reply(200, status)
        elif self.command == "DELETE" and not linking and body == {"user": user}:
            self.states.pop(user, None)
            self.reply(204)
        else:
            self.reply(400)

    do_GET = handle_admin
    do_POST = handle_admin
    do_DELETE = handle_admin


if __name__ == "__main__":
    print("fake gateway admin listening on loopback", flush=True)
    HTTPServer(("127.0.0.1", int(os.environ["SR_FAKE_GATEWAY_PORT"])), Admin).serve_forever()
