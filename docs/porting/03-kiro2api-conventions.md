# kiro2api Rust 惯例约定书（agnes2api-rs 的对标形状）

> 用途：agnes2api 从 TypeScript 重写为 Rust 时，**照着 kiro2api 的样子写**。本文把
> `/home/ubuntu/kiro2api`（axum 0.8 + tokio，71 个 `.rs` / 53,852 行，src 内 971 处
> `#[test]`/`#[tokio::test]`，`tests/` 内 7 处）读出来的成文与不成文规矩逐条抽出来。
>
> **本文只描述 kiro2api 现状，不替 agnes2api-rs 做决定。** 每一节末尾若有「移植取舍」，
> 是建议而非结论，最终由主线裁。
>
> 证据来源：全部来自直接阅读源文件。文末「我没读透的地方」诚实列出未覆盖面。

---

## 0. 一句话概括这套风格

**注释即设计文档，且注释里写的是「踩过什么坑」而不是「这段代码做了什么」。**

kiro2api 里几乎每一个非平凡的常量、每一个 `.layer()` 的顺序、每一个 `0`/`None` 的取值，
上面都挂着一段说明「为什么是这个值、不这样会出什么事、线上实际出过什么事」。测试同理：
大量测试的文档注释本身就是事故复盘（「回归：……此前……于是……」）。

这不是可选的装饰，它是这套代码的**主要防退化机制**——因为项目没有 thiserror、没有
tower-http、没有 utoipa，很多约束（连接不复用、HTTP/1.1 锁定、默认 TLS 后端）
无法由类型系统表达，只能靠「注释说清楚 + 测试钉死」这一对组合。

移植时如果只抄结构不抄这条纪律，抄到的是骨架不是方法。

---

## 1. Cargo.toml：依赖清单与那几段长注释

### 1.1 工程头

```toml
[package]
name = "kiro2api"
version = "0.17.2"      # 单一真源在 ./VERSION，此处须与之保持一致（由 scripts/set-version.sh 同步）
edition = "2024"
license = "MIT"
authors = ["xwteam"]
publish = false          # 不发 crates.io

[[bin]]
name = "kiro2api"
path = "src/main.rs"

[profile.release]
lto = "thin"
strip = true
```

- **edition 2024**：代码里大量使用 let-chains（`if let Some(x) = a && x > 0 {}`、
  `if let Ok(v) = h.to_str() && ...`），这是 2024 edition 才稳定的语法。移植时工具链
  必须够新（Rust 1.85+ 才有 edition 2024；let-chains 稳定在 1.88）。
- **版本单一真源 = `VERSION` 文件**，`Cargo.toml` 的 `version` 只是它的副本，由
  `scripts/set-version.sh` 同步。`/health` 与 admin `server-info` 都读
  `env!("CARGO_PKG_VERSION")`。
- `publish = false` + `lto = "thin"` + `strip = true`：这是「自部署二进制」而非「库」的取向。

### 1.2 features：native-tls 是默认，且理由是 TLS 指纹

```toml
[features]
default = ["native-tls-backend"]
native-tls-backend = ["reqwest/native-tls-vendored"]
```

原文注释（逐条记下，这是典型的「踩过坑才写得出来」）：

> 默认走 native-tls(OpenSSL)：真实 Kiro 客户端是 Electron/Node，TLS 握手用的是 OpenSSL，
> 其 ClientHello 指纹与 rustls 截然不同。**TLS 指纹在任何 HTTP 内容发出之前就暴露**，
> 是最标准的机器人识别手段之一 —— 故与真实客户端对齐。
> 关掉此特性则回落 rustls（编译更快、无需 OpenSSL 工具链，但指纹对不上）。

配套的运行期开关在 `src/http.rs`：`tlsBackend` 配置项可在**运行时**二选一，理由是
「native-tls 用系统证书库、rustls 用内置根证书，走自签 CA 代理时往往只有一个握得上手，
而现象是『刷不出令牌』或『直接连不上』，与 TLS 毫无字面关系；此前是编译期二选一，
换后端得重新出镜像 —— 排查时最不该卡在这种地方」。

⚠️ **代价**：`native-tls-vendored` 会在构建时用 Perl 编译一份 OpenSSL，Dockerfile 因此
必须装 `perl` + `make`，交叉编译还必须把 `AR` 指向交叉工具链（见 §11.2）。

### 1.3 gzip/brotli/deflate 与 Accept-Encoding 的耦合（最隐蔽的一条）

```toml
reqwest = { version = "0.12", default-features = false, features = [
  "rustls-tls", "stream", "json", "socks", "http2", "charset",
  "gzip", "brotli", "deflate" ] }
```

原文注释：

> gzip/brotli/deflate：**必须**与 Social 刷新声明的 `Accept-Encoding` 同时成立。真实桌面端
> 用 axios 打自家 auth 服务，固定声明 `gzip, compress, deflate, br`；我们要对齐这个声明，
> 就必须真能解开上游据此压缩的响应，否则每次刷新都拿到一堆压缩字节、JSON 解析全挂。
> 注意这会让 reqwest 给**没有**显式 accept-encoding 的请求自动补一个 —— 数据面不能带这个头
> （真实 aws-sdk-js on Node 不发），故 `http.rs` 里数据面/控制面客户端一律
> `no_gzip/no_brotli/no_deflate` 关掉自动补，只在刷新那条链路上显式声明。

**这条是本仓最值得抄走的工程教训**：开一个 cargo feature 会改变**所有**客户端的默认行为，
于是「为 A 链路开的能力」必须在 B、C 链路显式关掉。三个客户端工厂
(`streaming()` / `unary()` / `axios()`) 就是为此分家的。

### 1.4 其余依赖与各自的职责

| crate | 用途 | 备注 |
|---|---|---|
| `axum` 0.8 | HTTP 框架 | 无 tower-http，CORS/限流/日志全手写 |
| `tokio` (full) | 运行时 | `#[tokio::main]`、`interval`、`spawn_blocking`、`signal` |
| `reqwest` 0.12 | 出站 | 见 §1.3；`socks` 是为凭据级代理 |
| `serde` + `serde_json` | DTO | `#[serde(rename_all = "camelCase")]` 是对外契约 |
| `chrono` | 时间 | API-KEY 用 `DateTime<Utc>`，池/统计用 unix 秒 |
| `tracing` + `tracing-subscriber` (env-filter, registry) | 日志 | registry 是为了挂自定义捕获层 |
| `anyhow` | 顶层错误 | 只用在 6 个文件，见 §5 |
| `clap` (derive) | CLI | 只有 `-c` 与 `--credentials` 两个参数 |
| `parking_lot` | 同步锁 | 只在**不跨 await** 的场合 |
| `subtle` | 常数时间比较 | key 校验用 `ConstantTimeEq` |
| `rust-embed` + `mime_guess` | 面板嵌入 | 见 §7 |
| `async-stream` + `futures-core` | SSE 流 | `stream!` 宏 |
| `base64`/`hex`/`sha2`/`crc`/`getrandom` | 协议与 id | `getrandom` 手搓 uuid_v4，不引 uuid crate |
| `url` | 解析 | |

dev-dependencies 只有两个：`tower`(util，取 `oneshot`) 与 `wiremock` 0.6。

**注意缺席的**：没有 `thiserror`、没有 `tower-http`、没有 `uuid`、没有 `once_cell`
（用 std 的 `OnceLock`）、没有 `dashmap`、没有 `axum-extra`。这是刻意的「依赖极简」取向：
能用 std + 手写 30 行解决的，不引 crate。`src/kiro/mod.rs` 里手写的 `uuid_v4()`
就是例子（还顺带修了一个坑：以前发的是 32 位无连字符十六进制，形状不对，
而这个字段出现在每一个请求上，是零成本 100% 命中的判别器）。

---

## 2. 模块布局：`src/` 下各目录的职责边界

```
src/
├── main.rs                 # 只做装配：解析 CLI → 载配置 → 建日志捕获 → server::serve
├── lib.rs                  # 模块树 + 全仓 clippy allow 清单
├── cli.rs                  # clap Cli（47 行）
├── config.rs               # Config（启动期不可变） + RuntimeConfig（运行期可变）
├── http.rs                 # 出站 reqwest 客户端工厂（streaming/unary/axios + 按代理分桶缓存）
├── logcap.rs               # tracing 自定义 Layer：环形缓冲 + broadcast 扇出
├── models_catalog.rs       # 静态模型目录（动态清单拿不到时的回落）
├── test_tmp.rs             # #[cfg(test)] 专用：测试临时目录统一出口
│
├── server/                 # ← HTTP 装配层
│   ├── mod.rs              #   build_router*（路由组装）、serve()、优雅停机、PersistHandles
│   └── auth.rs             #   鉴权中间件 + 两种闸（Protocol / Admin）+ 消费上限预留
│
├── protocol/               # ← 对外四协议（DTO + handler + 转换 + 流）
│   ├── anthropic/          #   types / handler(含 relay 内核) / stream / websearch
│   ├── openai/             #   types / convert / handler
│   ├── gemini/             #   types / convert / handler
│   └── responses/          #   types / convert / handler
│
├── kiro/                   # ← 上游后端适配（相当于 agnes2api 的 provider 层）
│   ├── pool.rs             #   账号池：选号 / 失败分级 / 冷却 / 负载均衡
│   ├── credential.rs       #   凭据 DTO + 落盘（原子写、韧性加载、id 高水位）
│   ├── ensure_fresh.rs     #   令牌保鲜：per-credential 单飞锁 + 刷新后落盘
│   ├── refresh.rs          #   刷新的具体上游调用
│   ├── keepalive.rs        #   活跃账号提前续期的后台任务
│   ├── provider.rs         #   数据面调用 + 伪装身份（UA/machineId）
│   ├── convert.rs          #   上游事件流 ↔ Anthropic 语义（4188 行，最大的单文件）
│   ├── endpoint.rs         #   端点回退清单
│   ├── eventstream/        #   AWS event-stream 二进制帧解码（crc/decoder/frame/header）
│   ├── login/              #   三种登录方式（builderid / iam_sso / sso_token）
│   ├── machine_id.rs       #   machineId 派生与归一
│   ├── mcp.rs              #   服务端内置搜索（JSON-RPC）
│   └── wire.rs             #   线格式 DTO
│
├── apikey/mod.rs           # ← 对外 API-KEY 存储（sk-*）：CRUD、过期、消费上限、绑定
├── stats/                  # ← 统计与持久化基建
│   ├── persist.rs          #   ★ 通用：原子写 + 脏标记 + 5s 去抖刷盘循环（被 3 个存储复用）
│   ├── usage.rs / eventlog.rs / model.rs / pricing.rs
├── balance/                # ← 上游余额缓存（5 分钟 TTL）
├── models_cache/           # ← 动态模型清单缓存
├── admin/                  # ← 管理面 REST（handler / log_handler / login_session）
├── user/                   # ← 用户面 REST（key 自鉴权，不走 admin 闸）
└── webui/mod.rs            # ← rust-embed 静态资源服务（/admin 与 /user 两套）
```

