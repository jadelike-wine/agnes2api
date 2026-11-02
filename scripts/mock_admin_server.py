# 只跑前端预览用的本地假后端：
#   - 静态文件按 /admin/... 从 admin-ui/ 目录出
#   - /health 返回 {"status":"ok"}
#   - /admin/api/* 一律返回 200 + {}（足够让登录闸通过、板块能渲染骨架）
# 用法：python scripts/mock_admin_server.py [端口]   默认 8787
import http.server
import json
import mimetypes
import os
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "admin-ui")
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8787


class Handler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, fmt, *args):  # 安静一点
        pass

    def _json(self, status, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _static(self, rel):
        path = os.path.normpath(os.path.join(ROOT, rel.lstrip("/")))
        if not path.startswith(os.path.abspath(ROOT)) or not os.path.isfile(path):
            self._json(404, {"error": {"message": "not found"}})
            return
        ctype = mimetypes.guess_type(path)[0] or "application/octet-stream"
        with open(path, "rb") as f:
            data = f.read()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _handle(self):
        path = self.path.split("?", 1)[0]
        if path in ("/", "/admin", "/admin/"):
            self._static("index.html")
            return
        if path == "/health":
            self._json(200, {"status": "ok"})
            return
        if path.startswith("/admin/api"):
            # 登录探针与各板块数据：给个空 JSON 让前端渲染骨架。
            # POST /session 验签：任何非空 key 都放行（预览用）。
            if path == "/admin/api/session" and self.command == "POST":
                self._json(200, {"ok": True})
            else:
                self._json(200, {})
            return
        if path.startswith("/admin/"):
            self._static(path[len("/admin"):])
            return
        self._json(404, {"error": {"message": "not found"}})

    do_GET = _handle
    do_POST = _handle
    do_PUT = _handle
    do_PATCH = _handle
    do_DELETE = _handle


if __name__ == "__main__":
    os.chdir(os.path.abspath(ROOT))
    print(f"mock admin UI: http://127.0.0.1:{PORT}/admin/  (Ctrl+C 停止)")
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
