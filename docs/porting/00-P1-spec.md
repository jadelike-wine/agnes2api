# 00 · P1 实施规格书（Rust 骨架：HTTP + 四协议转发 + key 池调度 + 鉴权）

> 本文合成自同目录的五份勘察：
> `01-core-rules.md`（core 业务规则，4,417 行）、`02-http-contract.md`（HTTP 与管理 API 契约）、
> `03-kiro2api-conventions.md`（对标形状）、`04-test-inventory.md`（判据资产）、`05-feature-matrix.md`（功能去留）。
>
> **本文是 P1 的可执行规格**：模块布局、关键裁定、交付边界、依赖、验收判据、风险。
> 凡本文与五份勘察冲突之处，**本文为准**，且冲突一律在 §7 点名。
> 凡本文没有覆盖的细节，**回对应勘察文件读原文**，不要凭直觉补。
>
> 纪律：
> - 🔴 **绝不 git commit / push / tag**。本轮只产出文件。
> - 🔴 公开仓、MIT、作者只署 `xwteam <xwteam@xwteam.cn>`。**零内置凭据**（真实 key / 私有域名 /
>   服务器 IP / 真实邮箱 / 私有仓名一律不进任何文件）。本文凡涉及上游默认基址一律**引用而不抄写**。
> - 🔴 **诚实第一**：本文每一处「不做」都写了理由与归期；每一处「没弄清」都在 §8 如实列着。
>
> 工程目录：`/home/ubuntu/agnes2api-rs`。**不许改动 `/home/ubuntu/agnes2api` 与 `/home/ubuntu/kiro2api` 的任何文件。**

---

## 1. 裁定一览（先给结论，理由在各自小节）

| # | 议题 | 裁定 | 小节 |
|---|---|---|---|
| A | 端口-适配器分层在 Rust 里怎么办 | **保留 3 条端口（Storage / Fetcher / Logger），用 `Arc<dyn Trait>` 动态分发；砍掉 Runtime 端口；MailProvider 留 P3。这是一处对 kiro2api 形状的刻意偏离，进「刻意偏离名册」** | §3 |
| B | 持久化类型的 serde 分岔（04 §5 的陷阱） | **两段式：先 `serde_json::Value`，再手写 `narrow_*` 按 01 §12 那张方向表逐处窄化。DTO 只派生 `Serialize`** | §3.6 |
| C | 存储引擎 | **P1 沿用 `FileStorage`（单文件 `store.json` + 整份原子重写 + 串行写队列 + TTL），不换 sled/redb** | §3.7 |
| D | 母协议 | **OpenAI**（上游是 OpenAI 兼容 `/chat/completions`）；另三条是它的适配器；**只有一条 relay 内核** | §3.8 |
| E | 管理面路径前缀 | **保留 `/admin/api/*`**，不改成 kiro 的 `/api/admin/*`；P2 抄面板时改的是前端一个常量 | §3.9 |
| F | 负载均衡（05 的 B2/B3） | **两种模式都落**：先把游标轮询逐字移植并销账（`LbMode::Balanced`），再加 `LbMode::Priority` 粘滞；**默认 `priority`，并把切默认值登记为一次行为变更** | §3.10 |
| G | 别名前缀 / `count_tokens` / `/v1/ping`（05 的 A2、A8） | 别名前缀 **P1 做**；`count_tokens` **P1 做**（纯本地估算、零上游，口径登记为 `assumed`）；**`/v1/ping` P1 不做**（它落在 `/v1/*` 鉴权闸射程内，见 §7 矛盾 6） | §3.11 |
| H | 媒体三条端点（05 排 P1.5） | **P1 做**：同步超时档与 `attribute_sync_timeouts` 的唯一真实消费者就是它们 | §7 矛盾 2 |
| I | 用量统计形态（05 的 E1） | **P1 只落接线与「关=零成本」**；桶 vs 逐条之争随 E1 在 P1.5 一次性定 | §7 矛盾 4 |
| J | 日志采集面（05 的 F1） | **P1 照 TS 的白名单事件 + 事件环**；tracing 全量捕获归 P1.5/P2 | §7 矛盾 5 |
| K | TLS 后端 / 连接不复用 / UA 伪装 | **P1 只 rustls，不引 vendored OpenSSL，不做连接不复用、不做 UA 伪装**——这些是 kiro 对 Kiro 上游的风控对抗，在 agnes 侧**没有证据** | §5、§4.2 |
| L | 选 key 游标放哪 | **`AtomicUsize`，每进程一份，语义与 TS 逐字相同**；面板仍不许展示「下一把 key」 | §3.12 |

---

## 2. Rust 工程的模块布局

### 2.1 总体形状

顶层照 kiro2api 的**特性分组**（`server` / `protocol` / 上游适配 / `apikey` / `admin` / `webui`），
但保留 agnes2api 独有的两样：**`core/`（零 IO 纯逻辑岛）** 与 **`ports/`+`adapters/`（测试接缝）**。
两处偏离各有理由，分别写在 §2.3 与 §3。

```
agnes2api-rs/
├── VERSION                      # 版本单一真源（03 §1.1）
├── Cargo.toml                   # version 是 VERSION 的副本，由 scripts/set-version.sh 同步
├── build.rs                     # best-effort 注入 rustc 版本；任何失败一律回落 "unknown"（03 §11.1）
├── scripts/{set-version.sh, scan-secrets.sh, prepush.sh}
└── src/
    ├── main.rs                  # ~40 行：CLI → 配置 → 存储 → build_app → serve。fail-closed 在这里
    ├── lib.rs                   # 模块树 + 全仓 clippy allow 清单（集中声明，禁止函数级零散 #[allow]）
    ├── cli.rs                   # clap：-c/--config、--data-dir（不设 default_value，见 03 §3）
    ├── test_tmp.rs              # #[cfg(test)] pub mod：测试临时目录统一出口 + 自动回收（03 §10.3，必抄）
    │
    ├── ports/                   # 【裁定 A】唯一保留的抽象层——它的用途只有一个：测试接缝
    │   ├── mod.rs
    │   ├── storage.rs           # trait Storage { get / put(expires_at) / delete / list }
    │   ├── fetcher.rs           # trait Fetcher { send(OutboundRequest) -> Result<OutboundResponse> }（流式体）
    │   └── logger.rs            # trait Logger { log(LogEntry) }；LogEntry{ts,level,event,msg,corr?,fields}
    │
    ├── adapters/                # 端口的生产实现。**每个适配器都要有自己的判据**（见 §3.4）
    │   ├── storage_file.rs      # store.json：tmp+fsync+rename 原子写、串行写队列、TTL、韧性加载
    │   ├── fetcher_reqwest.rs   # 客户端工厂（数据面 / 控制面两个）+ accept-encoding 显式决定（§7 矛盾 3）
    │   ├── logger_console.rs    # logfmt 引用（调用方数据撕不开字段结构）
    │   ├── logger_store.rs      # 事件环落存储：最小间隔闸、maybe_flush / flush、畸形条目逐条丢并计数
    │   └── logger_multi.rs      # 扇出：一个 sink 抛错不影响别的 sink，更不影响调用方
    │
    ├── core/                    # 🔴 零 IO 岛：不读时钟、不随机、不定时、不网络、不读 env、不碰存储
    │   ├── types.rs             # KeyRecord / KeyStats（可选字段必须真的可选）
    │   ├── keypool.rs           # is_disabled / is_available / counts_toward_target / select_key /
    │   │                        #   pool_health / apply_success|cooldown|strike|evict
    │   ├── errors.rs            # classify_status / classify_thrown → UpstreamAction（**全局唯一那张表**）
    │   ├── stats.rs             # Tier-1：normalize_stats / max_stats / stats_delta / with_outcome / sum_stats
    │   ├── pool_index.rs        # PoolIndex 形状与分层窄化、ids_from_key_names、same_id_set
    │   ├── refreshable.rs       # TTL + invalidate(gen) + set(写穿透) + 失败保留上一份合法快照
    │   ├── config.rs            # GatewayConfig 与默认值
    │   ├── config_provenance.rs # env>存储>默认、FIELD_EXPOSURE、ENV_LOCK_MAP、FieldSource、num()
    │   ├── config_errors.rs     # ConfigErrorCode（22 条，数组是真源、类型是派生）
    │   ├── narrow.rs            # 【裁定 B】存储窄化总入口：01 §12 那张方向表的唯一实现
    │   ├── keys/
    │   │   ├── view.rs          # KeyView 投影（永不含明文）、key_bucket、mask_key
    │   │   └── api_keys.rs      # 我们签发的 sk-*：摘要、掩码、分档、findByDigest（纯函数那一半）
    │   ├── admin/
    │   │   ├── errors.rs        # AdminErrorCode 闭集（25 条）+ ADMIN_ERROR_PARAMS
    │   │   ├── config_validate.rs # 逐字段校验（纯函数，写之前跑，失败一个字节都不写）
    │   │   ├── event_ring.rs event_entry.rs
    │   │   ├── protocol_catalog.rs # PROTOCOLS / MEDIA_ENDPOINTS / MODEL_CATALOG（面板四个消费者的单一真源）
    │   │   ├── upstream_models.rs  # parse_upstream_models / diff_against_catalog
    │   │   └── upstream_facts.rs   # 上游事实登记表（含「这条未经核实」的可执行标注）
    │   └── protocol/
    │       ├── request_shape.rs # 三档参数裁定表 + require_object/array/string + 错误类型
    │       ├── sse.rs           # frame_end / extract_payloads / parse_sse_stream / sse_event
    │       └── openai.rs  anthropic.rs  gemini.rs  responses.rs
    │
    ├── pool/                    # ← 相当于 kiro 的 `kiro/`：唯一知道「上游长什么样」的地方
    │   ├── repo.rs              # KeyPoolRepo：快照缓存、写消除、pending_stats、索引、损坏恢复
    │   └── dispatcher.rs        # dispatch 主循环、超时两档、脱敏、503 reason 表、同步档超时归因
    │
    ├── apikey/mod.rs            # 对外 API 密钥存储（签发/表版本/乐观并发/落盘）——纯函数那一半在 core/keys
    │
    ├── server/
    │   ├── mod.rs               # serve()：监听、SIGTERM、有界排空、停机显式刷盘
    │   ├── app.rs               # 路由装配 + 中间件顺序（02 §2/§3 是它的规格）
    │   ├── wire.rs              # env + storage → AppState（02 §1.2 的 12 步，顺序有意义）
    │   ├── state.rs             # AppState：全 Arc + #[derive(Clone)]，三组路由共用同一份
    │   ├── errors.rs            # 三种信封 = 三个类型（**不许合成一个带 Option<code> 的结构**）
    │   ├── client_ip.rs         # 唯一消费者是 admin.login_failed；不参与鉴权、不参与限速
    │   ├── cors.rs              # 手写中间件：协议面默认开、管理面默认关（05 A6）
    │   ├── middleware/{auth.rs, config_refresh.rs, log_flush.rs, usage_flush.rs, nosniff.rs}
    │   ├── holders/{config_holder.rs, apikey_holder.rs}
    │   ├── usage_sink.rs        # Tier-2：P1 只落接线与「关=零成本」
    │   └── routes/{health.rs, openai.rs, anthropic.rs, gemini.rs, responses.rs, media.rs}
    │
    ├── admin/
    │   ├── mod.rs               # 33 条方法级端点的注册顺序（02 §3.3）
    │   ├── auth.rs              # 只认 X-Admin-Key；四条硬规则；常数时间比较
    │   ├── probe_guard.rs       # 出站探测两道闸，按 kind 分槽，**一把 guard 交给全部探测端点**
    │   ├── tend_lock.rs         # 两把锁（进程内 gate + 存储锁）——P1 只留类型与 not_wired 形态
    │   └── handlers/{session, capabilities, overview, keys, keys_write, api_keys, models,
    │                 upstream_models, model_test, verify, events, config, usage, registrar}.rs
    │
    ├── registrar/mod.rs         # P3：本轮只有「未接线」的类型与常量占位，不实现
    └── webui/mod.rs             # P2：本轮不存在（`/admin` 静态树与 /favicon.ico 随 P2 一起落）
```

