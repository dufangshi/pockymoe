"""Standard-library WebSocket client for fixture and actual Supervisor checks."""
import base64
import hashlib
import json
import os
import socket
import struct
import urllib.parse


class WebSocket:
    def __init__(self, url, headers=None):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != "ws" or parsed.hostname != "127.0.0.1":
            raise ValueError("Test WebSocket must use explicit loopback ws://")
        self.socket = socket.create_connection((parsed.hostname, parsed.port), timeout=8)
        self.socket.settimeout(8)
        self.reader = self.socket.makefile("rb")
        key = base64.b64encode(os.urandom(16)).decode()
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        request_headers = {"Host": parsed.netloc, "Upgrade": "websocket", "Connection": "Upgrade",
            "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": key}
        request_headers.update(headers or {})
        request = "GET " + path + " HTTP/1.1\r\n" + "".join(k + ": " + v + "\r\n" for k, v in request_headers.items()) + "\r\n"
        self.socket.sendall(request.encode())
        status = self.reader.readline()
        response_headers = {}
        while True:
            line = self.reader.readline()
            if line in {b"\r\n", b"\n", b""}:
                break
            name, value = line.decode().split(":", 1)
            response_headers[name.lower()] = value.strip()
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        if b" 101 " not in status or response_headers.get("sec-websocket-accept") != expected:
            self.close()
            raise ValueError("WebSocket handshake rejected")

    def exact(self, size):
        data = b""
        while len(data) < size:
            chunk = self.reader.read(size - len(data))
            if not chunk:
                raise EOFError("WebSocket stream ended")
            data += chunk
        return data

    def send(self, data, opcode=1):
        if isinstance(data, str):
            data = data.encode()
        mask = os.urandom(4)
        size = len(data)
        head = bytes([0x80 | opcode, 0x80 | size]) if size < 126 else bytes([0x80 | opcode, 0xFE]) + struct.pack("!H", size)
        self.socket.sendall(head + mask + bytes(byte ^ mask[index % 4] for index, byte in enumerate(data)))

    def send_json(self, data):
        self.send(json.dumps(data))

    def receive(self):
        while True:
            first, second = self.exact(2)
            size = second & 0x7F
            if size == 126:
                size = struct.unpack("!H", self.exact(2))[0]
            elif size == 127:
                size = struct.unpack("!Q", self.exact(8))[0]
            if size > 16 * 1024 * 1024:
                raise ValueError("Test WebSocket frame too large")
            mask = self.exact(4) if second & 0x80 else None
            payload = self.exact(size)
            if mask:
                payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
            opcode = first & 0x0F
            if opcode == 9:
                self.send(payload, opcode=10)
                continue
            if opcode == 8:
                raise EOFError("WebSocket closed")
            if opcode == 1:
                return payload.decode()

    def close(self):
        try:
            self.socket.close()
        finally:
            self.reader.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
