"""Loopback-only management fixture; never starts or updates a real Supervisor."""
import copy
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import threading


class FakeHub:
    def __init__(self):
        self.rows = [{"id": agent, "transport": "adapter", "base": {"installed": False, "path": ""},
                      "adapter": {"installed": False, "version": None, "path": None}, "job": None}
                     for agent in ("codex", "claude")]
        self.rows.append({"id": "grok", "transport": "native", "base": {"path": "/bin/grok"}})
        self.requests = []
        self.healthy = True
        self.health_failures = 0
        self.job_results = {}
        self.running_polls = 1
        self.pending = {}
        self.post_error = None
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, body, status=200):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                fixture.requests.append(("GET", self.path, None))
                if self.path == "/healthz":
                    if fixture.health_failures:
                        fixture.health_failures -= 1
                        self.reply({"status": "starting"}, 503)
                    else:
                        self.reply({"status": "ok" if fixture.healthy else "starting"})
                elif self.path == "/api/management/harnesses":
                    self.reply(fixture.rows)
                elif self.path == "/api/management/jobs":
                    for agent, count in list(fixture.pending.items()):
                        row = fixture.row(agent)
                        if count:
                            fixture.pending[agent] -= 1
                        else:
                            row["job"] = copy.deepcopy(fixture.job_results.get(agent, {
                                "state": "completed", "action": "update", "component": "adapter",
                                "connectionVerified": True}))
                            if row["job"]["state"] == "completed":
                                row["adapter"] = fixture.managed_adapter(agent)
                            del fixture.pending[agent]
                    self.reply({row["id"]: row.get("job") for row in fixture.rows})
                else:
                    self.reply({"message": "Unknown route"}, 404)

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                fixture.requests.append(("POST", self.path, body))
                if fixture.post_error:
                    self.reply({"message": fixture.post_error}, 409)
                    return
                agent = self.path.rsplit("/", 1)[1]
                row = fixture.row(agent)
                row["job"] = {"state": "running", "action": "update", "component": "adapter"}
                fixture.pending[agent] = fixture.running_polls
                self.reply({"state": "running"}, 202)

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_port
        self.thread = threading.Thread(target=self.server.serve_forever,
                                       kwargs={"poll_interval": 0.01}, daemon=True)
        self.thread.start()

    def row(self, agent):
        return next(row for row in self.rows if row["id"] == agent)

    @staticmethod
    def managed_adapter(agent):
        return {"installed": True, "manager": "pockymoe", "version": "1.2.3",
                "path": f"/fixture/.local/share/remote-codex/adapters/installations/{agent}-new/bin/{agent}-acp"}

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