### 2.2 逐模块职责与边界（**边界 = 这个模块不许依赖谁**）

| 模块 | 职责 | 不许依赖 |
|---|---|---|
| `main.rs` | 只做装配与 fail-closed。逻辑一行都不许写在这里 | — |
| `lib.rs` | 模块树 + clippy allow 清单。`main.rs`/`lib.rs` 分家**必须照抄**（03 §2），否则集成测试只能打端口 | — |
| `ports/` | 三个 trait + 它们的 DTO。**不含任何实现、任何业务** | 除 `serde_json`/`bytes`/`futures` 外的一切 |
| `adapters/` | 端口的生产实现。文件 IO、reqwest、时钟、随机**全部住在这里** | `core/`（单向：core 不知道 adapters 存在）、`server/`、`admin/` |
| `core/` | 纯函数与纯数据。**零 IO**：不读时钟（收 `now: i64` 参数）、不随机、不定时、不网络、不读 env、不碰存储 | `ports/`、`adapters/`、`tokio`、`reqwest`、`std::time::SystemTime`、`std::fs`、`std::env` |
| `core/protocol/` | 四条对外协议 ↔ `InternalRequest` 的双向转换 + SSE 原语。**不发 HTTP** | `pool/`、`server/` |
| `core/admin/` | 管理面的纯逻辑（错误码闭集、配置校验、事件环算术、协议目录常量、投影） | `ports/`、`adapters/` |
| `pool/repo.rs` | key 池的存储形态：索引、快照缓存、写消除、`pending_stats`、损坏恢复 | `server/`、`admin/`、`core/protocol/` |
| `pool/dispatcher.rs` | 转发热路径：选 key → 发上游 → 分档记账 → 换下一把。**唯一持有 `Fetcher` 打上游的转发入口** | `admin/`、`core/admin/` |
| `apikey/` | 我们签发的 `sk-*` 的存储侧（表版本、乐观并发、落盘） | `pool/` |
| `server/` | 装配与闸。**业务一律在各子模块**；各 `*_router()` 返回已 `.with_state()` 的 `Router`，顶层 `.merge()` | `registrar/` 的实现（只依赖它的「未接线」类型） |
| `admin/` | 管理面 REST。handler 薄，逻辑在 `core/admin/` | `adapters/` 的具体类型（只经端口） |
| `registrar/` | P3。P1 只提供「装不起来 = `None`」这一种形态 | 一切（它是叶子） |

三条从 kiro2api 学来、必须原样落地的边界规矩（03 §2）：

1. **只有一条 relay 内核**（`pool::dispatcher::dispatch`）。四条协议路由是它的适配器，
   **不许任何协议自己拉一条到上游的链路**。
2. **`AppState` 是唯一的状态载体**：全 `Arc` / 克隆廉价，`#[derive(Clone)]`，
   协议组 / 管理组两组路由 `.with_state()` 同一份，不建第二份存储。
   每个字段上写注释说明**谁消费它、谁不消费**。
3. **原子写 + 停机显式刷盘**（03 §9）是通用基建，住在 `adapters/storage_file.rs`，
   它不知道任何业务。

### 2.3 为什么留 `core/`（kiro2api 没有这一层）

kiro2api 把纯逻辑与 IO 混在同一个文件里，靠 `now_unix` 参数注入与 inline `#[test]` 保证可测。
agnes2api 多一条**路径级**的硬约束「`src/core` 零 IO」，它值得保留，理由有三：

1. 它是 `unit/` 那 445 格 + `unit/admin/` 那 330 格判据**能写出来的前提**：那些格子全部是
   「给定这份记录 + 这个 now，函数该返回什么」，没有零 IO 岛就得给每一格搭一套替身。
2. **它可以由一条可执行的门禁钉住**（路径前缀是最容易表达的判据形态），
   而 04 §4.2 把 `source-guards.test.ts`（228 格）裁为「B 换对象重写」——
   被测对象换成 Rust 源码，**「用 `COVERED` / `BLIND_SPOTS` 两张探针表明写边界」这套方法必须继承**。
3. kiro2api 的 `now_unix` 注入纪律与它不冲突，是它的一个子集：**core 里的函数一律收 `now: i64`**。

⚠️ 代价明写：多一层目录、多一次「这个函数该放哪」的判断。
⚠️ 边界明写：**零 IO 不等于零不确定性**。`core` 里仍然可以有「读不懂就当停用」这种取舍，
零 IO 只保证「同样的输入永远给同样的输出」。

---

## 3. 关键裁定：端口-适配器分层在 Rust 里怎么办

### 3.1 事实先摆清楚

- agnes2api TS 版有 `src/ports/`（5 个 trait）+ `src/adapters/`（8 个实现）。
- kiro2api **没有任何端口抽象**：`Pool` 直接持 `reqwest::Client`，存储就是 `stats/persist.rs` 的具体函数。
- P0 砍掉 Cloudflare Worker 之后，端口层在 TS 版**只剩一个真实用途**：
  **它是测试注入替身的接缝**。实物核对（`tests/helpers/`）：
  - `MemoryStorage`：带 `delayMs`（「瞬时返回的替身让任何 happens-before 性质都不可观测」）
    与可注入假时钟（用来测 TTL 过期）；
  - `CountingStorage`：**分开数** `get/put/delete/list` 四个桶；
  - `KeyedCountingStorage`：**按键**数写次数（「这一把键这一轮被写了几次」）；
  - `FakeFetcher`（同样有 `delayMs`）、`RecordingLogger`、`FakeMailbox`。

### 3.2 裁定

> **保留端口层，但收缩到 3 条：`Storage` / `Fetcher` / `Logger`。用 `Arc<dyn Trait>` 动态分发，不用泛型单态化。
> 砍掉 `Runtime` 端口。`MailProvider` 随注册机留到 P3。**
>
> **并且：把端口层的用途写死成「测试接缝」一条，登记进「刻意偏离名册」，不许以「将来换后端」为由扩大它。**

### 3.3 为什么不照抄 kiro2api 的无抽象形状

因为会丢掉一整族**买单买过的**判据，且这些判据用 kiro2api 的手法（`endpoint_override` + wiremock）**写不出来**：

| 判据 | 它要观察什么 | 无端口时能不能写 |
|---|---|---|
| `admin-apikeys-quota`（8 格，🔴 事故） | **1000 次错口令 = 恰好 1 次存储 get** | ❌ 数不出来 |
| `quota-panel`（7 格，🔴 事故） | 面板轮询路径**零 `list()`**、零额外读，数的是**精确次数** | ❌ |
| `keypool-repo`（37 格）/ `pool-cache`（42 格，🔴） | 「20 把 key 导入只产生 21 次 put」「50 次成功之后落盘的 requests 是 1」 | ❌ |
| `wiring`（13 格，🔴 事故） | `/health` **零存储 I/O**，且计数桩必须 `get/put/delete/list` **四种全数**（上一版只数 get，被违反的恰恰是写那一侧） | ❌ |
| `storage`（42 格，🔴） | Storage 契约：`put` 第三参 `expiresAt`、过期后 `get` 与 `list` **都看不见**——**对每种实现各跑一遍** | ❌ 没有「端口契约」就没有「对每种实现各跑一遍」这件事 |
| `pool-index-corrupt`（3 格，🔴 事故） | 存储层**先抛**（坏字节），`parse` 根本没机会跑；`all()` 与 `reconcile_index()` **两条路径**都不许抛 | ⚠️ 能用坏文件构造，但构造「get 抛而 list 不抛」这种精确组合要靠替身 |
| `logger-store`（27 格，🔴） | 「写侧遇到畸形分片值时的两条真实缺陷」「读侧丢了几条要数得出来」 | ⚠️ 很难 |
| `admin-verify`（24 格，🔴） | **写零个存储字段**、闸的粒度是 `verify:<id>`、明文一个字节不回 | ❌ |
| `admin-upstream-models`（18 格，🔴） | 传输失败分「响应头没来 / 正文没来 / 根本连不上」**三档，不许混成一句** | ⚠️ wiremock 只能确定性地构造「头来了」那一档 |

这不是「测试友好」的偏好问题。04 §3 的结论是 **166 个判据文件里 160 个命中「记录着真实事故」**——
丢掉接缝就是丢掉这一族事故的回归。

### 3.4 为什么不用泛型单态化（尽管它「零运行时开销」）

