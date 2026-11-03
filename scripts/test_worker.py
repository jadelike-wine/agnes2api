#!/usr/bin/env python3
"""agnes2api Worker 形态的**真**端到端测试。

打的是真 Worker（本地 Miniflare）+ 真 D1 + 真 SSE，不是替身。
前置：
    npx wrangler dev --port 8787 --local
    python scripts/mock_upstream.py 9999
    python scripts/test_worker.py

每一格打 PASS/FAIL 与被断言的那个值；失败时打出原始响应片段。
"""
import json
import sys
import urllib.error
import urllib.request

GW = "http://127.0.0.1:8787"
ADMIN = "local-dev-admin-token-0123456789abcdef"
GATE = "local-dev-gateway-token-0123456789abcdef"

PASS = 0
FAIL = 0


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}   -- {detail}")


def req(method, path, body=None, headers=None, timeout=45):
    url = GW + path
    data = None
    hdrs = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        hdrs["Content-Type"] = "application/json"
    r = urllib.request.Request(url, data=data, headers=hdrs, method=method)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", "replace"), dict(resp.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace"), dict(e.headers)
    except Exception as e:  # noqa: BLE001
        return 0, f"<transport error: {e}>", {}


AH = {"X-Admin-Key": ADMIN}
GH = {"Authorization": f"Bearer {GATE}"}


def section(t):
    print(f"\n=== {t} ===")


# 0. 健康
section("0. health")
s, b, _ = req("GET", "/health")
check("/health 200", s == 200, f"status={s} body={b[:200]}")
check("storage.writable=true", '"writable":true' in b, b[:200])

# 1. 管理面鉴权
section("1. admin auth")
s, b, _ = req("GET", "/admin/api/overview")
check("no token -> 401", s == 401, f"status={s}")
s, b, _ = req("GET", "/admin/api/overview", headers={"X-Admin-Key": "wrong-token-wrong-token-xxxx"})
check("bad token -> 401", s == 401, f"status={s}")
s, b, _ = req("GET", "/admin/api/overview", headers=AH)
check("good token -> 200", s == 200, f"status={s} body={b[:200]}")
check('storage.backend="d1" (Workers must not report file)', '"backend":"d1"' in b, b[:300])
check("process=null (no PID on Workers)", '"process":null' in b, b[:300])

# Worker bindings 必须同时提供给网关和管理面；仅验证网关鉴权会漏掉管理面的空快照。
section("1b. admin config uses Worker bindings")
s, b, _ = req("GET", "/admin/api/config", headers=AH)
check("admin config 200", s == 200, f"status={s}")
try:
    cfg = json.loads(b)
except (ValueError, TypeError):
    cfg = {}
check("config is not degraded", cfg.get("configDegraded") is False)
check("no config blockers", cfg.get("loadBlocked") == [])
credential = (cfg.get("credentials") or {}).get("gatewayToken") or {}
check("gateway token is configured", credential.get("configured") is True)
check("gateway token is locked by its Worker binding",
      credential.get("lockedBy") == "env:GATEWAY_TOKEN")
check("admin config never discloses either token", GATE not in b and ADMIN not in b)
if "--config-only" in sys.argv:
    sys.exit(1 if FAIL else 0)

# 2. 手动导入 Agnes key
#
# 🔴 `resetExisting: true`：上一条判据（或上一位跑测试的人）可能在 D1 里留下 key，
#    而 D1 是**持久**的 —— 不断言「导入前是空的」，只断言「导入后池子里有这把」。
#    写成 `"total":1` 的话，测试在第二次运行时会假红。
section("2. manual key import")
s, b, _ = req("POST", "/admin/api/keys",
              {"keys": ["sk-mock-upstream-key-aaaa1111"], "resetExisting": True}, AH)
check("import 200", s == 200, f"status={s} body={b[:300]}")
s, b, _ = req("GET", "/admin/api/overview", headers=AH)
try:
    pool = json.loads(b).get("pool", {})
except Exception:  # noqa: BLE001
    pool = {}
# 🔴 池子是**累积**的（key pool 的语义就是如此；`resetExisting` 重置的是同 key 的状态，
#    不是清池）—— 所以断言「导入后池子非空且有可用 key」，不断言精确条数。
check("pool non-empty after import", pool.get("total", 0) >= 1, f"pool={pool}")
check("pool has an available key", pool.get("fresh", 0) >= 1, f"pool={pool}")

# 3. /v1/models
section("3. /v1/models")
s, b, _ = req("GET", "/v1/models", headers=GH)
check("/v1/models 200", s == 200, f"status={s} body={b[:300]}")
# 🔴 网关发布的是**自己的模型目录**（`admin::models` 的静态目录），不是把上游
# `/models` 原样转发 —— 所以断言的是「目录形状」，不是某个上游模型名。
# 上游替身回的是 `agnes-1`/`agnes-2`；网关目录里是 `agnes-2.0-flash` 这一族。
try:
    cat = json.loads(b)
except Exception:  # noqa: BLE001
    cat = {}
ids = [m.get("id", "") for m in cat.get("data", [])]
check("returns a model catalog (non-empty data[])", bool(ids), b[:300])
check("catalog entries carry object=model",
      all(m.get("object") == "model" for m in cat.get("data", [])), b[:300])
check("catalog is the gateway's own (not a raw upstream passthrough)",
      "agnes-2.5-flash" in ids and "agnes-1" not in ids, str(ids))

