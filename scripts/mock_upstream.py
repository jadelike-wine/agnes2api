# agnes2api 的**上游替身**：本地真跑协议测试用。
#
# 为什么要有它：网关的判据是「四协议 × 流式/非流式 × 上游真回包」，
# 靠 `curl` 打真 Agnes 既不可重复也要花钱。这台替身只回**固定的**上游形状，
# 于是「网关转发得对不对」变成一条确定性判据（见 docs/porting/04-test-inventory.md）。
#
# 用法：
#   python scripts/mock_upstream.py 9999
# 然后把 AGNES_BASE_URL 指过来（.dev.vars 或环境变量）：
#   AGNES_BASE_URL=http://127.0.0.1:9999/v1
#
# 🔴 它**不是**产品的一部分，只服务于本地/CI 测试。绝不进 Docker 镜像。
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9999

# 上游看到的 key 会回显在响应里 —— 用来验证「网关照约定换 key」这条判据。
# 🔴 只回显**后四位**：日志/响应里不该出现完整上游 key（本仓的红线之一）。

# ── 故障注入 ────────────────────────────────────────────────────────────────
# 冷却 / 剔除的判据要求上游**真回 429 / 401**，光靠代码走查证明不了。
# 用法：POST /__control {"status": 429} —— 之后所有 /chat/completions 都回这个码；
#       POST /__control {"status": 0}    —— 恢复正常。
FAULT = {"status": 0}


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        pass

    # ── 工具 ────────────────────────────────────────────────────────────────
    def _json(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {}

    def _key_tail(self):
        auth = self.headers.get("Authorization") or ""
        k = auth[7:] if auth.lower().startswith("bearer ") else auth
        return k[-4:] if len(k) >= 4 else ""

    def _sse_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        # 分块传输：**不设 Content-Length**，这正是「流式」的形状。
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

    def _chunk(self, data: bytes):
        self.wfile.write(f"{len(data):X}\r\n".encode() + data + b"\r\n")
        self.wfile.flush()

    def _sse_end(self):
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def _sse(self, obj):
        self._chunk(f"data: {json.dumps(obj)}\n\n".encode())

    # ── 路由 ────────────────────────────────────────────────────────────────
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/models"):
            return self._json(
                200,
                {
                    "object": "list",
                    "data": [
                        {"id": "agnes-1", "object": "model", "owned_by": "agnes"},
                        {"id": "agnes-2", "object": "model", "owned_by": "agnes"},
                    ],
                },
            )
        if "/videos/" in path:
            vid = path.rsplit("/", 1)[-1]
            return self._json(
                200,
                {
                    "id": vid,
                    "object": "video",
                    "status": "completed",
                    "progress": 100,
                    "url": f"http://127.0.0.1:{PORT}/files/{vid}.mp4",
                    "model": "agnes-video",
                },
            )
        if path == "/health":
            return self._json(200, {"status": "ok"})
        self._json(404, {"error": {"message": f"no route {path}"}})

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self._read_json()

        # 故障注入开关（只服务测试，不是产品的一部分）。
        if path.endswith("/__control"):
            FAULT["status"] = int(body.get("status", 0))
            return self._json(200, {"fault": FAULT["status"]})

        if path.endswith("/chat/completions"):
            if FAULT["status"]:
                return self._json(
                    FAULT["status"],
                    {"error": {"message": f"injected {FAULT['status']}", "type": "injected"}},
                )
            return self._chat(body)
        if path.endswith("/images/generations"):
            return self._json(
                200,
                {
                    "created": int(time.time()),
                    "data": [{"url": f"http://127.0.0.1:{PORT}/files/img.png", "revised_prompt": "x"}],
                },
            )
        if path.endswith("/videos"):
            return self._json(
                200,
                {
                    "id": "vid_123",
                    "object": "video",
                    "status": "queued",
                    "progress": 0,
                    "model": body.get("model", "agnes-video"),
                },
            )
        self._json(404, {"error": {"message": f"no route {path}"}})

    def _chat(self, body):
        stream = bool(body.get("stream"))
        model = body.get("model", "agnes-1")
        # 上游回显 key 尾四位 —— 网关必须换掉客户端的 key。
        tail = self._key_tail()

        if not stream:
            return self._json(
                200,
                {
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": int(time.time()),
                    "model": model,
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": f"hello from mock (key …{tail})",
                            },
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8},
                },
            )

        # 流式：**逐块发**，块与块之间留一点间隔 —— 一次性写完测不出「逐块透传」。
        self._sse_start()
        base = {
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": int(time.time()),
            "model": model,
        }
        try:
            self._sse({**base, "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": None}]})
            for piece in ["hello", " from", " mock", " stream"]:
                self._sse(
                    {**base, "choices": [{"index": 0, "delta": {"content": piece}, "finish_reason": None}]}
                )
                time.sleep(0.02)
            self._sse({**base, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            self._chunk(b"data: [DONE]\n\n")
            self._sse_end()
        except (BrokenPipeError, ConnectionResetError):
            # 客户端中途断开 —— 网关的取消路径正是要测这个，不是错误。
            pass


if __name__ == "__main__":
    print(f"mock upstream on http://127.0.0.1:{PORT}/v1", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