1. **泛型会传染到每一个 handler**。axum 的 handler 要求 state 是一个具体类型；
   `AppState<S: Storage, F: Fetcher, L: Logger>` 一旦出现，`Router<AppState<..>>`、每个 handler 签名、
   每个 `middleware::from_fn_with_state` 都要带三个类型参数——这与 kiro2api「**一个具体 `MessagesState` 被三组路由共用**」
   的形状正面冲突，而那个形状是用户明确要对标的那一边最值得抄的一条。
2. **零开销在这里换不到东西**。三条端口的每一次调用**本身就是一次 IO**（文件读写 / 网络往返 / 日志写入），
   一次 vtable 间接跳转在噪声里。唯一算得上热的 `Storage::get` 恰恰被判据要求在转发热路径上是**零次**
   （`all()` 走进程级快照、零 `list`）——**优化一条判据要求不许发生的调用，是本末倒置。**
3. **编译期成本是实打实的**：三个类型参数的笛卡尔积会让每一条路由的单态化实例翻几倍，
   而这个仓的 CI 有四道阻塞门（fmt / clippy -D warnings / test / 面板测试），编译时间直接换成迭代速度。

⇒ **`Arc<dyn Trait>`。** 代价老实写：多一层间接、trait 要 `dyn` 兼容（见 §5 对 `async-trait` 的取舍）。

### 3.5 端口层的**边界**（写死，不许扩大）

- **只有 3 条。** 新增端口要在评审里被点名，且必须回答「它挡住的是哪一格判据」。
- **端口不是可移植层。** P0 之后没有第二个运行时，也不打算有。
  `Storage` 的第二个实现只应该出现在 `#[cfg(test)]` 里。
  ⚠️ 例外：`storage.test.ts` 的 42 格是「**对每种实现各跑一遍**」——P1 的「每种实现」= `FileStorage` + `MemoryStorage`。
- **`Runtime` 端口砍掉**：它在 TS 里的用途是抹平 Worker 与 Node 的进程指标差异，P0 之后只剩一种。
  Rust 侧直接读 `std::process::id()` + `/proc/self/statm`，失败时 `overview.process` 如实回 `null`
  （02 §9.2：**某块失败就该块返回 `null`，绝不伪造 0**）。⚠️ 这条让本仓显式依赖 Linux，要写进部署文档。
- **适配器自己要有判据。** 端口的最大风险是「替身全绿而真实现坏了」。所以：
  - `storage_file.rs` 要跑完整的 `Storage` 契约判据（含 TTL 物理删除）；
  - `fetcher_reqwest.rs` 要用 **wiremock** 跑一组「真 HTTP」判据（状态码、流式分块、content-type、超时）；
  - 这是引入 wiremock 的**唯一**理由（见 §5）。

### 3.6 裁定 B：持久化类型的 serde 分岔（04 §5 的陷阱，**必须在写第一个 struct 之前定**）

TS 里 `disabled: "true"` 被 `JSON.parse` 原样带进来、代码显式判真值性，**记录仍然可用**。
Rust 里 `serde` 遇到 `"true"` 给 `bool` 字段会**整条记录反序列化失败** ⇒ 这把 key 直接从池子里消失。

> **裁定：所有从存储读回来的类型走两段式。**
>
> 1. `Storage::get` 返回 `Option<serde_json::Value>`（**不带泛型、不做类型推导**）；
> 2. `core/narrow.rs` 提供每种持久化数据的 `narrow_*` 函数，**方向逐处不同**，
>    严格照 01 §12 那张《存储窄化方向表》——`pool:index` 分层剔除、`key:<id>` 的 `stats` 逐字段补零、
>    `config` 字段级降级、`apikeys` **绝不当空表**、`event:*` 的 `ts` 坏才丢、
>    `usage:*` 的数组必须单独拒掉、`tend:history` 逐字段任一不合整条丢……**不许统一成一种**；
> 3. 持久化 DTO **只派生 `Serialize`**，不派生 `Deserialize`（或仅在 `#[cfg(test)]` 下派生）；
> 4. 一格断言钉住「结构体字段数 == 窄化函数覆盖的字段数」，加字段时有人必须表态。

代价老实写：约 10 个手写窄化函数，DTO 与窄化两处要同步。
收益：04 §5 那条分岔（一条脏记录让一把 key 消失）不会发生，且 01 §12 那张表逐行有对象。

### 3.7 裁定 C：P1 不换存储引擎

01 §15.5 把这件事标成「值得主线裁的分叉点：换存储引擎会让本文至少 8 处论证失效（全部是写放大那一族）」。

> **裁定：P1 沿用 `FileStorage`（单文件 `store.json`、整份原子重写、串行写队列、支持 `expiresAt` TTL）。**

理由：
1. P1 的定义是**行为等价的 Rust 骨架**。同一轮里既换语言又换存储引擎，任何一条判据失败时都无法归因。
2. 那 8 处论证正是 P1 要落的判据的依据：写消除、`pending_stats`、`READ_PATH_LIST_BACKOFF_MS`、
   `MAX_KEY_LENGTH`、`USAGE_WRITES_PER_DAY`……先让它们在 Rust 下逐条成立，再谈换引擎。
3. **但适配器内部要照抄 kiro2api 的持久化纪律**（03 §9），这不冲突：
   - `tmp → fsync → rename` 原子写，**临时文件名带 pid + 进程内单调计数器**；
   - 孤儿 tmp 只删 mtime 早于 5 分钟的；
   - **韧性加载**：文件解析不动时 **原样备份成 `.corrupt.<unix秒>.<序号>` + 逐条抢救 + 大声报错**，
     🔴 **绝不静默当空**（03 §9.2 那条「上千个 refreshToken 不可逆蒸发」的事故链在 agnes 侧同构：
     空池启动 → 运维重新导入 → 落盘用空池覆写 → 池子里的 key 全没了）；
   - **落盘自保阀**：内存里一条都没有 **且** 磁盘上文件存在却解析不动 ⇒ **跳过写盘**；
   - 同步写 + fsync 一律在 `spawn_blocking` 里做。
4. **P1 不做去抖批量刷盘**（kiro 的 5 秒去抖）：TS 的语义是「`put` 返回时已落盘」，
   而 `still_exists()`（删除不许被陈旧写回撤销）与跨副本删除持久性判据都建立在这条上。
   改成去抖是 P1.5 的候选，**改之前要先改判据**。

### 3.8 裁定 D：母协议 = OpenAI

kiro2api 的母协议是 Anthropic，因为上游语义就是 Anthropic 形状。
agnes 的上游是 **OpenAI 兼容的 `/chat/completions`**，所以：

- `InternalRequest` 就是 OpenAI chat 请求体的子集（01 §6 的定义逐字照搬）；
- `openai.rs` 是**最薄的一条**：路由不传 `expect_json`，`dispatch` 走 `sanitize(res)` 原样搬运，
  **网关从头到尾不解析它的请求体与响应体**；
- `anthropic` / `gemini` / `responses` 三条 = 自家 DTO + 自家 ↔ `InternalRequest` 转换 + handler；
- **只有一条 relay 内核**：`pool::dispatcher::dispatch`。

⚠️ P1 保留 TS 版「内部规范格式只有纯文本」这条现状，`tools` / `tool_choice` / 多模态块**一律 400 并明说**
（01 §6.1 的三档裁定表 + `UnsupportedContentError`）。
05 的 A3（工具调用双向映射）是 **P1.5**，理由见 §4.2。

### 3.9 裁定 E：管理面路径前缀保留 `/admin/api/*`

05 §0.1 点出两边刚好颠倒（agnes `/admin/api` vs kiro `/api/admin`），必须选一个。

> **裁定：保留 `/admin/api/*`。**

1. 它是**已发布的对外契约**：五语言 `API.md` 逐条写着它；02 §9.30 明确记着
   「`/admin/api/keys` 与 `/admin/api/apikeys` 刻意不共用前缀，因为前者已经是发布过的公开契约」。
2. 用户那条规则管的是「**功能和面板 UI**」，URL 不在其中。
3. **成本不对称**：P2 抄 kiro 面板 JS 时改的是前端 baseURL 一个常量（而且 P2 本来就要接一条
   「前端不许硬编码网关端点」的绊线判据，05 §6 的 `no-hardcoded-endpoints` 那 9 格）；
   改后端要动五语言文档 N 处 + 破坏既有部署。
4. 若 P2 实测发现 kiro 的面板 JS 里散着硬编码，**再加一层 `/api/admin/*` 别名**——
   kiro 自己就同时挂新旧两套。这是可加项，不是必须先定的。

### 3.10 裁定 F：负载均衡（两模式，分两步）

05 把 B2/B3（priority 粘滞 + 优先级/权重）列为 **P1 必做**，用户记忆里也钉着「`loadBalancingMode` 必须 priority」。
而 01 §2.4 + `dispatcher`(52 格) + `keypool`(27 格) 的判据钉的全是**游标轮询**。

> **裁定：P1 同期落两种模式，但顺序不可反。**
>
> **第 1 步（必做，先）**：把 TS 的 `select_key`（游标轮询）**逐字移植**，连同它的全部判据一起销账。
> 在 Rust 里它叫 `LbMode::Balanced`——**改的只是名字，语义一个字不许动**。
> **第 2 步**：加 `LbMode::Priority`：当前 key 还能用就继续用它不换；换人时取**优先级最小**的可用 key
> （kiro 的注释明写「而不是 cursor 的下一个——后者会让每次换人都往后漂，长期看又退化成轮询」）。
> `KeyRecord` 加 `priority: i64`（默认 0）与 `weight`，两者都进 `FIELD_ROLE` 的 **Scheduling** 档。
> **默认值 = `priority`。**

为什么默认切到 `priority`（而不是保守地留 `balanced`）：
粘滞选号解决的是「几百个身份在同一个 IP 上交替刷屏」这个形状。agnes 的 key 全是注册机铸出来的新号，
**这个形状在 agnes 侧比在 kiro 侧更成立**（记忆里 agnes 上游有 CF 1015 边缘限流与应用层 400 两道限流）。

