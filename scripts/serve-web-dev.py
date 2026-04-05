import http.server
import os
import sys
import socketserver
import threading
import time

PORT = 8000
WEB_DIR = "web"

clients = []
clients_lock = threading.Lock()


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=WEB_DIR, **kwargs)

    def do_GET(self):
        if self.path == "/__reload":
            self.send_response(200)
            self.send_header("Content-type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "keep-alive")
            self.end_headers()

            with clients_lock:
                clients.append(self.wfile)

            try:
                while True:
                    # Keep the SSE connection active and detect disconnects.
                    self.wfile.write(b": ping\n\n")
                    self.wfile.flush()
                    time.sleep(15)
            except Exception:
                with clients_lock:
                    if self.wfile in clients:
                        clients.remove(self.wfile)
        else:
            super().do_GET()


def trigger_reload():
    print("[reload] Triggering browser refresh...")

    with clients_lock:
        current_clients = list(clients)

    dead = []
    for client in current_clients:
        try:
            client.write(b"data: reload\n\n")
            client.flush()
        except Exception:
            dead.append(client)

    if dead:
        with clients_lock:
            for d in dead:
                if d in clients:
                    clients.remove(d)


def latest_web_mtime():
    latest = 0.0
    for root, _, files in os.walk(WEB_DIR):
        for f in files:
            path = os.path.join(root, f)
            try:
                latest = max(latest, os.path.getmtime(path))
            except FileNotFoundError:
                # File was removed between os.walk and stat.
                continue
    return latest


def watch():
    last_mtime = latest_web_mtime()
    while True:
        mtime = latest_web_mtime()
        if mtime > last_mtime:
            last_mtime = mtime
            trigger_reload()
        time.sleep(0.5)


threading.Thread(target=watch, daemon=True).start()


class DevServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def handle_error(self, request, client_address):
        ex = sys.exc_info()[1]
        if isinstance(ex, (ConnectionResetError, BrokenPipeError, ConnectionAbortedError)):
            return
        super().handle_error(request, client_address)


with DevServer(("", PORT), Handler) as httpd:
    print(f"[web] Serving with live reload at http://localhost:{PORT}")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n[web] Shutting down cleanly...")
        httpd.shutdown()
