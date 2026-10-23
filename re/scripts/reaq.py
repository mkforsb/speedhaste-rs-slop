#!/usr/bin/env python3
"""Query the REA daemon.

usage: reaq.py TOOL ['{"json": "arguments"}']     call an MCP tool
       reaq.py --list                             list tool names
       reaq.py --schema TOOL                      show a tool's input schema
       --full                                     print the whole Evidence envelope
Env REA_SOCK overrides the socket path (default re/out/rea.sock).
Prints text content of the result (JSON pretty-printed when possible).
"""
import json
import os
import socket
import sys

SOCK = os.environ.get('REA_SOCK', os.path.join(os.path.dirname(__file__), '..', 'out', 'rea.sock'))


def call(method, params=None):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(SOCK)
    f = s.makefile('rwb')
    f.write((json.dumps({'method': method, 'params': params}) + '\n').encode())
    f.flush()
    return json.loads(f.readline())


def main():
    a = sys.argv[1:]
    full = '--full' in a
    a = [x for x in a if x != '--full']
    if a and a[0] in ('--list', '--schema'):
        tools = call('tools/list', {})['result']['tools']
        if a[0] == '--list':
            for t in tools:
                print(t['name'])
        else:
            t = next(t for t in tools if t['name'] == a[1])
            print(t.get('description', ''))
            print(json.dumps(t['inputSchema'], indent=1))
        return
    args = json.loads(a[1]) if len(a) > 1 else {}
    resp = call('tools/call', {'name': a[0], 'arguments': args})
    if 'error' in resp:
        print(json.dumps(resp['error'], indent=1))
        sys.exit(1)
    res = resp['result']
    out = res.get('structuredContent')
    if out is None:
        for c in res.get('content', []):
            if c.get('type') == 'text':
                try:
                    out = json.loads(c['text'])
                except ValueError:
                    print(c['text'])
    if out is None:
        return
    if not full and isinstance(out, dict) and 'normalized_result' in out:
        out = out['normalized_result']
    if isinstance(out, str):
        print(out)
    else:
        print(json.dumps(out, indent=1))


if __name__ == '__main__':
    main()