🔴 **但必须同时登记两件事，不许糊过去**：
1. **切默认值是一次行为变更**，要写进 CHANGELOG 与五语言文档，不是「移植的顺手改进」；
2. **第 2 步不许复用第 1 步的判据**。`priority` 要有它自己的一组新判据；
   `balanced` 的 79 格（52+27）仍然按游标轮询销账。
   如果 P1 时间不够，**可以顺延的是第 2 步，不是第 1 步**。

⚠️ B4（每凭据 RPM 限速）/ B5（逐模型不支持记忆）/ B8（全池判停自愈）：**P1 只留字段，不接逻辑**，见 §4.2。

### 3.11 裁定 G：别名前缀 / `count_tokens` / `/v1/ping`

- **别名前缀 `/openai/v1/*`、`/claude/v1/*`、`/gemini/v1beta/*`：P1 做。**
  成本是几行 `.route()`，收益是一个网关地址同时喂多个只认固定基址的客户端。
  🔴 必须有一格判据钉住「别名与裸路径走的是**同一个 handler**」（否则将来改一处漏一处，
  而现象是「换个 base_url 行为就不一样」这种最难查的东西）。
- **`POST /v1/messages/count_tokens`：P1 做。**
  实物核对：kiro2api 的 `count_tokens` 是**纯本地估算、零网络、空池也 200**
  （`chars / CHARS_PER_TOKEN + images * IMAGE_TOKEN_ESTIMATE`，下界 1）。
  agnes 可以同样诚实地做：**零上游、零存储**，口径写进 `upstream_facts` 并标成 `assumed`，
  五语言 API.md 写明「这是本网关的本地估算，不是上游计数」。
  ⚠️ 畸形请求体必须回 **Anthropic 形状的 400**，不是 axum 默认的 422 纯文本（03 §5.4(c)）。
- **`GET /v1/ping`：P1 不做。** 理由见 §7 矛盾 6（它落在 `/v1/*` 鉴权闸的射程内，
  开豁免口是净负收益），归 P2 再议，届时若确有客户端要它，放 `/ping` 而不是 `/v1/ping`。

### 3.12 裁定 L：选 key 游标放 `AtomicUsize`

01 §15.3 把它列为要主线裁的一项。
**裁定：`AppState` 里一个 `AtomicUsize`，每进程一份，与 TS 的模块级 `let cursor` 逐字同语义。**
不做「全局一致的分布式游标」——那是行为变更，且要先回答「用什么做跨副本的原子推进」。
🔴 连带约束照旧：**面板不许展示「下一把 key / 轮换顺序」**（展示了就是给一个与实际不符的值）。

---

## 4. P1 的交付边界

### 4.1 本轮做什么

按三波排，**波内顺序不敏感，波间顺序敏感**（后一波依赖前一波的产物）。
如果要砍，从 P1-c 的尾巴砍起，不许从 P1-a 砍。

#### P1-a　骨架与转发面

| # | 交付物 | 规格出处 |
|---|---|---|
| a1 | Cargo 工程骨架：`lib.rs`+瘦 `main.rs`、`cli.rs`、`build.rs`、`VERSION`、clippy allow 清单、`test_tmp.rs` | 03 §1/§2/§3/§10.3/§11.1 |
| a2 | 三条端口 + 四个适配器（`storage_file` / `fetcher_reqwest` / `logger_console` / `logger_multi`） | §3、03 §9 |
| a3 | `core/`：`types` / `keypool` / `errors` / `stats` / `pool_index` / `refreshable` / `narrow` | 01 §2、§5、§4.1、§4.6、§12 |
| a4 | `pool/repo.rs` 全部：快照缓存、五个退避常量、写消除白名单、`pending_stats`、`still_exists`、索引重建与损坏恢复 | 01 §4 全节 |
| a5 | `pool/dispatcher.rs` 全部：主循环、两档超时、`SYNC_ATTEMPT_SHARE`、同步档超时归因、响应头白名单、MIME 钳制、流式脱敏、五条 503 reason | 01 §3 全节 |
| a6 | `core/protocol/`：三档参数裁定表、三个守卫、SSE 原语、四条协议的双向转换（含四条流式事件序列） | 01 §6 全节 |
| a7 | `server/routes/`：`/health`、四协议 10 条 + 别名前缀 + `count_tokens`、媒体 3 条 | 02 §3.1/§3.2，§3.11、§7 矛盾 2 |
| a8 | 中间件链与装配顺序：`configRefresh` / `logFlush` / `usageFlush` / 全局 `nosniff` / CORS，且**顺序按 axum 由外向内重推一遍** | 02 §2、§13；03 §8.1 |
| a9 | 三种错误信封 = 三个 Rust 类型 + `IntoResponse` 三段式（status / err_type / message） | 02 §4；03 §5.3 |
| a10 | 优雅停机：SIGTERM + **有界排空窗口（8 秒）** + 停机显式刷盘 | 03 §9.3/§9.4 |

#### P1-b　鉴权与凭据面

| # | 交付物 | 规格出处 |
|---|---|---|
| b1 | `/v1/*`+`/v1beta/*` 鉴权：凭据提取三处（`Authorization: Bearer` / `x-api-key` / `x-goog-api-key` / `?key=`）、**两段验证顺序不可交换**、常数时间比较、401 不区分四种原因 | 02 §5.1 |
| b2 | `ApiKeyHolder`：5 分钟 TTL、**刻意不挂 `use("*")`**、冷启动降级成空表而不是抛、三档日志 | 02 §7.2 |
| b3 | `apikey/`：签发（`sk-`+32 hex，CSPRNG）、摘要鉴权、表版本乐观并发、`apikeys` **绝不当空表** | 02 §9.25–9.30；01 §12 |
| b4 | `admin/auth.rs`：**只认 `X-Admin-Key`**、四条硬规则（空白 / 字符集 / ≥24 / 不得等于网关口令，**第 4 条每请求复查**）、未设则整棵 `/admin` 树 404 | 02 §5.2 |
| b5 | `ConfigHolder` + `config_provenance`：env>存储>默认、`num()` 的两向处置、`degradeOnUnreadable` 的两个调用身份、唯一 fail-closed = `gateway_token`、`RESET_CONFIG` 逃生口 | 01 §7 |
| b6 | `client_ip.rs`（唯一消费者 `admin.login_failed`，`TRUST_PROXY` 之外一律 `None`） | 02 §6 |
| b7 | `admin/probe_guard.rs`：按 kind 分槽的两道闸，**一把 guard 交给全部探测端点** | 02 §10 |

#### P1-c　管理面（key 池 / 配置 / 事件）

🔴 **硬规则先摆在这里**：02 §1.4 —— **路由表绝不许随运行时配置变化。**
「注册机没开」「Tier-2 没开」「没接线」一律是 handler 里的一个状态码，
**不是「这条路由不存在」**。所以 P1 必须把 **33 条方法级端点全部注册**，
但实现深度分三档：

| 档 | 端点 | P1 的形态 |
|---|---|---|
| **完整实现** | `session`、`capabilities`、`overview`、`models`、`keys`（list/import/bulk/purge/reveal/delete/patch/usage/verify 共 9 条）、`apikeys`（6 条）、`config`（5 条）、`events`（2 条）、`upstream/models`、`models/:id/test` | 照 02 §9 逐条 |
| **薄版（as-designed）** | `usage`、`usage/:date` | 照常 **200 + `tier:"off"`**。🔴 回 503 会让面板把「运维没打开统计」渲染成「后端坏了」（02 §1.4） |
| **占位 `not_wired`** | `registrar/tend`、`registrar/status`、`registrar/channels/:ch/test` | 照常注册、照常鉴权，回 `503 {reason:"not_wired"}`（P3 的对象） |

为什么 `keys` / `apikeys` / `config` / `events` 这四族**不能**推到 P2：

- `keys` 9 条 = key 池的**写侧**。没有它，P1 的 key 池只能靠手改 `store.json`——
  而 `pool-delete-durability`（🔴 事故）那条判据要的正是「面板删除 vs 裸存储删除」两条路径的对照。
- `apikeys` 6 条 = 鉴权第二段（子密钥）的对象。没有它，`admin-apikeys-quota`
  那条「1000 次错口令 = 1 次读」的 🔴 事故判据没有被测对象。
- `config` 5 条 = 转发热路径的依赖（`ConfigHolder`），而 `config-effective` 判据要的是
  「没设 `GATEWAY_TOKEN` 环境变量的部署，网关口令能被面板撤销，且 `dispatch()` 内部读到的配置同样跟着变」。
- `events` 2 条 = `Logger` 端口的唯一出口。`errors.test`（🔴 事故）要求
  「未预期异常落进 JSON 错误信封**并留下一条事件**」——没有读端点就没法验证那条事件真的能被查到。

#### P1 同期的治理判据（04 §4.2 里「越早接上越省事」的那几条）

| 判据 | P1 落不落 | 理由 |
|---|---|---|
| `scan-secrets`（29 格，🔴） | **落** | 它是「零内置凭据」这条硬约束的**机器保障**，六条规则含裸 IP，还能 `--history` 扫历史 |
| `check-no-binary`（14 格） | **落** | 成本近零，且是 `scan-secrets` 的咬合件 |
| `version`（1 格） | **落** | 三处一致：`VERSION` / `Cargo.toml` / `Cargo.lock`。⚠️ **不要第四处**（见 §7 矛盾 8） |
| `pretag-guard`（16 格） | **落骨架** | 用户踩过的坑：修复落在 tag 之后 = 等于没发出去。P1 先把 `prepush.sh` 与门禁表建起来 |
| 其余 19 个治理判据 | **不落** | 被测对象（40 份出货文档、面板资产、CI 徽章、i18n 字典）P1 还不存在 |

### 4.2 本轮不做什么（逐项写理由与归期）

