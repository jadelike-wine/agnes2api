# Cloudflare Workers Deployment

agnes2api ships **two deployment targets from one codebase**:

| Target | Entry point | Storage | Outbound | Use it when |
|---|---|---|---|---|
| Docker / bare metal | `main.rs` + `server::serve` | `store.json` (`FileStorage`) | reqwest | you run a VPS or want to own the process |
| **Cloudflare Workers** | `src/worker.rs` `#[event(fetch)]` | **D1** (`store` table) | `worker::Fetch` | you want no VPS, no containers, no server |

Both targets **share one axum router and all business logic** (`server::wire::build_app`).
The Workers target is not a reverse proxy in front of a VPS.

## Why it runs without a VPS

- The gateway runs in Cloudflare Workers edge isolates — nothing listens on a TCP port.
- State lives in Cloudflare D1 (managed SQLite), not in a temp file.
- Cold start, instance recycling and redeploy all leave the data in place — D1 is a real
  database, not instance memory.
- A custom domain binds directly to the Worker route.

## Platform limits (honest list)

| Docker has | Workers does not / substitute |
|---|---|
| `tokio` timer loop + `spawn` | `#[event(scheduled)]` Cron Trigger — **index reconcile only** |
| `std::fs` / `store.json` | D1 `store` table (`key`/`value`/`expires_at`) |
| `tokio::signal` graceful shutdown | no process lifecycle; the platform recycles isolates |
| reqwest (TLS / pool / decompression) | `worker::Fetch` (Cloudflare edge fetch) |
| registrar (`spawn_scheduler`) | **not supported** — panel shows `503 not_wired` |

The registrar (auto-minting Agnes keys) is **explicitly disabled** on Workers:
`AppState.registrar = None`, the three `/admin/api/registrar/*` endpoints answer
`503 not_wired`, and the panel says "not supported" rather than pretending it works.
**Importing Agnes keys by hand is unaffected.**

## Persistence: why D1, not KV or Durable Objects

| Option | Consistency | Cost | Verdict |
|---|---|---|---|
| KV | eventual (~60s) | free tier 100k reads/day | ❌ key cooldown and key revocation need read-your-writes |
| **D1** | **read your own writes** | free tier 5M reads / 100k writes per day | ✅ row-level atomic + free quota is ample |
| Durable Objects SQLite | strong | paid (no free tier) | fallback if you need more scale or multi-statement transactions |

The `Storage` port contract (`put` is visible once it returns) matches D1's read-your-writes
semantics. Cross-key read-modify-write races are the same as with `FileStorage`; they are
bounded by `pool::repo::reconcile` plus the Cron trigger.

## One-time setup

```bash
# 1. install deps (worker-build compiles Rust -> wasm)
npm install
cargo install worker-build        # once

# 2. create the D1 database, then put database_id into wrangler.toml
npx wrangler d1 create agnes2api-store

# 3. set the two passwords (secrets — never committed)
npx wrangler secret put GATEWAY_TOKEN   # the token your clients send
npx wrangler secret put ADMIN_TOKEN     # panel login; >= 24 chars, must differ from GATEWAY_TOKEN
```

## Deploy

```bash
npm run deploy        # = worker-build --profile wasm-release && wrangler deploy
```

Local development (Miniflare, D1 stored in a local file):

```bash
cp .dev.vars.example .dev.vars   # then fill in the two passwords
npm run dev
```

## GitHub Actions

`.github/workflows/worker-deploy.yml` runs `worker-build --profile wasm-release` and
`wrangler deploy` whenever a Worker-related file changes on `main`.

It needs two repository secrets: `CF_API_TOKEN` (Workers + D1 write) and `CF_ACCOUNT_ID`.

## Cost

| Tier | Workers | D1 | Fits |
|---|---|---|---|
| **Zero cost** | free tier 100k requests/day, 10ms CPU | free tier 5M reads, 100k writes/day, 5GB | personal / low-traffic gateway |
| **~$5/month** | Workers Paid from $5 (10M requests, higher CPU) | same free tier | moderate traffic |
| Scale up | Paid, billed per request | billed past the free tier | high concurrency / large pools |

- **CPU time** is the main constraint: a long-lived SSE connection costs wall-clock time but
  almost no CPU.
- D1 writes: key-pool state, issued client keys, panel config, usage/event rows — a handful
  per request.
- Cron: the free plan includes scheduled triggers; `*/30 * * * *` is 48 runs a day.

## Tests

```bash
cargo check --target wasm32-unknown-unknown --features worker   # wasm compile gate
cargo test                                                     # native regression
npm run dev && curl http://localhost:8787/health               # Miniflare smoke test
python scripts/test_worker.py                                  # 42 end-to-end checks
python scripts/test_worker_lifecycle.py                        # 18 lifecycle checks
```

The rest of the documentation in this directory (`API.md`, `ADMIN.md`, `USAGE.md`) applies
to **both** targets unchanged — the external contract does not move, only the runtime wiring.
