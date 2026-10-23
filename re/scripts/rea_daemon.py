#!/usr/bin/env python3
"""Keep one REA MCP stdio session alive and expose it on a Unix socket, so
Ghidra's analysis is done once and individual queries are fast.

usage: rea_daemon.py SOCKET_PATH        (run with re/rea-env.sh sourced)

Protocol: client connects, sends one JSON line {"method": ..., "params": ...}
(any MCP request, e.g. "tools/call" with {"name":..., "arguments":...}),
receives one JSON line with the MCP result (or {"error": ...}).
"""
import json
import os
import socket
import subprocess
import sys
import threading

sock_path = sys.argv[1]
proc = subprocess.Popen(['rea', '--mcp'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=open(sock_path + '.stderr', 'ab'), bufsize=0)
lock = threading.Lock()
next_id = [0]


def rpc(method, params=None, notify=False):
    msg = {'jsonrpc': '2.0', 'method': method}
    if params is not None:
        msg['params'] = params
    if not notify:
        next_id[0] += 1
        msg['id'] = next_id[0]
    proc.stdin.write((json.dumps(msg) + '\n').encode())
    proc.stdin.flush()
    if notify:
        return None
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError('rea exited')
        resp = json.loads(line)
        if resp.get('id') == msg['id']:
            return resp
        # Server-initiated notifications/requests (progress, logs): ignore.


rpc('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                   'clientInfo': {'name': 'speedhaste-rea-daemon', 'version': '1'}})
rpc('notifications/initialized', notify=True)

if os.path.exists(sock_path):
    os.unlink(sock_path)
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(4)
print(f'rea daemon listening on {sock_path}', flush=True)


def handle(conn):
    with conn:
        f = conn.makefile('rwb')
        req = json.loads(f.readline())
        try:
            with lock:
                resp = rpc(req['method'], req.get('params'))
        except Exception as e:  # noqa: BLE001 - report everything to the client
            resp = {'error': repr(e)}
        f.write((json.dumps(resp) + '\n').encode())
        f.flush()


while True:
    c, _ = srv.accept()
    threading.Thread(target=handle, args=(c,), daemon=True).start()