| 项 | 不做的理由 | 归期 |
|---|---|---|
| **A3 工具调用 / 多模态双向映射** | 它不是「从零发明工具协议」，而是「把 Anthropic/Gemini/Responses 三种工具 schema 双向映射到 OpenAI 那一种」——有确定形状、可分步做，但**体量与四条协议的转换层同量级**。P1 先把「做不到就明说」这条现状（400 + 逐字说明哪一格）移植完整，它本身就是一条 🔴 事故判据。另外**多模态那一半的前提没核实**：agnes 上游 `/chat/completions` 认不认 `image_url` 块，05 §9.2 明记「我没核实」 | **P1.5** |
| **B4 每凭据 RPM 限速 / B5 逐模型不支持记忆 / B8 全池判停自愈** | 三条都是选号层的**新行为**，各自需要新判据与新字段，且 B8 必须与 agnes 既有的「冷却不触发铸币」严格区分（05 §B8 已论证两者可并存但不许混淆）。P1 塞进去会把 `dispatcher` 那 52 格的销账搅浑 | **P1.5**（B4/B5 P1 只留字段） |
| **B11 多格式批量导入容错**（数组 / `{accounts}` 包装 / 单对象） | 纯增量，不影响任何既有判据，但它改的是 `POST /admin/api/keys` 的请求体契约（今天有 `unknown_field` 400 兜着） | **P1.5** |
| **B14 每凭据出站代理 / B15 TLS 后端可切** | B14 要 reqwest 的 `socks` feature + 按代理分桶的客户端缓存；B15 要同时编译 native-tls 与 rustls，**而 native-tls-vendored 会给 Dockerfile 拖来 perl/make 与交叉编译 `AR` 两个坑**（03 §11.2）。两条都是对「被墙 / 自签 CA 代理」场景的投资，**agnes 侧今天没有这个场景的证据** | **P1.5**，开工前先确认场景 |
| **TLS 指纹对齐 / 连接不复用 / `.http1_only()` / UA 与 machineId 伪装** | 这四条是 kiro 对 **Kiro 上游风控**的对抗，成本实在（每请求多一次 TLS 握手、必须 vendored OpenSSL）。03 §13 明写「Agnes 上游是否有同类风控**必须单独验证**，不能因为 kiro2api 这么做就照搬」；记忆里 agnes 已知的是 CF 1015 边缘限流与应用层 400 两道限流，**与 TLS 指纹不是一回事** | **不做**，除非拿到实测证据 |
| **C1/C2 上游余额与配额重置时刻** | 前提是「Agnes 上游有一条查余额的接口」，而 05 §9.1 在 agnes 全仓**零命中**。🔴 **没有答案之前不许用恒为 0 或恒为 null 的字段冒充** | **阻塞项**，P2 前必须有答案 |
| **C3 成本估算 / 定价表** | 依赖 12 个模型 × 3 种模态的定价，**不许照抄 Anthropic 的价格**；且它的消费者（仪表盘「今日花费」、API Key 消费上限）都在 P2 | **P1.5** |
| **D1–D4 对外密钥补 4 格**（消费上限 / 惰性激活 / 绑定账号） | 05 §5 已论证 TS 版砍掉它们的两条理由「在 Rust 单进程下不成立了」，所以该做——但它们的消费者全在面板，且消费上限要配套「预留-对账」与两个经过论证的常量 | **P2** |
| **E1 逐条用量记录 + 默认开** | 05 §9.4 自己写着「**逐条记录的有界性没定之前不许开工 E1**」。P1 落桶的形态再拆掉是双倍工 | **P1.5**，先定有界性 |
| **F1/F2/F5 全量 tracing 捕获 / SSE 实时日志 / 失败限流专门流水** | 见 §7 矛盾 5。另：SSE 鉴权方式是一处 🟡 未定项（kiro 用 `?api_key=`，而 agnes 有**成文的相反立场**且有判据钉着「URL 上一个字节都没有」） | **P1.5 / P2**，SSE 鉴权需主线裁 |
| **面板（`webui/` + `admin-ui`）** | 用户明确「面板 UI 完全对标 kiro2api」，而 kiro 的面板是 12,501 行、CSS 3,465 行。03 §14.7 明写「P2 要对标 UI，这部分必须单独重读并出一份 UI 契约文档」 | **P2** |
| **`/favicon.ico` 与 `/admin` 静态树** | 它们由 `uiRoutes()` 注册，跟着面板一起存在或消失 | **P2** |
| **M1 用户面门户** | kiro 的 `user-ui` 是 React+Vite，会让 `cargo build` 强依赖 node。03 §7.1 建议 agnes 只做零构建手写静态那一套 | **P2.5** |
| **M2 MCP / web_search、M3 令牌 keepalive、三种交互式登录、`region` 配置** | **无对象**：agnes 上游没有 `/mcp` 端点；agnes 的 key 是长期 API key，不过期、无刷新流程；登录的等价物是注册机；上游是单一固定基址无区域概念 | **永不做**（05 §8 逐条记着理由） |
| **注册机（`registrar/` 的实现）** | P3，且 kiro2api **没有任何可参照物**。P1 只提供「装不起来 = `None`」这一种形态，并把 `config-degrade`（🔴 真机 74% 请求 500 那次事故）那条判据落成断言 | **P3** |
| **Cloudflare / KV 相关的一切**（写预算 `canWrite`/`consume`、`waitUntil`） | P0 已砍 Worker，这些机制唯一的存在理由就是 KV 写配额与平台取消后台任务。⚠️ **但背后那条纪律要留**：「有界性必须是存储自己的性质，不许绑在落盘节奏上」 | **不做**（纪律保留） |

---

## 5. 依赖清单（`Cargo.toml` 草案）

```toml
[package]
name = "agnes2api"
version = "0.1.0"          # 单一真源在 ./VERSION；此处须与之一致，由 scripts/set-version.sh 同步
edition = "2024"           # let-chains；本机工具链 rustc 1.97.1，够（edition 2024 需 1.85+）
license = "MIT"
authors = ["xwteam <xwteam@xwteam.cn>"]
publish = false

[lib]
name = "agnes2api"
path = "src/lib.rs"

[[bin]]
name = "agnes2api"
path = "src/main.rs"

[profile.release]
lto = "thin"
strip = true

[dependencies]
axum            = "0.8"                    # HTTP 框架。对标 kiro2api；不引 tower-http，CORS/限流/日志全手写
tokio           = { version = "1", features = ["full"] }   # 运行时：#[tokio::main]、interval、spawn_blocking、signal（SIGTERM 必须处理，容器里本进程常是 PID 1）
reqwest         = { version = "0.12", default-features = false, features = [
                    "rustls-tls",          # 只 rustls：TLS 指纹对抗在 agnes 侧没有证据，不背 vendored OpenSSL 的 Dockerfile 成本
                    "stream",              # 流式转发要按块读响应体（SSE 不整体缓冲是一条 🔴 判据）
                    "http2", "charset",
                    "gzip", "brotli", "deflate" ] }
                                           # 🔴 这三个压缩 feature **必须开**：出站响应头白名单剥掉了
                                           #    content-encoding / content-length，前提是「响应体到手时已解压」
                                           #    （TS 的 fetch 就是这个行为）。不开就会把压缩字节裸传给客户端。
                                           #    ⚠️ 开了之后 reqwest 会给没带 accept-encoding 的请求自动补一个
                                           #    —— 见 §7 矛盾 3，客户端工厂里要显式决定这件事
serde           = { version = "1", features = ["derive"] }  # DTO；对外字段名是契约
serde_json      = "1"                      # 存储值与窄化的载体（裁定 B：先 Value 再手写 narrow_*）
chrono          = "0.4"                    # UTC 日期算术：usage 的 `YYYY-MM-DD` 往返、日序号、事件窗口
tracing         = "0.1"                    # 进程日志。P1 只用它输出，全量捕获层是 P1.5
tracing-subscriber = { version = "0.3", features = ["env-filter", "registry"] }
                                           # registry 是为了 P1.5 能挂自定义捕获层，现在先按 kiro 的形状留好
anyhow          = "1"                      # 只用在启动路径与文件持久化（错误是终局的，调用方不按类型分支）
                                           # 业务错误一律自定义枚举 + 手写 Display，**不引 thiserror**（对标 kiro）
clap            = { version = "4.5", features = ["derive"] }  # J4 对标，成本≈0；`--config` 刻意不设 default_value
parking_lot     = "0.12"                   # 不跨 await 的锁（运行期配置、日志环、快照）。跨 await 一律 tokio::sync
subtle          = "2.6"                    # 常数时间比较：网关口令与管理口令
sha2            = "0.10"                   # key id（SHA-256 前 8 字节）、对外密钥摘要
hex             = "0.4"                    # 同上的十六进制编解码
getrandom       = "0.2"                    # CSPRNG：对外密钥 128 bit、shardId、messageId。🔴 绝不用非 CSPRNG
regex           = "1"                      # 四处：MIME deny-list、`sk-` 脱敏、IPv6 形态校验、视频任务 id
                                           # ⚠️ 线性无回溯，但 `Regex::replace` 会把 `$&` 当引用展开 ⇒ 不许用它
bytes           = "1"                      # 流式体的块类型（端口 DTO 与 axum/reqwest 共用）
futures-core    = "0.3"                    # `Stream` trait（端口签名要它）
futures-util    = "0.3"                    # `StreamExt`：流式脱敏与 SSE 解析
async-stream    = "0.3"                    # `stream!` 宏写显式状态机（四条协议的流式事件序列）
async-trait     = "0.1"                    # 三条端口要 `dyn` 兼容，而 AFIT 在 1.97 下不是 dyn 安全的
                                           # ⚠️ 这是对 kiro「依赖极简」的一次偏离。替代方案是手写
                                           #    `Pin<Box<dyn Future>>` 返回类型（零依赖，可读性差一档）——
                                           #    主线若要零依赖，换掉它不影响其它任何设计
url             = "2"                      # 配置校验的 `not_a_url` 那一格

[dev-dependencies]
tower           = { version = "0.5", features = ["util"] }  # 取 `.oneshot()`：打 Router 不开端口
wiremock        = "0.6"                    # **唯一用途**：给 `fetcher_reqwest` 这个适配器自己写判据
                                           # （端口替身全绿而真实现坏了，是端口层最大的风险，见 §3.5）
```

**刻意缺席的**（与 kiro2api 一致或有额外理由）：
`thiserror`（手写 `Display`）、`tower-http`（CORS 手写）、`uuid`（手搓 uuid_v4，只要 `getrandom`）、
`once_cell`（用 std 的 `OnceLock`）、`dashmap`、`axum-extra`、`tempfile`（用 `test_tmp.rs`）、
`rust-embed`+`mime_guess`（P2 面板）、`socks`（P1.5 B14）、`native-tls-vendored`（见 §4.2）。