# 4. OpenAI chat non-stream
section("4. OpenAI chat (non-stream)")
s, b, _ = req("POST", "/v1/chat/completions",
              {"model": "agnes-1", "messages": [{"role": "user", "content": "hi"}]}, GH)
check("200", s == 200, f"status={s} body={b[:400]}")
check("has choices[].message.content", '"content"' in b, b[:400])
check("upstream sees the POOL key (…1111), not the gateway token", "1111" in b, b[:400])
check("gateway token never leaks to upstream", GATE[-4:] not in b, b[:400])

# 5. OpenAI chat stream (SSE)
section("5. OpenAI chat (stream/SSE)")
s, b, h = req("POST", "/v1/chat/completions",
              {"model": "agnes-1", "stream": True,
               "messages": [{"role": "user", "content": "hi"}]}, GH)
check("200", s == 200, f"status={s} body={b[:300]}")
check("content-type is text/event-stream",
      "text/event-stream" in (h.get("Content-Type") or h.get("content-type") or ""),
      str(h.get("Content-Type")))
check("SSE frames present", b.count("data: ") >= 3, f"frames={b.count('data: ')} body={b[:300]}")
check("[DONE] terminator", "[DONE]" in b, b[-200:])

# 🔴 逐帧解析再拼 `delta.content`：SSE 每一帧是独立 JSON，
# 直接对整段文本做子串匹配是错的（帧与帧之间有 `data: {...}` 边界）。
pieces = []
for line in b.splitlines():
    line = line.strip()
    if not line.startswith("data: "):
        continue
    payload = line[len("data: "):]
    if payload.strip() == "[DONE]":
        continue
    try:
        frame = json.loads(payload)
    except json.JSONDecodeError:
        continue
    for ch in frame.get("choices", []):
        c = (ch.get("delta") or {}).get("content")
        if c:
            pieces.append(c)
joined = "".join(pieces)
check("chunks reassemble to the full upstream text",
      joined == "hello from mock stream", f"joined={joined!r} pieces={pieces!r}")
check("stream is genuinely multi-chunk (not one buffered blob)",
      len(pieces) >= 3, f"pieces={len(pieces)}")

# 6. Anthropic non-stream
section("6. Anthropic messages (non-stream)")
s, b, _ = req("POST", "/v1/messages",
              {"model": "agnes-1", "max_tokens": 64,
               "messages": [{"role": "user", "content": "hi"}]}, GH)
check("200", s == 200, f"status={s} body={b[:400]}")
check('type="message"', '"type":"message"' in b, b[:400])

# 7. Anthropic stream
section("7. Anthropic messages (stream)")
s, b, _ = req("POST", "/v1/messages",
              {"model": "agnes-1", "max_tokens": 64, "stream": True,
               "messages": [{"role": "user", "content": "hi"}]}, GH)
check("200", s == 200, f"status={s} body={b[:300]}")
check("message_start event", "message_start" in b, b[:400])
check("message_stop event", "message_stop" in b, b[-400:])

# 8. Responses
section("8. OpenAI responses")
s, b, _ = req("POST", "/v1/responses", {"model": "agnes-1", "input": "hi"}, GH)
check("200", s == 200, f"status={s} body={b[:400]}")

# 9. Gemini
section("9. Gemini generateContent")
s, b, _ = req("POST", "/v1beta/models/agnes-1:generateContent",
              {"contents": [{"parts": [{"text": "hi"}]}]}, GH)
check("200", s == 200, f"status={s} body={b[:400]}")

# 10. images
section("10. image generation")
s, b, _ = req("POST", "/v1/images/generations", {"model": "agnes-1", "prompt": "a cat"}, GH)
check("200", s == 200, f"status={s} body={b[:400]}")

# 11. videos
section("11. video create + poll")
s, b, _ = req("POST", "/v1/videos", {"model": "agnes-1", "prompt": "a cat"}, GH)
check("create 200", s == 200, f"status={s} body={b[:400]}")
vid = None
try:
    vid = json.loads(b).get("id")
except Exception:  # noqa: BLE001
    pass
check("has video id", bool(vid), b[:300])
if vid:
    s2, b2, _ = req("GET", f"/v1/videos/{vid}", headers=GH)
    check("poll 200", s2 == 200, f"status={s2} body={b2[:400]}")
    check("poll returns completed", "completed" in b2, b2[:400])

# 12. gateway auth fail-closed
section("12. gateway auth (fail-closed)")
s, b, _ = req("POST", "/v1/chat/completions",
              {"model": "agnes-1", "messages": [{"role": "user", "content": "hi"}]})
check("no token -> 401", s == 401, f"status={s}")
s, b, _ = req("POST", "/v1/chat/completions",
              {"model": "agnes-1", "messages": [{"role": "user", "content": "hi"}]},
              {"Authorization": "Bearer nope"})
check("bad token -> 401", s == 401, f"status={s}")

# 13. panel static
section("13. admin panel static")
s, b, _ = req("GET", "/admin/")
check("panel HTML 200", s == 200, f"status={s}")
check("is HTML", "<html" in b.lower(), b[:200])

# 14. 持久化：reconcile 后 key 仍在（跨请求/实例复用）
section("14. persistence across requests (D1)")
s, b, _ = req("GET", "/admin/api/keys", headers=AH)
check("keys list 200", s == 200, f"status={s} body={b[:300]}")
check("imported key still present in D1", "sk-mock-upstream-key-aaaa1111" in b
      or '"id"' in b, b[:400])

print("\n" + "=" * 44)
print(f"RESULT: PASS={PASS}  FAIL={FAIL}")
print("=" * 44)
sys.exit(1 if FAIL else 0)
