# Cloudflare Workers 部署

agnes2api 一套代码同时支持两种部署形态：

| 形态 | 入口 | 存储 | 出站 | 适用 |
|---|---|---|---|---|
| Docker / 裸机 | `main.rs` + `server::serve` | `store.json`（FileStorage） | reqwest | 有 VPS / 想自己管进程 |
| **Cloudflare Workers** | `src/worker.rs` `#[event(fetch)]` | **D1**（`store` 表） | `worker::Fetch` | 无服务器、零 VPS |

两种形态**共用同一棵 axum 路由树与全部业务逻辑**（`server::wire::build_app`），
不是另写一套反向代理。

## 为什么不买 VPS 也能跑

- API 网关跑在 Cloudflare Workers（边缘 isolate），不监听 TCP。
- 数据落 Cloudflare D1（托管 SQLite），不是临时文件。
- 冷启动 / 实例回收 / 重新部署后数据仍在 —— D1 是持久化数据库。
- 自定义域名直接绑在 Worker route 上。

## 平台边界（诚实清单）

| Docker 有 | Workers 没有 / 替代 |
|---|---|
| `tokio` 定时轮 + `spawn` | `#[event(scheduled)]` Cron Trigger，**只做索引对账** |
| `std::fs` / `store.json` | D1 `store` 表（`key`/`value`/`expires_at`） |
| `tokio::signal` 优雅停机 | 无进程生命周期；Worker 由平台回收 |
| reqwest（TLS/连接池/解压） | `worker::Fetch`（CF 边缘 fetch） |
| 自动注册机（`spawn_scheduler`） | **不支持** —— 面板显示 `503 not_wired` |

注册机（自动铸 Agnes key）在 Workers 上**明确禁用**：
`AppState.registrar = None`，三条 `/admin/api/registrar/*` 端点回 `503 not_wired`，
面板如实显示「不支持」，而不是假装它能跑。**手动导入 Agnes key 不受影响。**

## 持久化：为什么是 D1 而不是 KV / Durable Objects

| 方案 | 一致性 | 成本 | 结论 |
|---|---|---|---|
| KV | 最终一致（~60s） | 免费层 100k 读/天 | ❌ key 池冷却 / 子密钥吊销要「写完立即可见」，KV 不满足 |
| **D1** | **读自己的写** | 免费层 5M 读/100k 写/天 | ✅ 行级原子 + 免费额度够，与 FileStorage 语义对齐 |
| Durable Objects SQLite | 强一致 | 付费（无免费层） | 备用：请求量大或要跨语句事务时升级 |

`Storage` 端口的语义（`put` 返回时已可见）与 D1「读自己的写」一致；
跨 key 的读改写竞态与 FileStorage 相同，靠 `pool::repo::reconcile` + Cron 对账收敛。

## 一次性准备

```bash
# 1. 装依赖（worker-build 编译 Rust → wasm）
npm install
cargo install worker-build        # 一次性

# 2. 建 D1 数据库，把 database_id 填进 wrangler.toml
npx wrangler d1 create agnes2api-store

# 3. 灌两把口令（secrets，绝不进版本库）
npx wrangler secret put GATEWAY_TOKEN   # 发给下游用的网关口令
npx wrangler secret put ADMIN_TOKEN     # 面板登录口令，≥24 位，与 GATEWAY_TOKEN 不同
```

## 部署

```bash
npm run deploy        # = worker-build --release && wrangler deploy
```

本地开发（Miniflare，D1 落本地文件）：

```bash
cp .env.example .dev.vars   # 填两把口令
npm run dev
```

## GitHub Actions

`.github/workflows/worker-deploy.yml`：push 到 `main` 且 Worker 相关文件变更时
自动 `worker-build --release` + `wrangler deploy`。

需要两把仓库 secret：`CF_API_TOKEN`（Workers+D1 写权限）、`CF_ACCOUNT_ID`。

## 费用（按 Cloudflare 官方免费/付费口径）

| 层级 | Workers | D1 | 适用 |
|---|---|---|---|
| **零成本** | 免费层 100k 请求/天，CPU 10ms | 免费层 5M 读、100k 写/天，5GB | 个人 / 低频网关 |
| **~$5/月** | Workers Paid $5 起（含 10M 请求、更高 CPU） | 同上免费层 | 中小流量 |
| 扩展 | Paid 按请求计费 | 超免费层按读写计费 | 高并发 / 大池 |

- **CPU 时间**是主要约束：SSE 长连接的「墙钟时长」不占 CPU，只有实际计算占。
- D1 写：key 池状态回写、子密钥签发、配置保存、用量/事件落盘 —— 每次请求少量。
- Cron：免费层含定时触发，`*/30 * * * *` 一天 48 次，可忽略。

## 测试

```bash
cargo check --target wasm32-unknown-unknown --features worker   # wasm 编译门禁
cargo test                                                     # 原生回归
npm run dev && curl http://localhost:8787/health               # Miniflare 实测
```

`docs/zh/` 里其余文档（API / ADMIN / USAGE）对两种形态**同样成立** ——
对外契约不变，变的只有运行时装配。