---

## 6. P1 的验收判据清单（**这就是 P1 完工的定义**）

### 6.0 三条先决纪律（在写第一格断言之前）

1. 🔴 **先把 04 §3.3 那七条「已登记的假阳性形态」抄成一份 checklist，再写第一格断言。**
   顺序反了的话，搬过来的是绿色而不是保障。七条原样重述：
   ① 空遍历 / 空集合下整组断言全绿；② 只数调用次数而两条路径返回值相同；
   ③ stub 用 `resolve(状态码)` 而不是 `throw`；④ 冻结时钟下 TTL 类变异的结果是**挂起**而非失败；
   ⑤ 测试覆盖的状态让被测的选择不可观测（同实例 delete+save、计数桩只统计一半操作）；
   ⑥ 期望值从被测对象现算（同义反复）；⑦ 测试照抄一遍装配去验证装配。
2. 🔴 **「收集门禁」换形态照落**（04 §4.3）：Rust 下 glob 那一半消失了，但另一半更隐蔽——
   忘了 `mod xxx;` 整块不编译零信号、`#[ignore]`/feature 关掉整块照样绿、CI 命令带 filter。
   落法：**期望侧从磁盘扫 `.rs`、实际侧取 `cargo test -- --list`，两侧不许互相推导。**
3. 🔴 **格数在 Rust 下不可比**（04 §8.5）。销账要对着**行为清单**逐条打勾，
   **不许用「Rust 侧有多少个 `#[test]`」去证明移植完成度。**

### 6.1 契约层（`tests/contract/` 对应物）

| # | 原文件 | 格 | P1 要钉住的行为 |
|---:|---|---:|---|
| 1 | 🔴 `admin-apikeys-quota` | 8 | 拿错口令打业务口时，「子密钥表不存在/读不出来」也必须当成一份合法快照按 TTL 缓存；**1000 次错口令只许产生 1 次存储读** |
| 2 | `admin-apikeys` | 19 | 签发后真能用、停用/过期/删除后真用不了；明文只在签发那一次出现；主口令是逃生口；写侧乐观并发 + 数量上限 |
| 3 | 🔴 `admin-auth` | 70 | 常数时间比较；口令形状四条规则（**第 4 条每请求复查**）；未设 `ADMIN_TOKEN` 整棵 `/admin` **404** |
| 4 | `admin-capabilities` | 6 | 如实报本部署开了哪些能力；`tier2Enabled` 来自「建没建 sink」而非现读配置 |
| 5 | 🔴 `admin-config` | 41 | 凭据只写不读；`FIELD_EXPOSURE` 双向哨兵；写入前校验；保存后同进程立刻回读到新值 |
| 6 | `admin-danger` | 18 | 配置重置 / key 池清空两条端点的鉴权与真实作用面（`expect` / `confirm` 必填不许改成可选） |
| 7 | 🔴 `admin-events` | 38 | 游标契约（`cursor` 只有「有限数字」或 `null`）、`limit` 截断如实报 `truncated`、畸形条目不打死端点、**响应返回前必须已 flush** |
| 8 | 🔴 `admin-keys-write` | 58 | 批量导入上限 200、删除、`PATCH` 白名单六字段、边界常量 |
| 9 | `admin-keys` | 15 | 分页、默认/最大 size、明文不出现、`counts` 永远按整池算 |
| 10 | `admin-model-test` | 20 | 打哪条上游、用哪把 key、带哪个模型；只测对话模型；整轮共用一个探测闸；**一个字节上游正文都不回** |
| 11 | `admin-models` | 6 | 回本网关自己的模型目录（零存储、零网络） |
| 12 | 🔴 `admin-overview` | 12 | 存储坏掉时字段降级成 `null` 而不是硬编码 0；`config` 块刻意不包降级；响应里没有消费者的字段不许留 |
| 13 | 🔴 `admin-registrar`（部分） | ~5 | **只取 `not_wired` 那几格**：三条端点照常注册、照常鉴权、回 503。其余 46 格归 P3 |
| 14 | `admin-session` | 3 | 登录探针，**不返回任何配置或池子信息** |
| 15 | 🔴 `admin-upstream-models` | 18 | 打哪条、用哪把；传输失败分「响应头没来 / 正文没来 / 根本连不上」**三档不许混** |
| 16 | 🔴 `admin-usage`（部分） | ~5 | **只取 `tier:"off"` 那几格**：Tier-2 关着照常 200、本端点不带 `tokensCoverage`。其余 58 格归 P1.5 |
| 17 | 🔴 `admin-verify` | 24 | 验哪把、打哪条、**写零个存储字段**、明文一字节不回、闸的粒度是 `verify:<id>`、自带超时 |
| 18 | `anthropic` | 6 | `POST /v1/messages` 端到端 |
| 19 | `auth` | 12 | 网关鉴权中间件，含空凭据的防御 |
| 20 | 🔴 `config-degrade` | 4 | **注册机装不起来时网关照常活着**（真机 74% 请求 500 那次事故的回归）。必须是断言，不是注释 |
| 21 | `config-effective` | 2 | 没设 `GATEWAY_TOKEN` 环境变量的部署，网关口令能被面板撤销，且 `dispatch()` 内部读到的配置同样跟着变 |
| 22 | 🔴 `errors` | 32 | 畸形 JSON 一律 400 JSON 不是 500 纯文本；合法 JSON 结构不对 400 且**零上游调用**；未预期异常落信封**并留一条事件**；404 是 JSON 信封、405 带 `Allow` |
| 23 | 🔴 `events-cursor-heal` | 8 | 事件轮询在**持续**时钟偏移下的自愈稳态 |
| 24 | `freshness` | 10 | 配置与 key 池「刚改完多久能看见」是同一套语义——**用同一组断言分别跑两者** |
| 25 | `gemini` | 14 | `/v1beta/models`、`:generateContent`、`:streamGenerateContent` 端到端 + 方法名白名单 |
| 26 | `health` | 5 | 200/503 两态、不回显底层异常、**零存储 I/O** |
| 29 | `media` | 36 | 图片生成、视频两段式、轮询；上游 content-type 不许变成同源文档；路径穿越防护 |
| 30 | `openai` | 5 | `/v1/models`、`/v1/chat/completions` 端到端 |
| 31 | 🔴 `pool-delete-durability` | 4 | **删除不许被另一个实例的陈旧快照写回撤销**。必须**跨实例**构造（同实例内 `delete()` 会 `invalidate()`，被测动作根本发生不了） |
| 32 | 🔴 `pool-index-corrupt` | 3 | `pool:index` 存了非 JSON 字节时 `all()` 与对账**都不许抛**（Rust 侧 = serde 解析失败的**双路径**都不 panic、不向上抛） |
| 33 | 🔴 `protocol-catalog` | 11 | 协议目录与真实路由表对账，且目录里那些示例请求**真的调得通** |
| 34 | 🔴 `quota-panel` | 7 | 面板轮询路径不许出现 `list()`，也不许产生额外的读；数的是**精确次数** |
| 36 | `responses` | 7 | `POST /v1/responses` 端到端 |
| 37 | 🔴 `security-headers` | 10 | 全应用级 `nosniff`，**刻意覆盖非 `/admin` 路径** |
| 38 | 🔴 `storage` | 42 | Storage 端口契约：读写删、`list`、`put` 第三参 `expiresAt`、**过期后 `get` 与 `list` 都看不见**——**对每种实现各跑一遍** |
| 39 | 🔴 `stream-parity` | 21 | **网关不整体缓冲**：第一个正文字必须在上游第二块之前到达；CRLF 下同样成立；`streamTextPath` 与真吐出去的字节对得上 |
| 41 | 🔴 `usage-tier2`（部分） | ~8 | **只取接线那几格**：开关→装配、关闭时零存储调用零累加器、`apiKeyId` 归属传递。落盘契约与 flush 间隔归 P1.5 |
| 42 | 🔴 `wiring` | 13 | 装配、存储可写性探测、**`/health` 零存储 I/O**（两处例外写明）、管理端接线。🔴 计数桩必须 `get/put/delete/list` **四种全数** |

**P1 不取**：`admin-usage` 的 58 格、`mailbox`(20)、`manual-tend`(21)、`registrar-backoff`(3)、`ui-serve`(24)、
`admin-registrar` 的 46 格、`usage-tier2` 的 25 格。

### 6.2 单元层（`tests/unit/` 根）

