#!/usr/bin/env python3
"""Smoke-test the scopeql-lsp binary over stdio LSP."""
import json
import subprocess
import sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/debug/scopeql-lsp"

SRC = """-- sample
CREATE TABLE events (
    time timestamp,
    service string,
    name string,
    message string,
    var object
);
SELECT lower(name), count(*) FROM events WHERE time > now() - interval '1 hour' ORDER BY time DESC LIMIT 10;
"""


def send(proc, msg):
    body = json.dumps(msg, separators=(",", ":")).encode()
    proc.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    proc.stdin.flush()


def recv(proc):
    while True:
        headers = {}
        while True:
            line = proc.stdout.readline()
            if not line:
                return None
            line = line.decode().strip()
            if not line:
                break
            k, _, v = line.partition(":")
            headers[k.strip().lower()] = v.strip()
        n = int(headers["content-length"])
        msg = json.loads(proc.stdout.read(n))
        if "method" in msg:
            print(f"   [notification] {msg['method']}")
            continue
        return msg


proc = subprocess.Popen(
    [BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
)

send(
    proc,
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": None,
            "rootUri": "file:///tmp",
            "capabilities": {
                "textDocument": {
                    "semanticTokens": {
                        "dynamicRegistration": True,
                        "requests": {"full": True, "range": False},
                        "tokenTypes": [
                            "namespace", "type", "class", "enum", "interface",
                            "struct", "typeParameter", "parameter", "variable",
                            "property", "enumMember", "event", "function",
                            "method", "macro", "keyword", "modifier", "comment",
                            "string", "number", "regexp", "decorator", "label",
                            "operator",
                        ],
                        "tokenModifiers": [
                            "declaration", "definition", "readonly", "static",
                            "deprecated", "abstract", "async", "modification",
                            "documentation", "defaultLibrary",
                        ],
                        "formats": ["relative"],
                    }
                }
            },
        },
    },
)
resp = recv(proc)
print("== initialize (id=1) ==")
caps = resp["result"]["capabilities"]
print("semanticTokensProvider:", json.dumps(caps.get("semanticTokensProvider"))[:160])
legend = caps["semanticTokensProvider"]["legend"]
print("legend types:", legend["tokenTypes"])

send(proc, {"jsonrpc": "2.0", "method": "initialized", "params": {}})

send(
    proc,
    {
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///tmp/events.scopeql",
                "languageId": "scopeql",
                "version": 1,
                "text": SRC,
            }
        },
    },
)

send(
    proc,
    {
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/semanticTokens/full",
        "params": {"textDocument": {"uri": "file:///tmp/events.scopeql"}},
    },
)
resp = recv(proc)
assert resp["id"] == 2, resp
data = resp["result"]["data"]
print("\n== semanticTokens/full (id=2) ==")
print("token count:", len(data) // 5)
for i in range(0, min(len(data), 60), 5):
    dl, ds, ln, tt, tm = data[i : i + 5]
    print(f"  delta_line={dl} delta_start={ds} len={ln} type={tt} mods={tm}")

send(
    proc,
    {
        "jsonrpc": "2.0",
        "id": 3,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {"uri": "file:///tmp/events.scopeql"},
            "position": {"line": 8, "character": 9},
        },
    },
)
resp = recv(proc)
assert resp["id"] == 3, resp
print("\n== hover (id=3) ==")
print(json.dumps(resp.get("result"))[:300])

send(proc, {"jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": None})
resp = recv(proc)
assert resp["id"] == 4, resp
print("\n== shutdown (id=4) ==", resp)
send(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
proc.wait(timeout=5)
print("exit code:", proc.returncode)
print("stderr:", proc.stderr.read().decode()[:500] or "(none)")