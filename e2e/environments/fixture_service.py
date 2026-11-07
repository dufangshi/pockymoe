#!/usr/bin/env python3
"""Small secret-free HTTP, SSE and WebSocket origin for environment previews."""
import argparse
import base64
import hashlib
import json
import struct
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def exact(stream, count):
    data = b""
    while len(data) < count:
        part = stream.read(count - len(data))
        if not part:
            raise EOFError
        data += part
    return data


def ws_send(stream, payload, opcode=1):
    length = len(payload)
    head = bytes([0x80 | opcode, length]) if length < 126 else bytes([0x80 | opcode, 126]) + struct.pack("!H", length)
    stream.write(head + payload)
    stream.flush()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.split("?", 1)[0] == "/ws":
            key = self.headers.get("Sec-WebSocket-Key", "")
            accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
            self.send_response(101)
            self.send_header("Upgrade", "websocket")
            self.send_header("Connection", "Upgrade")
            self.send_header("Sec-WebSocket-Accept", accept)
            self.end_headers()
            try:
                while True:
                    first, second = exact(self.rfile, 2)
                    count = second & 0x7F
                    if count == 126:
                        count = struct.unpack("!H", exact(self.rfile, 2))[0]
                    elif count == 127:
                        count = struct.unpack("!Q", exact(self.rfile, 8))[0]
                    if count > 65536:
                        return
                    mask = exact(self.rfile, 4) if second & 0x80 else None
                    data = exact(self.rfile, count)
                    if mask:
                        data = bytes(byte ^ mask[index % 4] for index, byte in enumerate(data))
                    opcode = first & 0x0F
                    if opcode == 8:
                        ws_send(self.wfile, b"", 8)
                        return
                    if opcode == 9:
                        ws_send(self.wfile, data, 10)
                    elif opcode == 1:
                        # The client sends only a public fixture label.
                        ws_send(self.wfile, self.server.label.encode() + b":" + data)
            except (EOFError, OSError):
                return
        elif self.path.split("?", 1)[0] in {"/sse", "/sse-live"}:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Connection", "close")
            self.end_headers()
            try:
                for index in range(200 if self.path == "/sse-live" else 3):
                    data = json.dumps({"label": self.server.label, "index": index})
                    self.wfile.write(("event: fixture\ndata: " + data + "\n\n").encode())
                    self.wfile.flush()
                    time.sleep(0.15)
            except (BrokenPipeError, ConnectionResetError):
                pass
            self.close_connection = True
        else:
            payload = json.dumps({"fixture": "pocky-environment-origin", "label": self.server.label}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=18765)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    server.label = args.label
    server.serve_forever(poll_interval=0.1)


if __name__ == "__main__":
    main()