### 边界规矩（从代码里读出来的，不是文档里写的）

1. **`protocol/*` 只管「对外协议形状」，不碰上游**。四个协议各有自己的 `types.rs`
   （serde DTO）与 `convert.rs`（本协议 ↔ Anthropic 语义），handler 里统一调
   `protocol::anthropic::handler::relay_core*` / `relay_stream*` 这条**唯一的中转内核**。
   → 移植启示：agnes2api-rs 也应只有**一条** relay 内核，其它协议是它的适配器。

2. **`kiro/` 是唯一知道上游长什么样的地方**。`protocol` 层不直接发 HTTP。

3. **`stats/persist.rs` 是通用基建**，被 `stats`、`apikey`、`balance` 三个存储复用
   （`dirty_channel()` / `spawn_flush_loop*()` / `atomic_write_json()` / `read_json()`）。
   它不知道任何业务，快照闭包返回 `Vec<u8>`，「由调用方在持锁下拿快照并序列化，
   避免把锁类型泄漏到本模块」。

4. **`server/` 只做装配与闸**，业务一律在各子模块。`admin/mod.rs`、`user/mod.rs`、
   各 `*_router()` 都返回**已 `.with_state()` 的 `Router`**，顶层用 `.merge()` 拼。

5. **`lib.rs` + `main.rs` 分家**：所有逻辑在 lib crate 里，`main.rs` 只有 40 行装配
   + 60 行测试。好处是集成测试（`tests/*.rs`）能 `use kiro2api::...` 直接调内部函数。
   **这条务必照抄**——否则 `tests/` 只能打 HTTP 端口。

### `lib.rs` 的 clippy allow 清单（连同理由）

```rust
#![allow(clippy::result_large_err)]        // axum handler 的错误枚举天然较大，统一装箱会污染大量签名
#![allow(clippy::result_unit_err)]         // 少数内部函数用 Result<_, ()> 表达「成功/跳过」
#![allow(clippy::unnecessary_sort_by)]     // 纯风格
#![allow(clippy::doc_lazy_continuation)]   // 纯风格
#![allow(clippy::doc_nested_refdefs)]      // 纯风格
```

CI 跑 `cargo clippy --all-targets -- -D warnings`，所以「允许什么」必须在此集中声明，
不许在函数上零散 `#[allow]`。

---

## 3. `main.rs` 怎么装配（顺序本身是约束）

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::parse();                       // 1. CLI
    let mut cfg = config::Config::load(&args.config)?;  // 2. 配置（内含 env 覆盖）
    resolve_credentials_path(&mut cfg, &args);     // 3. 补最高优先级那一层

    // 4. 日志捕获器必须在 init_tracing 之前建好并作为可选层挂进 registry，
    //    才能拦截全部事件
    let log_capture = if cfg.log_capacity > 0 {
        Some(Arc::new(logcap::LogCapture::new(cfg.log_capacity)))
    } else { None };
    logcap::init_tracing(log_capture.clone());

    server::serve(cfg, log_capture).await          // 5. 交棒
}
```

**四层配置优先级**（这套优先级本身是被坑出来的，注释写得极细）：

```
CLI (--credentials)  >  环境变量 (CREDENTIALS_PATH)  >  config.json  >  内置默认（就近解析）
```

关键设计：
- `cli.rs` 里 `--credentials` **刻意不设 `default_value`**——「有默认值就无法区分
  『用户显式指定』与『没给』，会把优先级更高的环境变量与 config.json 一起压掉」。
- 「内置默认」不是裸相对名 `credentials.json`，而是
  `default_credentials_beside_config(&args.config)`：**就近解析到配置文件所在目录**。
  理由：凭据路径的父目录同时承载 `usage_records.json` / `api_keys.json` / 余额缓存，
  容器里必须落在挂载卷内；容器以 `-c /app/data/config.json` 启动，故默认落在 `/app/data/`。
  而**不能**在镜像里烘焙 `ENV CREDENTIALS_PATH` ——那会让环境变量层凌驾于 config.json 之上，
  把用户在 config.json 里设的自定义路径静默改道。
- `Config::load` 还额外解析一遍原始 JSON，判「`credentialsPath` 这个键在文件里到底有没有
  出现」，而**不是**拿「值 == 内置默认」来判——后者会误伤显式写了
  `"credentialsPath": "credentials.json"` 的老用户，把他们的数据目录静默搬家。

`main.rs` 的 5 个测试全部在钉这套优先级（空串不得清空下层、CLI 压过 config.json、等等）。

**移植取舍**：agnes2api 的数据落点（key 池、统计）同样要「全部由一个路径的父目录推断」，
这套四层优先级 + 就近解析建议照抄，它挡住的是「容器重建即丢数据」这类不可逆事故。

---

## 4. 状态管理

### 4.1 `MessagesState`：一个被所有人共享的巨型 Clone 结构

```rust
#[derive(Clone)]
pub struct MessagesState {
    pub pool: Arc<tokio::sync::Mutex<Pool>>,
    pub client: reqwest::Client,          // 数据面（流式，无整请求超时）
    pub control_client: reqwest::Client,  // 控制面（一问一答，有硬顶）
    pub cfg: Arc<Config>,                 // 启动期不可变
    pub runtime_cfg: SharedRuntimeConfig, // = Arc<parking_lot::RwLock<RuntimeConfig>>
    pub endpoint_override: Option<String>,// 测试 / 自建网关用
    pub stats: Arc<StatsManager>,
    pub api_keys: Arc<ApiKeyStore>,
    pub balance: Arc<BalanceCache>,
    pub models_cache: Arc<ModelsCache>,
    pub builderid_sessions: LoginSessions<BuilderIdSession>,
    pub iam_sso_sessions: LoginSessions<IamSsoSession>,
    pub log_capture: Option<Arc<LogCapture>>,
    pub refresh_ctx: RefreshCtx,
}
```

规矩：
- **全是 `Arc` / 克隆廉价的句柄**，`#[derive(Clone)]`，axum 每请求克隆一次不心疼。
- **protocol / admin / user 三组路由共用同一个 `MessagesState`**（`.with_state()` 各自一次），
  所以 admin 的只读查询与 relay 的写入天然共享同一份内存存储，不需要第二份。
- 每个字段上都有注释说明**谁消费它、谁不消费**（例如 balance「relay 热路径不消费」），
  这在 14 个字段的结构里是必须的。
- `endpoint_override: Option<String>` 是**为可测性付的设计税**：`Some(url)` 直接 POST
  到该 URL 并自行反馈池，`None` 走生产端点回退。wiremock 测试全靠它。

### 4.2 锁的选型纪律（本仓最硬的一条不成文规矩）

| 场景 | 用什么 | 理由（原文） |
|---|---|---|
| 账号池 `Pool` | `Arc<tokio::sync::Mutex<Pool>>` | 「handler 在 `call_with_fallback` 内的网络 `.await` 期间持有 `&mut Pool`，若用同步锁跨 `.await` 持锁是 bug」 |
| 运行期配置 | `Arc<parking_lot::RwLock<RuntimeConfig>>` | 读取是纯内存、瞬时，不跨 await |
| API-KEY 存储 | `parking_lot::RwLock` + `Mutex` | 同上 |
| 日志环形缓冲 | `parking_lot::Mutex<VecDeque>` | 同上 |
| 出站客户端缓存 | `std::sync::OnceLock<std::sync::Mutex<HashMap>>` | 进程级、构造一次 |
| 进程启动时刻 | `std::sync::OnceLock<Instant>` | 幂等、单调钟（避免系统时钟回拨） |
| TLS 后端 | `std::sync::OnceLock<TlsBackend>` | 启动时设一次 |

**判据一句话：跨 `.await` 就用 `tokio::sync`，不跨就用 `parking_lot`。**

配套的更硬一条：**池锁绝不跨网络 `.await` 持有**。`ensure_fresh` 的模块注释写死：
「取快照时短暂持锁 → 释放 → 网络刷新 → 再短暂持锁写回」。`keepalive::due_for_refresh`
也是「只读一次池锁后立即释放」。

### 4.3 单飞（single-flight）：`RefreshCoordinator`

这是本仓并发模型里最值得抄的一块：

```rust
pub struct RefreshCoordinator {
    locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}
```

- **两层锁**：外层 map 锁只在「取/建 per-id 锁」这一极短临界区持有，绝不跨网络；
  真正的单飞临界区是取出来的那把 `Arc<Mutex<()>>`。
- **拿到锁后要双检**：赢家若已换出新 token，后来者直接返回，不再重复刷新。
- **顺手 GC**：`map.retain(|k, v| k == credential_id || Arc::strong_count(v) > 1)`
  —— 取锁时清掉「无人引用」的陈旧条目，避免已删账号的锁无限堆积。注释还论证了
  为什么这样删不会误伤并发任务。

解决的问题：refresh_token 是**单次轮换**的，三方并发刷新同一凭据会各自拿着同一个旧 token
发请求，先到者换走新的、后到者的旧 token 已作废 → **级联 401**。

**移植取舍**：agnes2api 的 key 池若也存在「一把 key 的状态被多个消费方并发更新」
（例如补池、健康检查、限流恢复同时写同一条 key），这套两层锁 + 双检 + GC 的模板可以直接搬。

### 4.4 后台任务：怎么起、怎么停

