#!/usr/bin/env python3
"""Smoke-test the scopeql-lsp binary over stdio LSP."""
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/debug/scopeql-lsp"

SRC = """-- sample
CREATE TABLE events (
    time timestamp,
    service string,
    name string,
    message string,
    var object
);
SELECT lower(name), count(*) FROM events JOIN customers c ON c.id = events.id WHERE time > now() - interval '1 hour' ORDER BY time DESC LIMIT 10;
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


def pos_of(src, byte_idx):
    """Byte index (ASCII source) -> LSP position."""
    line = src.count("\n", 0, byte_idx)
    line_start = src.rfind("\n", 0, byte_idx) + 1
    return {"line": line, "character": byte_idx - line_start}


# Workspace: a temp dir holding customers.scopeql on disk. events.scopeql is
# opened via didOpen below (its on-disk text is the same, so the overlay is
# a no-op and results are deterministic).
ws = Path(tempfile.mkdtemp(prefix="scopeql-lsp-smoke-"))
events_uri = (ws / "events.scopeql").as_uri()
customers_uri = (ws / "customers.scopeql").as_uri()
(ws / "customers.scopeql").write_text("CREATE TABLE customers (id int);\n")

# Positions inside SRC (all ASCII, byte index == UTF-16 index).
ev_idx = SRC.index("events", SRC.index("FROM ") + len("FROM "))
cu_idx = SRC.index("customers")
events_def_pos = pos_of(SRC, SRC.index("events") + 3)
events_ref_pos = pos_of(SRC, ev_idx + 3)
customers_ref_pos = pos_of(SRC, cu_idx + 4)

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
            "rootUri": ws.as_uri(),
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
print("definitionProvider:", caps.get("definitionProvider"))
print("referencesProvider:", caps.get("referencesProvider"))
assert caps.get("definitionProvider") is True
assert caps.get("referencesProvider") is True
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
                "uri": events_uri,
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
        "params": {"textDocument": {"uri": events_uri}},
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
            "textDocument": {"uri": events_uri},
            "position": {"line": 8, "character": 9},
        },
    },
)
resp = recv(proc)
assert resp["id"] == 3, resp
print("\n== hover (id=3) ==")
print(json.dumps(resp.get("result"))[:300])


def definition(proc, rid, uri, position):
    send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": rid,
            "method": "textDocument/definition",
            "params": {"textDocument": {"uri": uri}, "position": position},
        },
    )
    return recv(proc)


def references(proc, rid, uri, position):
    send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": rid,
            "method": "textDocument/references",
            "params": {
                "textDocument": {"uri": uri},
                "position": position,
                "context": {"includeDeclaration": True},
            },
        },
    )
    return recv(proc)


# gd on the `events` reference in FROM -> CREATE TABLE events in the same file.
resp = definition(proc, 4, events_uri, events_ref_pos)
assert resp["id"] == 4, resp
loc = resp["result"]
print("\n== definition FROM events (id=4) ==")
print(json.dumps(resp["result"]))
assert loc["uri"] == events_uri
assert loc["range"]["start"] == {"line": 1, "character": 13}
assert loc["range"]["end"] == {"line": 1, "character": 19}

# gr on `events` -> definition + reference (2 locations).
resp = references(proc, 5, events_uri, events_ref_pos)
assert resp["id"] == 5, resp
locs = resp["result"]
print("\n== references events (id=5) ==")
print(json.dumps(locs))
assert len(locs) == 2, locs

# gd on `customers` (defined on disk in another file) -> cross-file jump.
resp = definition(proc, 6, events_uri, customers_ref_pos)
assert resp["id"] == 6, resp
loc = resp["result"]
print("\n== definition JOIN customers (id=6, cross-file) ==")
print(json.dumps(resp["result"]))
assert loc["uri"] == customers_uri
assert loc["range"]["start"] == {"line": 0, "character": 13}
assert loc["range"]["end"] == {"line": 0, "character": 22}

# gr on `customers` -> its definition (other file) + the JOIN reference here.
resp = references(proc, 7, events_uri, customers_ref_pos)
assert resp["id"] == 7, resp
locs = resp["result"]
print("\n== references customers (id=7) ==")
print(json.dumps(locs))
assert len(locs) == 2, locs
assert locs[0]["uri"] == customers_uri, locs
assert locs[1]["uri"] == events_uri, locs

# gd on a non-object identifier (`lower` function call) -> null, no error.
resp = definition(proc, 8, events_uri, {"line": 8, "character": 9})
assert resp["id"] == 8, resp
print("\n== definition on function call (id=8) ==")
print(json.dumps(resp.get("result")))
assert resp.get("result") is None, resp

send(proc, {"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": None})
resp = recv(proc)
assert resp["id"] == 9, resp
print("\n== shutdown (id=9) ==", resp)
send(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
proc.wait(timeout=5)
print("exit code:", proc.returncode)
print("stderr:", proc.stderr.read().decode()[:500] or "(none)")
shutil.rmtree(ws, ignore_errors=True)