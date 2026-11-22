#!/usr/bin/env python3
"""Ephemeral test-only TLS endpoint and outbound-connected byte relay."""
import base64
import http.server
import json
import pathlib
import select
import socket
import ssl
import subprocess
import tempfile
import threading

AUDIO = (pathlib.Path(__file__).parent / 'fixtures/audio_sine.mp3').read_bytes()
received = []

class Handler(http.server.BaseHTTPRequestHandler):
    def do_HEAD(self):
        self.send_response(200)
        self.send_header('Content-Length', str(len(AUDIO)))
        self.send_header('Accept-Ranges', 'bytes')
        self.end_headers()
    def do_GET(self):
        received.append(self.headers.get('Authorization'))
        if self.path == '/received':
            data = json.dumps(received).encode()
            self.send_response(200)
        elif self.path == '/audio':
            first, last = self.headers['Range'].removeprefix('bytes=').split('-')
            first, last = int(first), int(last)
            data = AUDIO[first:last + 1]
            self.send_response(206)
            self.send_header('Content-Range', f'bytes {first}-{last}/{len(AUDIO)}')
        else:
            data = b'fixture api'
            self.send_response(200)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *args):
        pass

class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request, client_address):
        pass


def relay(home_listener, phone_listener):
    while True:
        phone, _ = phone_listener.accept()
        home, _ = home_listener.accept()
        def pump(a, b):
            try:
                while True:
                    ready, _, _ = select.select([a, b], [], [], 10)
                    if not ready:
                        break
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        (b if source is a else a).sendall(data)
            except OSError:
                pass
            finally:
                a.close()
                b.close()
        threading.Thread(target=pump, args=(phone, home), daemon=True).start()


def home_connector(port, target):
    while True:
        home = socket.create_connection(('127.0.0.1', port))
        first = home.recv(65536)
        if not first:
            home.close()
            continue
        upstream = socket.create_connection(('127.0.0.1', target))
        upstream.sendall(first)
        def pump(a, b):
            try:
                while True:
                    ready, _, _ = select.select([a, b], [], [], 10)
                    if not ready:
                        return
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        (b if source is a else a).sendall(data)
            except OSError:
                pass
            finally:
                a.close()
                b.close()
        pump(home, upstream)


def listen():
    sock = socket.socket()
    sock.bind(('127.0.0.1', 0))
    sock.listen()
    return sock


def main():
    with tempfile.TemporaryDirectory() as tmp:
        key, cert = f'{tmp}/key.pem', f'{tmp}/cert.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', cert, '-days', '1', '-subj', '/CN=fixture'], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pub = subprocess.check_output(['openssl', 'x509', '-in', cert, '-pubkey', '-noout'])
        spki = subprocess.check_output(['openssl', 'pkey', '-pubin', '-outform', 'DER'], input=pub)
        server = Server(('0.0.0.0', 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        home, phone = listen(), listen()
        # Emulator reaches the relay through the host interface.
        phone.close()
        phone = socket.socket()
        phone.bind(('0.0.0.0', 0))
        phone.listen()
        threading.Thread(target=relay, args=(home, phone), daemon=True).start()
        for _ in range(4):
            threading.Thread(target=home_connector, args=(home.getsockname()[1], server.server_port), daemon=True).start()
        print(json.dumps({'port': server.server_port, 'relay': phone.getsockname()[1], 'pin': base64.b64encode(spki).decode()}), flush=True)
        threading.Event().wait()

if __name__ == '__main__':
    main()