| # | 原文件 | 格 | P1 要钉住的行为 |
|---:|---|---:|---|
| 43 | `anthropic` | 22 | 双向转换（system 提到 messages 首位、`system` 同时接受字符串与内容块数组、`content_filter → refusal`、流式 usage 取真实值、断流必须发 error 事件） |
| 44 | `config-env-locked` | 5 | `envLockedFields`：哪些字段被 env 锁死（**空串也算设了**）；逆表存数组不许塌 |
| 45 | `config-fatal-matrix`（部分） | ~2 | 「缺 `GATEWAY_TOKEN` 拒绝服务」这条 fatal 不受任何子系统影响 |
| 46 | `config-holder` | 11 | TTL 与刷新；存储非法值做**字段级降级**；`fixedConfigHolder` |
| 47 | `config-provenance` | 16 | env > 存储 > 默认逐字段说清谁赢；凭据只写不读；`FIELD_EXPOSURE` 是唯一真源 |
| 48 | `config` | 33 | `load_config` 的解析、数值下界、key 池两个旋钮 |
| 49 | 🔴 `dispatcher` | 52 | 选 key、失败归因闭集、上游 content-type 不许变成可渲染同源文档、错误体里的凭据片段要抹掉、**commit 抛错后仍要推进本请求内的视图** |
| 50 | `entry-fail-closed` | 10 | 缺 `GATEWAY_TOKEN` 与数值校验失败一律拒绝启动 |
| 52 | `entry-node`（部分） | ~6 | 留空的 `DATA_DIR`/`PORT` 视同没设；真实启动路径。（**补池的存储级锁归 P3**） |
| 53 | `errors` | 9 | `classify_status` / `classify_thrown` 的归因闭集 |
| 54 | `gemini` | 18 | 双向转换、模型清单（`supportedGenerationMethods` 按形态给）、流式终帧必须有 `finishReason` 与 `usageMetadata` |
| 55 | 🔴 `keypool-repo` | 37 | `add`/`get`/`save`、**`all()` 热路径零 `list`**、「权威的空索引不许让手工导入的 key 隐身」、`pool:index` 非 JSON 时的行为 |
| 56 | `keypool` | 27 | `isAvailable`/`isDisabled` 取值边界（含存储里存了字符串 `"true"` 这种脏值）、`selectKey` 选择策略、状态迁移 |
| 57 | `logger-multi` | 5 | 一个 sink 抛错不影响别的 sink，更不影响调用方 |
| 58 | 🔴 `logger-store` | 27 | 写侧畸形分片值的两条真实缺陷、读侧畸形条目逐条丢**且丢了几条要数得出来**、`flush()` 绕过最小间隔闸 |
| 59 | `logger` | 13 | logfmt 引用（调用方的数据撕不开字段结构）、`NULL_LOGGER` |
| 60 | 🔴 `pool-cache` | 42 | 进程级快照缓存；**写消除**（只有 `lastUsedAt` 变化时不落盘）及其合法性前提；两个旋钮的默认值 |
| 61 | `pool-index` | 16 | `parse` / `ids_from_key_names` / `same_id_set`（**两边各自先 dedupe 再比**）与索引常量 |
| 62 | 🔴 `quota-account` | 13 | 配额账的可执行版本：稳态与冷池两笔账不许混；必须按**生产默认值**算而不是测试夹具的 0 |
| 63 | `refreshable` | 15 | TTL、并发去重、失败语义、`gen` 代数、写穿透不推进计时；**假时钟必须递进而不是冻结** |
| 64 | 🔴 `responses` | 22 | 双向转换与**流式事件序列**（官方 SDK 能用的最小集，`sequence_number`/`item_id`/`logprobs` 一条都不许少；断流发 `response.failed` 之后不再发 `completed`） |
| 65 | `sse` | 13 | `frame_end` 两种行尾取靠前者、`extract_payloads`、收尾 flush 不许丢最后一个事件 |
| 66 | `storage-file-ttl` | 5 | `FileStorage` 的 TTL：过期键真的从落盘文件里**物理删掉** |
| 67 | `storage-health` | 10 | `watch_storage` / `probe_writable` |

**P1 不取**：`entry-node-tend-failure`(2)、`tend-scheduler`(11)、`entry-node` 的补池锁部分。

### 6.3 管理面纯逻辑层（`tests/unit/admin/`）

| # | 原文件 | 格 | P1 要钉住的行为 |
|---:|---|---:|---|
| 69 | `admin-errors` | 13 | 管理接口错误码是**闭集**；码是码、文案在字典里；面不许增长；网关业务口本期不做且不许半做 |
| 70 | `api-keys` | 30 | 掩码、摘要、过期与分档、`findByDigest` |
| 71 | 🔴 `config-validate` | 54 | 逐字段校验；凭据缺席或空串 = 不改；`EDITABLE` 与 `FIELD_EXPOSURE` / `envLockedFields` **三表逐条对账** |
| 72 | `event-entry` | 14 | `narrow_entries` 逐条窄化、`narrow_shard` 整片窄化 + 丢弃计数 |
| 73 | `event-ring` | 39 | `window_index`、`slot_of` 确定性且值域有界（🔴 `slot_of` 必须 `i32::wrapping_*`）、`shard_key`/`candidate_keys` 可从时钟直接算出来 |
| 74 | 🔴 `key-view` | 26 | `KeyView` 永不含明文；`key_bucket` 与 `is_available` 等价；序号与返回顺序；**搜索绝不能变成明文预言机**。🔴 `mask_key` 必须 `chars().take(5)` 不许 `&s[..5]` |
| 75 | 🔴 `probe-guard` | 26 | 出站探测两道闸；两条端点共用**单一真源** |
| 76 | `protocol-catalog` | 15 | 常量表自洽：`MODEL_CATALOG` 与 `MODELS` 一致、四协议 id 固定（**期望值手写字面量，不写 `PROTOCOLS.len()`**） |
| 77 | `stats` | 21 | `normalize_stats`；`max_stats` 基线只增不减（`last_error_kind` 严格大于，不可交换是有意的）；`with_outcome` 三条终态；写被消除时把计数攒起来 |
| 80 | 🔴 `upstream-facts` | 15 | 登记表本体、**依据强度**判据、真源锚判据（「这条上游事实未经核实」必须写出来） |
| 81 | 🔴 `upstream-models` | 15 | `parse_upstream_models`：读不出来是 `None`、坏记录逐条跳过；`diff_against_catalog` 两个方向都现算 |

**P1 不取**：`tend-guard`(25)、`tend-history`(16)（P3）、`usage-stats`(21)（P1.5）。

### 6.4 P1 新增的判据（TS 侧没有对象）

| 判据 | 为什么必须有 |
|---|---|
| 别名前缀与裸路径走**同一个 handler** | 否则改一处漏一处，现象是「换个 base_url 行为就不一样」 |
| `count_tokens` 零上游、零存储、空池也 200；畸形体回 **Anthropic 形状的 400** | kiro 实测形态；axum 默认 422 纯文本会让 SDK 二次抛错 |
| CORS：预检不带凭据也能过（层在鉴权闸**之外**）；管理面默认关；`Vary: Origin`；`Allow-Headers` **回声**客户端声明的那份 | 03 §8.2 逐条都是坑 |
| 出站客户端的 `accept-encoding` 行为被显式钉死 | §7 矛盾 3 |
| `LbMode::Priority` 的粘滞与「优先级最小者」（**不许复用 balanced 的判据**） | §3.10 |
| 停机：SIGTERM → 有界排空（8 秒）→ 显式刷盘；用假 future 测边界，不必真开监听 socket | 03 §9.4 |
| 韧性加载：坏存储文件 → 备份 `.corrupt.*` + 逐条抢救 + 大声报错，**绝不静默当空** | 03 §9.2 |
| 落盘自保阀：内存空 + 磁盘坏 ⇒ 跳过写盘 | 03 §9.3 |
| `Storage` 契约对 `MemoryStorage` 与 `FileStorage` 各跑一遍 | 端口层的代价，见 §3.5 |
| 窄化覆盖断言：结构体字段数 == 窄化函数覆盖数 | 裁定 B |
| 收集门禁（期望侧扫磁盘 / 实际侧 `cargo test -- --list`） | 04 §4.3 |

### 6.5 规模口径

P1 射程内的 ① 类判据约 **1,320 格**（契约 ~626 + 单元根 ~430 + 管理面纯逻辑 ~268），
与 04 §7.1 的「约 1400 格」估算一致。
🔴 再说一遍：**这个数只用来排期，不用来证明完成度。** 销账对着上面的行为清单逐条打勾。

---

## 7. 五份勘察之间的矛盾（点名 + 裁定）

| # | 矛盾 | 裁定 |
|---|---|---|
| 1 | **管理端点数：33 还是 31？** 02 §0 与 §3.3 的枚举都是 33 条方法级端点；05 §0.1 也说 33；但 02 §13 的建议分层写「adminRouter：**31 条**端点的注册顺序」 | **以 02 §3.3 的枚举为准 = 33 条**（编号 1–33，外加 0 号 `use` 与 34 号静态兜底）。§13 的 31 是笔误 |
| 2 | **媒体三条端点的归期。** 05 §10 排 P1.5；02 §3.1 把它们列在 P1 的路由表里；01 §3.5/§3.7 的同步超时档、`SYNC_ATTEMPT_SHARE`、`attribute_sync_timeouts` 三条 P1 必落的判据**唯一的真实消费者**就是它们 | **P1 做。** 不做的话那三条判据只能对着一个没有消费者的代码路径写，正是 04 §3.3 第 ① 条假阳性（空集合下全绿） |
| 3 | **B2 负载均衡。** 05 说 priority 粘滞 P1 必做；01 §2.4 与 79 格判据钉的是游标轮询 | **两模式都落，顺序不可反**（详见 §3.10）：先逐字移植游标轮询并销账，再加 priority，默认切 priority 并登记为行为变更 |
| 4 | **E1 用量统计形态。** 05 裁「对标 kiro 的逐条记录、默认开」；02 §7.3/§9.23 与 01 §8.4 的全部判据基于「桶 + 默认关 + 关=零成本」；而 05 §9.4 自己又写着「逐条记录的有界性没定之前不许开工 E1」 | **P1 只落接线与「关=零成本」三条**（`tier2Enabled` 来自建没建 sink、关闭时零存储调用、`apiKeyId` 归属传递）。形态之争随 E1 在 P1.5 一次性定，**改形态时对应判据一起重写，不是删掉** |
| 5 | **F1 日志采集面。** 05 裁「全量 tracing 捕获」；02/01 的判据是「白名单约 60 个事件 + 事件环 + 写预算 + 最小间隔闸」 | **P1 照白名单事件**。并点明：改成全量捕获会让 `EVENT_FLUSH_MIN_INTERVAL_MS` / `EVENT_WRITES_PER_DAY` 的**分母彻底改变**（01 §11 已把后者标 🟡「分母已不存在」），而 05 §6 自己也写着全量捕获让「落盘侧必须有节流 + 容量上界」这条**变得更要紧** ⇒ 两件事必须同一轮做 |
| 6 | 🔴 **`/v1/ping` 与鉴权闸冲突（五份勘察都没点出来）。** 05 A8 裁「对标 kiro，成本≈0」；但 02 §2 的装配里 `app.use("/v1/*", auth)` 覆盖整棵 `/v1` 树 ⇒ `/v1/ping` **会被鉴权闸挡住**。kiro 那边不冲突是因为它的 `/health` 与 `/v1/ping` 挂在顶层 router、鉴权只 layer 在协议组上 | **P1 不做 `/v1/ping`。** 它的信息量与 `/health` 完全重合，而为它在 `/v1/*` 鉴权闸上开一个豁免口是**净负收益**（豁免口本身需要一格判据钉死「只豁免这一条」）。归 P2 再议；届时若确有客户端要它，**放 `/ping` 而不是 `/v1/ping`** |
| 7 | 🔴 **reqwest 压缩 feature 与出站响应头白名单的耦合（五份勘察都没点出来）。** 01 §3.1 的白名单**不含** `content-length` / `content-encoding`，理由是「响应体在这里被重新包装（**fetch 已解压**）」。Rust 的 `reqwest` 默认**不**解压 | **必须开 `gzip`/`brotli`/`deflate` 三个 feature**，让 reqwest 解压，白名单剥掉那两个头才成立。⚠️ 连带代价照 03 §1.3：开了之后 reqwest 会给**没有**显式 `accept-encoding` 的请求自动补一个，所以客户端工厂里要**显式决定**这件事并写一格判据钉住。替代方案（把 `content-encoding` 加回白名单原样透传）**不采纳**：那会让流式脱敏面对压缩字节 |
| 8 | **版本一致性的处数。** 04 §4.2 #23 说「`VERSION` / `Cargo.toml` / `Cargo.lock` / **Rust 侧版本常量**四处一致」；03 §1.1 说 kiro 的代码读 `env!("CARGO_PKG_VERSION")`，**没有第四处** | **三处**：`VERSION` / `Cargo.toml` / `Cargo.lock`。代码一律 `env!("CARGO_PKG_VERSION")`，**不另存常量**——多一处就多一个会漂的地方 |
| 9 | **`Storage` 端口的方法名。** 01 §0.1 抄成 `get_raw` / `put_raw`；实物核对（`tests/helpers/counting-storage.ts`）是 `get<T>` / `put<T>` / `delete` / `list` | **以适配器实物为准**：`get` / `put(key, value, expires_at)` / `delete` / `list(prefix)`；值类型一律 `serde_json::Value`（泛型推导那一半在 Rust 侧由裁定 B 的窄化函数接管） |
| 10 | **`?key=` 查询参数到底许不许。** 02 §5.1 明写网关鉴权接受 URL 查询参数 `key`；05 §9.3 引 agnes 的成文立场「口令只走请求头，禁止查询参数」并说照抄 kiro 的 SSE `?api_key=` 等于推翻自家纪律 | **两者不矛盾，是两把钥匙。** `/v1/*` 的 `?key=` 是 **Gemini SDK 的形态**，对象是网关口令/子密钥；那条禁令的对象是**管理口令**（`X-Admin-Key`）。🔴 Rust 侧**不许**因为读到 05 §9.3 就把 `/v1/*` 的 `?key=` 删掉（删了 Gemini 官方 SDK 当场不通）；同样**不许**给 `/admin` 加 `?key=` |

