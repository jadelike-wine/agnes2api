#!/usr/bin/env python3
"""并发压测：确认「并发下偶发 500 / worker hung」是否可复现、是否与 D1 写有关。

用法：python scripts/stress_concurrency.py [rounds] [workers]
"""
import json
import sys
import urllib.error
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

GW = "http://127.0.0.1:8787"
GATE = "local-dev-gateway-token-0123456789abcdef"
ADMIN = "local-dev-admin-token-0123456789abcdef"
GH = {"Authorization": f"Bearer {GATE}", "Content-Type": "application/json"}
AH = {"X-Admin-Key": ADMIN}

ROUNDS = int(sys.argv[1]) if len(sys.argv) > 1 else 3
WORKERS = int(sys.argv[2]) if len(sys.argv) > 2 else 20


def chat():
    body = json.dumps({
        "model": "agnes-1",
        "messages": [{"role": "user", "content": "hi"}],
    }).encode()
    r = urllib.request.Request(GW + "/v1/chat/completions", data=body, headers=GH, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=60) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001
        return 0, f"<transport: {e}>"


# 确保池子里有 key
urllib.request.urlopen(urllib.request.Request(
    GW + "/admin/api/keys",
    data=json.dumps({"keys": ["sk-stress-key-dddd4444"], "resetExisting": True}).encode(),
    headers={**AH, "Content-Type": "application/json"}, method="POST"), timeout=30).read()

total = Counter()
for rnd in range(ROUNDS):
    with ThreadPoolExecutor(max_workers=WORKERS) as ex:
        results = list(ex.map(lambda _: chat(), range(WORKERS)))
    c = Counter(s for s, _ in results)
    total.update(c)
    bad = [(s, b) for s, b in results if s != 200]
    print(f"round {rnd + 1}: {dict(c)}")
    for s, b in bad[:3]:
        print(f"    {s}: {b[:300]}")

print(f"\nTOTAL: {dict(total)}")
sys.exit(0 if total.get(200, 0) == ROUNDS * WORKERS else 1)