kiro2api 有**两类**后台任务，都用 `tokio::spawn` + `tokio::time::interval`，
且**都不保存 JoinHandle、都不主动 abort**：

**(a) 业务型（`kiro::keepalive::spawn`）**

```rust
pub fn spawn(pool: Arc<Mutex<Pool>>, client: reqwest::Client, ctx: RefreshCtx) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_SECS));
        // 漏过的拍不补：落后时一次性追平即可，补拍只会在恢复瞬间打出一串刷新
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            /* ... */
        }
    });
}
```

三条纪律写在模块头注释里：
1. **不自己实现刷新**——一律走 `ensure_fresh`，与数据面共用同一把单飞锁、同一套落盘与失败处置。「另写一份必然漂移」。
2. **失败不额外处置**——`ensure_fresh` 内部已反馈池，这里只记一条 `debug`，重复反馈会让同一次失败被记两次。
3. **每轮有上限**（`MAX_PER_TICK = 5`）——避免刚导入一大批账号时突发扇出。

另外这个模块本身就是一条设计教训：**不做「全池定时刷新」**，因为那会给每个账号造出一条
永不停歇的后台心跳；「无人使用却持续产生规律流量」正是风控最容易盯上的形状。
改为**与真实使用量挂钩**：只有近 24 小时内被选中过的账号才享受提前续期。

**(b) 持久化型（`stats::persist::spawn_flush_loop_opt`）**

```rust
tokio::spawn(async move {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => flush_if_dirty(..).await,
            msg = rx.recv() => {
                if msg.is_none() {          // 通道关闭 = 所有 DirtyFlag 被 drop
                    flush_if_dirty(..).await; // 最后一次刷盘
                    break;
                }
                // 收到脏通知：不立刻刷，等下一拍统一批量落盘（去抖）
            }
        }
    }
});
```

**「怎么停」的真实答案**：后台任务本身不被停——**进程退出时 tokio 运行时随之销毁，
那一拍根本轮不到**。所以停机不是靠停任务，而是靠 §9.3 的 `flush_before_exit()`
**显式同步刷一遍**。这一点在 `server/mod.rs` 里被反复强调。

---

## 5. 错误处理

### 5.1 `anyhow` 的边界：只在 6 个文件出现

| 出现处 | 用法 |
|---|---|
| `main.rs` | `async fn main() -> anyhow::Result<()>` |
| `server::serve` | `-> anyhow::Result<()>` |
| `config::load` / `RuntimeConfig::persist` | 文件 IO + serde |
| `kiro::credential::{load, save}` | 文件 IO（用 `anyhow::Context`） |
| `kiro::ensure_fresh` | `anyhow::anyhow!("凭据落盘任务执行失败: {e}")` |
| `admin::handler::persist_failed(e: &anyhow::Error)` | 把落盘失败折成 500 |

**判据**：`anyhow` 只用于**启动路径与文件持久化**——这些地方错误是终局的（起不来 / 存不下），
调用方不需要按类型分支，只需要一段人能读的说明。

### 5.2 业务错误一律自定义枚举，手写 `Display`（不用 thiserror）

四个代表：

```rust
pub enum RelayError {        // protocol/anthropic/handler.rs —— 中转失败
    Convert(ConvertError),   // → 400 invalid_request_error
    NoAccount,               // → 503 overloaded_error
    Upstream(String),        // → 502 api_error
    UpstreamTransient(String),   // → 502，但重试策略不同（值得退避）
    InvalidRequest(String),  // → 400，确定性，换账号无用
    ModelUnavailable(String),// → 400，但**会触发换账号重试**
    QuotaExhausted,          // → 429（「稍后再来」，不是请求本身有问题）
}
pub enum ConvertError { UnknownModel(String), EmptyMessages, RemoteImageUrl(String) }
pub enum LoginError { Http, Upstream(String), UpstreamHttp{status,body},
                      Transient{status,detail}, Pending, SlowDown, Expired, Denied, BadCallback }
pub enum FailureKind { Quota, AuthInvalid, AuthAmbiguous, Transient,
                       InvalidRequest, ModelUnavailable }   // 池的失败分类
```

规矩：
- **`impl fmt::Display` + `impl std::error::Error` 手写**（见 `ConvertError`）。没有 thiserror。
- **枚举的分档依据是「处置方式不同」，不是「错误来源不同」**。`Upstream` 与
  `UpstreamTransient` 对外**完全一样**（都是 502），分开只为重试策略；
  `InvalidRequest` 与 `ModelUnavailable` 对外都是 400，分开只为「要不要换账号再试」。
  → 这是很好的品味：**枚举变体 = 一个决策分支**。
- 变体改名时注释会记录原名与改名理由（`InvalidRequest` 曾名 `InvalidModel`，
  「在它开始承载『上下文超长』之后，那个名字本身就是个假陈述」）。
- `LoginError::is_retryable()`：把「是否终态」收敛成一个方法，而不是让每个调用方各自
  `matches!` 一遍。

### 5.3 映射成 HTTP：`IntoResponse` + 三段式

```rust
impl RelayError {
    fn status(&self) -> StatusCode { /* 每个变体一行 */ }
    fn err_type(&self) -> &'static str { /* 对外错误类型串，不泄露内部细节 */ }
    pub(crate) fn message(&self) -> String { /* 粗粒度，不含令牌/堆栈 */ }
}

impl IntoResponse for RelayError {
    fn into_response(self) -> Response {
        let body = json!({"type":"error","error":{"type":self.err_type(),"message":self.message()}});
        (self.status(), Json(body)).into_response()
    }
}
```

三条硬约束：
1. **对外消息永不含令牌、内部堆栈、上游原始体**。`Upstream(_)` 对外恒为
   `"upstream request failed"`，真正的细节只进日志。
2. **能对客户端说清楚的就说全**：`ModelUnavailable` 的对外文案有四行，解释了
   「模型清单是全池并集、不同订阅档位可用模型不同、请换模型或加账号」。
   这类「确定性、可安全外露」的错误必须把人能照着做的动作写出来。
3. **每个协议有自己的错误体形状**：Anthropic 是 `{"type":"error","error":{...}}`，
   OpenAI 是 `{"error":{message,type}}`。所以 `relay_core_outcome` 把上游 exception
   当**一等结果**返回（`CoreOutcome::Exception{status, e}`），由各协议层自己成型。

### 5.4 三个反复出现的错误处理模式

**(a) 上游把错误藏在 HTTP 200 里** —— 最重要的一条。上游把限流/鉴权/参数错误放在
200 的事件流里以 `:message-type == "exception"` 帧下发。若直接把帧序列交给还原函数，
客户端只会拿到 `200 + 空内容 + end_turn`，**既察觉不到失败也不会重试**。
故本仓的「验真」= 读完体、查有没有 exception，且这件事必须发生在**换账号重试循环内**、
**流开始下发之前**（`VerifyMode::{Buffered, Stream}` + 预读开局 `preroll`）。

**(b) 传输层中断 ≠ in-band exception**。前者是连接断了（504 读超时 / 502 其余），
后者是上游「说」自己出错了。**两者都必须以 error 事件收尾**——若照常发
`message_delta(end_turn) + message_stop`，客户端会把半截回答当成正常完成。

**(c) 解析失败必须回本协议形状的错误体**。handler 用
`payload: Result<Json<T>, JsonRejection>` 提取，失败时自己回 400 + 协议错误体，
**而不是** axum 默认的 `422 text/plain`——「OpenAI 生态的客户端一律按 `error.message`
解析错误，拿到纯文本会在解析处二次抛错（JSONDecodeError），把『少传 messages』这种
一眼可修的参数问题变成客户端内部错误」。

---

## 6. 四协议层：内部结构与 SSE

### 6.1 目录形状

```
protocol/anthropic/   types.rs(408)  handler.rs(4382)  stream.rs(198)  websearch.rs(320)
protocol/openai/      types.rs(529)  handler.rs(1820)  convert.rs(1142)
protocol/gemini/      types.rs(476)  handler.rs(2309)  convert.rs(1303)
protocol/responses/   types.rs(520)  handler.rs(1867)  convert.rs(824)
```

- **anthropic 是「母协议」**：它没有 `convert.rs`（因为上游语义就是 Anthropic 形状，
  转换在 `kiro/convert.rs`），但它多了 `stream.rs`（SSE 事件构造器）和整个 relay 内核。
- **另外三个协议 = `types.rs`（自家 DTO） + `convert.rs`（自家 ↔ Anthropic） + `handler.rs`**，
  handler 里把请求转成 `MessagesRequest` → 调 `relay_core*/relay_stream*` → 把结果转回来。

> 移植到 agnes2api-rs：应先确定哪个协议是「母协议」（取决于上游 Agnes 的原生语义），
> 其余三个做适配。**不要**四个协议各写一条到上游的链路。

### 6.2 路由挂载惯例

每个协议导出一个 `*_router(state) -> Router`，**内部 `.with_state(state)`**，
并同时挂**裸路径**与**带协议前缀的等价变体**：

```rust
pub fn messages_router(state: MessagesState) -> Router {
    Router::new()
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/claude/v1/messages", post(messages))           // 前缀变体
        .route("/claude/v1/messages/count_tokens", post(count_tokens))
        .route("/claude/v1/models", get(anthropic_models))
        .with_state(state)
}
// openai:    /v1/chat/completions   /openai/v1/chat/completions   /v1/models   /openai/v1/models
// gemini:    /v1beta/models/{model_action}   /gemini/v1beta/...
// responses: /v1/responses          /openai/v1/responses
```

注意 `/v1/models` 归 OpenAI，Anthropic 的挂在 `/claude/v1/models`——**路径归属要写进注释**，
否则后人会重复注册导致 panic。

### 6.3 中转内核的三个入口

```rust
// 非流式：兼容入口（归属 id=0）
pub async fn relay_core(state, req, now_unix) -> Result<MessagesResponse, RelayError>
// 非流式：带 store-key 归属 + client_ip + 账号绑定白名单
pub async fn relay_core_attributed(state, req, api_key_id, client_ip, bound, now_unix)
// 非流式：完整结果（把上游 in-band exception 当一等公民交出来）
pub async fn relay_core_outcome(..) -> Result<CoreOutcome, RelayError>
//   enum CoreOutcome { Response(MessagesResponse), Exception { status: u16, e: StreamException } }

// 流式：同样三档
pub async fn relay_stream(..) / relay_stream_attributed(..)
    -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, RelayError>
```

