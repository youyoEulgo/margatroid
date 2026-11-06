"""Local MCP fixture. No external service or package is used."""
import http.server
import json
import os
import subprocess
import sys
import threading

held = []
lock = threading.Lock()

def trace(value):
    path = os.getenv("MCP_FIXTURE_TRACE")
    if path:
        with open(path, "a", encoding="utf-8") as output:
            output.write(json.dumps(value) + "\n")

def result(request, value):
    return {"jsonrpc": "2.0", "id": request["id"], "result": value}

def dispatch(request):
    method = request.get("method")
    if method == "initialize":
        assert request["params"]["capabilities"] == {}
        return result(request, {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1"}})
    if method == "tools/list":
        if os.getenv("MCP_FIXTURE_LARGE_CATALOG") == "1":
            assert "cursor" not in request["params"]
            tools = []
            for index in range(1024):
                marker = f"{index:04d}"
                tools.append({
                    "name": f"catalog_{marker}",
                    "description": f"catalog-description-{marker}:" + "x" * 8192 + f":description-tail-{marker}",
                    "inputSchema": {
                        "type": "object",
                        "title": f"catalog-tool-{marker}",
                        "properties": {
                            "text": {"type": "string", "description": f"schema-property-tail-{marker}"},
                            "reverse": {"type": "boolean"},
                            "pid": {"type": "boolean"},
                            "hang": {"type": "boolean"},
                        },
                        "additionalProperties": True,
                        "$comment": f"catalog-schema-tail-{marker}",
                    },
                })
            trace({"catalog_tools": len(tools), "catalog_json_bytes": len(json.dumps(tools, separators=(",", ":")).encode())})
            return result(request, {"tools": tools})
        if "cursor" not in request["params"]:
            return result(request, {"tools": [{"name": "echo", "description": "回声", "inputSchema": {"type": "object"}}], "nextCursor": "second"})
        assert request["params"]["cursor"] == "second"
        return result(request, {"tools": [{"name": "other", "inputSchema": {"type": "object"}}]})
    if method == "tools/call":
        args = request["params"]["arguments"]
        trace({"received": request["params"]["name"], "arguments": args})
        if "result_case" in args:
            return result(request, args["result_case"])
        if args.get("error"):
            return {"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32602, "message": "fixture failure", "data": {"detail": 42}}}
        if args.get("exit"):
            os._exit(17)
        if args.get("hang"):
            return None
        if args.get("pid"):
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(3600)"])
            return result(request, {"content": [{"type": "text", "text": json.dumps({"pid": os.getpid(), "host": os.getppid(), "child": child.pid, "env": os.getenv("MCP_FIXTURE_ENV")})}]})
        return result(request, {"content": [{"type": "text", "text": args.get("text", "ok")}], "isError": bool(args.get("isError"))})
    return None

def stdio():
    def emit(message):
        trace({"emitted": message})
        # Fragment bytes, including non-ASCII UTF-8, across pipe writes.
        data = (json.dumps(message, ensure_ascii=False) + "\n").encode()
        cut = data.find("回".encode()) + 1
        if cut > 0:
            os.write(1, data[:cut])
            os.write(1, data[cut:])
        else:
            os.write(1, data)
    for line in sys.stdin:
        request = json.loads(line)
        if request.get("method") == "notifications/initialized":
            emit({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            emit({"jsonrpc": "2.0", "id": "server-ping", "method": "ping"})
            continue
        if "method" not in request:
            assert request["id"] == "server-ping" and request["result"] == {}
            continue
        response = dispatch(request)
        if request.get("params", {}).get("arguments", {}).get("reverse"):
            held.append(response)
            if len(held) == 2:
                for response in reversed(held):
                    emit(response)
                held.clear()
        elif response:
            emit(response)

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *args):
        pass
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert "application/json" in self.headers["Accept"] and "text/event-stream" in self.headers["Accept"]
        assert self.headers["X-Fixture"] == "test"
        if request.get("method") != "initialize":
            assert self.headers["Mcp-Session-Id"] == "fixture-session"
            assert self.headers["MCP-Protocol-Version"] == "2025-06-18"
        response = dispatch(request)
        if not response:
            self.send_response(202)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        args = request.get("params", {}).get("arguments", {})
        if args.get("reverse"):
            event = threading.Event()
            with lock:
                held.append(event)
                if len(held) == 2:
                    held[1].set()
            assert event.wait(5), "client serialized HTTP requests"
        if args.get("json") or request.get("method") == "initialize":
            body = json.dumps(response).encode()
            kind = "application/json"
        else:
            body = (": heartbeat\r\n\r\nevent: message\r\ndata: " + json.dumps({"jsonrpc": "2.0", "method": "notifications/message"}) + "\r\n\r\ndata: " + json.dumps(response) + "\r\n\r\n").encode()
            kind = "text/event-stream"
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Mcp-Session-Id", "fixture-session")
        if kind == "application/json":
            self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        for start in range(0, len(body), 7):
            self.wfile.write(body[start:start + 7])
            self.wfile.flush()
        if args.get("reverse"):
            with lock:
                if len(held) == 2:
                    held[0].set()

if len(sys.argv) > 1 and sys.argv[1] == "http":
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()
else:
    stdio()
