#!/usr/bin/env python3
"""Private TLS OAuth/Anthropic/GitHub protocol fixture; never records secrets.

This is not a subscription service. Run only in the dedicated nested lab and
supply a generated mode-0600 config. No production DNS or auth is consulted.
"""
import argparse, json, ssl, time, urllib.parse, uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def record(self, **values):
        with self.server.lock:
            self.server.events.append(values)
            self.server.evidence.write_text(
                json.dumps(self.server.events, indent=2) + "\n"
            )

    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def do_GET(self):
        self.handle_request(None)

    def do_POST(self):
        size = int(self.headers.get("Content-Length", "0"))
        if size > 16 * 1024**2:
            return self.reply(413, {})
        data = self.rfile.read(size)
        try:
            body = json.loads(data)
        except ValueError:
            body = {k: v[-1] for k, v in urllib.parse.parse_qs(data.decode()).items()}
        self.handle_request(body)

    def handle_request(self, body):
        c = self.server.config
        p = urllib.parse.urlsplit(self.path).path
        host = self.headers.get("host", "").split(":")[0]
        auth = self.headers.get("authorization", "")
        is_codex = auth == "Bearer " + c["codexAccess"]
        is_claude = auth == "Bearer " + c["claudeAccess"]
        is_gh = auth in ["token " + c["ghToken"], "Bearer " + c["ghToken"]]
        if p in ["/oauth/token", "/v1/oauth/token"]:
            tool = "codex" if host == "auth.openai.com" else "claude"
            ok = (
                body is not None
                and body.get("refresh_token")
                in [c[tool + "Refresh"], c[tool + "RotatedRefresh"]]
                and body.get("grant_type") == "refresh_token"
            )
            self.record(
                tool=tool, path=p, refresh=True, authenticated=ok, jsonOrForm=True
            )
            if not ok:
                return self.reply(401, {"error": "invalid_grant"})
            return self.reply(
                200,
                {
                    "access_token": c[tool + "Access"],
                    "refresh_token": c[tool + "RotatedRefresh"],
                    "id_token": c["codexId"],
                    "expires_in": 3600,
                    "token_type": "Bearer",
                    "scope": "user:inference user:profile user:sessions:claude_code",
                },
            )
        self.record(
            host=host,
            path=p,
            method=self.command,
            codexAuthenticated=is_codex,
            claudeAuthenticated=is_claude,
            githubAuthenticated=is_gh,
            apiKeyAbsent=not self.headers.get("x-api-key"),
            model=(body or {}).get("model"),
            stream=(body or {}).get("stream") is True,
        )
        if host in ["github.com", "api.github.com"]:
            return self.reply(
                200 if is_gh else 401,
                {"login": "synthetic-owner", "id": 123, "type": "User"},
            )
        if p.endswith("/accounts/check"):
            return self.reply(
                200,
                {
                    "accounts": [
                        {
                            "id": "synthetic-account",
                            "workspace_backend_origin": "https://chatgpt.com",
                            "account_routing_override": "NO_CONSTRAINT",
                        }
                    ]
                },
            )
        if p in ["/api/oauth/profile", "/api/oauth/claude_cli/client_data"]:
            return self.reply(
                200,
                {
                    "account": {
                        "uuid": "synthetic-user",
                        "email": "fixture@test.invalid",
                        "has_claude_max": True,
                        "has_claude_pro": True,
                        "display_name": "Synthetic fixture",
                    },
                    "organization": {
                        "uuid": "synthetic-org",
                        "organization_type": "claude_max",
                        "rate_limit_tier": "default_claude_max_5x",
                        "billing_type": "stripe_subscription",
                        "has_extra_usage_enabled": False,
                    },
                },
            )
        if p.endswith("/models"):
            return self.reply(
                200,
                {
                    "data": [
                        {
                            "id": "claude-sonnet-4-6",
                            "type": "model",
                            "display_name": "Synthetic Sonnet",
                            "created_at": "2026-01-01T00:00:00Z",
                        }
                    ],
                    "models": [],
                    "has_more": False,
                },
            )
        if p == "/v1/messages/count_tokens":
            return self.reply(200, {"input_tokens": 12})
        if p in ["/v1/messages", "/messages"]:
            if not is_claude or self.headers.get("x-api-key"):
                return self.reply(
                    401,
                    {
                        "type": "error",
                        "error": {
                            "type": "authentication_error",
                            "message": "Fixture requires native OAuth",
                        },
                    },
                )
            message = {
                "id": "msg_" + uuid.uuid4().hex,
                "type": "message",
                "role": "assistant",
                "model": body["model"],
                "content": [{"type": "text", "text": "SYNTHETIC_NATIVE_OAUTH_OK"}],
                "stop_reason": "end_turn",
                "stop_sequence": None,
                "usage": {"input_tokens": 10, "output_tokens": 5},
            }
            if not body.get("stream"):
                return self.reply(200, message)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            events = [
                (
                    "message_start",
                    {
                        "type": "message_start",
                        "message": dict(
                            message,
                            content=[],
                            stop_reason=None,
                            usage={"input_tokens": 10, "output_tokens": 0},
                        ),
                    },
                ),
                (
                    "content_block_start",
                    {
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {"type": "text", "text": ""},
                    },
                ),
                (
                    "content_block_delta",
                    {
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {
                            "type": "text_delta",
                            "text": "SYNTHETIC_NATIVE_OAUTH_OK",
                        },
                    },
                ),
                ("content_block_stop", {"type": "content_block_stop", "index": 0}),
                (
                    "message_delta",
                    {
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                        "usage": {"output_tokens": 5},
                    },
                ),
                ("message_stop", {"type": "message_stop"}),
            ]
            try:
                for name, value in events:
                    self.wfile.write(
                        (
                            "event: " + name + "\ndata: " + json.dumps(value) + "\n\n"
                        ).encode()
                    )
                    self.wfile.flush()
                    time.sleep(0.01)
            except OSError:
                pass
            self.close_connection = True
            return
        return self.reply(200, {})


def main():
    import threading

    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--config", type=Path, required=True)
    p.add_argument("--certificate", required=True)
    p.add_argument("--key", required=True)
    p.add_argument("--evidence", type=Path, required=True)
    a = p.parse_args()
    s = ThreadingHTTPServer(("0.0.0.0", 443), Handler)
    s.daemon_threads = True
    s.lock = threading.Lock()
    s.config = json.loads(a.config.read_text())
    s.events = []
    s.evidence = a.evidence
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(a.certificate, a.key)
    s.socket = ctx.wrap_socket(s.socket, server_side=True)
    s.serve_forever(0.1)


if __name__ == "__main__":
    main()