**「注入时钟」纪律**：内核一律接收 `now_unix: u64` 参数，**业务逻辑内不直接读系统时钟**；
axum handler 负责算真实 now 再委托。统计层、池、API-KEY 全部同规约。这是为了单测能钉死边界。

### 6.4 SSE 怎么产：`async-stream` 的 `stream!` 宏

```rust
let body = async_stream::stream! {
    let to_event = |e: stream::SseEvent| Event::default().event(e.event).data(e.data);
    let mut usage_guard = StreamUsageGuard { .. };   // 必须在读循环之前建立

    yield Ok(to_event(stream::message_start(&id, &model, estimated_input)));

    let mut dec = StreamDecoder::new();
    if !preroll.is_empty() { dec.push(&preroll); }   // 预读阶段取走的字节喂回去

    loop {
        for frame in dec.drain() { /* 状态机：yield 各种事件 */ }
        match resp.chunk().await { .. }
    }

    /* 收尾：message_delta(stop_reason, output_tokens) + message_stop */
    usage_guard.flush();
};

Ok(Sse::new(body).keep_alive(
    KeepAlive::new().interval(Duration::from_secs(25)).text("keep-alive")
))
```

要点：

1. **`stream!` 里是一台显式状态机**，不变量写在注释里：
   「同一时刻至多一个内容块打开（先关后开）；每块分配唯一递增 index；工具块按 toolUseId 键控；
   文本块懒开（首个文本增量到达才发 `content_block_start`），故纯工具轮不产出空文本块；
   已发过 `content_block_stop` 的 index 记进 `closed_blocks`，此后不再产出任何 delta」。
   最后一条是为了「上游偶发的迟到 input/stop 帧会让严格校验的官方 SDK 报解析错误」。

2. **`preroll` 回喂**：因为「验真」需要预读流的开局字节判断有没有 exception，
   那些字节已经从连接上取走了，必须喂回解码器，「它们本就是这条流的一部分，
   绝不能凭空丢掉，也不该另起一份处理逻辑」。

3. **`StreamUsageGuard` —— RAII 记账哨兵**（很值得抄）：
   ```rust
   struct StreamUsageGuard { usage: Arc<UsageTracker>, /* 累计量 */ recorded: bool, .. }
   impl Drop for StreamUsageGuard {
       fn drop(&mut self) { if !self.recorded { tokio::spawn(async move { /* 落库 */ }); } }
   }
   ```
   无论流怎么结束（正常收尾 / 客户端断连 / 上游中途出错），只要 `stream!` future 被 drop，
   Drop 就用当时已累计的量落一条用量。正常收尾路径显式 `flush()` 并置 `recorded`，防重复。
   **`Drop` 不能 `.await`，所以 Drop 里 `tokio::spawn` 一个短任务落库**（持 `Arc`，与流生命周期解耦）。

4. **返回类型的 `+ use<>`**：edition 2024 的精确捕获语法，让 `impl Stream` 不捕获输入生命周期。

5. **SSE 保活 25 秒**：「上游『想』得久时（长推理、长工具链）会有几十秒一个字节都不出，
   中间的 CDN / 反代 / 客户端读超时会把这条静默的连接掐掉，表现成『会话莫名其妙断了』。
   25 秒既低于常见的 30/60 秒空闲阈值，又不干扰任何客户端解析（`:` 开头的注释行
   是 SSE 规范里明确要求忽略的）」。

6. **Gemini 是例外，要手写保活**：它同时支持 SSE 与 JSON 数组两种线格式，故用裸
   `Body::from_stream` 而非 `Sse`，拿不到 `keep_alive` 包装。于是读循环写成：
   ```rust
   let next = match tokio::time::timeout(STREAM_KEEPALIVE_INTERVAL, resp.chunk()).await {
       Ok(next) => next,
       Err(_) => { yield Ok(keepalive_frame(wire)); continue; }
   };
   ```
   两种线格式各有各的「合法空转」：SSE 是 `: keep-alive\n\n`，JSON 数组是 `\n`
   （元素之间的空白在 JSON 里没有意义）。

7. **流式透传的重试边界**：跨账号重试必须在**流开始之前**完成。
   「等 SSE 都开始下发了才发现就换不了账号了 —— 用户直接吃到失败，而账号池只会记下一次
   『成功』。预读把『一开口就被拒』挡在流开始之前，于是它能照常换号重试」。
   开局就被拒且账号试尽时，把 exception **种进** `stream_error`，复用同一条收尾路径
   （以 `event: error` 下发），「不另起一份下发逻辑」。

---

## 7. 面板：`rust-embed` + `admin-ui-v2` 的形态

### 7.1 两套 UI，一套零构建、一套要构建

```rust
#[derive(RustEmbed)] #[folder = "admin-ui-v2"]   struct AdminAssets;  // 手写静态，零构建
#[derive(RustEmbed)] #[folder = "user-ui/dist"]  struct UserAssets;   // vite 产物，需先构建
```

- **`admin-ui-v2/` 是零构建的手写静态站**：`index.html` + `css/*.css`(5 个,3465 行)
  + `js/*.js`(14 个,8857 行) + `logo.png`。没有打包器、没有 `node_modules`、没有 import map，
  就是一串 `<script src=...>` 按顺序加载，共享一个全局 `window.K` 命名空间：
  ```html
  <script src="js/i18n-dict.js"></script>  <!-- window.I18N 字典 -->
  <script src="js/i18n.js"></script> <script src="js/theme.js"></script>
  <script src="js/api.js"></script>        <!-- K.api，fetch 封装 -->
  <script src="js/sec-dashboard.js"></script> ... <script src="js/sec-settings.js"></script>
  <script src="js/app.js"></script>        <!-- 最后：装配外壳、跑登录闸 -->
  ```
  每个 `sec-*.js` 是一个页面区块（dashboard/accounts/models/apikeys/modeltest/usage/logs/settings），
  IIFE 里 `'use strict'`，ES5 写法（`var` + `function`，只在 fetch 处用 `async`）。
  `<base href="/admin/">` 让所有相对路径可用。
  **Dockerfile 注释明说：「admin-ui 已改为静态手写 UI，无需构建，直接进 Rust 构建上下文嵌入」。**

- **`user-ui/` 是 React 18 + Vite + Tailwind + TanStack Query + axios**，必须先 `pnpm build`
  产出 `dist/`，`rust-embed` 才能在 `cargo build` 时找到它。
  → **这是一个构建顺序陷阱**：CI 和 Dockerfile 都必须在 `cargo build` 之前跑前端构建，
  否则编译直接失败。CI 里专门有一步做这件事。

**移植取舍（重要）**：用户要求「面板 UI 完全对标 kiro2api」。kiro2api 的管理面板正是
`admin-ui-v2` 这套**零构建手写静态**。所以 agnes2api-rs 的面板建议**直接照这个形态做**
（零构建 = `cargo build` 不依赖 node），避免把 user-ui 那套 vite 依赖一并背过来。

### 7.2 资源怎么服务（四个坑各对应一段代码）

```rust
fn serve<A: RustEmbed>(path: &str, inm: Option<&str>) -> Response {
    if path.contains("..") { return StatusCode::BAD_REQUEST.into_response(); }   // 坑 0：路径穿越
    let candidate = if path.is_empty() { "index.html" } else { path };
    match A::get(candidate) {
        Some(f) => serve_asset(f, mime_guess::from_path(candidate).first_or_octet_stream().as_ref(), inm),
        None if is_asset_path(candidate) => StatusCode::NOT_FOUND.into_response(),  // 坑 1
        None => /* SPA 回退到 index.html */,
    }
}
```

**坑 1 —— SPA 回退不能无条件**。以前不存在的 `assets/index-abc123.js` 也拿到 `200` + 一整篇 HTML。
两个坏处：浏览器把 HTML 当 JS 解析报 `Unexpected token '<'`；**更坏的是用 curl 核对某个构建
产物是否上线时，不存在的文件也返回 200 —— 校验本身在骗人**。
判据：`is_asset_path()` 取路径最后一段，含 `.` 即当资源（404），否则当前端路由（回 index）。

**坑 2 —— 必须发缓存协商头**。原来一个缓存头都不发，HTTP 规定这种情况下浏览器可以
**启发式缓存**（自己决定存多久），于是发版后用户看到的仍是旧面板：
**实际发生过「后端已是 0.7.9，面板却显示 0.7.6」，而这类现象最难查，因为服务端每个接口
查出来都是对的**。做法：`Cache-Control: no-cache` + **强 ETag（取 rust-embed 的
`metadata.sha256_hash()`）**。`no-cache` 不是「不缓存」，是「可以存但每次必须回源验证」；
内容没变回 304 且不带 body。

**坑 3 —— 不要用 `.nest()`**：
> axum 0.8 的 nest 对挂载路径末尾斜杠敏感（`nest("/admin", ..)` 不匹配 `/admin/`，反之亦然），
> 导致 `GET /admin`（无斜杠）404。

解法：在 `prefix` 上直接注册**三条绝对路径**，顶层用 `.merge()` 拼：

```rust
fn ui_router<A: RustEmbed>(prefix: &str) -> Router {
    Router::new()
        .route(prefix,                     get(|h| async { serve::<A>("", inm(&h)) }))  // /admin
        .route(&format!("{prefix}/"),      get(|h| async { serve::<A>("", inm(&h)) }))  // /admin/
        .route(&format!("{prefix}/{{*file}}"), get(|Path(f), h| async { serve::<A>(&f, inm(&h)) }))
}
```

**坑 4（本文推断，未在 kiro2api 注释中出现，但移植必踩）**：`rust-embed` 8 默认在
**debug 构建下从磁盘读文件**（只有 release 或开 `debug-embed` feature 才真正嵌入）。
kiro2api 没开 `debug-embed`，所以 `cargo test` 时 webui 的测试能过是因为文件就在工作目录里。
agnes2api-rs 若要在别处运行 debug 二进制，要么开 `debug-embed`，要么接受这个行为。

### 7.3 面板本身的测试

