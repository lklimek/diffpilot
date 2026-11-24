#!/usr/bin/python3
"""Fake listeners and live reachability assertions for the throwaway Compose deployment."""

import json
import os
import socket
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


class Handler(BaseHTTPRequestHandler):
    """Return a fixed reply without credentials, upstreams or request logging."""

    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"fake-only")


def connect(url, allowed):
    """Assert TCP reachability; explicit positive controls prevent vacuous denials."""
    parsed = urlsplit(url)
    try:
        with socket.create_connection((parsed.hostname, parsed.port), timeout=2):
            reached = True
    except OSError:
        reached = False
    if reached != allowed:
        raise RuntimeError(f"reachability mismatch for {url}: expected {allowed}, got {reached}")


def probe(shape, addresses):
    """Exercise private listeners, proxy LAN denial, judge isolation and metadata denial."""
    for path in ('/var/run/docker.sock', '/run/launcher/launcher.sock'):
        if os.path.exists(path):
            raise RuntimeError('privileged socket visible in a worker')
    connect(addresses['judge' if shape == 'judge' else 'model'], True)
    # The gateway is multihomed: its model-facing IP must not serve the admin port either.
    model = urlsplit(addresses['judge' if shape == 'judge' else 'model'])
    connect(f'http://{model.hostname}:8091', False)
    for target in (addresses['admin'], addresses['runner'], 'http://169.254.169.254:80',
                   'http://1.1.1.1:443'):
        connect(target, False)
    if shape == 'judge':
        connect(addresses['model'], False)
        connect(addresses['memcan'], False)
        connect('http://egress-proxy:3128', False)
    else:
        connect('http://egress-proxy:3128', True)
        for target in (addresses['admin'], addresses['runner'], 'http://169.254.169.254/latest/meta-data/'):
            with socket.create_connection(('egress-proxy', 3128), timeout=3) as stream:
                stream.sendall(f'GET {target} HTTP/1.0\r\n\r\n'.encode())
                if b' 403 ' not in stream.recv(512).split(b'\r\n')[0]:
                    raise RuntimeError('proxy allowed private destination')
        # The sole private-address exception remains reachable through Squid.
        with socket.create_connection(('egress-proxy', 3128), timeout=3) as stream:
            stream.sendall(f'GET {addresses["memcan"]} HTTP/1.0\r\n\r\n'.encode())
            if b' 200 ' not in stream.recv(512).split(b'\r\n')[0]:
                raise RuntimeError('configured MemCan exception unreachable')
    print(f'{shape} network probes PASS')


if __name__ == '__main__':
    if sys.argv[1] == 'listen':
        listeners = [os.environ[key] for key in ('SR_GATEWAY_MODEL_ADDR', 'SR_GATEWAY_JUDGE_ADDR', 'SR_GATEWAY_ADMIN_ADDR')]
        # Bind to interface-specific addresses just like the real gateway.
        memcan = os.environ['SR_GATEWAY_ADMIN_ADDR'].rsplit(':', 1)[0] + ':8190'
        for address in listeners + [memcan]:
            host, port = address.rsplit(':', 1)
            server = ThreadingHTTPServer((host, int(port)), Handler)
            threading.Thread(target=server.serve_forever, daemon=True).start()
        threading.Event().wait()
    else:
        probe(sys.argv[1], json.load(sys.stdin))
