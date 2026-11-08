"""Isolated real Relay, private fixture accounts, preview sharing and revocation."""
import http.client
import base64
import hashlib
import json
import os
import secrets
import signal
import socket
import subprocess
import threading
import time
import urllib.parse
import uuid
from wire import WebSocket


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class Relay:
    def __init__(self, root, binary, evidence, api_factory, env_factory, private_write, require):
        self.root, self.binary, self.evidence = root, binary, evidence
        self.api_factory, self.env_factory = api_factory, env_factory
        self.private_write, self.require = private_write, require
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.process, self.log = None, None
        self.tokens = {}
        self.config = None

    def api(self, path, body=None, method=None, role=None, allowed=(200,)):
        client = self.api_factory(self.base, self.evidence)
        client.bearer = self.tokens.get(role)
        return client.request(path, body, method, allowed)

    def start(self):
        home = self.root / "private/relay-home"
        home.mkdir(mode=0o700, exist_ok=True)
        password, session = secrets.token_urlsafe(36), secrets.token_urlsafe(48)
        self.evidence.secrets.extend([password, session])
        self.data_dir = self.root / ("private/relay-data-" + secrets.token_hex(3))
        environment = self.env_factory(home)
        environment.update({"HOST": "127.0.0.1", "PORT": str(self.port), "RUST_LOG": "warn",
            "POCKYMOE_RELAY_DATA_DIR": str(self.data_dir),
            "POCKYMOE_RELAY_REGISTRATION_ENABLED": "true", "POCKYMOE_ADMIN_USERNAME": "fixture-admin",
            "POCKYMOE_ADMIN_PASSWORD": password, "POCKYMOE_RELAY_SESSION_SECRET": session,
            "POCKYMOE_PUBLIC_BASE_URL": self.base,
            "POCKYMOE_PORT_PREVIEW_BASE_URL": f"http://preview.localhost:{self.port}"})
        fd = os.open(self.root / "private/relay.log", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        self.log = os.fdopen(fd, "ab", buffering=0)
        self.process = subprocess.Popen([str(self.binary), "relay"], cwd=self.root, env=environment,
            stdout=self.log, stderr=self.log, start_new_session=True)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            self.require(self.process.poll() is None, "Dedicated fixture Relay exited")
            try:
                self.api("/healthz")
                break
            except (OSError, RuntimeError):
                time.sleep(0.15)
        else:
            self.require(False, "Dedicated fixture Relay did not become healthy")
        for role in ["owner", "reader", "stranger"]:
            # The HTTP-only fixture supplies an opaque vault and valid fresh
            # P-256 account key. Browser password derivation/unlocking is not
            # part of this protocol test and is covered by account-key E2E.
            encoded = lambda data: base64.urlsafe_b64encode(data).decode().rstrip("=")
            key_file = home / (role + "-account.pem")
            generated = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                "-out", str(key_file)], capture_output=True, env=self.env_factory(home))
            self.require(generated.returncode == 0, "Private fixture account key generation failed")
            key_file.chmod(0o600)
            public = subprocess.run(["openssl", "pkey", "-in", str(key_file), "-pubout", "-outform", "DER"],
                capture_output=True, env=self.env_factory(home))
            self.require(public.returncode == 0, "Private fixture public key derivation failed")
            auth_key, recovery_key = encoded(secrets.token_bytes(32)), encoded(secrets.token_bytes(32))
            self.evidence.secrets.extend([auth_key, recovery_key])
            user_id = str(uuid.uuid4())
            slot = lambda: {"iv": encoded(secrets.token_bytes(12)), "ciphertext": encoded(secrets.token_bytes(64))}
            vault = {"version": 1, "revision": 1, "userId": user_id,
                "accountPublicKey": encoded(public.stdout), "accountKeyId": encoded(hashlib.sha256(public.stdout).digest()),
                "kdf": {"name": "argon2id", "memoryKiB": 65536, "iterations": 3, "parallelism": 1, "salt": encoded(secrets.token_bytes(16))},
                "slots": {"password": slot(), "recovery": slot()}, "body": slot()}
            credentials = {"userId": user_id, "authKey": auth_key, "recoveryAuthKey": recovery_key, "vault": vault}
            self.private_write(home / (role + "-auth.json"), json.dumps(credentials) + "\n")
            self.api("/relay/auth/register", dict(credentials, username=role, email=role + "@test.invalid"))
            _, logged_in = self.api("/relay/auth/login", {"username": role, "authKey": auth_key})
            self.tokens[role] = logged_in["token"]
            self.evidence.secrets.append(logged_in["token"])
        self.api("/relay/account/preferences", {"requireSafetyCode": False}, "PATCH", "owner")
        _, device = self.api("/relay/devices", {"name": self.root.name}, role="owner")
        self.device_id = device["device"]["id"]
        self.evidence.secrets.append(device["token"])
        self.config = {"baseUrl": self.base, "agentToken": device["token"], "deviceId": self.device_id, "dataDirectory": str(self.data_dir)}
        self.private_write(self.root / "private/relay-connection.json", json.dumps(dict(self.config,
            accountTokens=self.tokens, password=password, sessionSecret=session)) + "\n")
        return self.config

    def connected(self):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.api("/healthz")[1].get("connectedSupervisors") == 1:
                self.evidence.check("dedicated-real-relay-connected", True)
                return
            time.sleep(0.2)
        self.require(False, "Dedicated test Supervisor did not connect to its isolated Relay")

    def preview(self, authority, path="/", cookie=None, hold=False):
        started, timestamp = time.monotonic(), time.time()
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=20)
        headers = {"Host": authority, "Accept": "application/json"}
        if cookie:
            headers["Cookie"] = cookie
        connection.request("GET", path, headers=headers)
        response = connection.getresponse()
        if hold:
            return connection, response
        try:
            body = response.read(1048576)
        except http.client.IncompleteRead as error:
            # Preserve the real truncation as a failure, while allowing other
            # protocols to collect evidence from the received public payload.
            body = error.partial
            self.evidence.check("relay-preview-http-body-complete", False,
                {"path": urllib.parse.urlsplit(path).path, "status": response.status,
                 "timestampUnix": timestamp, "elapsedSeconds": round(time.monotonic() - started, 3),
                 "headers": {name: response.getheader(name) for name in ["Content-Type", "Transfer-Encoding", "Content-Length", "Connection"]},
                 "partialBytes": len(body), "fixtureEventCount": body.count(b"event: fixture"),
                 "lastFramingBytesHex": body[-2:].hex() if body.endswith((b"\n\n", b"\r\n")) else None,
                 "chunkedTerminatorParsed": False if response.chunked else None})
        result = response.status, {name.lower(): value for name, value in response.getheaders()}, body
        connection.close()
        return result

    def wire_sse(self, authority, cookie, index):
        """Inspect HTTP framing in memory; never serialize origin data/cookies."""
        started = time.monotonic()
        raw = b""
        eof = timed_out = False
        with socket.create_connection(("127.0.0.1", self.port), timeout=8) as stream:
            stream.sendall(("GET /sse HTTP/1.1\r\nHost: " + authority + "\r\nCookie: " + cookie + "\r\nConnection: close\r\n\r\n").encode())
            try:
                while len(raw) < 131072:
                    part = stream.recv(65536)
                    if not part:
                        eof = True
                        break
                    raw += part
            except socket.timeout:
                timed_out = True
        head, separator, wire_body = raw.partition(b"\r\n\r\n")
        lines = head.split(b"\r\n")
        headers = {}
        for line in lines[1:]:
            name, colon, value = line.partition(b":")
            if colon and name.lower() in {b"content-type", b"transfer-encoding", b"content-length", b"connection"}:
                headers[name.decode().lower()] = value.decode(errors="replace").strip()
        body, terminal = wire_body, None
        chunked = "chunked" in headers.get("transfer-encoding", "").lower()
        if chunked:
            body, position, terminal = b"", 0, False
            while position < len(wire_body):
                line_end = wire_body.find(b"\r\n", position)
                if line_end < 0:
                    break
                try:
                    count = int(wire_body[position:line_end].split(b";", 1)[0], 16)
                except ValueError:
                    break
                position = line_end + 2
                if count == 0:
                    terminal = wire_body[position:position + 2] == b"\r\n"
                    break
                if position + count + 2 > len(wire_body):
                    body += wire_body[position:]
                    break
                body += wire_body[position:position + count]
                position += count + 2
        status = int(lines[0].split()[1]) if separator and len(lines[0].split()) >= 2 else None
        complete = terminal if chunked else len(body) == int(headers["content-length"]) if "content-length" in headers else eof
        self.evidence.check("relay-raw-http-sse-framing-" + str(index), status == 200 and complete and body.count(b"event: fixture") == 3,
            {"status": status, "headers": headers, "wireBytes": len(raw), "decodedBytes": len(body),
             "fixtureEventCount": body.count(b"event: fixture"), "terminalChunkReceived": terminal,
             "wireEndsWithTerminalChunk": wire_body.endswith(b"0\r\n\r\n") if chunked else None,
             "eof": eof, "timedOut": timed_out, "elapsedSeconds": round(time.monotonic() - started, 3)})

    def open_cookie(self, mapping, role):
        path = f"/relay/devices/{self.device_id}/port-mappings/{mapping['id']}/open"
        _, opened = self.api(path, {"path": "/"}, role=role)
        url = urllib.parse.urlsplit(opened["url"])
        for name, value in urllib.parse.parse_qsl(url.query):
            if name in {"__rc_launch", "lease"}:
                self.evidence.secrets.append(value)
        status, headers, _ = self.preview(url.netloc, url.path + "?" + url.query)
        self.require(status == 303, "Preview launch ticket redemption failed")
        cookie = headers.get("set-cookie", "").split(";", 1)[0]
        self.require(cookie.startswith("rc_port_preview="), "Preview did not create its private session cookie")
        self.evidence.secrets.append(cookie.split("=", 1)[1])
        self.evidence.check("relay-" + role + "-preview-cookie-http-only", "HttpOnly" in headers.get("set-cookie", ""))
        status, _, _ = self.preview(url.netloc, url.path + "?" + url.query)
        self.evidence.check("relay-" + role + "-launch-ticket-single-use", status == 401)
        return url.netloc, cookie

    def legacy_host_binding(self, supervisor, thread_id, environment_id, binding_probe=None):
        """A previously valid host preview must lose its host origin on bind."""
        from http.server import ThreadingHTTPServer
        from fixture_service import Handler
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.daemon_threads = True
        server.label = secrets.token_urlsafe(32)
        self.evidence.secrets.append(server.label)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        websocket = live = live_connection = mapping = None
        bound = False
        try:
            supervisor.api(f"/api/threads/{thread_id}/environment", {"environmentId": None, "resetSession": True}, "PUT")
            mapping = supervisor.prompt(thread_id, {"case": "preview", "port": server.server_port})
            self.require(mapping.get("created") and mapping.get("threadId") == thread_id, "Legacy host preview fixture was not thread-authenticated")
            authority, cookie = self.open_cookie(mapping, "owner")
            status, _, payload = self.preview(authority, cookie=cookie)
            self.evidence.check("legacy-host-preview-reaches-own-canary-before-bind", status == 200 and json.loads(payload).get("label") == server.label)
            live_connection, live = self.preview(authority, "/sse-live", cookie, hold=True)
            self.require(live.status == 200 and live.readline().startswith(b"event: fixture"), "Legacy host SSE did not start")
            websocket = WebSocket(f"ws://127.0.0.1:{self.port}/ws", {"Host": authority, "Cookie": cookie})
            websocket.socket.settimeout(8)
            websocket.send("before-environment-bind")
            self.require(websocket.receive() == server.label + ":before-environment-bind", "Legacy host WebSocket did not start")
            if binding_probe:
                binding_probe.before_bind()
            supervisor.api(f"/api/threads/{thread_id}/environment", {"environmentId": environment_id, "resetSession": True}, "PUT")
            bound = True
            if binding_probe:
                binding_probe.capture_bind_ack()
            status, _, payload = self.preview(authority, cookie=cookie)
            self.evidence.check("legacy-host-preview-cookie-denied-after-environment-bind", status >= 400 and server.label.encode() not in payload,
                {"httpStatus": status, "hostCanaryObserved": server.label.encode() in payload})
            if binding_probe:
                binding_probe.after_bind()
            closed = False
            try:
                websocket.receive()
            except (EOFError, ConnectionError):
                closed = True
            except socket.timeout:
                pass
            self.evidence.check("legacy-host-preview-active-websocket-closes-after-bind", closed)
            closed = False
            try:
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    if not live.readline():
                        closed = True
                        break
            except (http.client.IncompleteRead, ConnectionError):
                closed = True
            except socket.timeout:
                pass
            self.evidence.check("legacy-host-preview-active-sse-closes-after-bind", closed)
            time.sleep(0.3)
            status, _, payload = self.preview(authority, cookie=cookie)
            self.evidence.check("legacy-host-preview-remains-denied-after-active-stream-revocation", status >= 400 and server.label.encode() not in payload,
                {"httpStatus": status, "hostCanaryObserved": server.label.encode() in payload})
        finally:
            if binding_probe:
                binding_probe.close()
            if websocket:
                websocket.close()
            if live:
                live.close()
                live_connection.close()
            if mapping and mapping.get("id"):
                # Binding may already have synchronously removed the old lease.
                supervisor.api.request("/api/cli", {"operation": "previewStop", "target": mapping["id"]}, allowed=(200, 400, 404))
            server.shutdown()
            server.server_close()
            if not bound:
                supervisor.api(f"/api/threads/{thread_id}/environment", {"environmentId": environment_id, "resetSession": True}, "PUT")

    def checks(self, supervisor, threads):
        mappings = []
        for index, thread in enumerate(threads):
            mapping = supervisor.prompt(thread["id"], {"case": "preview"})
            self.require(mapping.get("created") is True, "Environment CLI preview creation failed")
            mappings.append(mapping)
            self.evidence.check("relay-preview-bound-to-thread-" + str(index), mapping.get("threadId") == thread["id"])
        self.evidence.check("relay-same-port-distinct-environment-mappings", mappings[0]["id"] != mappings[1]["id"])
        _, share = self.api("/relay/shares", {"targetIdentifier": "reader", "deviceId": self.device_id,
            "threadId": threads[0]["id"], "threadAccess": "read", "workspaceAccess": "read"}, role="owner")
        self.evidence.check("relay-thread-share-immediately-active", share.get("pending") is False)
        authorities, owner_cookies = [], []
        for index, mapping in enumerate(mappings):
            authority, cookie = self.open_cookie(mapping, "owner")
            authorities.append(authority)
            owner_cookies.append(cookie)
            status, _, payload = self.preview(authority, cookie=cookie)
            self.evidence.check("relay-public-http-environment-" + str(index), status == 200 and json.loads(payload).get("label") == ["original", "linked"][index])
            status, _, payload = self.preview(authority, "/sse", cookie)
            self.evidence.check("relay-public-sse-environment-" + str(index), status == 200 and payload.count(b"event: fixture") == 3)
            self.wire_sse(authority, cookie, index)
            with WebSocket(f"ws://127.0.0.1:{self.port}/ws", {"Host": authority, "Cookie": cookie}) as websocket:
                websocket.send("relay-acceptance")
                self.evidence.check("relay-public-websocket-environment-" + str(index), websocket.receive() == ["original", "linked"][index] + ":relay-acceptance")
            self.evidence.check("relay-anonymous-denied-" + str(index), self.preview(authority)[0] == 401)
        self.evidence.check("relay-preview-cookie-cannot-cross-environments", self.preview(authorities[1], cookie=owner_cookies[0])[0] == 401)
        open_path = f"/relay/devices/{self.device_id}/port-mappings/{mappings[0]['id']}/open"
        status, _ = self.api(open_path, {"path": "/"}, role="stranger", allowed=(401, 403))
        self.evidence.check("relay-unshared-account-denied", status == 403)
        second_path = f"/relay/devices/{self.device_id}/port-mappings/{mappings[1]['id']}/open"
        status, _ = self.api(second_path, {"path": "/"}, role="reader", allowed=(401, 403))
        self.evidence.check("relay-thread-share-cannot-open-other-environment", status == 403)
        authority, reader_cookie = self.open_cookie(mappings[0], "reader")
        status, _, payload = self.preview(authority, cookie=reader_cookie)
        self.evidence.check("relay-shared-recipient-http", status == 200 and json.loads(payload).get("label") == "original")
        live_connection, live = self.preview(authority, "/sse-live", reader_cookie, hold=True)
        websocket = WebSocket(f"ws://127.0.0.1:{self.port}/ws", {"Host": authority, "Cookie": reader_cookie})
        websocket.socket.settimeout(20)
        try:
            self.evidence.check("relay-shared-recipient-live-sse", live.status == 200 and live.readline().startswith(b"event: fixture"))
            websocket.send("shared-before-revoke")
            self.evidence.check("relay-shared-recipient-live-websocket", websocket.receive() == "original:shared-before-revoke")
            self.api("/relay/shares/" + share["id"], method="DELETE", role="owner")
            status = self.preview(authority, cookie=reader_cookie)[0]
            self.evidence.check("relay-share-revocation-denies-existing-cookie", status == 401)
            closed = False
            try:
                websocket.receive()
            except (EOFError, ConnectionError):
                closed = True
            except socket.timeout:
                pass
            self.evidence.check("relay-share-revocation-closes-live-websocket", closed)
            deadline = time.monotonic() + 20
            sse_closed = False
            try:
                while time.monotonic() < deadline:
                    if not live.readline():
                        sse_closed = True
                        break
            except (http.client.IncompleteRead, ConnectionError):
                sse_closed = True
            except socket.timeout:
                pass
            self.evidence.check("relay-share-revocation-closes-live-sse", sse_closed)
            self.evidence.check("relay-revoke-preserves-owner-and-other-environment", all(
                self.preview(host, cookie=cookie)[0] == 200 for host, cookie in zip(authorities, owner_cookies)))
        finally:
            websocket.close()
            live.close()
            live_connection.close()
        stopped = supervisor.prompt(threads[0]["id"], {"case": "preview-stop", "id": mappings[0]["id"]})
        self.evidence.check("relay-original-thread-stops-own-preview", stopped.get("stopped") is True)
        deadline = time.monotonic() + 5
        status = 200
        while status == 200 and time.monotonic() < deadline:
            status = self.preview(authorities[0], cookie=owner_cookies[0])[0]
            if status == 200:
                time.sleep(0.1)
        self.evidence.check("relay-stopped-preview-cookie-refused", status == 404)
        supervisor.prompt(threads[1]["id"], {"case": "preview-stop", "id": mappings[1]["id"]})

    def stop(self):
        if self.process and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=3)
        if self.log:
            self.log.close()
        log_path = self.root / "private/relay.log"
        if log_path.exists():
            (self.root / "relay.redacted.log").write_text(self.evidence.redact(log_path.read_text(errors="replace")))