`admin-ui-v2/tests/*.mjs` 三个文件，用 **Node 内置 `node --test`**（无 jest/vitest）。
CI 里单独一步跑，且注释记了个坑：
> 逐文件传入而非传目录：`node --test <目录>` 在本仓会把整个目录当一个失败用例报出来。

---

## 8. 鉴权与中间件：手写一切，且 `.layer()` 顺序是硬约束

### 8.1 路由分三组，各自 `.layer()`

```rust
// (1) 协议组：四协议 merge 成一组，再统一挂闸
let protocol = messages_router(st.clone())
    .merge(openai_router(st.clone()))
    .merge(gemini_router(st.clone()))
    .merge(responses_router(st.clone()))
    .layer(from_fn_with_state(AuthState::protocol(runtime_cfg, api_keys, stats), require_api_key))
    .layer(DefaultBodyLimit::max(32 * 1024 * 1024));
let protocol = with_cors(protocol, cors.as_ref());        // CORS 必须在最外层

// (2) 用户面：**不叠加任何鉴权 layer**，由 handler 自己从请求取 key 校验
let user = user_api_router(st.clone());

// (3) 管理面：用 admin_api_key（缺省回退主 key）
let admin = admin_api_router(st)
    .layer(from_fn_with_state(AuthState::admin(runtime_cfg, api_keys), require_api_key))
    .layer(DefaultBodyLimit::max(32 * 1024 * 1024));

let admin_cors = cors.as_ref().filter(|_| cfg.cors_allow_admin);   // 默认不给 admin 开 CORS
let (admin, user) = (with_cors(admin, admin_cors), with_cors(user, admin_cors));

let router = Router::new().route("/health", get(health)).route("/v1/ping", get(ping))
    .with_state(cfg);
let router = with_cors(router, cors.as_ref())
    .merge(webui::admin_router()).merge(webui::user_router())
    .merge(protocol).merge(admin).merge(user);
```

**三条必须理解的 axum 语义**（注释里都写了）：

1. **`.layer()` 只作用于其之前已注册到该 `Router` 的路由**。所以必须在 merge 进顶层之前
   单独 layer，否则 `/health`、`/v1/ping`、webui 会被一并挡住。
2. **后加的 `.layer()` 在更外层**。所以 CORS 写在最后 = 最外层。
3. **`.merge()` 而不是 `.nest()`**（理由见 §7.2 坑 3）。

### 8.2 CORS 是手写中间件，不是 tower-http

```rust
struct CorsPolicy { allow_any: bool, origins: Vec<String> }
async fn cors_middleware(State(policy): State<Arc<CorsPolicy>>, req: Request, next: Next) -> Response
```

设计要点（每条都是坑）：

- **CORS 必须挂在鉴权闸与体积上限之外**。
  > 预检请求按浏览器规范**不携带任何凭据**，继续往下走会先被鉴权闸判 401；
  > 而协议路由只注册了 GET/POST，即便开放模式下预检也只会吃一个 405。两种结果都会让
  > 浏览器把随后的真实请求整个掐掉，现象是「网页端一个端点都调不通，同样的 key 用 curl 全通」。

- **`Access-Control-Allow-Headers` 一律回声客户端声明的那份**，不写死清单。
  > 各家 SDK 各带一堆自有头（`anthropic-version`、`anthropic-beta`、`x-goog-api-key`、
  > `x-stainless-*`…），写死清单必定漏，而漏一个的后果就是整条请求被浏览器拦下。
  只有客户端没声明 `Access-Control-Request-Headers` 时才用兜底清单。

- **不写 `Access-Control-Allow-Methods: *`** ——「那个值在带凭据的请求里无效，
  而客户端是否带凭据我们左右不了」。写实际用到的全集。

- **`Vary: Origin` 必须追加**：「响应内容随来源而变，缓存（CDN / 浏览器）必须按 Origin 分桶，
  否则允许来源拿到的那份带 `Allow-Origin` 的响应会被喂给下一个不允许的来源」。

- **来源比对前两边都归一**（小写、去尾斜杠），但**回写时用调用方原样的 Origin**
  （该头必须与浏览器发出的值逐字相同）。「浏览器发出的 Origin 永远不带尾斜杠，
  而配置里顺手写成 `https://a.com/` 是最常见的写法，不归一就永远匹配不上、且毫无提示」。

- **预检的判据是 `OPTIONS` + 有 `Access-Control-Request-Method`**，
  普通 OPTIONS 照旧交给路由，不被这层吞掉。

- **空来源清单 = 整个 CORS 层不挂**（零开销，响应里一个 `Access-Control-*` 都没有）。

- **admin/user 的 CORS 默认关**，理由写得很长：未配置 key 时管理面是完全开放的，
  此时若再放开跨源读取，运营者浏览器打开的任意一个恶意网页都能把
  `/api/admin/credentials` 读走并回传，服务只要对那台机器可达（localhost / 内网）就成立。

### 8.3 鉴权闸：两种角色，判据不同

```rust
pub enum AuthRole { Protocol, Admin }
```

| | 期望的全局 key | 接受 store key？ | 「是否已配置鉴权」的判据 |
|---|---|---|---|
| `Protocol` | 主 `api_key`（从 `runtime_cfg` **实时读**，轮换即时生效） | ✅ 接受，且把解析出的 `ApiKeyId`/`BoundCredentialIds` 塞进请求扩展 | 主 key **或** store 里任意一条 key |
| `Admin` | `admin_api_key` 非空否则回退主 `api_key` | ❌ 永不放行 | **只看管理员级凭据**，store key 不参与 |

Admin 判据那条的理由很关键：
> 若让 store key 把闸判成已配置，**首次部署建出第一把 key 就会自锁**（能开锁的管理员凭据一把都没有）。

其它约定：
- **key 比较用 `subtle::ConstantTimeEq`**（防时序侧信道）。
- **key 可以走三处**：`Authorization: Bearer`、`x-api-key` 头、`?api_key=` query。
  query 那条是为 SSE —— **EventSource 无法设请求头**。
- **鉴权闸把解析结果经 `Extension` 下传**（`ApiKeyId`、`BoundCredentialIds`），
  handler 用 `Option<axum::Extension<T>>` 读取。理由：绑定值必须与本次鉴权命中的那条 key
  **同一份快照**，否则请求在途时管理员改绑定会出现「按旧 key 放行、按新绑定选号」的错配。
- **`BoundCredentialIds::allows()` fail-closed**：解析不出数值 id 的一律不放行，
  「宁可选不出账号（503）也不能把请求漏给未授权账号」。
- **空绑定列表 = 不受限**（而非「一个都不准用」），因为面板把空列表显示为「未绑定」，
  「绑定这类只在管理面配置的策略，最忌看到的和执行的两回事」。
- **消费上限用「预留-对账」而非「检查-再做」**：准入时按 `EST_COST_PER_REQUEST_USD = 0.05`
  / `EST_CREDITS_PER_REQUEST = 0.25` 做在途预留（RAII，完成即释放），
  把并发的 check-then-act 收敛为原子 reserve-then-reconcile。这两个常量上各有一整段
  取值推导（旧值 1.0 credits 导致「面板显示还剩九成、请求一个都发不出去」，实际踩过）。

### 8.4 启动期鉴权体检

`auth_startup_warnings(cfg) -> Vec<String>` 抽成**纯函数**（便于单测钉住覆盖面），
`serve()` 只负责逐条 `warn!`。三种组合三种文案，且措辞精确到「协议那条带
『在未创建任何 API-KEY 前』限定，管理面那条**不得**带该限定，否则低估暴露面」。

### 8.5 客户端 IP 提取（若 agnes2api 也要记 IP，照抄）

`extract_client_ip(headers, peer, trusted_proxy_hops)`：
- **转发头只在对端是私网/回环时才采信**（反代/CDN 回源必然来自私网）。
- **XFF 取从右往左数第 `trusted_proxy_hops` 项，不是最左项**。
  「最左项是最原始客户端没错，但它同样是**调用方自己就能写死的那一项**；
  每一跳可信反代追加的是它**亲眼看到的**地址，所以倒数第 n 项伪造不了」。
- `hops = 0` = 裸跑无反代，一律不采信任何转发头。
- **`sanitize_ip()`**：解析不出 IP 字面量的值一律丢弃（不是 trim 一下就落库）。
  「审计字段的第一要求是可信且可比对，解析不出 IP 的值没有任何审计价值，宁可留空」。
  顺带剥掉 `[::1]:443` 这类带端口/方括号的形态。
- 诚实标注了局限：经 Docker `-p` 端口映射时，直连公网端口的请求在容器内看到的对端
  同样是 docker 网关（私网），与经反代回源无法区分。

---

## 9. 持久化：原子写 + 去抖 + 停机刷盘 + 自保阀

### 9.1 通用基建（`stats/persist.rs`，472 行）

```rust
pub fn atomic_write_json<T: Serialize>(path, value) -> io::Result<()>   // tmp → fsync → rename
pub fn read_json<T: DeserializeOwned>(path) -> io::Result<Option<T>>    // NotFound → Ok(None)
pub fn dirty_channel() -> (DirtyFlag, FlushHandle)
pub fn spawn_flush_loop(path, handle, interval_secs, snapshot: Fn() -> Vec<u8>)
pub fn spawn_flush_loop_opt(path, handle, interval_secs, snapshot: Fn() -> Option<Vec<u8>>)
pub const FLUSH_INTERVAL_SECS: u64 = 5;
```

模型：**热路径只改内存 + 置脏 + 经 MPSC(cap=1) 捅一下后台任务，绝不做 IO**；
后台每 5 秒一拍，脏则 `spawn_blocking` 落盘。

细节：
- **临时文件名带 pid + 进程内单调计数器**（`{path}.tmp.{pid}.{n}`），
  避免固定 `{path}.tmp` 的交错写/rename 竞态损坏落盘文件。
- **孤儿 tmp 清理**：只删 mtime 早于 5 分钟的 `{basename}.tmp.` 前缀文件，
  「远大于一次正常刷盘（亚秒级），故绝不会误删并发写者正在写的 tmp」。
- **抢占式清脏**：`dirty.swap(false)` 先置 false，期间的新写会再次置脏、下拍再落；
  落盘失败则把脏标记放回去，下拍重试。
