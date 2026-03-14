"""Private Input Server for World ZK Compute.

A minimal HTTP server that stores private inputs for TEE enclave consumption.
Uses only Python standard library (no external dependencies).

Endpoints:
    POST /inputs         -- Store JSON input, returns SHA-256 hash
    GET  /inputs/<hash>  -- Retrieve stored input by hash
    GET  /health         -- Health check
"""

import hashlib
import json
import threading
from http.server import HTTPServer, BaseHTTPRequestHandler

# Thread-safe in-memory store: hash -> input data
_store: dict[str, dict] = {}
_lock = threading.Lock()

PORT = 3001


class InputHandler(BaseHTTPRequestHandler):
    """HTTP request handler for private input storage."""

    def _send_json(self, status: int, data: dict) -> None:
        body = json.dumps(data).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(length)

    def do_GET(self) -> None:
        if self.path == "/health":
            self._handle_health()
        elif self.path.startswith("/inputs/"):
            self._handle_get_input()
        else:
            self._send_json(404, {"error": "not found"})

    def do_POST(self) -> None:
        if self.path == "/inputs":
            self._handle_store_input()
        else:
            self._send_json(404, {"error": "not found"})

    def _handle_health(self) -> None:
        with _lock:
            count = len(_store)
        self._send_json(200, {"status": "healthy", "inputs_stored": count})

    def _handle_store_input(self) -> None:
        body = self._read_body()
        if not body:
            self._send_json(400, {"error": "empty body"})
            return

        # Validate JSON
        try:
            data = json.loads(body)
        except json.JSONDecodeError:
            self._send_json(400, {"error": "invalid JSON"})
            return

        # Compute SHA-256 hash of the raw body
        digest = hashlib.sha256(body).hexdigest()

        with _lock:
            _store[digest] = data

        self._send_json(201, {"hash": digest, "size": len(body)})

    def _handle_get_input(self) -> None:
        # Extract hash from /inputs/<hash>
        parts = self.path.split("/")
        if len(parts) != 3 or not parts[2]:
            self._send_json(400, {"error": "missing hash parameter"})
            return

        input_hash = parts[2]

        with _lock:
            data = _store.get(input_hash)

        if data is None:
            self._send_json(404, {"error": "input not found"})
            return

        self._send_json(200, data)

    def log_message(self, format: str, *args) -> None:
        """Override to use a cleaner log format."""
        print(f"[private-input-server] {args[0]} {args[1]} {args[2]}")


def main() -> None:
    server = HTTPServer(("0.0.0.0", PORT), InputHandler)
    print(f"[private-input-server] listening on 0.0.0.0:{PORT}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[private-input-server] shutting down")
        server.server_close()


if __name__ == "__main__":
    main()