---

## 8. 风险与未解决的问题

### 8.1 开工前必须先做的一件事（阻塞 P1-a）

🔴 **`src/adapters/` 至今没有一手核对过。** 01 §15.2 明记「`src/http/` 与 `src/adapters/` 完全没读」，
02 补齐了 `src/http/`，**但 `adapters/` 仍然只有转述**：
`FileStorage` 的写队列与 `store.json` 整份重写、`logger-store` 的分片与最小间隔闸、
`apikey-store` 的 `issueSecret`、`fetcher-native` 的形态、`storage-file` 的 TTL 物理删除。
而本文至少 8 处论证（写放大那一族的全部常量）建立在这些性质上。
⇒ **P1-a 的 a2（适配器）开工前补抽一轮 `src/adapters/`**，成本估计半轮，不做的话等于在转述上盖楼。

### 8.2 风险（按「不处理会怎样」排）

| # | 风险 | 不处理会怎样 | 处置 |
|---|---|---|---|
| 1 | **P1 的真实工作量被「路由表不许随配置变化」这条硬规则放大**：33 条端点里 P1 要动 31 条 | 排期爆掉，或者有人偷偷让某条路由「配置没开就不注册」——而那正是 02 §1.4 点名禁止的形态 | 用 §4.1 的三波切分排期；要砍从 P1-c 尾巴砍，**不许砍成「路由不存在」** |
| 2 | **端口层是我裁定的偏离**，后人可能读成「为了将来换后端」并往里加东西 | 端口层膨胀成一个没有第二实现的抽象税 | 写进「刻意偏离名册」（04 §4.2 #12 那条判据的对象），并在 `ports/mod.rs` 的模块注释里写死用途 |
| 3 | **替身全绿而真适配器坏了** | 端口层最典型的失效形态 | `storage_file` 跑完整端口契约；`fetcher_reqwest` 用 wiremock 跑真 HTTP（§3.5） |
| 4 | **`FileStorage` 的写放大在 Rust 下原样存在**，而 P2 会往上叠面板轮询与（P1.5 的）全量日志捕获 | 每次 put 重写整份 `store.json`，与 key 池状态回写走同一条队列 | 01 §15.5 已把「换存储引擎」标为分叉点。**P1 不换**，但 P1.5 前要重估；换的那一轮必须同时重估那 8 处论证 |
| 5 | **kiro2api 的 971 处 inline `#[test]` 我们一格都没对照过**（04 §8.4） | 在 key 池调度、SSE 流式、鉴权三块重复造轮子，或者错过 kiro 已经踩过的坑 | P1-a 开工前扫一轮 kiro 的 `pool.rs` / `stream.rs` / `auth.rs` 的测试模块（只读形状，不抄业务） |
| 6 | **多副本共卷部署下的语义**：选 key 游标每进程一份、`still_exists()` 的时序、两把补池锁没有 CAS | 「丢更新的窗口消灭不了，只是被降级了」（02 §11 原话） | 照搬 TS 语义，**并把这条如实写进部署文档**，不许在面板上声称跨副本一致 |
| 7 | **`count_tokens` 的估算口径是我们自己定的** | 客户端拿它做预算规划时会偏 | 进 `upstream_facts` 标 `assumed`，五语言 API.md 写明「本地估算，不是上游计数」 |
| 8 | **`VIDEO_TASK_ID_RE` 至今是未核实的假设**（01 §11 标 🔴） | 放宽了就是给路径穿越留门；收紧过头会让合法任务 id 被 400 | **一个字符都不许放宽**，等 P3 铸出第一把真 key 之后当场重验 |

### 8.3 未解决 / 需要主线裁的（诚实清单）

| # | 问题 | 阻塞谁 |
|---|---|---|
| 1 | 🔴 **Agnes 上游到底有没有「查余额/额度」的接口？** 05 §9.1 在 agnes 全仓零命中。没有答案之前，仪表盘的「全局剩余积分」「今日积分」两张卡**做不出来**，且**不许用恒为 0/null 的字段冒充** | C1/C2、P2 仪表盘 |
| 2 | 🔴 **Agnes 上游 `/chat/completions` 认不认多模态输入块（`image_url`）？** 工具调用那一半有实测把握，图片这一半 05 §9.2 明写没核实 | A3 的多模态那一半（P1.5） |
| 3 | 🟡 **SSE 日志流的鉴权方式。** kiro 用 `?api_key=`（`EventSource` 设不了请求头），代价是管理口令进 URL、进反代日志；agnes 有**成文的相反立场**且有判据钉着「URL 上一个字节都没有」。备选：先 `POST` 换一张短时效一次性票再连 SSE | F2（P1.5/P2） |
| 4 | 🟡 **逐条用量记录的有界性**（按天分文件 + 保留 N 天？按条数环形？）。kiro 侧 05 没读到条数上限 | E1（P1.5） |
| 5 | 🟡 **`docs-parity` 那 614 格里「从真源常量 import 期望值」的那一半在 Rust 没有现成落法**（04 §8.1）：要么 Rust 导出一份 JSON 清单给 Node 侧门禁读（多一道生成物，要防漂移），要么把这批门禁改用 Rust 写（要在 Rust 里重做 markdown 解析）。两条成本都不小 | P2 前 |
| 6 | 🟡 **对外密钥的明文落盘（`secret` 字段）Rust 版要不要沿用。** 01 §15.7 明记「它是用户拍板的，不是技术结论」；沿用的话备份/快照的处置级别要跟着升，并写进部署文档 | P1-b（b3 的存储形态） |
| 7 | 🟡 **`constant_time_equal` 的「长度不同立刻 false」在 `subtle` 下怎么写清楚**（01 §15.8）：`subtle::ConstantTimeEq` 对不等长切片的处理要显式写明，并进 code review checklist | P1-b（b1/b4） |
| 8 | 🟡 **两个「分母已不存在」的常量**（`EVENT_WRITES_PER_DAY=12`、`MANUAL_TENDS_PER_DAY=24`）要不要在 Docker-only 形态下重算 | P1-c（前者）/ P3（后者）。**P1 照抄，不重算** |
| 9 | 🟡 **管理接口的「网关业务口只说中文」这笔账**（01 §15.9）：原仓明确裁定「本期不改、且不许顺手给网关那一族也加码」。移植时照旧不改，但**这笔账要转写进新仓的 README / CHANGELOG** | P2 文档 |
| 10 | 🟡 **`MODEL_CATALOG[].endpoints` 那几行 01 没有逐条抄**（01 §15.1）。P1 建 `protocol_catalog.rs` 时**必须回 TS 原文件对一遍** | P1-a（a6） |
| 11 | 🟡 **面板 UI 契约文档还不存在。** 03 §14.7 明写 kiro 的 `admin-ui-v2` 那 12,322 行 JS/CSS「只读了骨架与 `api.js` 前 70 行」，P2 前必须单独重读并出一份 UI 契约文档；同时 03 §14.2 说 `admin/handler.rs`（6,767 行）定义了面板的全部 API 契约（camelCase 字段名逐个对齐前端），**P2 必须重读** | P2 |
| 12 | 🟡 **`/admin` 静态树在 P1 期间不存在，但 `ADMIN_TOKEN` 未设时「整棵树 404」这条 P1 就要落。** 需要确认：P1 期间 `/admin/api/*` 存在而 `/admin`（HTML）不存在，是不是会让 `admin-auth` 那 70 格里若干格暂时没有对象 | P1-b（b4） |

---

## 9. 给执行者的最后三句话

1. **先补 `src/adapters/` 那一轮勘察，再写 `adapters/` 的第一行 Rust**（§8.1）。
2. **先抄那七条假阳性红线，再写第一格断言**（§6.0）。顺序反了的话，搬过来的是绿色而不是保障。
3. **裁定 B（serde 两段式窄化）必须在写第一个持久化 `struct` 之前落地**——
   它会影响每一个持久化类型的定义，事后改等于重写一遍 `core/`。