- **`spawn_flush_loop_opt` 的 `None` = 本轮跳过写盘**，给两类场景用：
  快照源已析构（后台任务只持 `Weak`）、序列化失败。
  **两种情况下都绝不能把空内容原子覆写上去。**
- **`snapshot` 闭包返回已序列化的 `Vec<u8>`**，由调用方在持锁下取快照并序列化，
  「避免把锁类型泄漏到本模块」。
- 落盘是同步写 + fsync（可达数十毫秒），一律在 `spawn_blocking` 里做。

### 9.2 韧性加载：坏文件不许静默当空

`server::load_credentials_resilient()` 是这条纪律的范本，三件事：

1. **原样备份**成 `{path}.corrupt.{unix秒}.{序号}`，此后任何原子写都覆不掉它；
2. **逐条抢救**：文件本体仍是 JSON 数组时按条反序列化，坏条目跳过、好账号照常入池；
3. **大声报错**：给出坏条目位置、备份路径与后果说明（且最多逐条打印 20 条，
   「生产文件有约千条账号，整体格式走样时不能让启动日志被刷爆——日志同时进实时日志面板
   的环形缓冲，刷爆等于把别的线索一起冲掉」）。

为什么必须这么做（原文事故链）：
> 旧写法 `credential::load(..).unwrap_or_default()` 把 `Err` 丢掉：进程带着 0 账号池启动、
> 日志里一个字都没有，运营者只看到 relay 全 503、面板 `total: 0`，于是「那就重新加一个账号」
> —— admin 侧的原子落盘会用内存里的**空池整份覆写** credentials.json，
> 约千个账号的 refreshToken 当场蒸发且不可逆。

### 9.3 停机刷盘：`PersistHandles`

```rust
pub struct PersistHandles {
    pub stats: Arc<StatsManager>,
    pub balance: Arc<BalanceCache>,
    pub api_keys: Arc<ApiKeyStore>,
    pub api_keys_path: PathBuf,
    pub pool: Arc<tokio::sync::Mutex<Pool>>,
    pub persist_lock: Arc<tokio::sync::Mutex<()>>,
    pub credentials_path: String,
}
impl PersistHandles { pub async fn flush_before_exit(&self) { .. } }
```

- `build_router_with_persist_handles()` 是唯一的真实构造函数，
  `build_router()` / `build_router_with_logs()` / `build_router_with_handles()` 都是它的窄化包装
  —— **为了不打断既有调用方与测试**，这是本仓处理 API 演进的一贯手法。
- 每个字段的文档注释都写明「丢一拍会怎样」：
  - stats：少算这段时间的钱 + 刚出的报错现场在弹窗里查无此事；
  - balance：删账号时清掉的条目随旧文件复活，复用同 id 的新账号在 TTL 内显示已删账号的余额；
  - api_keys：**刚吊销的 key 会随旧文件复活**（重启后照样鉴权通过）；
  - pool：配额恢复时刻 / 封号判定 / 冻结的 machineId 全丢，下次启动拿用户的额度重新学一遍。
- **落盘的自保阀**（`api_keys_overwrite_is_safe`）：仅当「内存里一把 key 都没有」**且**
  「磁盘上文件存在却解析不动」时**跳过写盘**——这组合说明启动时那份文件根本没读进内存，
  此刻原子覆写等于把仅存的、也许还能人工挽救的密钥文件抹掉。
- 各项之间无依赖，任一失败只记 `error`、绝不阻断停机。

### 9.4 优雅停机：有上界的排空窗口

```rust
async fn shutdown_signal() { /* select!{ ctrl_c, SIGTERM } */ }
const SHUTDOWN_DRAIN_WINDOW: Duration = Duration::from_secs(8);
async fn drain_with_deadline<F: IntoFuture, S: Future<Output=()>>(server, shutdown_started, window)
    -> Option<F::Output>
```

- **必须处理 SIGTERM**：容器里本进程常是 PID 1，不处理就会一直等到宽限期结束被 SIGKILL。
- **`with_graceful_shutdown` 必须封顶**：
  > admin 实时日志 SSE 是**无限流**——它只在广播发送端释放时才结束，而发送端由路由 state 持有，
  > 服务器活着就不会释放。于是只要有一个管理页开着日志流，停机就会永远卡住，最后被 SIGKILL
  > 硬掐——在途中转流照样被切断，统计最终刷盘反而彻底跳过，和不做优雅停机相比只坏不好。
- **8 秒的取值理由**：必须短于容器宽限期（`docker stop` 默认 10s），给窗口到期后的刷盘留余量；
  同时长于绝大多数在途一问一答请求。
- `drain_with_deadline` 抽成泛型函数（取 `IntoFuture` 而非 `Future`，因为 axum 的
  `WithGracefulShutdown` 只实现前者），**是为了能用假 future 单测边界行为，不必真开监听 socket**。
- `tokio::select!` 里用 `biased;` 优先轮询服务器，使「信号与结束同时就绪」时确定性地报干净结束。
- **服务器自身的错误放到刷盘之后再上抛**，避免用 `?` 提前返回把统计吞掉。
- 管理面的「重启」端点是 `tokio::spawn` 里 sleep 500ms → 显式刷盘 → `std::process::exit(0)`
  （**`exit` 连析构都不跑**，所以必须显式刷）。

---

## 10. 测试

### 10.1 inline `#[test]` 压倒性为主

- **src 内 971 处** `#[test]` / `#[tokio::test]`，`tests/` 内只有 7 处。
- 每个模块底部一个 `#[cfg(test)] mod tests { use super::*; .. }`。
- **测试的文档注释就是事故复盘**，格式基本固定：
  ```rust
  /// 不存在的**资源**必须 404，不能回 200 + index.html。
  ///
  /// 回归：SPA 回退此前是无条件的，于是 `js/does-not-exist.js` 也拿到 `200` + 一整篇 HTML。
  /// 浏览器把 HTML 当 JS 解析，报的是 `Unexpected token '<'`；更坏的是，**用 curl 核对
  /// 某个构建产物是否上线时，不存在的文件也返回 200 —— 校验本身在骗人。**
  #[tokio::test]
  async fn missing_assets_404_while_routes_still_fall_back_to_index() { .. }
  ```
- 断言消息也写理由：`assert_eq!(POOL_MAX_IDLE_PER_HOST, 0, "复用连接会把整池账号串在同一条连接上暴露给上游");`

### 10.2 怎么起一个测试用的 app

```rust
use tower::ServiceExt;                                  // 取 .oneshot()
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

fn state(server_uri: &str, creds: Vec<Credential>) -> MessagesState {
    MessagesState {
        pool: Arc::new(Mutex::new(Pool::new(creds, LbMode::Priority))),
        client: reqwest::Client::new(),
        control_client: reqwest::Client::new(),
        cfg: Arc::new(Config::default()),
        runtime_cfg: config::shared_runtime_config(&Config::default()),
        endpoint_override: Some(format!("{server_uri}/generateAssistantResponse")),  // ← 关键
        stats: StatsManager::load_from_dir(&std::env::temp_dir()),
        api_keys: ApiKeyStore::load(/* 每进程唯一路径 */),
        /* ... 其余字段用 ::new() / None */
    }
}

#[tokio::test]
async fn xxx() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/generateAssistantResponse"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(frames, "application/vnd.amazon.eventstream"))
        .mount(&server).await;

    let app = openai_router(state(&server.uri(), vec![cred()]));
    let resp = app.oneshot(Request::builder().method("POST").uri("/v1/chat/completions")
        .header("content-type","application/json").body(Body::from(body)).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
}
```

要点：
- **`tower::ServiceExt::oneshot` 打 Router，不开端口**。
- **`wiremock::MockServer` 假扮上游**，靠 `endpoint_override` 把数据面指过去。
  wiremock **只用在 src 内的单元测试里**，`tests/` 的四个文件都不用它。
- 各协议的 tests 模块里都有一份**几乎相同的 `event_frame()` 帮手**（手工拼 AWS event-stream
  二进制帧，含 prelude crc + message crc）。这是有意的重复（每个协议自带上下文），
  注释写「照 anthropic handler 测试同法」。
- 测试用同步 `#[test]` 时不需要运行时；碰 `.await` 就 `#[tokio::test]`。

### 10.3 临时目录：`src/test_tmp.rs`（这个必须抄）

问题：各模块测试各自 `std::env::temp_dir().join(format!("xxx_{pid}_{nanos}"))`，建完从不删。
**跑一轮完整测试会在 /tmp 顶层留下数千个目录，且永不回收 —— 实际排查磁盘问题时，
/tmp 里堆了 9582 个这样的残留（仅 4 天积累）**。

解法：统一出口 `test_tmp::{dir, file, stable_dir, stable_file}`，
所有临时产物收进 `/tmp/<crate>-tests/<pid>/`，每个测试进程启动时（`Once`）
回收掉**属于已退出进程**的旧根目录（Linux 上看 `/proc/<pid>` 在不在）。

注意：`test_tmp` 在 `lib.rs` 里以 `#[cfg(test)] pub mod test_tmp;` 声明，**不进产物**。
（现状：仓里仍有一批老测试没迁过来，还在直接用 `std::env::temp_dir()`。）

### 10.4 `tests/` 里放什么（只放 inline 装不下的四类）

| 文件 | 性质 |
|---|---|
| `dockerfile_libc.rs` | **对 Dockerfile 本身做断言**：`include_str!("../Dockerfile")`，检查 builder 与运行镜像钉的是同一个 Debian 世代 |
| `proxy_routing.rs` | 起一个**真的 TCP 代理**（只认 absolute-form 请求行），验证代理是否真生效 |
| `live_relay.rs` / `live_stream.rs` / `live_tools.rs` | `#[ignore]` 的真机端到端，须手动 `--ignored` |

`live_*` 的安全约束写在模块头，值得逐条照抄：
> - 只读凭据文件，绝不写回、绝不调用任何持久化保存、绝不写 `.tmp` 兄弟文件。
> - 绝不 `println!`/日志任何 token/refreshToken/accessToken 值。
> - 只发一次真实调用（tiny prompt "ping"），失败即报错、不对真实后端重试。
> - 凭据路径来自环境变量，**必须显式设置，无默认路径**；未设置时直接跳过（打印提示后
>   `return`，不 panic、不失败）。

