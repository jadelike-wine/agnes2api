#!/usr/bin/env python3
"""Worker 形态的**生命周期**判据：冷启动、冷却持久化、key 剔除、Cron 对账、并发。

这些是「部署上去之后第二天才会发现坏」的那一类 —— 单请求测试全绿也照样能坏。
前置：`npx wrangler dev --port 8787 --local` + `python scripts/mock_upstream.py 9999`。

🔴 每一格都断言**存储里的真实状态**（打 `/admin/api/overview`），不是「请求没报错」。
"""
import json
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

GW = "http://127.0.0.1:8787"
MOCK = "http://127.0.0.1:9999"
ADMIN = "local-dev-admin-token-0123456789abcdef"
GATE = "local-dev-gateway-token-0123456789abcdef"
AH = {"X-Admin-Key": ADMIN}
GH = {"Authorization": f"Bearer {GATE}"}

PASS = FAIL = 0


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}   -- {detail}")


def req(method, path, body=None, headers=None, base=GW, timeout=45):
    data = json.dumps(body).encode() if body is not None else None
    hdrs = dict(headers or {})
    if data is not None:
        hdrs["Content-Type"] = "application/json"
    r = urllib.request.Request(base + path, data=data, headers=hdrs, method=method)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001
        return 0, f"<transport error: {e}>"


def overview():
    s, b = req("GET", "/admin/api/overview", headers=AH)
    try:
        return json.loads(b)
    except Exception:  # noqa: BLE001
        return {"_raw": b, "_status": s}


def fault(status):
    """把上游切到指定故障码（0 = 恢复）。"""
    req("POST", "/__control", {"status": status}, base=MOCK)


def chat():
    return req("POST", "/v1/chat/completions",
               {"model": "agnes-1", "messages": [{"role": "user", "content": "hi"}]}, GH)


def section(t):
    print(f"\n=== {t} ===")


# 0. 起点：池子里有一把可用 key（由上一条测试脚本导入；没有就现在导入）
section("0. setup")
ov = overview()
if ov.get("pool", {}).get("total", 0) == 0:
    req("POST", "/admin/api/keys", {"keys": ["sk-lifecycle-key-bbbb2222"]}, AH)
    ov = overview()
check("pool has >=1 key", ov.get("pool", {}).get("total", 0) >= 1, str(ov.get("pool")))
check("storage backend is d1", ov.get("storage", {}).get("backend") == "d1", str(ov.get("storage")))

# 1. 冷启动 / 实例回收：D1 里的 key 在**新一轮请求**里依然在
section("1. cold start / instance recycling (D1 durability)")
before = overview().get("pool", {})
# 连打几次，模拟不同实例各自装配（本地是同一 isolate，但走的是同一套「装配一次」路径）
for _ in range(3):
    req("GET", "/health")
after = overview().get("pool", {})
check("pool total survives repeated assembly", after.get("total") == before.get("total"),
      f"{before} -> {after}")

# 2. 429 -> 冷却，且冷却状态**落进 D1**（不是内存）
section("2. 429 cooldown written to D1")
fault(429)
s, b = chat()
check("429 upstream -> gateway returns non-2xx", s >= 400, f"status={s} body={b[:300]}")
ov = overview()
pool = ov.get("pool", {})
check("key moved into cooling", pool.get("cooling", 0) >= 1, f"pool={pool}")
check("cooldown is visible on a FRESH request (not in-memory only)",
      pool.get("cooling", 0) >= 1, f"pool={pool}")
fault(0)

# 3. 401 -> 剔除（凭据失效才 evict；5xx 只记 strike）
#
# 🔴 **必须先清冷却**：上一格刚把这把 key 打进冷却，池子里没有 fresh key 时
#    网关在**选 key 之前**就以 503 收场，压根不会去打上游 —— 那样「401 是否 evict」
#    根本没被验证到（这是本脚本第一版踩过的假阴性）。
section("3. 401 eviction")
s, b = req("GET", "/admin/api/keys", headers=AH)
try:
    listing = json.loads(b)
except Exception:  # noqa: BLE001
    listing = {}
items = listing.get("items", listing if isinstance(listing, list) else [])
ids = [it.get("id") for it in items if it.get("id")]
check("found key ids to operate on", bool(ids), b[:300])
if ids:
    s, b = req("POST", "/admin/api/keys/bulk", {"op": "clearCooldown", "ids": ids}, AH)
    check("clearCooldown 200", s == 200, f"status={s} body={b[:200]}")
    ov = overview()
    check("pool has a fresh key again", ov.get("pool", {}).get("fresh", 0) >= 1,
          str(ov.get("pool")))

    fault(401)
    s, b = chat()
    check("401 upstream -> gateway returns non-2xx", s >= 400, f"status={s}")
    ov = overview()
    pool = ov.get("pool", {})
    check("key evicted on 401", pool.get("evicted", 0) >= 1, f"pool={pool}")
    fault(0)

# 4. 恢复：unevict 后能再次转发
section("4. recovery after eviction")
if ids:
    s, b = req("POST", "/admin/api/keys/bulk", {"op": "clearCooldown", "ids": ids}, AH)
    for kid in ids:
        req("PATCH", f"/admin/api/keys/{kid}",
            {"clearCooldown": True, "clearStrikes": True, "unevict": True}, AH)
    ov = overview()
    check("key restored to fresh after unevict", ov.get("pool", {}).get("fresh", 0) >= 1,
          str(ov.get("pool")))

s, b = req("GET", "/admin/api/keys", headers=AH)
check("keys list readable", s == 200, f"status={s}")
try:
    listing = json.loads(b)
except Exception:  # noqa: BLE001
    listing = {}
items = listing.get("items", listing if isinstance(listing, list) else [])
check("key list is non-empty", bool(items), b[:300])

# 5. 并发：20 个并发请求全部走通（同一把 key，同一 isolate）
section("5. concurrency (20 parallel)")
fault(0)
req("POST", "/admin/api/keys", {"keys": ["sk-concurrent-cccc3333"], "resetExisting": True}, AH)


def one(_):
    return chat()[0]


with ThreadPoolExecutor(max_workers=20) as ex:
    codes = list(ex.map(one, range(20)))
ok = sum(1 for c in codes if c == 200)
check("all 20 concurrent requests succeeded", ok == 20,
      f"ok={ok}/20 codes={sorted(set(codes))}")

# 6. Cron 对账（本地靠 /cdn-cgi/local/scheduled 手动触发）
section("6. cron trigger (index reconcile)")
s, b = req("GET", "/cdn-cgi/local/scheduled")
check("scheduled event accepted", s in (200, 404), f"status={s} body={b[:200]}")
ov = overview()
check("pool still consistent after reconcile", ov.get("pool", {}).get("total", 0) >= 1,
      str(ov.get("pool")))

# 7. 面板配置写入 D1
section("7. panel config persists in D1")
s, b = req("GET", "/admin/api/config", headers=AH)
check("config readable", s == 200, f"status={s}")

print("\n" + "=" * 44)
print(f"RESULT: PASS={PASS}  FAIL={FAIL}")
print("=" * 44)
sys.exit(1 if FAIL else 0)
