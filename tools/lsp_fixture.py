"""Deterministic stdio server for transport tests; never used as a production server."""
import json
import sys


def send(value):
    data = json.dumps(dict(jsonrpc='2.0', **value)).encode()
    sys.stdout.buffer.write(f'Content-Length: {len(data)}\r\n\r\n'.encode() + data)
    sys.stdout.buffer.flush()


while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line == b'\r\n':
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    message = json.loads(sys.stdin.buffer.read(length))
    method = message.get('method')
    if method == 'initialize':
        send({'id': message['id'], 'result': {'capabilities': {'positionEncoding': 'utf-16', 'hoverProvider': True}}})
    elif method == 'textDocument/didOpen':
        document = message['params']['textDocument']
        send({'method': 'textDocument/publishDiagnostics', 'params': {'uri': document['uri'], 'diagnostics': [{'message': 'Fixture diagnostic', 'severity': 2, 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 1}}}]}})
    elif method == 'textDocument/hover':
        send({'id': message['id'], 'result': {'contents': 'Fixture hover'}})
    elif 'id' in message:
        send({'id': message['id'], 'result': None})