`dockerfile_libc.rs` 的思路尤其值得学：**把「不写代码里的约束」写成对配置文件的断言测试**，
「换任一边都必须同步换另一边，否则测试先红，而不是生产先红」。
`http.rs` 里也有同类：读 `include_str!("../Cargo.toml")` 断言默认 feature 含 `native-tls-backend`，
读 `include_str!("http.rs")` 数 `.http1_only()` 出现 3 次（且只数真正的调用行，不数注释）。

---

## 11. 构建与发版

### 11.1 `build.rs`（21 行，全部是 best-effort）

```rust
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let v = Command::new(rustc).arg("--version").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=KIRO_RUST_VERSION={v}");
}
```

给 admin `server-info` 端点展示「Rust 版本」用。纪律：
**任一环节失败一律回落 `"unknown"`，绝不让构建失败**，因此下游 `env!("KIRO_RUST_VERSION")` 始终有值。

### 11.2 Dockerfile：三阶段 + 交叉编译（坑最密集的一个文件）

```
stage 1  FROM --platform=$BUILDPLATFORM node:22-alpine AS frontend-builder   # 只构建 user-ui
stage 2  FROM --platform=$BUILDPLATFORM rust:1-slim-bookworm AS builder      # 交叉编译
stage 3  FROM debian:bookworm-slim                                            # 运行镜像
```

逐条坑：

1. **`--platform=$BUILDPLATFORM` 让 builder 永远原生跑（amd64），再用交叉工具链编 arm64**。
   否则多架构构建时 builder 会被 QEMU 模拟，「慢 5-10 倍、单次上小时」。

2. **必须钉 `-bookworm`，且 builder 与运行镜像必须同代**（有测试钉死，见 §10.4）。
   > 二进制链接时按 builder 里的 libc 头定符号版本，运行时却在运行镜像的 glibc 上跑，
   > builder 比运行镜像新就会缺符号。裸 `rust:1-slim` 已随上游滚到 trixie(glibc 2.41)，
   > 而运行镜像是 bookworm(2.36)；vendored OpenSSL 又会用到 glibc 2.38 才加入的
   > strlcpy/strlcat，于是产出的二进制启动即 `version GLIBC_2.38 not found` —— 两个架构都中招。
   （这条把生产打挂过一次：**编译、镜像构建、推送全部成功，只在容器启动那一刻炸，CI 一路绿灯**。）

3. **`AR` 也必须指向交叉工具链**：
   ```
   ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
       CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
       AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
   ```
   > OpenSSL 交叉编译时要打静态库，用宿主 ar 会产出 x86 归档、链接阶段才报错
   > （错误信息与真实原因相距很远）。

4. **`perl` + `make` 必装**：vendored OpenSSL 自带 Perl 写的 `./Configure`，
   `rust:1-slim` 里两样都没有，缺了会在 `./Configure line 15` 处直接失败。

5. **凭据路径不在镜像里烘焙成 ENV，也不写进 CMD**（见 §3 的理由）。
   容器以 `CMD ["kiro2api", "-c", "/app/data/config.json"]` 启动，靠应用的「就近解析」
   把默认数据目录落进挂载卷。

6. **`HEALTHCHECK` 按与应用完全相同的优先级解析端口**：
   `PORT` 环境变量 > 挂载的 config.json 里的 `port` > 8080。
   > 少了 PORT 这一层，用户按文档改端口后健康检查仍打旧端口，容器会永远 unhealthy。

7. **非 root 运行 + `gosu` entrypoint**：
   ```sh
   if [ "$(id -u)" = "0" ]; then chown -R appuser:appuser /app/data; exec gosu appuser "$@"; fi
   exec "$@"
   ```
   以 root 启动时先修正挂载卷属主（无缝升级 legacy root-created `./data`），再降权。
   （记忆里 gemini2api 踩过同款坑：非 root 镜像 + bind mount 必须配 gosu-entrypoint chown。）

8. **`.dockerignore` 把 `user-ui/dist` 排除掉**——它由 stage 1 现构建，宿主机的旧产物不许混进来。

9. `cargo build --release --locked --target "$RUST_TARGET"`：**`--locked`**，
   构建绝不允许改 `Cargo.lock`。

### 11.3 `docker-compose.yml`

- **宿主与容器端口都跟着 `.env` 的 `PORT` 走**：`"${PORT:-8080}:${PORT:-8080}"`。
  「否则改了 PORT 只会让容器换端口、映射还停在旧端口，宿主机上新旧端口都连不通」。
- healthcheck 与 Dockerfile 同一套端口优先级（compose 里 `$$` 转义，展开留给容器内 shell）。
- 卷：`./data:/app/data` + `/etc/localtime:/etc/localtime:ro`，`TZ: Asia/Shanghai`。
- `.env.example` 里把「留成空串会覆盖 config.json 里已配的密钥，反而把服务变成开放模式」
  这种坑直接写在注释里。

### 11.4 `scripts/set-version.sh`

一处改、多处同步：`VERSION`、`Cargo.toml` 的 `[package] version`、
根 README 与全部多语言文档（`docs/{en,ja,ko,zh-CN,zh-TW}/README.md`）顶部的版本徽章。

- `set -euo pipefail`，容忍带前导 `v` 的输入，正则校验版本号格式。
- **`sed -i -E '0,/^version = "[^"]*"/s//.../' Cargo.toml`** —— 只改第一处**顶格**的
  `version =`（依赖项不顶格，不受影响）。
- 末尾打印同步结果供人工核对。
- 脚本头注释即发版流程：`set-version.sh → 更新 CHANGELOG → 提交 → 打 tag vX.Y.Z → 推送`。

### 11.5 CI（`.github/workflows/`）

**`ci.yml`**（push 到 `main`/`feat/**`/`fix/**`/`chore/**` 与所有 PR）：
```
checkout → dtolnay/rust-toolchain@stable (rustfmt, clippy) → Swatinem/rust-cache@v2
→ setup-node 22 → 构建 user-ui/dist（rust-embed 编译期要它）
→ cargo fmt --all -- --check          (阻塞)
→ cargo clippy --all-targets -- -D warnings  (阻塞)
→ cargo test --all                    (阻塞)
→ node --test admin-ui-v2/tests/*.mjs (阻塞)
```
`concurrency: cancel-in-progress`，`permissions: contents: read`。

**`docker-publish.yml`**（`push: tags: ["v*"]` + `workflow_dispatch`）：
qemu + buildx + ghcr 登录 + `docker/metadata-action`
（tags: `{{version}}` / `{{major}}.{{minor}}` / `latest`）+ `build-push-action`
（`platforms: linux/amd64,linux/arm64`，`cache-from/to: type=gha`）。

⚠️ 面板测试那一步有段注释值得记：
> 面板逻辑此前完全没有门禁：33 个 admin-ui 测试一次都没在 CI 里跑过，改坏了要等人工点开面板才发现。

---

## 12. 「踩过坑才写得出来」的约定——汇总清单

按「不知道就会再踩一次」排序，移植时逐条核对：

