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
    id int,
    time timestamp,
    service string,
    name string,
    message string,
    var object
);
SELECT service, count(*) AS total FROM events JOIN customers c ON c.id = events.id WHERE time > now() - interval '1 hour' GROUP BY service ORDER BY time DESC LIMIT 10;
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
# opened via didOpen (same content as on disk, so the overlay is a no-op and
# results are deterministic).
ws = Path(tempfile.mkdtemp(prefix="scopeql-lsp-smoke-"))
events_uri = (ws / "events.scopeql").as_uri()
customers_uri = (ws / "customers.scopeql").as_uri()
(ws / "customers.scopeql").write_text("CREATE TABLE customers (id int);\n")

# Cursor positions (bytes == UTF-16 offsets in this ASCII source).
events_def_pos = pos_of(SRC, SRC.index("events") + 3)
ev_ref = SRC.index("events", SRC.index("FROM ") + len("FROM "))
events_ref_pos = pos_of(SRC, ev_ref + 3)
customers_ref_pos = pos_of(SRC, SRC.index("customers") + 4)
service_ref_pos = pos_of(SRC, SRC.index("service", SRC.index("SELECT ") + 7) + 3)
c_id_pos = pos_of(SRC, SRC.index("c.id") + 2 + 1)
events_id_pos = pos_of(SRC, SRC.index("events.id") + len("events.") + 1)
time_pos = pos_of(SRC, SRC.index("WHERE time") + len("WHERE ") + 2)
count_pos = pos_of(SRC, SRC.index("count") + 2)

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
                        "tokenTypes": ["keyword", "type", "property", "variable",
                                       "function", "string", "number", "operator",
                                       "comment"],
                        "tokenModifiers": ["declaration", "readonly"],
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
print("definitionProvider:", caps.get("definitionProvider"))
print("referencesProvider:", caps.get("referencesProvider"))
assert caps.get("definitionProvider") is True
assert caps.get("referencesProvider") is True

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


def call(rid, method, uri, position, extra=None):
    params = {"textDocument": {"uri": uri}, "position": position}
    if extra:
        params.update(extra)
    send(proc, {"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    return recv(proc)


def one_loc(rid, method, uri, position):
    resp = call(rid, method, uri, position)
    assert resp and resp["id"] == rid, resp
    result = resp["result"]
    assert isinstance(result, dict) and "uri" in result, resp
    return result


def many_locs(rid, uri, position):
    resp = call(rid, "textDocument/references", uri, position,
                {"context": {"includeDeclaration": True}})
    assert resp and resp["id"] == rid, resp
    return resp["result"]


# --- objects ------------------------------------------------------------
loc = one_loc(10, "textDocument/definition", events_uri, events_ref_pos)
print("\n== definition FROM events ==", json.dumps(loc))
assert loc["uri"] == events_uri and loc["range"]["start"] == {"line": 1, "character": 13}

loc = one_loc(11, "textDocument/definition", events_uri, customers_ref_pos)
print("== definition JOIN customers (cross-file) ==", json.dumps(loc))
assert loc["uri"] == customers_uri and loc["range"]["start"] == {"line": 0, "character": 13}

locs = many_locs(12, events_uri, events_ref_pos)
print("== references events ==", json.dumps(locs))
assert len(locs) == 2, locs

# --- columns ------------------------------------------------------------
loc = one_loc(13, "textDocument/definition", events_uri, service_ref_pos)
print("\n== definition SELECT service ==", json.dumps(loc))
assert loc["uri"] == events_uri
assert loc["range"]["start"] == {"line": 4, "character": 4}
assert loc["range"]["end"] == {"line": 4, "character": 11}

locs = many_locs(14, events_uri, service_ref_pos)
print("== references service (def + SELECT + GROUP BY) ==", json.dumps(locs))
assert len(locs) == 3, locs

loc = one_loc(15, "textDocument/definition", events_uri, c_id_pos)
print("== definition c.id (alias, cross-file) ==", json.dumps(loc))
assert loc["uri"] == customers_uri
assert loc["range"]["start"] == {"line": 0, "character": 24}
assert loc["range"]["end"] == {"line": 0, "character": 26}

loc = one_loc(16, "textDocument/definition", events_uri, events_id_pos)
print("== definition events.id ==", json.dumps(loc))
assert loc["uri"] == events_uri
assert loc["range"]["start"] == {"line": 2, "character": 4}

locs = many_locs(17, events_uri, time_pos)
print("== references time (def + WHERE + ORDER BY) ==", json.dumps(locs))
assert len(locs) == 3, locs

# A function name is not a column and resolves to nothing.
resp = call(18, "textDocument/definition", events_uri, count_pos)
assert resp and resp["id"] == 18, resp
print("== definition on count() == null:", resp["result"] is None)
assert resp["result"] is None, resp

send(proc, {"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": None})
resp = recv(proc)
assert resp and resp["id"] == 99, resp
send(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
proc.wait(timeout=5)
print("exit code:", proc.returncode)
print("stderr:", proc.stderr.read().decode()[:300] or "(none)")
assert proc.returncode == 0
shutil.rmtree(ws, ignore_errors=True)