| # | 约定 | 不这么做会怎样 |
|---|---|---|
| 1 | 默认 TLS 后端 = native-tls(vendored OpenSSL)，且**运行时可切** | rustls 的 ClientHello/JA3 与真实客户端对不上，在任何 HTTP 内容发出前就暴露；自签 CA 代理下换不了后端只能重出镜像 |
| 2 | reqwest 开了 gzip/brotli/deflate 就必须在数据面/控制面**显式 `no_gzip/no_brotli/no_deflate`** | reqwest 会给没带 accept-encoding 的请求自动补一个，而真实客户端数据面不发这个头 |
| 3 | `pool_max_idle_per_host(0)` —— **绝不复用连接** | 同一条 TCP/TLS 上依次出现几十个不同账号的令牌 + 不同 machineId，是账号共享最直接的证据（同 IP 还能用 NAT 解释，同一条连接解释不了）。线上症状吻合：账号不经中转是活的，经中转就被封 |
| 4 | 三个客户端全部 `.http1_only()` | 真实客户端是 aws-sdk-js on Node（默认 1.1），而 ALPN 会协商到 h2；且 `Connection: close` 是 1.1 的头、h2 禁止它——不锁 1.1 那个头只是摆设 |
| 5 | 数据面**不设整请求 timeout**，只设 `connect_timeout` + `read_timeout` | 整请求超时会误杀合法 SSE 长流；没有任何超时则上游卡死时连接无限期挂起 |
| 6 | 控制面单独一个客户端（有整请求硬顶） | 一问一答的请求卡住时无上限失败 |
| 7 | 每个账号的**控制面必须与数据面走同一个出口**（同代理） | 「数据面从代理出、刷新却从主 IP 出，比不配代理更糟：等于主动告诉上游这两条流量属于同一个中转」 |
| 8 | 代理地址不合法时**大声报错并仍然直连**，不静默直连也不拒绝构造 | 静默直连 = 运营者以为走代理、流量却从主 IP 出（最糟的「看起来配好了」）；拒绝构造 = 整个账号不可用 |
| 9 | 面板资源必须 `Cache-Control: no-cache` + 强 ETag | 浏览器启发式缓存导致「后端 0.7.9、面板显示 0.7.6」，而服务端每个接口查出来都是对的 |
| 10 | SPA 回退只对**不带扩展名**的路径生效 | 不存在的 `.js` 返回 200 + HTML；**用 curl 核对产物是否上线时校验本身在骗人** |
| 11 | 用 `.merge()` 绝对路径而非 `.nest()` | axum 0.8 的 nest 对末尾斜杠敏感，`/admin` 与 `/admin/` 只能匹配一个 |
| 12 | CORS 层必须在鉴权闸与体积上限**之外** | 预检不带凭据 → 401 / 405 → 浏览器掐掉真实请求 → 「网页端全不通、curl 全通」 |
| 13 | `Access-Control-Allow-Headers` 回声客户端声明的那份 | 各家 SDK 的自有头写死清单必定漏，漏一个整条请求被拦 |
| 14 | 协议端点 body limit 必须显式抬到 32MB | axum 默认 2MB，base64 内联图片放大 4/3，约 1.5MB 的图就被拦下，客户端只看到「发图必失败」 |
| 15 | 上游 in-band exception（HTTP 200 里的 error 帧）必须当失败处理，且验真要在换号循环内、流开始前 | 客户端拿到 200 + 空内容 + end_turn，既察觉不到失败也不会重试；账号池把被拒记成成功 |
| 16 | 传输层中断也必须以 error 事件收尾 | 客户端把半截回答当正常完成，既不报错也不重试 |
| 17 | 请求体解析失败要回**本协议形状**的 400，不用 axum 默认 422 text/plain | SDK 在解析错误体时二次抛错，把参数问题变成客户端内部错误 |
| 18 | SSE 保活 25 秒 | 长推理时几十秒无字节，CDN/反代按空闲超时掐连接，表现为「会话莫名其妙断了」 |
| 19 | 凭据文件解析失败**绝不静默当空池**：备份 + 逐条抢救 + 大声报错 | 空池启动 → 运营者重加账号 → 原子落盘用空池整份覆写 → 上千个 refreshToken 不可逆蒸发 |
| 20 | 停机必须显式 `flush_before_exit()`，且排空窗口必须封顶 8s | 进程退出时 tokio 运行时销毁，后台刷盘那一拍轮不到；无限 SSE 日志流会让 graceful shutdown 永远卡住直到被 SIGKILL |
| 21 | API-KEY 落盘自保阀（内存空 + 磁盘坏 = 跳过写盘） | 用空存储覆写仅存的、也许还能挽救的密钥文件 |
| 22 | 数据目录由凭据路径父目录推断，且**不在镜像里烘焙 ENV** | ENV 层凌驾 config.json，把用户配置静默改道；或数据落到卷外，容器重建即丢 |
| 23 | Dockerfile builder 与运行镜像必须钉同一 Debian 世代（且有测试钉死） | 编译/构建/推送全绿，容器启动即 `GLIBC_2.38 not found` |
| 24 | 交叉编译必须同时设 LINKER/CC/**AR** | 用宿主 ar 产出 x86 归档，链接阶段才报错，错误信息与真实原因相距很远 |
| 25 | healthcheck 端口优先级必须与应用一致 | 用户改端口后容器永远 unhealthy |
| 26 | compose 端口映射跟随 `${PORT}` | 改 PORT 后新旧端口都连不通 |
| 27 | XFF 取**倒数第 n 跳**，不是最左项；且转发头只在对端私网时采信 | 取最左 = 采信客户端伪造值 |
| 28 | key 比较用 `ConstantTimeEq` | 时序侧信道 |
| 29 | admin 闸**不据 store key 判定「已配置鉴权」** | 首次部署建出第一把 key 就把管理员锁在门外 |
| 30 | 消费上限的单次预留值必须 ≥ 单次真实花费、又不能吃掉整个上限 | 旧值 1.0 credits：1 credit 上限的 key 从第一发起就 402，面板显示还剩九成却发不出请求 |
| 31 | 后台续期只覆盖「近期活跃」账号，且每轮有上限 | 全池定时刷新 = 每个账号一条永不停歇的心跳，「无人使用却持续产生规律流量」正是风控最爱盯的形状 |
| 32 | 测试临时目录统一收口 + 自动回收 | /tmp 顶层 4 天堆 9582 个残留目录 |
| 33 | `node --test` 逐文件传入，不传目录 | 整个目录被当成一个失败用例报出来 |
| 34 | 面板逻辑也要进 CI 门禁 | 33 个前端测试一次都没跑过，改坏了要人工点开面板才发现 |

---

## 13. 移植到 agnes2api-rs 的取舍建议（建议，非结论）

### 建议照抄（几乎无条件）

- `lib.rs` + 瘦 `main.rs` 的分家（否则集成测试只能打端口）。
- `stats/persist.rs` 那套「原子写 + 脏标记 + 去抖刷盘 + 停机显式 flush + 自保阀」**整块搬**。
  agnes2api 的 key 池、统计、用量都需要它。
- `http.rs` 的「多客户端工厂 + 按代理分桶缓存」结构（agnes2api 上游是否需要 TLS 指纹对齐
  要单独判断，但**客户端分家**这件事本身是通用的）。
- 锁选型纪律（跨 await → tokio，不跨 → parking_lot）+ 「池锁绝不跨网络 await」。
- `RefreshCoordinator` 的两层单飞锁模板（若 agnes2api 的 key 存在并发状态更新）。
- 优雅停机（SIGTERM + 有界排空 + 显式刷盘）。
- 手写 CORS 中间件（连同 §8.2 的每一条）。
- 错误枚举 + `IntoResponse` 三段式（status / err_type / message），不引 thiserror。
- 「注入时钟」：所有业务函数收 `now_unix`，不散读系统时钟。
- 测试形态：inline `#[test]` + `tower::oneshot` + `wiremock` + `endpoint_override` + `test_tmp`。
- `build.rs` 的 best-effort 版本注入；`set-version.sh` 的单一真源同步。
- Dockerfile 的三阶段 + 交叉编译 + 同代 glibc 断言测试 + gosu entrypoint + 端口优先级。
- CI 四道阻塞门（fmt / clippy -D warnings / test / 面板测试）。

### 建议照抄但要改形状

- **面板**：用户明确要求 UI 完全对标 kiro2api，而 kiro2api 的管理面板正是
  `admin-ui-v2` 的**零构建手写静态**形态。建议 agnes2api-rs **只做这一套**，
  不要把 `user-ui` 的 React/Vite 链路一并背过来（那会让 `cargo build` 强依赖 node）。
  `webui/mod.rs` 那 270 行（ETag/304/SPA 回退/三条路由）可近乎原样复用。
- **四协议目录**：结构照搬，但「母协议」的选择要按 Agnes 上游的原生语义定，不能默认是 Anthropic。
- **`MessagesState` 的等价物**：字段照 agnes2api 的实际需要裁（注册机、key 池、
  限流两道闸这些 kiro2api 没有），但「全 Arc + Clone + 三组路由共用一份」的形态照抄。

### 明确**不**适用 / 需要重新判断

- **TLS 指纹与连接不复用（#1/#3/#4）**：这几条是针对 Kiro 上游的风控对抗，
  成本很实在（每请求多一次 TLS 握手、必须 vendored OpenSSL、Dockerfile 多装 perl/make）。
  Agnes 上游是否有同类风控**必须单独验证**，不能因为 kiro2api 这么做就照搬。
  记忆里 agnes2api 已知的是「CF 1015 边缘限流」与「Agnes 应用层 400」两道**不同**的限流，
  与 TLS 指纹不是一回事。→ **这是需要主线裁的一个分叉。**
- **`kiro/eventstream/`（AWS 二进制事件流解码）**：Agnes 上游若是普通 SSE/JSON，
  整个目录不需要，对应位置换成普通 SSE 解析。
- **`kiro/login/`（三种 AWS 登录方式）**：agnes2api 的等价物是**注册机**，形态完全不同，
  kiro2api 在这块没有可参照物（P3 的难点正在于此）。
- **`balance/`（上游余额）**：取决于 Agnes 是否有余额概念。
- **`user/`（用户面 + user-ui）**：kiro2api 有两套 UI，agnes2api 是否需要用户面要先定。
- **`stats/pricing.rs`（按模型计价）**：定价表是 Anthropic 模型的，agnes2api 要自己的。

---

## 14. 我没读透的地方（诚实说明）

以下部分只做了结构性浏览（看了模块头注释、公开 API 签名、部分实现），**没有逐行读完**。
若移植时要照抄其内部算法，需要再读一遍：

1. **`kiro/convert.rs`（4188 行）** —— 上游事件流 ↔ Anthropic 语义的核心转换，
   含 `ThinkingSplitter`、`extract_metering`、工具名还原等。只读了被调用处的签名与注释。
2. **`admin/handler.rs`（6767 行）** —— 只读了路由表（`admin/mod.rs`）、错误助手、
   `restart_server`、`server_info` 的片段。**P2 做面板对标时必须重读这一份**，
   它定义了面板的全部 API 契约（camelCase 字段名逐个对齐前端）。
3. **`protocol/*/convert.rs`（openai 1142 / gemini 1303 / responses 824 行）** ——
   只确认了它们的存在与职责，没读转换细节。
4. **`kiro/pool.rs` 的选号算法主体（`select_with_exclude`，约 150 行）** ——
   读了失败分类、冷却常量、`LbMode` 语义与 `Pool` 结构，但跨账号重试的自适应上限
   （`cross_account_attempts`）与粘滞选号的具体游标推进只看了注释。
5. **`apikey/mod.rs`（1860 行）的后半部分** —— 读了 `ApiKey` 结构、过期/惰性激活语义、
   存储的加载与落盘接线，没读 CRUD 与用量绑定的全部分支。
6. **`server/auth.rs`（2160 行）的后半部分** —— 读了两种闸的判据、`AuthState`、
   `BoundCredentialIds`、消费上限预留常量，没读 `SpendCache` 的实现与
   `require_api_key` 的完整分支树。
7. **`admin-ui-v2` 的 JS/CSS 实现（12,322 行）** —— 只读了 `index.html` 的骨架、
   `api.js` 的前 70 行（网络层约定：`x-api-key` 头 / `?api_key=` for SSE / 401 清会话 /
   `localStorage` 两把 key）。**P2 要对标 UI，这部分必须单独重读并出一份 UI 契约文档。**
8. **`stats/usage.rs`（1684 行）** —— 只读了对外方法签名与 `StatsManager` 的转发层。
9. `models_cache/`、`balance/`、`kiro/{provider,refresh,mcp,machine_id,wire,endpoint}.rs`
   —— 仅从调用点与模块头了解职责。
10. **CHANGELOG.md（91KB）与 README.md（42KB）完全没读** —— 里面很可能还有更多
    「踩过坑」的记录（尤其 CHANGELOG 的每条修复说明），值得在 P2/P3 前扫一遍。

---

*本文所述均为 `/home/ubuntu/kiro2api` 的现状（读取时该仓最新提交为 2026-08-23 前后的状态，
`Cargo.toml` 版本 0.17.2）。未对该仓做任何修改。*
