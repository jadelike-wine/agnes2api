# agnes2api `src/core/` 核心业务规则抽取（Rust 移植规格）

> 来源：`/home/ubuntu/agnes2api/src/core/`（TypeScript v0.4.0，42 文件 / 12,301 行），逐个通读。
> 目的：**照着这份文档能用 Rust 重新实现，不必再回头读 TS 源码**。
> 所有「出处」一律写成 `文件:行号`，行号对应 2026-09-11（北京时间）时的工作副本。
>
> 📌 **阅读纪律**：原仓注释里带 🔴 / ⚠️ 的段落，绝大多数是「某次实测缺陷的墓碑」。
> 本文把它们逐条抄下来并标注出处。**凡带数字的常量、带分档的错误处理，一个都不许在 Rust 侧
> 「凭直觉重新取值」——要改必须先推翻这里记的理由。**

---

## 0. 全局硬约束（`src/core/` 这一层的立身之本）

| 约束 | 原文位置 | Rust 侧怎么落 |
|---|---|---|
| **零 IO**：core 里不许有时间、随机、定时器、网络、环境变量 | `dispatcher.ts:1-35`、`config-provenance.ts:31-37`、`domain-ledger.ts:5-8` | 所有 `now: impl Fn() -> i64`、`rand`、`sleep`、`fetcher`、`storage`、`logger` 一律**参数注入**。Rust 用 trait 对象或泛型参数，不许在 core crate 里 `use std::time::SystemTime` / `reqwest::get` |
| **既定豁免清单不许变长** | `dispatcher.ts:3-35` | TS 侧只豁免了 3 处：① `setTimeout(abort)`（`dispatcher.ts:474`）；② 模块级 `let cursor`（`dispatcher.ts:65`）；③ `crypto.subtle.digest`（`keypool-repo.ts:248`、`api-keys.ts:188`）。Rust 侧 ①③ 天然不是问题（`tokio::time::timeout`、`sha2` crate），**②必须显式处理**，见 §2.4 |
| **从存储读回来的东西一律 `unknown` + 逐字段窄化** | `pool-index.ts:37-43`、`event-entry.ts:24-50`、`tend-guard.ts:132-140`、`domain-ledger.ts:126-133`、`api-keys.ts:408-426` | Rust 侧不能靠 `serde` 的 `Deserialize` 直接 `?` 掉——**坏数据的处置方向逐处不同**（有的丢整条、有的丢一个字段、有的回空、有的抛）。见 §7 那张「窄化方向表」 |
| **诚实第一：说不出来就说说不出来，不许编一个值** | `upstream-models.ts:31-56`、`url.ts:205-207`、`config-errors.ts:97-108` | `Option<T>` 的 `None` 与「零值」在这个项目里是两件事，不许用 `unwrap_or_default()` 抹平 |
| **同一条判据不许有第二份实现** | `keypool.ts:3-46`、`config.ts:155-165` | 见每节的「唯一真源」标记 |

### 0.1 端口（core 依赖的注入接口）

出处 `src/ports/*.ts`：

```rust
// storage.ts:39-66
trait Storage {
    async fn get_raw(&self, key: &str) -> Result<Option<serde_json::Value>>;
    async fn put_raw(&self, key: &str, value: &serde_json::Value, expires_at: Option<i64>) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
}
// fetcher.ts:1-3
trait Fetcher { async fn fetch(&self, url: &str, init: RequestInit) -> Result<Response>; }
// logger.ts:1-43  —— LogLevel = debug|info|warn|error；LogEntry { ts, level, event, msg, corr?, fields? }
//                     ⚠️ fields 只放标量（config-errors.ts:31 同一条纪律）
trait Logger { fn log(&self, e: LogEntry); }
// mailbox.ts:26-70 —— 见 §8
trait MailProvider {
    async fn list_domains(&self) -> Result<Vec<String>>;
    async fn create_mailbox(&self, domain: &str) -> Result<Mailbox>;
    async fn poll_code(&self, mb: &Mailbox, timeout_ms: u64) -> Result<Option<String>>;
    async fn delete_mailbox(&self, mb: &Mailbox) -> Result<bool>;
    async fn verify_credentials(&self, domain: &str) -> Result<CredentialProof>;
}
```

⚠️ `Storage::put` 的第三参 `expires_at` 是**可选**的，而「哪些键传 / 哪些键一律不传」是承重判据，
见 §7.0 的存储键全表。

---

## 1. 存储键全表（唯一真源，新增键必须进这张表）

出处：分散在各文件的 `export const *_KEY / *_KEY_PREFIX`，
原仓有一道按名字扫源码的门禁（`api-keys.ts:28-39` 记着这条约定）：
**`src/` 下叫 `*_KEY` / `*_KEY_PREFIX` 的导出常量一律是存储键**。

| 键 | 常量 | 出处 | 数量 | 传 TTL 吗 | 值形状 |
|---|---|---|---|---|---|
| `key:<id>` | `KEY_PREFIX` | `pool-index.ts:18` | 随池大小 | ❌ | `KeyRecord` |
| `pool:index` | `POOL_INDEX_KEY` | `pool-index.ts:19` | 1 | ❌ | `{ v: 1, ids: string[] }` |
| `config` | `CONFIG_KEY` | `config-provenance.ts:52` | 1 | ❌ | 面板可改的那份配置 |
| `apikeys` | （字面量，`api-keys.ts:10`） | — | 1 | ❌ | `{ version, keys: ApiKeyRecord[] }` |
| `event:<窗口>:<槽位>` | `EVENT_KEY_PREFIX` | `event-ring.ts:156` | 有界靠 TTL | ✅ `eventExpiresAt()` | `EventEntry[]`（环形 ≤100） |
| `usage:<day>:<slot>` | `USAGE_KEY_PREFIX` | `usage-stats.ts:303` | 有界靠 TTL | ✅ `usageExpiresAt()` | `UsageDayShard` |
| `tend:history` | `TEND_HISTORY_KEY` | `tend-history.ts:34` | 1 | ❌ | `TendRecord[]`（环形 ≤50） |
| `registrar_manual_guard` | `MANUAL_GUARD_KEY` | `tend-guard.ts:47` | 1 | ❌ | `{ day, used, cooldownUntil }` |
| `registrar:backoff` | `REGISTRAR_BACKOFF_KEY` | `backoff.ts:26` | 1 | ❌ | `BackoffState` |
| `registrar:domains` | `DOMAIN_LEDGER_KEY` | `domain-ledger.ts:47` | 1 | ❌ | `DomainLedger` |
| `health:probe` | `HEALTH_PROBE_KEY` | `storage-health.ts:96` | 0（写完即删） | ❌ | `{ at }` |

🔴 **「一律不传 `expiresAt`」这条对后四把键是承重判据，不是省事**：
`tend-guard.ts:7-27` 逐字论证过——冷却 10 分钟、而日预算按 `day` 字段判跨天，
让 TTL 兼任 `day` 的职责会让**日预算闸永远走不到耗尽**（键每 10 分钟蒸发一次、`used` 归零）。
同一条理由复述在 `backoff.ts:19-25`、`domain-ledger.ts:41-45`、`tend-history.ts:28-33`。
**代价明写**：这几把键会在存储里长期驻留，永久关掉注册机之后清理靠手工删键。

⚠️ `domain-ledger.ts:38-40`：台账键**刻意不用 `key:` 前缀**——`KeyPoolRepo` 兜底那条
`list("key:")` 会把它扫进来当成一把 key 记录。Rust 侧同理。

---

## 2. key 池（keypool.ts / types.ts / errors.ts）

### 2.1 `KeyRecord` / `KeyStats`（`types.ts:1-52`）

```rust
struct KeyStats {           // types.ts:5-16，**近似值**，两条误差要在面板上打 ≈
    requests: u64,          // 不变式：requests == success + failed + client_errors
    success: u64,           // 上游 2xx
    failed: u64,            // 归因到这把 key：超时 / 5xx / 非 JSON 响应体 / 凭据失效 / 限流
    client_errors: u64,     // 上游 4xx 直通 —— **客户端的错，不是这把 key 的错**，单列
    last_error_at: Option<i64>,
    last_error_kind: Option<String>,
}
struct KeyRecord {          // types.ts:18-52
    id: String,             // SHA-256 前 8 字节的 hex（16 字符）
    key: String,            // **明文**（要拿去用，见 api-keys.ts:6-13 那张对照表）
    added_at: i64,
    last_used_at: Option<i64>,
    cooldown_until: i64,    // 0 = 不在冷却
    cooldown_reason: Option<String>,   // 仅供排障，**不参与调度**
    strikes: u32,
    evicted: bool,
    evicted_reason: Option<String>,
    stats: Option<KeyStats>,           // 可选：存量记录没有，读取处统一 normalize
    disabled: Option<bool>,            // 可选：管理员**手工**停用
    note: Option<String>,              // 可选：运维备注
}
```

⚠️ **`disabled` 与 `evicted` 语义分离，不许合并**（`types.ts:31-40`）：
前者是**人的决定**、随时可撤销；后者是**系统按 401/403 的判定**、要换 key。
混为一谈会让 503 把运维指向一件完全没用的事（去换 key，而其实是他自己关的）。

⚠️ **三个可选字段必须真的可选**（`types.ts:36-38`、`key-view.ts:73-81`）：
写成必填的话存量记录读回来是 `undefined`，JSON 序列化会把该字段**整个丢掉** ⇒
面板收到的是「字段不存在」而不是 `false`，分不清「没停用」和「读不出来」。
**Rust 侧对应**：`Option<bool>` + `#[serde(default, skip_serializing_if = "Option::is_none")]`，
但**投影到面板时（`KeyView`）必须落成实打实的 `bool` / `Option<String>`，恒存在**。

### 2.2 唯一判据三件套（`keypool.ts`）

```rust
// keypool.ts:47-49  —— 全仓唯一一份「停用了吗」的判据
fn is_disabled(r: &KeyRecord) -> bool { r.disabled.unwrap_or(false) }
// keypool.ts:52-54  —— 「现在能不能拿去打上游」
fn is_available(r: &KeyRecord, now: i64) -> bool {
    !r.evicted && !is_disabled(r) && r.cooldown_until <= now
}
// keypool.ts:93-95  —— 「还算不算在 targetKeys 名额里」**补池专用**
fn counts_toward_target(r: &KeyRecord) -> bool { !r.evicted }
```

🔴 **`counts_toward_target` 与 `is_available` 必须是两个函数，合并是一条实测事故**
（`keypool.ts:56-95`，两次实测复现都记在那里）：

* 用 `is_available` 当补池缺口判据 ⇒ 「在面板上停用一把 key」＝「自动注册一个新 Agnes 账号」；
* 判据写成 `!evicted && cooldown_until <= now`（只放过 disabled）还留着两条同族缺陷：
  1. **停用一把正在冷却的 key 仍会触发一次补池**（实测 `targetKeys=3`：available 3→2、minted 1、池子 3→4，
     冷却到期后 `need` 转负，多出来那把**再也不会退掉**）。而运维去停用一把 key 的最常见起因，
     恰恰是面板显示它「冷却中」；
  2. **冷却导致池子永久膨胀，线性、不封顶**（实测：稳态 3 → 全池被限流冷却那轮 minted=3 ⇒ 池子 6 ⇒
     冷却到期 need=-3 不再铸 ⇒ **永久停在 6**；下一次风暴 9，再下一次 12）。
* **改成 `!evicted` 之后两条一起消失。** 代价明写：整池都在冷却时补池**不会**去铸替补，
  网关在冷却窗口内持续 503（最长一个 `COOLDOWN_PAYMENT_MS` / `COOLDOWN_STRIKE_MS`）。
* ⚠️ **这个函数与时间无关，不许加回 `now` 参数**（`keypool.ts:88-92`）——加回去就是把上面两条缺陷的入口重新打开。

⚠️ **`is_disabled` 用真值性而不是 `== true`**（`keypool.ts:36-41`）：记录来自裸 `JSON.parse` + `as` 断言，
运行期什么形状都可能是。对一个安全开关，「读不懂就当停用」是安全的那一侧。
**Rust 侧**：`serde` 已经保证类型，但**存储里非布尔的 `disabled` 会让整条记录反序列化失败** ——
必须用 `#[serde(deserialize_with = ...)]` 或 `Option<serde_json::Value>` 手动窄化成
「不是 `false` 就当 `true`」的方向，不许让一条脏记录把整把 key 变成读不出来。

⚠️ `is_disabled` 刻意**不做成返回结构体**（`keypool.ts:42-45`）：它在 `select_key` 的热路径循环里，
逐条记录新建一个对象只为读一个布尔，是白付的代价。

### 2.3 状态迁移（`keypool.ts:112-148`）

```rust
fn apply_success(r, now)  -> KeyRecord { KeyRecord { strikes: 0, last_used_at: Some(now), cooldown_reason: None, ..r } }
fn apply_cooldown(r, now, ms, reason) -> KeyRecord { KeyRecord { cooldown_until: now + ms, cooldown_reason: Some(reason), ..r } }
fn apply_strike(r, now, cfg, reason) -> KeyRecord {   // keypool.ts:133-143
    let strikes = r.strikes + 1;
    if strikes >= cfg.max_strikes {
        KeyRecord { strikes: 0, cooldown_until: now + cfg.cooldown_strike_ms, cooldown_reason: Some(reason), ..r }
    } else { KeyRecord { strikes, ..r } }
}
fn apply_evict(r, reason) -> KeyRecord { KeyRecord { evicted: true, evicted_reason: Some(reason), ..r } }
```

🔴 **`apply_strike` 达到 `max_strikes` 时是「进长冷却」，不是「永久剔除」**（`keypool.ts:120-132`）：
原实现在这里直接置 `evicted`，后果是**上游一次抖动就能永久摧毁整个 key 池**——三把 key 的池子在
上游持续 503 时只需五个请求即全部报废，而当时没有任何 un-evict 路径，上游恢复后网关也永远起不来。
**永久剔除只保留给凭据失效（401/403）。**

* `strikes` 在**进入**冷却时即清零（不是等冷却到期再清）：冷却期内这把 key 根本不会被 `select_key` 选中，
  两种写法对外行为完全一致，但前者不需要额外的惰性归一化步骤。

**所有 `apply_*` 都返回新对象、绝不就地改**（`keypool-repo.ts:496` 依赖这条性质）。
Rust 侧用 `Clone` + 结构体更新语法，不要 `&mut self`。

### 2.4 轮询选 key（`keypool.ts:97-110`）

```rust
fn select_key(records: &[KeyRecord], cursor: usize, now: i64) -> Option<(usize /*idx*/, usize /*next_cursor*/)> {
    if records.is_empty() { return None; }
    let start = cursor % records.len();
    for i in 0..records.len() {
        let idx = (start + i) % records.len();
        if is_available(&records[idx], now) { return Some((idx, idx + 1)); }
    }
    None
}
```

🔴 **`cursor` 在 TS 里是模块级 `let`（`dispatcher.ts:65`），这是一处登记在案的既定豁免**
（`dispatcher.ts:28-32`）：每个副本各一份，轮询起点因此不全局一致（多容器共卷时）。
**在修掉之前，面板不许展示「下一把 key / 轮换顺序」**——展示了就是给一个与实际不符的值。

**Rust 侧处置**：放进 `AppState` 里的 `AtomicUsize`（每进程一份），语义与 TS 逐字相同。
不许为了「好看」改成全局一致的分布式游标——那是行为变更，要先推翻上面那条登记。

### 2.5 四格计数（`keypool.ts:150-170`）

```rust
struct PoolHealth { total: usize, fresh: usize, cooling: usize, evicted: usize, disabled: usize }
fn pool_health(records: &[KeyRecord], now: i64) -> PoolHealth {
    // 判定顺序即优先级：disabled > evicted > cooling > fresh
    for r in records {
        if is_disabled(r) { disabled += 1 }
        else if r.evicted { evicted += 1 }
        else if r.cooldown_until > now { cooling += 1 }
        else { fresh += 1 }
    }
}
```

⚠️ **顺序必须与 `key_bucket`（`key-view.ts:53-58`）逐档同序**——两处不同序就会出现
「概览卡说 3 把冷却中、Key 池列表一把冷却中的都没有」这种自相矛盾。
四格互斥且穷尽 ⇒ `total == fresh + cooling + evicted + disabled` 恒成立。

⚠️ **被停用的 key 既不算 `fresh` 也不算 `evicted`**（`keypool.ts:154-158`）：
算进 `fresh` 会让 503 退回 `upstream_error`；算进 `evicted` 会让 503 告诉运维「去换 key」，
而其实是他自己在面板上关的。

### 2.6 上游状态码分档（`errors.ts:1-25`）—— **这张表是全局唯一的**

```rust
enum UpstreamAction {
    Success,
    Cooldown { ms: i64, reason: &'static str },
    Evict { reason: String },
    Strike { reason: String },
    Passthrough,
}
fn classify_status(status: u16, cfg: &Cfg) -> UpstreamAction {
    match status {
        200..=299 => Success,
        429       => Cooldown { ms: cfg.cooldown_rate_limit_ms, reason: "rate limited" },
        402       => Cooldown { ms: cfg.cooldown_payment_ms,    reason: "payment required" },
        401 | 403 => Evict  { reason: format!("upstream {status}") },
        s if s >= 500 => Strike { reason: format!("upstream {status}") },
        _         => Passthrough,       // 其余 4xx：客户端的错，直通
    }
}
fn classify_thrown(err) -> UpstreamAction {
    if err.is_abort() { Strike { reason: "timeout" } } else { Strike { reason: "network error" } }
}
```

Rust 侧 `classify_thrown` 的判据：`reqwest::Error::is_timeout()` / `tokio::time::error::Elapsed`
⇒ `"timeout"`，其余一律 `"network error"`。**这两个字符串是 `KeyStats.last_error_kind` 的取值，
会进面板，改字符串就是改对外可见的东西。**

---

## 3. dispatcher（`dispatcher.ts`，587 行）—— 转发热路径

### 3.1 出站响应头白名单（`dispatcher.ts:67-132`）

```rust
const SAFE_RESPONSE_HEADERS: [&str; 4] = ["content-type", "cache-control", "retry-after", "content-disposition"];
```

**上游响应头一律不原样转发**，三条理由（`dispatcher.ts:70-73`）：
① `set-cookie` / `www-authenticate` / `authorization` 带的是上游的凭据语义，透传等于把它们落到网关自己的域上；
② 上游的 `x-*` 内部头是不受控的泄漏面（**实测 `x-upstream-internal` 会原封不动到达客户端**）；
③ 池子每次请求都可能换一把 key，逐 key 的限流/配额头对客户端只是误导。

* **不含 `content-length` / `content-encoding`**：响应体在这里被重新包装（fetch 已解压），沿用会与实际字节不符。
  Rust 侧若用 `reqwest` + `axum`，同样必须**重建**这两个头，不许 `headers.clone()`。
* **含 `content-disposition`**：媒体路由承诺「上游返回什么就原样转发响应体」，剥掉它会让浏览器端下载丢失文件名。
* **不含 `accept-ranges` / `content-range`**：网关不支持 range（不解析、不转发 `Range` 请求头），
  放行只会让客户端发起注定失败的 range 请求 —— 属于误导而非帮助。

### 3.2 MIME 钳制（`dispatcher.ts:88-117`）

```rust
// 会被浏览器当成「同源可执行 / 可渲染文档」的 MIME ⇒ 改写成 application/octet-stream
static DOCUMENT_MIME: &str = r"(?i)^\s*(text/html|application/xhtml\+xml|image/svg\+xml|text/xml|application/xml|text/javascript|application/javascript|application/x-javascript|application/ecmascript|text/ecmascript|multipart/x-mixed-replace)\b";
fn clamp_content_type(v: &str) -> String { if DOCUMENT_MIME.is_match(v) { "application/octet-stream".into() } else { v.into() } }
```

**为什么是 deny-list 而不是 allow-list**（`dispatcher.ts:91-95`）：媒体路由承诺「上游返回什么就原样转发」，
allow-list 会把没预料到的媒体类型（新的图片/视频容器）打成下载，那是真的功能退化。
**边界写清楚**：deny-list 永远列不全，目标是把「同源文档」这一类关掉，不是做完备的 MIME 治理。

**为什么需要它**（`dispatcher.ts:96-105`）：`script-src 'self'` 的隐含前提是「本源只有面板脚本」，
而 `/v1/videos/:id` 是一条接受 `?key=` 的 GET 路由，响应的 content-type 是上游逐字透传的 ——
于是这个源上可以出现一个攻击者影响得了的同源文档，而 `ADMIN_TOKEN` 就存在这个 origin 的
localStorage 里（**localStorage 的作用域是 origin 而不是 path**）。

⚠️ **全局 `nosniff` 挡不住这一半**：`nosniff` 只否掉「按内容嗅探出一个更危险的类型」，
它不阻止浏览器渲染一个**显式声明**为 `text/html` 的响应。

⚠️ **`multipart/x-mixed-replace` 在列，它是 deny-list 最经典的那条绕法**：外层类型本身无害，
而 Firefox 会按**内层每个 part 自己的 `Content-Type`** 渲染 ⇒ 一个 `text/html` 的 part 照样变成同源文档。
它是**已知**绕法，一个 alternation 就堵上了，没有理由留着。

### 3.3 错误体流式脱敏（`dispatcher.ts:134-197`）

```rust
static CREDENTIAL_LIKE: &str = r"sk-[A-Za-z0-9_-]{6,}";   // dispatcher.ts:142
const REDACT_CARRY: usize = 256;                          // dispatcher.ts:150
const MASK: &str = "sk-***";
```

**只认 `sk-` 开头那一族**（`dispatcher.ts:134-141`）：上游（Agnes）与本网关签发的对外密钥都是这个前缀，
而普通英文错误文案里几乎不会出现它。
⚠️ **刻意不做更宽的匹配**（例如「任何 20 位以上的十六进制」）：错误体里本来就常带 request id、trace id、
模型指纹，把那些一并打码会让运维**丢掉排障线索**。**宁可漏掉一种没见过的凭据格式，
也不要把有用的错误体变成一片星号。**

**为什么需要它**（`dispatcher.ts:154-160`）：401/403 确实被合成成网关自己的错误体，
但 **429 / 402 / 5xx 与其余 4xx 的上游正文一直是逐字透传的**，而「凭据无效」并不是各家 API
唯一会回显 key 片段的地方——配额耗尽、账号被禁这类文案同样会带。

🔴 **必须是流式改写，不许 `await res.text()` 之后再替换**（`dispatcher.ts:161-166`）：
第一版就是那么写的，被判据当场抓住——一个**永不关闭**的 ReadableStream 会让 `text()` 挂死。
真实上游会关闭错误体，但**半开连接不会** —— 把整个请求卡死在读一个错误体上，
是拿一条可用性风险去换一条泄漏防护。

**算法**（`dispatcher.ts:168-192`）：

1. 每块：`text = carry + decode(chunk, stream=true)`；
2. 从尾部往回找最后一个 `sk-`，位置为 `idx`；
   `cut = if idx >= max(0, text.len() - REDACT_CARRY) { idx } else { text.len() }`；
3. `emit(text[..cut].replace(CREDENTIAL_LIKE, MASK))`；`carry = text[cut..]`；
4. **兜底**：`if carry.len() > REDACT_CARRY * 4 { emit(carry); carry.clear() }`
   （`sk-` 后面跟着几千个合法字符的东西不是凭据，别让 carry 无界增长）；
5. flush：`emit(carry + decoder.flush())`。

`REDACT_CARRY = 256` 的理由（`dispatcher.ts:144-150`）：`sk-` 之后是一串合法字符，而块边界可能正好落在中间
⇒ 必须把「可能才写了一半的那一段」留到下一块再判。256 远大于任何真实密钥长度，同时给内存一个硬上界。

**两个出口函数**：
* `sanitize(res)`（`dispatcher.ts:130-132`）：**成功路径**，白名单重建响应头，响应体（含流式）原样搬运；
* `sanitize_error(res)`（`dispatcher.ts:194-197`）：**错误路径**，白名单重建响应头 + 流式脱敏正文。

`discard(res)`（`dispatcher.ts:199-206`）：取消一个被丢弃的上游响应体。任何一条 return 路径都要先把
攒着的上一个上游错误响应体取消掉，否则它的响应体永远没人消费。Rust 侧 `drop(response)` 即可，
但**在 `reqwest` 下要确认底层连接真的被释放**（`Response` drop 会取消）。

### 3.4 池子不可用时的 503（`dispatcher.ts:208-298`）

```rust
const FAIL_REASONS: [&str; 5] = ["pool_empty", "all_cooling", "all_disabled", "all_evicted", "upstream_error"];
// 响应体：503 { "error": { "reason": <上面之一>, "message": "<中文>" } }，可选 Retry-After 头
```

⚠️ **`reason` 是对外 API 契约的一部分**（`dispatcher.ts:208-215`）：五份 API.md 各有一张表列着它们，
加一条新 reason 而不写文档在原仓会让一条用例变红。Rust 侧同样要把它写成运行期可枚举的常量数组。

**四条 reason 的排序判据**（`dispatcher.ts:234-242`）：池子里一把可用的都没有时，成因可能同时有三类
（冷却 / 停用 / 剔除），而 `reason` 只有一个位置。**取「最先能让池子重新可用的那一类」**，
因为运维读到 reason 之后要做的正是那件事：**冷却会自己好（等）→ 停用是他自己关的（点一下）→ 剔除要换 key（买）**。
精确的三个数一律在 `message` 里给全，reason 只负责把人指向该做的那件事。

**`all_disabled` 不复用 `all_evicted`，理由是物理不是口味**：`all_evicted` 对运维的含义是
「凭据都失效了，去换 key」，而这里的真相是「是你自己在面板上关的」。

判定顺序（`dispatcher.ts:255-298`）：

```rust
fn unavailable(records: &[KeyRecord], now: i64) -> Response {
    let h = pool_health(records, now);
    if h.total == 0 { return fail("pool_empty", "key 池为空，请先导入 key", None); }
    // 还有可用的 key ⇒ 走到这里只可能是「每一把都试过、上游都失败」
    if h.fresh > 0 { return fail("upstream_error", "已尝试池中每一把 key，上游均返回失败；key 本身仍可用", None); }
    if h.cooling > 0 {
        // ⚠️ 过滤条件里的 !is_disabled(r) 不是顺手加的（dispatcher.ts:267-271）
        let earliest = records.iter().filter(|r| !r.evicted && !is_disabled(r))
                              .map(|r| r.cooldown_until).min().unwrap();
        let retry_after_sec = max(1, ceil((earliest - now) as f64 / 1000.0));
        return fail("all_cooling",
            format!("全部 key 暂不可用：{} 把冷却中（到期自动恢复）、{}{} 把已永久剔除",
                    h.cooling,
                    if h.disabled > 0 { format!("{} 把被管理员停用、", h.disabled) } else { String::new() },
                    h.evicted),
            Some(retry_after_sec));
    }
    // 到这里只剩 disabled 与 evicted 两类，**都不会自动恢复 ⇒ 一律不带 Retry-After**
    if h.disabled > 0 {
        return fail("all_disabled",
            format!("全部 {} 把 key 均不可用且不会自动恢复：{} 把被管理员手工停用（在管理面板上重新启用即可）{}",
                    h.total, h.disabled,
                    if h.evicted > 0 { format!("、{} 把因凭据失效被永久剔除", h.evicted) } else { String::new() }),
            None);
    }
    fail("all_evicted", format!("全部 {} 把 key 均因凭据失效被永久剔除，不会自动恢复，请更换 key", h.total), None)
}
```

⚠️ **`!is_disabled(r)` 那个过滤条件是承重的**（`dispatcher.ts:267-271`）：被停用的 key 的 `cooldown_until`
通常是 0（它根本没在冷却），漏掉它会让 `min` 取到 0 ⇒ `Retry-After: 1` ⇒ 客户端每秒重试一次，
而那把 key 永远不会自己回来。这一段与 `pool_health` 的 `cooling` 计数**必须筛出同一批记录**，
否则 `h.cooling > 0` 却 `min()` 于空集 ⇒ `Infinity` ⇒ 一个非法的响应头。

⚠️ **message 文本上的两处条件拼接是刻意的**（`dispatcher.ts:248-253`）：无条件拼上「0 把被管理员停用」
会让每一条既有的 503 都多出一句废话。`disabled == 0` 时本函数的输出与旧版**逐字相同**。

### 3.5 超时档位（`dispatcher.ts:300-350`）

```rust
enum TimeoutProfile { FirstByte, Sync }
const SYNC_ATTEMPT_SHARE: i64 = 2;   // dispatcher.ts:330
```

**判据是「上游的第一个字节什么时候才可能到达」，不是端点的名字**（`dispatcher.ts:300-314`）：

| 档 | 用哪个预算 | 谁属于这一档 |
|---|---|---|
| `FirstByte` | `UPSTREAM_TIMEOUT_MS`（默认 **8000**） | **流式**对话（慢 key 在这里被快速甩掉）；视频轮询 `GET /videos/{id}`（不是流式，但只查一次任务状态，同样是快接口） |
| `Sync` | `UPSTREAM_SYNC_TIMEOUT_MS`（默认 **120_000**） | 图片生成、视频建任务、**以及非流式对话**——上游要把整段回答生成完才发响应头，与图片生成是同一种延迟语义 |

路由层判据：`timeout = if stream { FirstByte } else { Sync }`；
**档位仍由调用方显式传入而不在 dispatcher 里按 `stream` 自动推断**，因为视频轮询是
「非流式的快接口」这唯一的例外。

🔴 **`upstreamSyncTimeoutMs` 为什么必须与 `upstreamTimeoutMs` 分开**（`config.ts:20-28`）：
两者度量的**根本不是同一件事**。实测直连上游 `/images/generations` 首字节耗时 **11.99 秒**（HTTP 200）。
用 8 秒去卡它的结果是**图片生成 100% 失败**，且每次请求会把池中每把 key 各记一次 strike，
**三次请求就能把整个池子打进 30 分钟长冷却，连对话一起拖死。**

**`SYNC_ATTEMPT_SHARE = 2` 的取值论证**（`dispatcher.ts:317-329`）——同时满足两个互相拉扯的要求：
① 客户端最坏只等**一个**预算（不能是「池大小 × 预算」）；
② 一把挂起的 key（TCP 连得上但永不响应）不能吃掉整个请求，必须还能换一把再试——否则 20 把 key 的池子里
只要有 1 把挂起，就有 **5% 的请求硬失败**，且没有任何自愈通路。
两者只能靠切分预算调和：单次尝试 1/2 预算，超时后换下一把 key 再用剩下的 1/2。
默认配置下单次仍有 60 秒，是实测图片生成耗时（11.99 秒）的五倍。
**代价明写**：若单次调用真有可能超过预算的一半，就该把 `UPSTREAM_SYNC_TIMEOUT_MS` 设成
「单次最坏耗时的两倍以上」。

**同步档预算耗尽的 504**（`dispatcher.ts:332-350`）：

```json
504 { "error": { "reason": "upstream_timeout", "message": "同步端点用尽了 {budget} 毫秒的总预算：已尝试 {n} 把 key，均未在各自的尝试预算内收到上游响应。可能是上游整体变慢或预算配小（可调大 UPSTREAM_SYNC_TIMEOUT_MS），也可能是这几把 key 对应的上游会话被挂起；本次未惩罚任何 key" } }
```

⚠️ 文案**刻意不断言**「这是超时预算问题而非 key 故障」：原实现这么写，而在「key 对应的上游会话被挂起」时
它是一句假话，会把运维引向调大 `UPSTREAM_SYNC_TIMEOUT_MS`，只会让挂起挂得更久。
**这里只陈述事实，并把两种可能都摆出来。**

### 3.6 `dispatch()` 主循环（`dispatcher.ts:352-571`）—— 逐步照抄

**入参**：`path`、`body`、`stream`、`method`（默认 POST）、`timeout`（默认 FirstByte）、`expect_json`、`deps`。
**deps**：`repo` / `fetcher` / `config` / `now` / `logger?`。

```
1. records = repo.all()                     ← 一次快照，之后整轮都用它
2. if records.is_empty() → 503 pool_empty
3. sync_deadline       = now() + cfg.upstream_sync_timeout_ms
   sync_attempt_budget = max(1, cfg.upstream_sync_timeout_ms / SYNC_ATTEMPT_SHARE)
4. last_error: Option<Response> = None；timed_out_slots: Vec<usize> = vec![]
5. for i in 0..records.len():
     a. timeout_ms = cfg.upstream_timeout_ms
        if profile == Sync {
            remaining = sync_deadline - now();
            if remaining <= 0 { break }              // 预算耗尽就别再发注定被立刻 abort 的请求
            timeout_ms = min(sync_attempt_budget, remaining);
        }
     b. picked = select_key(records, cursor, now())
        if picked.is_none() { return last_error.unwrap_or_else(nothing_answered) }
        cursor = picked.next_cursor;  slot = picked.idx
     c. 发请求：
        POST → headers { content-type: application/json, authorization: Bearer <key> }, body = json(args.body)
        GET  → headers { authorization: Bearer <key> }（**不带 content-type**）
        URL = format!("{}{}", cfg.agnes_base_url, path)
        超时 = timeout_ms（**首字节超时**：首字节到达后立刻解除，让响应体自由流动）
     d. 发不出去 / 超时（catch）:
        action = classify_thrown(err);  reason = if strike { action.reason } else { "unknown" }
        ⚠️ if profile == Sync && reason == "timeout" {
              timed_out_slots.push(slot); continue;      // **先不记账**，见 3.7
           }
        commit_outcome(slot, apply_strike(rec, now(), cfg, reason), Failed, Some(reason)); continue;
     e. 拿到响应 ⇒ **立刻解除超时**，action = classify_status(res.status, cfg)
     f. Success:
          if expect_json {
              text = res.text().await.ok();
              if text.is_none() || !is_json(&text) {
                  commit_outcome(slot, apply_strike(rec, now(), cfg, "upstream non-JSON body"), Failed, Some("upstream non-JSON body"));
                  discard(last_error);
                  last_error = Some(502 { error: { reason: "upstream_bad_body", message: "上游返回了非 JSON 的成功响应" } });
                  continue;
              }
              attribute_sync_timeouts(slot);
              commit_outcome(slot, apply_success(rec, now()), Success, None);
              return done(Response::new(text, res.status, safe_headers(&res)));   // ← 用 safe_headers，不是 sanitize
          }
          attribute_sync_timeouts(slot);
          commit_outcome(slot, apply_success(rec, now()), Success, None);
          return done(sanitize(res));
     g. Passthrough（上游 4xx 直通）:
          commit_outcome(slot, rec /*状态不变*/, ClientError, None);
          return done(sanitize_error(res));
     h. Evict（401/403）:
          discard(res);                                  // ⚠️ 只丢弃这次的 401，**不动 last_error**
          commit_outcome(slot, apply_evict(rec, action.reason), Failed, Some(action.reason));
          continue;
     i. 其余（Cooldown / Strike）:
          discard(last_error);  last_error = Some(sanitize_error(res));
          if Cooldown { commit_outcome(slot, apply_cooldown(rec, now(), ms, reason), Failed, Some(reason)) }
          else        { commit_outcome(slot, apply_strike(rec, now(), cfg, reason), Failed, Some(reason)) }
6. return last_error.unwrap_or_else(nothing_answered)
```

**`expect_json` 的理由**（`dispatcher.ts:359-367`）：「上游 200 但响应体不是 JSON」是上游异常，不是客户端错误。
放任它冒泡到路由里的 `res.json()`，结果是一个纯文本 500，而且**这把持续吐 HTML 错误页的 key 不会被记账、
会被无限轮到**。故在这里就地校验：解析失败即记 strike 并换下一把 key，全部失败时 **502 `upstream_bad_body`**。
响应体本来就要被完整读取，因此不产生额外开销。

**`Evict` 分支为什么不动 `last_error`**（`dispatcher.ts:545-557`）：上游 401/403 说的是「池里这把 key 失效了」，
与客户端无关，**绝不能把上游的错误体透传出去**（凭据无效的错误体恰恰是各家 API 最爱回显 key 片段的地方）。
先前某把 key 的真实上游错误（例如 500）仍然是更有信息量的回复，不该被一次凭据失效抹掉。
**本档是「整个错误体都不透传」，与 `sanitize_error` 的「内容级打码」一严一宽** ——
因为这一档的错误体对客户端**没有任何信息价值**。

**`Passthrough` 分支的记账代价**（`dispatcher.ts:530-541`）：它只动 `stats`（在写消除白名单里）、不动 `last_used_at`
⇒ `only_elidable_changed` 为真且 `n - p == 0` ⇒ 被写消除吃掉、只进 `pending_stats`，
等攒满一个 `touch_interval_ms`（默认 6 小时）才随那次强制落盘一起下去。
**实测：10 次 4xx 只落 2 次盘（跨 2 个触达间隔），不是 10 次。**

### 3.7 记账与同步档超时归因（`dispatcher.ts:390-447`）

```rust
async fn commit(at: usize, updated: KeyRecord) {
    // 传上一份：save() 据此判断「这次改动是不是只动了 last_used_at」，只动了就不落盘
    if let Err(e) = repo.save(&updated, Some(&records[at])).await {
        // ⚠️ **必须吞掉。** 这里是成功分支上的最后一步，异常会把一次已经成功的上游转发变成 500
        logger.error("pool.commit_failed", "key 状态回写失败：本次请求的结果不受影响，但这一次调度记账丢了",
                     fields { id, err });
    }
    // **无论落盘成不成都要推进本请求内的视图**：同一个请求里换下一把 key 时 select_key 读的就是这个数组
    records[at] = updated;
}
async fn commit_outcome(at, updated, outcome, err_kind) {
    commit(at, with_outcome(updated, outcome, now(), err_kind)).await;   // 调度先、Tier-1 后，两步叠加
}
```

**分两步（`apply_*` 先算调度状态、再叠 Tier-1）而不是合并**（`dispatcher.ts:411-416`）：
调度那套是「丢一次就是事故」的，而 Tier-1 是明说了会少计的近似值，两套语义混在一个函数里迟早互相拖累。

**同步档超时归因**（`dispatcher.ts:422-441`）：

```rust
async fn attribute_sync_timeouts(success_slot: usize) {
    for at in timed_out_slots.iter().collect::<HashSet<_>>() {
        if at == success_slot { continue; }   // 同一把 key 先超时后成功：它刚刚自证还活着，不惩罚
        commit_outcome(at, apply_strike(records[at], now(), cfg, "sync timeout"), Failed, Some("sync timeout")).await;
    }
    timed_out_slots.clear();
}
```

🔴 **只有当同一次请求里另一把 key 真的成功了，才把先前那些超时记到对应 key 头上。**
这是能拿到的最强的对照：同一时刻、同一个上游、同一份预算，A 超时而 B 成功，说明上游当时是应答得了的，
超时来自 A 自己（例如会话挂起）。**反过来，如果整轮下来没有任何 key 成功，那更可能是预算配小了或上游整体变慢
——那是配置/上游的问题，不该记到 key 头上**（原实现正是在这里把一次图片请求变成「整池各记一次 strike」）。
记账复用既有的 strike 通路，不额外引入一个需要迁移的计数字段。

**兜底响应**（`dispatcher.ts:443-447`）：

```rust
fn nothing_answered() -> Response {
    if !timed_out_slots.is_empty() {
        sync_budget_exhausted(timed_out_slots.iter().unique().count(), cfg.upstream_sync_timeout_ms)
    } else {
        unavailable(records, now())
    }
}
```

---

## 4. `KeyPoolRepo`（`keypool-repo.ts`，1262 行）—— 存储形态、索引、写消除、损坏恢复

### 4.0 一条原则贯穿全文件（`keypool-repo.ts:370-386`）

> **记录（`key:<id>`）是真源，索引（`pool:index`）是派生缓存。**

两条写序都从这一句推出来：

| 操作 | 写序 | 中途失败留下什么 |
|---|---|---|
| `add` | ① 写记录 → ② 进索引 | **孤儿记录**（不被使用） |
| `delete` | ① 删记录 → ② 出索引 | **幽灵索引项**（读到 null 被 filter 掉） |

两种残留都是 fail-safe，且都由 `reconcile_index()` 以 `list()` 为准修回来。

🔴 **两条写序反过来各有各的坏处，且性质不同**：
* `delete` 若先出索引再删记录 ⇒ 中途失败留下孤儿记录，而对账以 `list()` 为准会把它**加回索引**
  ——**等于把一次删除悄悄撤销了。这是正确性问题。**
* `add` 若先进索引再写记录 ⇒ 中途失败时 key 材料压根没落盘，而调用方（注册机）此刻已经在 Agnes 侧
  真实建号并领到了 token ——**丢了就再也找不回来，对账也修不了（对账只认存储里已有的记录）。
  这是数据丢失问题。**

### 4.1 `pool:index` 的形状（`pool-index.ts`）

```rust
const KEY_PREFIX: &str = "key:";            // pool-index.ts:18
const POOL_INDEX_KEY: &str = "pool:index";  // pool-index.ts:19
const POOL_INDEX_VERSION: u32 = 1;          // pool-index.ts:26
struct PoolIndex { v: u32, ids: Vec<String> }
```

**索引存在的唯一理由**（`pool-index.ts:1-16`）：把 `list()` 从转发热路径上摘掉。
⚠️ 起因（Cloudflare KV 的 list 配额）在 v0.4.0 之后已不成立，**今天成立的理由是**：
`FileStorage::list()` 是一次 `read_file` + `JSON.parse` **整份 `store.json`**，
把它挂在每个转发请求上，池子一大就是逐请求的全量反序列化。索引把它降到 0 次 list。

**不退回「整池存成单条 JSON」**：那样确实只要 1 次 get，但一 key 一记录正是为了避免整池的
last-write-wins（两个并发请求各改一把 key，后写的覆盖先写的整份池子）。

**窄化规则**（`pool-index.ts:37-50`）——**方向是分层的，不许合并**：
* **结构级错误**（不是对象 / 版本不对 / `ids` 不是数组）⇒ 返回 `None`，让调用方走「索引缺失」那条**重建**路径；
* **元素级脏数据**（非字符串、空串、重复）⇒ **就地剔掉**，不让一条脏数据把整个索引作废
  ——那会退化成每次 `all()` 都 `list` 一遍。

`make_pool_index(ids)` 建构时**一律 dedupe**（`pool-index.ts:33-35`）。

`ids_from_key_names(names)`（`pool-index.ts:53-61`）：过滤 `KEY_PREFIX` 开头、切掉前缀、丢掉空 id、dedupe。

`same_id_set(a, b)`（`pool-index.ts:63-78`）：**集合相等，顺序无语义**。
⚠️ **两边各自先 dedupe 再比，不许图省事先比长度 + 单向包含**：原实现对含重复元素的入参不对称，
`same_id_set(["x","y"], ["x","x"])` 会返回 `true`。它的表现形式是**「对账认为一致因而不修」**，
正好是最难查的那种沉默。Rust 侧直接 `HashSet<&str>` 两侧比即可。

**版本号的升级路径**（`pool-index.ts:21-26`）：改索引形态时把 `POOL_INDEX_VERSION` +1；
`parse` 对不认识的版本返回 `None` ⇒ `all()` 当作「索引缺失」回落 `list()` 并重建
——**一条自动的、不需要迁移脚本的升级路径**。

### 4.2 两个旋钮与它们的默认值

```rust
const DEFAULT_POOL_CACHE_TTL_MS: i64 = 60_000;          // keypool-repo.ts:26
const DEFAULT_POOL_TOUCH_INTERVAL_MS: i64 = 21_600_000; // keypool-repo.ts:27  = 6 小时
```

* `cache_ttl_ms`（`keypool-repo.ts:308-348`）：进程级快照缓存的存活时长。**0 = 关闭缓存**（每次 `all()` 真读存储）。
  读取次数/天 ≈ `活跃副本数 × [ (86400 / 本值秒数) × (1 + 池中 key 数) + 86400 / CONFIG_TTL 秒数 ] + 对账(48~96)`。
  **中括号里第二项是 `ConfigHolder` 的，别漏**（它每 30 秒刷一次配置键）。
  默认值（60 秒 TTL、20 把 key、配置 TTL 30 秒）下每实例每天 `1440×21 + 2880 = 33,120` 次。
  🔴 **代价不只是「看得晚」**（`keypool-repo.ts:340-346`）：陈旧快照上的任意一次调度写会把整条记录覆写回去，
  **抹掉别的副本在这个窗口里刚写下的 `evicted` / `cooldown_until`** —— 那次判定要重新发生一遍。
* `touch_interval_ms`（`keypool-repo.ts:349-359`）：`last_used_at` 最多多久落盘一次。**0 = 关闭写消除**。
  代价：面板「最后使用」的精度最粗到这个间隔；但**任何其他状态变更的落盘都会顺带刷新它**，
  所以有故障发生时它反而是新的。

⚠️ **两者都是「建 app 时读一次」的旋钮**（`config.ts:34-43`）：它们绑定的是部署形态（活跃副本数 × 池大小），
不是逐次可调的策略。**改它要重启容器，面板文案不许写「立即生效」。**

### 4.3 `FIELD_ROLE` 与写消除白名单（`keypool-repo.ts:29-161`）

```rust
enum FieldRole { Scheduling, Telemetry }
// keypool-repo.ts:54-117 —— 类型是 Record<keyof KeyRecord, ...>：加字段时编译期逼人表态
id: Scheduling         key: Scheduling
cooldown_until: Scheduling   cooldown_reason: Scheduling
strikes: Scheduling    evicted: Scheduling   evicted_reason: Scheduling
disabled: Scheduling   // ← 最容易填错、填错后最难被发现的一格
added_at: Telemetry    last_used_at: Telemetry   stats: Telemetry   note: Telemetry
```

* `Scheduling`：变化会改变「这把 key 还能不能用、什么时候能用」，**丢一次就是事故**
  （吃掉一次 strike ⇒ 坏 key 被无限重试，**而且测试全绿**——这是本模块最危险的失败形态）。
* `Telemetry`：纯展示，任何调度逻辑都不读它。

🔴 **写消除的判据是白名单，不是 `FIELD_ROLE`**（`keypool-repo.ts:119-144`）：

```rust
const MAY_ELIDE_FIELDS: [&str; 2] = ["last_used_at", "stats"];   // keypool-repo.ts:144
// 白名单之外的字段全部逐一比对：keypool-repo.ts:156-161
fn only_elidable_changed(a: &KeyRecord, b: &KeyRecord) -> bool {
    a.id == b.id && a.key == b.key && a.cooldown_until == b.cooldown_until
      && a.cooldown_reason == b.cooldown_reason && a.strikes == b.strikes
      && a.evicted == b.evicted && a.evicted_reason == b.evicted_reason
      && a.disabled == b.disabled && a.added_at == b.added_at && a.note == b.note
}
```

⚠️⚠️ **这是一次判据翻转，方向很重要，别再翻回去**（`keypool-repo.ts:122-137`）。
在此之前判据是**黑名单式**的——「scheduling 字段都没变 ⇒ 可以消除」，于是 telemetry 那一档里的
**每一个**字段都自动获得了「可以被静默丢弃」的许可，而其中两个根本不该有：

* `note`：一次「只改备注」的写两条判据都满足 ⇒ **整个被丢弃**，面板显示保存成功，
  备注既不在存储也不在快照。当时唯一的护栏是「做 PATCH 的人自己记得不传 `prev`」；
* `added_at`：**一条兜底都没有**（`last_used_at` 至少有 `touch_interval_ms` 强制落盘），
  它变化的那次写会被无条件丢弃。当时安全的唯一理由是「这条路径不可达」。

**两个白名单成员各自的许可证**：
* `last_used_at` —— 高频（每次成功转发）、纯展示，且有 `touch_interval_ms` 兜底，**写消除的全部收益就来自它**；
* `stats` —— 同样高频，且被消除掉的增量**不会丢**（由 `pending_stats` 攒着补写）。
  **它是唯一一个「消除≠丢弃」的字段。**

🔴 **`disabled` 判成 Scheduling 的后果**（`keypool-repo.ts:62-74`）：把它加进白名单，「停用一把 key」这次写
就会**被写消除整个吃掉**——面板显示已停用、调度器照常用它。而且它**只在「这把 key 的 `last_used_at`
距今不足 `touch_interval_ms`（默认 6 小时）」时发生，也就是只对正在被使用的那些 key 发生**
——恰恰是最需要能停下来的那些。

🔴 **`stats` 判成 Telemetry 的后果与 `last_used_at` 完全不同**（`keypool-repo.ts:88-97`）：
`last_used_at` 被消除只是「面板上的最后使用时间粗一点」，而**计数被消除是计数直接不涨**
（实测：50 次成功之后落盘的 `requests` 是 1）。所以写消除那条路径上必须把增量攒起来，
而不是像 `last_used_at` 那样直接丢。

**Rust 侧落法**：用一个 `const MUST_MATCH: &[&str]` + 手写比较函数（如上），
并在 `KeyRecord` 上加一条 `#[cfg(test)]` 的字段数断言，保证加字段时有人表态。

### 4.4 五个退避/重试常量（`keypool-repo.ts:163-238`）

```rust
const INDEX_WRITE_RETRY_MS:      i64 = 300_000;  // keypool-repo.ts:182  索引重建写失败后的静默窗口
const READ_PATH_LIST_BACKOFF_MS: i64 = 600_000;  // keypool-repo.ts:205  读路径两次 list() 的最小间隔
const LIST_FAIL_FAST_RETRY_MS:   i64 = 60_000;   // keypool-repo.ts:235  头几次连续失败用的快窗口
const LIST_FAIL_ESCALATE_AFTER:  u32 = 3;        // keypool-repo.ts:238  连续失败几次之后升到长退避
const MAX_KEY_LENGTH:            usize = 1024;   // keypool-repo.ts:260
```

**`INDEX_WRITE_RETRY_MS = 300_000`**（`keypool-repo.ts:163-182`）：**必须显著大于 `DEFAULT_POOL_CACHE_TTL_MS`，
否则这条退避是一条死条件**。曾经两者都是 60_000 而判据是 `since < RETRY`：读路径每个快照 TTL 回落一次，
两次尝试恰好间隔 60_000，`since == 60_000` 不小于 60_000 ⇒ **一次都不被抑制**（实测跨 10 个 TTL 有 10 次 put 尝试）。
取 5 倍，留出「TTL 被调小一档」的余量。
🟡 **如实登记**：它今天在两个调用点上是**功能性死代码**（无害的第二道保险，不是活跃防线）——
两个调用点都先过 `list_on_read_path` 的 600 秒闸，那道闸放行时距上一次写尝试必然已超过本值。

**`READ_PATH_LIST_BACKOFF_MS = 600_000`**（`keypool-repo.ts:184-205`）：**空池兜底与索引缺失回落共用这一个窗口**
（两扇门都通向同一个 `list` 桶，只堵一扇等于没堵——只堵前者时后者实测仍是 1,440 次/天/副本）。
今天它管的是「每 10 分钟至多一次整份 `store.json` 的 `read_file` + 反序列化」。
🔴 **代价（必须写进部署文档，别只留在代码里）**：池子为空、或索引坏掉时，**手工导入的 key 不再是
「下一个请求即生效」，而是最多等 `READ_PATH_LIST_BACKOFF_MS + 一个 POOL_CACHE_TTL_MS`。**
想立刻生效就同时更新 `pool:index`。

**`LIST_FAIL_FAST_RETRY_MS = 60_000` + `LIST_FAIL_ESCALATE_AFTER = 3`**（`keypool-repo.ts:207-238`）：
只有一个长窗口时，**一次**瞬时 list 抖动会把那份异常粘住整整 10 分钟：期间一个**真的是空的**池子
会报 500「list 配额耗尽」而不是 503 `pool_empty` —— 把运维指向完全相反的方向。

⚠️ **时序要按秒对齐，别凭直觉数「头几次」**：判据是 `failures > 0 && failures < 3`，
门槛看的是**已经失败的次数**，不是尝试的序号：

| 尝试 | 时刻 | 用哪个窗口 |
|---|---|---|
| 第 1 次 | `t0` | 没有 `last_read_list_at` 可比，不过闸 |
| 第 2 次 | `t0+60s` | 快窗口（failures=1） |
| 第 3 次 | `t0+120s` | 快窗口（failures=2） |
| 第 4 次 | `t0+720s` | 长窗口（failures=3，不再 `< 3`） |

**一共只有 2 个快窗口、累计 2 分钟**，不是 3 个、3 分钟。
此后长期持续故障维持在 ≤144 次/天（86400 ÷ 600），**全天上界 146 次**。

### 4.5 key id 与导入判据

```rust
// keypool-repo.ts:247-250 —— key 的 id = SHA-256 的前 8 字节的 hex（16 字符）
fn key_id(key: &str) -> String { hex(&sha256(key.as_bytes())[..8]) }
// keypool-repo.ts:304-306
fn is_importable_key(key: &str) -> bool {
    key.len() <= MAX_KEY_LENGTH && !key.is_empty()
      && key.bytes().all(|b| (0x21..=0x7e).contains(&b))   // 可打印 ASCII 且不含空白
}
```

**`MAX_KEY_LENGTH = 1024` 的理由**（`keypool-repo.ts:252-260`）：JWT 形态的凭据也就几百字节，
而这个值同时是**存储那一侧的保险**——`FileStorage` 每次 put 都重写整个 `store.json`；
没有上限时一次粘贴就能把整份存储撑成几十 MB，此后每一次 key 池状态回写都要重写它。
**它挡的不是攻击，是一次手滑**（把整个日志文件粘进导入框）。

**`is_importable_key` 判据的物理依据**（`keypool-repo.ts:262-269`）：key 唯一的用途是拼进
`authorization: Bearer <key>`，而请求头的值是 ByteString ——码点 > U+00FF 与 NUL/CR/LF
在构造 Headers 时**直接抛 TypeError**（Rust 侧：`HeaderValue::from_str` 返回 `Err`）。
放进池子就是一把每次被选中都让转发炸掉的 key，而它看起来完全正常。

⚠️ **它不是 `ADMIN_TOKEN` 那个 `SENDABLE`（`[\x20-\x7e]*`）的第二份实现，两条规则真的不同**
（`keypool-repo.ts:271-281`）：那一条量词是 `*` 且**刻意放行内部空格**（passphrase 是有价值的口令形态）。
上游 key 是**机器发的 token，不是人记的口令**，中间出现空格只有一种可能——粘贴事故。
所以这里是 `+` 且不含空格。

🔴🔴 **同一个判据在两条路上处置不同，而这个不对称是刻意的**（`keypool-repo.ts:282-302`）：

| 路径 | 处置 | 理由 |
|---|---|---|
| **导入**（`add_many`） | **拒绝**（记进 `invalid`，报第 N 行） | 那串东西还在运维的剪贴板里，**拒绝是免费的** |
| **铸号**（`tender.rs` 的 `repo.add(out.key)`） | **照存不误**，另打一条 `registrar.minted_key_suspicious` 事件 + 在 `failures` 里记一条 `key_suspicious` | 那里拒绝是**销毁凭据**——Agnes 侧账号已真实建出、key 材料只有手上这一份，扔掉就再也找不回来，连对账都修不了 |

🔴 **如实登记的残留**：铸号那条路存进去的可疑 key **会进池子、会被 `select_key` 选中**，
每次被选中都会让转发失败，走 strike → 长冷却 → 再来一遍，**不会自己退出池子**；
而 `counts_toward_target` 是 `!evicted`，所以它还会一直占着一个 `target_keys` 名额。
**处置是人工的**（面板上停用/删除）。
**评估过、没有选的那条**：自动 evict 掉它 —— 那样「自动剔除 + 自动补铸」会变成一个
**自动烧邮箱名额**的循环，比一把占着名额的死 key 糟得多。

> **别看到 `is_importable_key` 就以为池子里每一把 key 都在门口被拦过。**

### 4.6 `Refreshable`（`refreshable.ts`，153 行）—— 快照缓存原语

> 「TTL + 失效 + 加载失败保留上一份合法快照」的最小原语。**零 IO**。
> 它同时是 `ConfigHolder`（配置）与 `KeyPoolRepo` 池快照的底座 ——
> 两者回答的是同一个问题「刚改完，什么时候能看见？」，**各写一份实现必然漂移出两套语义**。

```rust
struct Refreshable<T> {
    value: Option<T>,
    ever_loaded: bool,
    loaded_at: f64,        // 初值 f64::NEG_INFINITY（见下）
    in_flight: Option<SharedFuture>,
    gen: u64,              // 失效代数
}
```

**`ensure_fresh()`（`refreshable.ts:65-82`）—— 永不抛**：

```rust
let age = now() - loaded_at;
if ever_loaded && age >= 0.0 && age < ttl_ms { return; }   // 命中缓存
// 并发调用共享同一次加载：突发流量下不做这一步，N 个请求就是 N 次存储读取
if in_flight.is_none() { in_flight = Some(reload()) }
in_flight.await
```
⚠️ `age < 0` = **时钟回拨（NTP）**。不挡的话判据恒成立，这个 Refreshable 会冻结「回拨量 + 一个 TTL」那么久。
`loaded_at` 的初值取 `-∞` ⇒ `now - (-∞) = +∞`，`age >= 0` 成立、`age < ttl` 不成立 ⇒ 冷启动照常触发真加载。
**Rust 侧**：`loaded_at: Option<i64>`（`None` 当 `-∞` 处理）或直接用 `f64::NEG_INFINITY`。
**本仓有五处 `age < 0` 立刻恢复的同一套语义**：`Refreshable`、`list_on_read_path`、
`write_index_best_effort`、`pending_is_stale`、`WriteBudget::in_window`、`check_manual_tend` 的跨天判据
——**同一份代码库里不许有两套时钟语义。**

**`prime()`（`refreshable.ts:84-89`）—— 强制装载一次，失败会抛**。供启动时的 fail-closed 路径用。

**`invalidate()`（`refreshable.ts:91-100`）**：`loaded_at = -∞` + `gen += 1`。
🔴 **`gen` 不加的话 `invalidate()` 会被在途 reload 静默吃掉**（`refreshable.ts:32-45`）：
真存储上一次装载是一个网络往返（先取样、后交付），期间跑完的 `delete()` 已经把记录删掉、
`invalidate()` 也真调了，可紧接着落地的那次 reload 会把**删除之前**那份快照原样盖回来并推进 `loaded_at`
⇒ **已删的 key 在快照里活满一个 TTL**。对 `ConfigHolder` 就是「已撤销的口令再活一个 TTL」。
代数变了就**不推进 `loaded_at`**（`value` 照留——它仍是手上最新的一次取样）。

**`set(v)`（`refreshable.ts:102-128`）—— 写穿透**：
* **刻意不推进 `loaded_at`**：推进了就意味着一个持续写入的实例永远不会再去读存储，
  于是它永远看不到别的实例（或面板）的改动。
* 🔴 **刻意不递增 `gen`（与 `invalidate` 不同，这不是省略）**：写穿透最密集的时刻恰恰是上游出事的时刻
  ——429 突发 ⇒ 一批 key 同时进冷却 ⇒ 一批 `save()+set()`。递增代数就意味着每次写穿透都作废掉
  正在途中的那次装载并立刻重来，于是**在最需要缓存的时候引发 reload 风暴**。
  代价：在途 reload 落地时会盖掉刚写穿透的那份，该状态在**本实例的快照**里最多晚一个 TTL 才可见
  （**存储里始终是对的**）。根据是两者承诺强度不同：`invalidate()` 的「下一次一定重载」是**硬保证**，
  写穿透只是「我自己写的我立刻看得见」的尽力优化——只有硬保证值得付那个代价。

**`reload()`（`refreshable.ts:130-152`）**：

```rust
let gen = self.gen;                       // 取样代数
match load().await {
    Ok(v)  => { self.value = Some(v); self.ever_loaded = true;
                if gen == self.gen { self.loaded_at = now(); } }
    Err(e) => { let _ = catch_unwind(|| on_error(e));    // sink 自身故障不许影响主流程
                // **失败也推进计时，但只在已有兜底快照时**
                if self.ever_loaded && gen == self.gen { self.loaded_at = now(); } }
}
```
* **有兜底**：故障期每个请求都重试等于把存储打爆；代价是失败后最多再等一个 TTL 才重试。
* **无兜底（冷启动就失败）**：不推进，让下一次调用继续尝试——否则冷启动撞一次网络抖动就要空等满一个 TTL。
* **装载途中被 invalidate 过同样不推进**：面板刚改完就撞上一次读失败，不该因此再等满一个 TTL 才看见新值。

### 4.7 读路径：`all()` / `load_all()` / `list_on_read_path()`

**`all()`（`keypool-repo.ts:474-498`）**：

```rust
if cache_ttl_ms <= 0 { return self.load_all().await; }        // 关闭缓存
self.snapshot.ensure_fresh().await;
let cur = self.snapshot.current();
// ① 从未成功装载过（冷启动就撞上存储故障）：再走一次真加载把**真实异常**抛出来
if cur.is_none() { return self.load_all().await; }
// ② 空池态的 list 兜底失败了：**绝不许把「存储读不出来」伪装成「池子是空的」**
if cur.unwrap().is_empty() && self.last_read_list_error.is_some() {
    return Err(self.last_read_list_error.clone());
}
// ③ **浅拷贝**：dispatch 的 commit 会就地改数组元素，直接把缓存数组交出去等于让一次请求的
//    中间状态污染进程级缓存。（记录对象本身永不被就地修改 —— apply_* 全部返回新对象。）
Ok(cur.unwrap().to_vec())
```

* **①的代价要说清楚**（`keypool-repo.ts:481-487`）：这条分支下每个请求会读两遍存储（`ensure_fresh` 内部那次
  异常被吞掉了，这里再来一次），并多打一条 `pool.load_failed`。接受它的理由：这段时间里网关本来就在返 500、
  根本没有有效流量，而把**真实异常**（哪个文件、哪个键）交到运维手上比省一次注定失败的读重要得多。
* **②的理由**（`keypool-repo.ts:489-493`）：沿用 `[]` 的话调用方得到的是 503 `pool_empty` ——
  一条把运维指向「你还没导 key」的错误结论，而真相是存储故障，且此时 `reconcile_index()` 读同一个桶同样在抛，
  **文档指定的两条自愈路径都已经死了**。抛出去 ⇒ 500 + 真实原因。

**`load_all()`（`keypool-repo.ts:500-514`）—— 「一个字都不许精简」**：

```rust
let idx = self.read_index().await;                        // 抛错也当「缺失」
let indexed = match idx { Some(i) => i.ids,
                          None => self.bootstrap_from_list_throttled().await? };
let alive = self.load_records(&indexed).await?;
// 这一趟没经过空池兜底，上一次兜底失败的记忆就此作废（否则陈年失败会永远把 all() 钉在抛异常上）
if !alive.is_empty() || idx.is_none() { self.last_read_list_error = None; return Ok(alive); }
self.rescan_empty_result(&indexed).await
```

**`list_on_read_path()`（`keypool-repo.ts:516-557`）—— 读路径 `list()` 的唯一入口**：

```rust
let at = now();
let failures = self.consecutive_list_failures;
// **默认（0 次连续失败）用长退避** —— 这才是这扇闸真正要防的稳态
let backoff = if failures > 0 && failures < LIST_FAIL_ESCALATE_AFTER { LIST_FAIL_FAST_RETRY_MS }
              else { READ_PATH_LIST_BACKOFF_MS };
if let Some(last) = self.last_read_list_at {
    let since = at - last;
    if since >= 0 && since < backoff {
        // 窗口内若上一次是失败的，**如实抛**而不是返回 None
        if let Some(e) = &self.last_read_list_error { return Err(e.clone()); }
        return Ok(None);            // 「退避窗口内，这次不 list」
    }
}
self.last_read_list_at = Some(at);   // **窗口在发起尝试之前就打上**
match storage.list(KEY_PREFIX).await {
    Ok(names) => { self.consecutive_list_failures = 0; self.last_read_list_error = None; Ok(Some(names)) }
    Err(e)    => { self.consecutive_list_failures += 1; self.last_read_list_error = Some(e.clone()); Err(e) }
}
```

🔴 **空池兜底 `rescan_empty_result()`（`keypool-repo.ts:564-600`）—— 不加这一步会出人命的场景**：
全新部署上定时轮先在空存储上跑了一次对账，写下**权威的空索引** `{"v":1,"ids":[]}`；
用户随后按文档手工往 `store.json` 里塞 key（注册机默认关闭时手工导入就是装 key 的路径之一）。
此时索引「合法」，永远不会走缺失回落，`all()` 恒返回 0 条，**网关一直 503 `pool_empty`**。

```rust
let names = match self.list_on_read_path().await? { None => return Ok(vec![]), Some(n) => n };
let actual = ids_from_key_names(&names);
self.last_known_ids = actual.clone();
let merged: Vec<_> = indexed.iter().chain(actual.iter()).unique().collect();
if !same_id_set(&merged, indexed) {
    self.write_index_best_effort(&merged, "pool.index_backfilled",
        "索引说池子是空的，但 list 找到了记录（多半是手工导入），已把它们补进索引").await;
}
self.load_records(&actual).await
```
⚠️ **写回只增不减**：只把 `list` 发现而索引不知道的 id 补进去，**绝不从索引里删**。
剪枝一律交给 `reconcile_index()`。于是「整池都成了幽灵索引项」时这里不写、不剪，
只是每个退避窗口多一次 list，直到对账。

**`load_records(ids)`（`keypool-repo.ts:602-611`）**：并发 `get(KEY_PREFIX + id)`，`None` 被 filter 掉（幽灵索引项）。
⚠️ **刻意不顺手把它从索引里剪掉**，硬理由：剪枝要写索引，而那是**热路径上多出来的一次 put**
——写配额恰恰是最紧的桶，这一下就把本模块存在的全部意义抵消掉了。
次要理由：`add` 的写序天然存在「索引已更新、记录尚未可见」的窗口，读路径剪枝会把一把刚铸出来的新 key 打成孤儿。

**`bootstrap_from_list_throttled()`（`keypool-repo.ts:1154-1175`）**：
走同一道退避闸；**窗口内返回上一次已知的 id 清单**（本实例内存里的 `last_known_ids`），一条都没有过就返回空
——那时 `all()` 会走到空池兜底，兜底同样在窗口内 ⇒ `[]`，**语义与「池子是空的」一致，
而这正是索引缺失且 list 不可用时唯一诚实的答案**。拿到清单后 `write_index_best_effort` 尽力重建。

**`read_index()`（`keypool-repo.ts:1125-1152`）—— 读取本身抛错也一律当「索引缺失」，返回 `None`**：
这一层 try/catch **不是防御性编程，是补一条真实的防线漏洞**（评审用真 KV 实测）：
`parse_pool_index` 的「结构脏 ⇒ 当作缺失 ⇒ 重建」架在 **JSON 之上**，而存储层遇到坏字节是**先抛**的，
`parse` 根本没机会执行。后果是**每个转发请求 500，并且被指定为修复者的 `reconcile_index()`
读同一个键同样抛** —— 它每 30 分钟徒劳地挂一次，永远修不好，只能人工删键。
抛了就当缺失 ⇒ 调用方回落 `list()` 并**顺手把坏值覆盖掉**，自愈。
打一条 `pool.index_unreadable`（warn）。

**`write_index_best_effort()`（`keypool-repo.ts:1177-1209`）—— 尽力而为**：
存储只读时读路径本来是能工作的，不能因为写索引失败就把读路径也弄挂。
连续失败时按 `INDEX_WRITE_RETRY_MS` 退避。
⚠️ **成功之后刻意不把 `index_write_failed_at` 清回 `None`**：那行代码是死的，不是省略
——一次尝试能发生的前提就是 `at - failed_at >= RETRY`；时钟单调，之后每次调用只会让 `at` 更大。
时钟**不**单调那一种（NTP 回拨）显式挡了 `since < 0`。

### 4.8 写路径：`save()` 与 `pending_stats`

**`pending_stats`（`keypool-repo.ts:422-454`）**：

```rust
struct PendingStats { base: KeyStats, delta: StatsDelta, since: i64 }
// key = KeyRecord.id
pending_stats: HashMap<String, PendingStats>
```

**存在的理由（实测）**：成功路径上的写会被写消除整个丢弃，连带把计数丢掉，于是 `stats.requests` 永远停在 1。
把增量攒起来、在**下一次本来就要发生的那次写**上一起带下去，写配额一次不增，
误差收敛成「最多晚一个 `touch_interval_ms` 落盘 + 进程在落盘前退出时丢这一段」。

🔴 **为什么必须存 `base` 而不只是 `delta`（评审实测）**：只存 delta 时，落盘写的是
「调用方交上来的 `next.stats` + 攒着的」，而这**默认了调用方的视图不落后于存储**。
`dispatch` 的 `commit()` 恰恰打破这条：它 `records[at] = updated` 存的是**未合并的 next**，
于是同一次请求里第二次提交同一把 key 时，`prev` 比存储旧、`pending` 又已在第一次落盘时清空
⇒ 第二次写把已经落盘的累计**往回退**（**实测：21 次成功落盘后被抹成 1**）。
而「同一次请求里连着提交同一把 key」不是假想——池里只剩一把可用时 `select_key` 会在一次 dispatch 内反复选中它。

`since` 是当前这批增量**最早**那一条的时刻：只靠 `last_used_at` 的间隔判据的话，
上游 4xx 直通（既不改调度字段也不改 `last_used_at`）会让 `n - p == 0` 恒成立，
**一个只被打 4xx 的 key 的计数永远不会落盘**。

**四条不变量**（`keypool-repo.ts:446-452`）：计数不丢 / 写配额不增 / 删掉的 key 不被复活 / 已落盘的不被回退。

**`save(next, prev)`（`keypool-repo.ts:617-683`）**：

```rust
let at = now();
// ── 新建（add()）：prev = None，写的是一条**本来就不存在**的记录 ──
if prev.is_none() {
    // 上一世攒着的增量绝不许跟过来。裸存储删除之后没有任何后续 save、直接重新导入同一把 key 时，
    // 这一行是唯一的闸
    self.pending_stats.remove(&next.id);
    storage.put(KEY_PREFIX + &next.id, &next, None).await?;
    self.replace_in_snapshot(&next);
    return Ok(());
}
let prev = prev.unwrap();
// ── next.id != prev.id：写的是**另一个键** ──
if next.id != prev.id {
    if self.should_elide(prev, next) { return Ok(()); }   // id 是 scheduling ⇒ 恒为 false
    storage.put(KEY_PREFIX + &next.id, &next, None).await?;
    self.replace_in_snapshot(&next); return Ok(());
}
// ── 更新同一条记录 ──
let entry = self.track_baseline(&next.id, normalize_stats(&prev.stats), at);
let delta = stats_delta(&prev.stats, &next.stats);
// 攒得比一个触达间隔还久就别再消除了，否则只被打 4xx 的 key 永远落不了盘
if self.should_elide(prev, next) && !self.pending_is_stale(entry, at) {
    self.stash_pending(entry, delta, at);     // 计数不能跟着丢：攒起来
    return Ok(());
}
if !self.still_exists(&next.id).await? {
    self.pending_stats.remove(&next.id);      // 增量没有归属 —— 留着它就等于给复活留了一条路
    return Ok(());
}
// 落盘 = **本实例记的基线** + 先前被消除掉的那些 + 本次这一笔
// 刻意**不**用 next.stats 当基数：那是调用方的视图，它可能落后于存储
let stats = apply_delta(&entry.base, &add_delta(&entry.delta, &delta));
let merged = KeyRecord { stats: Some(stats.clone()), ..next.clone() };
storage.put(KEY_PREFIX + &merged.id, &merged, None).await?;
// **put 成功之后才推进基线**：put 抛错时增量必须留着，下一次再合并一遍
entry.base = stats; entry.delta = ZERO_DELTA; entry.since = at;
self.replace_in_snapshot(&merged);
```

**`prev` 的两个作用**（`keypool-repo.ts:617-627`）：① 写消除的对照；
② **区分「更新」与「新建」**——给了 prev 就意味着「我手上有一份旧的，要把它改成新的」，
于是要先确认那份旧的还在（`still_exists`）；`add()` 不给 prev，因为它写的是一条**本来就不存在**的记录。
**不给 prev 就一定落盘——写消除是可选优化，缺少对照时必须保守。**

**`track_baseline()`（`keypool-repo.ts:685-717`）—— 只在还没有未落盘增量时才校准基线**：

```rust
match self.pending_stats.get_mut(id) {
    None => { insert(PendingStats { base: seen, delta: ZERO_DELTA, since: at }) }
    Some(cur) => { if is_zero_delta(&cur.delta) { cur.base = max_stats(&cur.base, &seen); } }
}
```
🔴 **只能在 `delta` 为零时吸收**：`delta` 非零时 `seen` 里含着的正是**仍攒在 `delta` 里、尚未落盘**的那一段
（调用方每次都把 next 回写进自己的视图），把它顶成新基线 ⇒ 落盘写成 `base + 2×delta + 本次` ⇒ **计数虚高**。
**实测：2 次成功被消除 + 1 次失败落盘，真值 `requests:3 / success:2`，不加这个条件落盘写出 `5 / 4`。**
**虚高比少计更糟**：面板已经对用户承诺过「并发下会**少计**」，虚高是在说一句它自己不再保证的话。

**`pending_is_stale()`（`keypool-repo.ts:726-744`）**：

```rust
if touch_interval_ms <= 0 || is_zero_delta(&entry.delta) { return false; }
let age = at - entry.since;
age < 0 || age >= touch_interval_ms       // age < 0 = 时钟回拨，老实落盘
```
⚠️ **空增量一律不算陈旧**：落盘之后 `delta` 清零而条目留着（基线要留），`since` 却还停在旧时刻。
⚠️ **真正被它挡住的是「什么都没变」的 no-op save，不是「只有 `last_used_at` 在动」的那种**
——这句话第一版写反了，是实测订正的。完全 no-op 的 save：带它 **0 次 put**，去掉它 **2 次**。

**`still_exists(id)`（`keypool-repo.ts:746-793`）—— 「删除不许被陈旧写回撤销」这条不变量的全部实现**：

```rust
if storage.get(KEY_PREFIX + id).await?.is_some() { return Ok(true); }
logger.warn("pool.stale_write_dropped",
    "记录已被删除，丢弃这次基于陈旧快照的写回（删除不会被撤销），并立刻刷新快照", fields { id });
self.invalidate();      // 记录确实没了，而本实例的快照里还留着它 —— 不失效的话它会继续被选中整整一个 TTL
Ok(false)
```

🔴 **缺它时的失效链（评审实测复现，单副本 Docker 也中招）**：加了池快照缓存之后 `all()` 交出的是一份
最长 `pool_cache_ttl_ms` 的进程级快照，而运维吊销一把泄漏的 key 时若绕开面板、**直接编辑 `store.json`**，
记录没了，可本实例的快照里它还在：下一个请求照样选中它，上游一报错就 `apply_strike` ⇒ 调度字段变了
⇒ 无条件 put 把整条记录原样写回去 ⇒ **被吊销的凭据复活**。更糟的是它此后不会自愈。

**三条备选方案都评估过、都不选**（`keypool-repo.ts:760-769`）：
* **墓碑法**只护得住 `delete()` 这一条路径，而唯一存在的吊销姿势恰恰是**绕过 `delete()` 的裸存储删除**；
  何况墓碑里仍然躺着 key 材料，对「吊销一把泄漏的 key」是反效果；
* **对账不再收养孤儿**要求一份「已删 id」的持久集合，同样在裸存储删除下拿不到；而且孤儿收养这条路是
  `add()` 崩在①②之间时**唯一**的补救（那时 key 材料只在这条孤儿记录里）。为了修 A 去拆掉 B 的唯一救生索，不划算；
* **本条改在写侧**，因而对三种删除路径一视同仁，且不碰对账语义。

**代价（老实算）**：每次真正要落盘的更新多一次 `get`。稳态下调度字段变化很稀疏。
**残余风险（同样老实算）**：KV 的 `get` 走边缘缓存，刚删掉的记录在本 colo 可能还能被读到，
这一窗口内确认仍会通过。`FileStorage`（Docker）没有这层缓存，**那里是精确的**。

**`should_elide(prev, next)`（`keypool-repo.ts:795-808`）**：

```rust
if touch_interval_ms <= 0 { return false; }
if !only_elidable_changed(prev, next) { return false; }
// 从未用过（None）⇒ 差值是 +∞ ⇒ 首次使用一定落盘，否则面板永远显示「从未使用」
let p = prev.last_used_at.unwrap_or(i64::MIN);
let n = next.last_used_at.unwrap_or(i64::MIN);
if n < p { return false; }          // 时钟回拨：老实写，不当成「没变化」
n - p < touch_interval_ms
```
⚠️ **刻意不再单独判 `prev.id != next.id`**：`id` 不在白名单里，`only_elidable_changed` 已经比过它了。
**变异实测确认那一行永远改变不了结果**（去掉它全绿），而写一行永远改变不了行为的代码等于给后人留一条不可证伪的注释。

**`replace_in_snapshot(r)`（`keypool-repo.ts:810-819`）—— 写穿透**：
在当前快照里找到同 id 的那一项换掉；**不在快照里（刚 add 的）就什么都不做**，交给 `invalidate` 后的下一次刷新。

### 4.9 增删改：`add` / `add_many` / `delete` / `delete_many` / `reconcile_index`

**`new_record(id, key)`（`keypool-repo.ts:821-837`）—— `add` 与 `add_many` 共用这一份**：

```rust
KeyRecord { id, key, added_at: now(), last_used_at: None,
            cooldown_until: 0, cooldown_reason: None, strikes: 0,
            evicted: false, evicted_reason: None,
            stats: None, disabled: None, note: None }   // ← 三者刻意不写
```
⚠️ **刻意不写 `stats` / `disabled` / `note`**：不写它们产出的就是「存量记录」那种形状，
而那正是全仓读取处都必须能处理的形状。
⚠️ **抄两份的代价不是「不好看」**：给 `KeyRecord` 加一个必填字段时，编译器会在两处各报一次，
改对一处、另一处随手补个不同的默认值就成了**两种新 key**。

**`add(key)`（`keypool-repo.ts:839-864`）**：

```rust
let r = new_record(key_id(key), key);
let res = async {
    self.save(&r, None).await?;       // ① 记录先写
    self.index_add(&r.id).await?;     // ② 再进索引
    Ok(())
}.await;
self.invalidate();                    // **finally**：②失败留下的孤儿在「空结果兜底」路径上是看得见的
res?; Ok(r)
```
⚠️ **别把这次 `invalidate` 读成「补池刚铸出来的 key 下一个请求就能被选中」——生产上不成立**
（`keypool-repo.ts:849-857`）：那句话假设补池与转发用的是同一个实例，而实际接线是**两个**
（app 一个 TTL=`poolCacheTtlMs`，补池另一个 `cacheTtlMs: 0`）。
`Refreshable` 是实例私有状态 ⇒ **真实可见上界是转发进程自己的一个 `POOL_CACHE_TTL_MS`**。
这一行今天真实的用法是**面板**（它与转发路径共用同一个 repo 实例）。
**Rust 侧同理：`Arc<KeyPoolRepo>` 给谁、不给谁，是一次行为决定，不是接线细节。**

**`add_many(keys, reset_existing)`（`keypool-repo.ts:866-1020`）**：

形态：**一次读索引 → M 次存在性探测 → M 次记录 put → 一次写索引**。

🔴 **不许退化成循环调 `add()`**：`add()` 是「1 次记录 put + 1 次索引读 + 1 次索引 put」，
循环 M 次就是 `3M` 次操作；`FileStorage` 形态下每次 put 都重写整个 `store.json`，
**几百把 key 的导入能卡住整个进程**。（原仓判据：一次导入 20 把只产生 21 次 put，且零 list
——**循环版结果是对的，只是贵 3 倍，所以判据只能是计数**。）

```rust
let idx = self.read_index().await;
// 索引只用来**算这次要写回去的那份**，不参与重复判定
let known_ids = match &idx { Some(i) => i.ids.clone(), None => self.list_for_index_write().await? };
let (mut added, mut duplicated, mut invalid) = (vec![], vec![], vec![]);
let mut seen = HashSet::new();
for (at, raw) in keys.iter().enumerate() {
    let key = raw.trim();
    if key.is_empty() { continue; }                       // **空行整条跳过**
    if !is_importable_key(key) { invalid.push(at + 1); continue; }   // **1 基**，面板当行号用
    let id = key_id(key);
    if !seen.insert(id.clone()) { duplicated.push(id); continue; }
    match self.get(&id).await? {
        None => { added.push(id.clone()); self.save(&new_record(id, key), None).await?; }
        Some(existing) => {
            duplicated.push(id);
            if reset_existing {
                // **不传 prev**：传了就要过写消除那道闸，而重置一把本来就干净的 key 一个字段都没变
                // ⇒ 整个被丢弃，于是「被重置了几把」这个数字会与真正落盘的次数对不上
                self.save(&KeyRecord { cooldown_until: 0, cooldown_reason: None, strikes: 0,
                                       evicted: false, evicted_reason: None, ..existing }, None).await?;
            }
        }
    }
}
// ② 再进索引。**把 duplicated 也并进去**（顺手把孤儿捡回来）；正常情形集合不变 ⇒ 一次 put 都不写
let merged: Vec<_> = known_ids.iter().chain(added.iter()).chain(duplicated.iter()).cloned().collect();
if idx.is_none() || !same_id_set(&merged, &known_ids) {
    // **这一次必须抛**：记录已经写进去了，索引没跟上就是一批孤儿
    storage.put(POOL_INDEX_KEY, &make_pool_index(&merged), None).await?;
}
self.invalidate();     // finally
```

🔴 **「这把 key 在不在」的判据是记录本身（`get(id)`），不是索引**（`keypool-repo.ts:885-892`）：
拿索引当判据在两个方向上都会坏，而两次都坏在最要紧的时刻：
* **索引缺失/损坏**（多半正是写桶被打穿的时候）⇒ 已知集合是空的 ⇒
  **整池每一把都被当成新 key 覆盖一遍**，「重新导入即解封」那个后门从另一扇门原样回来；
* **幽灵索引项**（索引里有、记录已被裸存储删除）⇒ 那把 key 会被报成「已存在（未改动）」而其实
  **根本没有导进去**——运维看着成功提示，池子里没有。

**重复项默认跳过、不覆盖**（`keypool-repo.ts:879-883`）：`key_id` 是内容哈希，同一把 key 恒得同一个 id。
`add()` 直接 `save()` 覆盖 ⇒ 重复粘贴一次就把 `strikes` / `cooldown_until` / `evicted` 全清零、
`added_at` 刷新，**无任何提示** ⇒ 等于给用户一个「重新导入即可解封」的隐藏后门。

**`reset_existing: true` 是知情动作**（`keypool-repo.ts:894-904`），它重置的是**系统判定的失败态**
（`strikes` / `cooldown_until` / `cooldown_reason` / `evicted` / `evicted_reason`），
**刻意不动这四样**：
| 字段 | 不动的理由 |
|---|---|
| `disabled` | **人的决定**，随时可撤销。让一次粘贴顺手把它翻回去，就是把「停用一把可疑 key」这个安全动作静默撤销 |
| `added_at` | 重置状态不等于抹掉这把 key 的来历 |
| `stats` | 用量是历史，不是失败态 |
| `note` | 运维写给自己看的话 |

**勾了重置就是每把已存在的 key 各一次 put，刻意不做「没变化就不写」的优化**：
那会让「被重置了几把」与调用方拿到的 `duplicated` 对不上，而面板要拿这两个数说同一件事
——**一个动作两个数字，正是面板开始撒谎的方式。**

🔴 **返回值的三个数组类型不同，那不是随手写的，是口径本身**（`keypool-repo.ts:911-932`）：

```rust
struct AddManyResult { added: Vec<String>, duplicated: Vec<String>, invalid: Vec<usize> }
//                          ↑ id（内容哈希）      ↑ id              ↑ **输入里的位置（1 基）**
```
第一版三个都是 `string[]`，评审判定那会让下一个任务建在流沙上：类型完全相同、字段名又都是名词复数，
**前端没有任何东西能提醒它「这一个不是 id 数组」**。改成 `usize` 之后误读在类型上就成立不了
（JSON 那一侧同样看得见：`[2]` 不是 `["2"]`）。

**位置而不是掩码，两条理由，第二条更硬**：
① 导入框是**逐行粘贴**的，「第 2、7 行不是合法的 key」比「`sk-ab…tail` 不是合法的 key」更能让人直接去改那一行；
② 要掩码就得 import 面板投影模块 `key-view`，而那会让**调度侧的模块开始依赖面板投影模块**
（原仓有一格断言：`key-view.ts` 只服务管理读路径，`src/core` 里 `admin/` 之外没有任何模块 import 它）。
**返回位置之后，「明文出不去」变成了这个函数自己的性质：它手上根本没有一条通往明文的返回路径。**

**空行：整条跳过，不进任何一个数组**（`keypool-repo.ts:934-947`）。第一版把空行判成非法项，
那让前端只剩两个都不对的选项：原样发 ⇒ 文本框末尾那个换行被报成「第 N 行不合法」（一条**不存在的错误**）；
前端先过滤空行 ⇒ 位置与运维眼里的行号**错位**。
**现在：前端原样发（一行一个元素，空行也发），位置就是行号。**
计数关系：`added + duplicated + invalid + 空行数 == keys.len()`。
同一把 key 在同一批里粘两遍 ⇒ 第一条进 `added`（或 `duplicated`），第二条进 `duplicated`。

**`delete(id)`（`keypool-repo.ts:1022-1034`）**：

```rust
let res = async { storage.delete(KEY_PREFIX + id).await?;   // ① 记录先删
                  self.index_remove(id).await }.await;
// finally：
self.pending_stats.remove(id);   // 记录没了，攒着的增量没有归属（留着=给「重新导入时合并旧计数」留路）
self.invalidate();               // **必须放 finally**：②失败时不失效就等于「删了还在用」整整一个 TTL
res
```

**`delete_many(ids)`（`keypool-repo.ts:1036-1074`）—— 与 `add_many` 是同一条判据的另一半**：
一次读索引 → M 次记录 delete → 一次写索引。
🔴 **不许退化成循环调 `delete()`**，理由与批量导入逐字相同、**而且更常发生**：
`delete()` 每一次都要「读索引 + 写索引」，循环 N 次就是 **N 次索引 put**。
面板的批量条一次最多勾 200 行 ⇒ **一次点击 200 次 put**，而它换来的信息量与一次 put 完全相同。
**批量删除比 200 把 key 的导入常见得多** —— 只给导入那条路做了合并、把这条留在循环里，
正是「守住了罕见路径、放掉了常见路径」。

```rust
if ids.is_empty() { return Ok(()); }
let idx = self.read_index().await;
for id in ids { storage.delete(KEY_PREFIX + id).await?; self.pending_stats.remove(id); }
if let Some(idx) = idx {
    let rest: Vec<_> = idx.ids.iter().filter(|x| !ids.contains(x)).cloned().collect();
    if rest.len() != idx.ids.len() { storage.put(POOL_INDEX_KEY, &make_pool_index(&rest), None).await?; }
}
// finally: self.invalidate();
```

**`reconcile_index()`（`keypool-repo.ts:1076-1123`）—— 以 `list()` 的结果为准**：

```rust
let actual = ids_from_key_names(&storage.list(KEY_PREFIX).await?);
let cur = self.read_index().await;
let indexed = cur.as_ref().map(|i| i.ids.clone()).unwrap_or_default();
let added:   Vec<_> = actual.iter().filter(|x| !indexed.contains(x)).cloned().collect();
let removed: Vec<_> = indexed.iter().filter(|x| !actual.contains(x)).cloned().collect();
let repaired = cur.is_none() || !same_id_set(&indexed, &actual);
if repaired {
    storage.put(POOL_INDEX_KEY, &make_pool_index(&actual), None).await?;
    self.invalidate();     // 对账刚刚改变了「all() 会返回哪些 id」，与 add/delete 同类
    logger.log(if cur.is_none() { info "pool.index_bootstrapped" "key 池索引缺失，已按实际记录建立" }
               else             { warn "pool.index_repaired"    "key 池索引与实际记录不一致，已按实际记录修复" },
               fields { indexed: indexed.len(), actual: actual.len(), added: added.len(), removed: removed.len() });
}
ReconcileResult { indexed: indexed.len(), actual: actual.len(), repaired, added, removed }
```

**这是全系统除「索引缺失回落」之外唯一用 `list()` 的地方，且不在热路径上。**
调用者两处：① 定时轮每一轮开头（默认每 30 分钟）；② `tender` 的 `reconcile_after_mint()`
（**那一轮真的铸出了 key 才走**，面板的「立即补池」把它变成了一次点击的成本，
上界 `MANUAL_TENDS_PER_DAY = 24` 次/天）。

**`index_add(id)`（`keypool-repo.ts:1226-1254`）**：

```rust
match self.read_index().await {
    None => {
        // 索引整个缺失：add() 已经 save() 过这条记录，但**不能想当然地信这次 list() 一定读得到它**
        let ids = self.list_for_index_write().await?;    // ← 写路径专用：**不走读路径的退避闸**
        // 也不能沿用「ids 已含 id 就不写」那条短路：索引缺失时第一次 add() 必须无条件重建并写一次
        storage.put(POOL_INDEX_KEY, &make_pool_index(&union(ids, [id])), None).await?;   // **必须抛**
    }
    Some(idx) => {
        if idx.ids.contains(id) { return Ok(()); }       // 已在索引里：不写，省一次 put
        storage.put(POOL_INDEX_KEY, &make_pool_index(&append(idx.ids, id)), None).await?;  // **必须抛**
    }
}
```
🔴 **必须显式把 `id` 并进去，不能只信 `list()` 的结果**（`keypool-repo.ts:1235-1243`）：
`list()` 在 KV 上是最终一致的。评审用一个「`list()` 滞后一拍」的存储实测复现：
**全新部署第一次 `add()` 会写下一份不含这把新 key 的「权威」空索引，这把 key 就此隐身，
直到下一次 cron 对账（默认 30 分钟）才捡得回来。**

**`list_for_index_write()`（`keypool-repo.ts:1211-1224`）—— 写路径专用，不走退避闸**：
理由：`index_add` 只在**索引缺失**时才走到这里，而它的调用方是低频操作，且这次 list 的结果会立刻被写成索引。
**拿一份最多 10 分钟前的陈旧清单去写索引，会把窗口内别人加进来的 id 从索引里抹掉
——那是正确性问题，不是配额问题。**

**`index_remove(id)`（`keypool-repo.ts:1256-1261`）**：索引缺失/不可读 ⇒ 直接返回（对账会建）；
不在索引里 ⇒ 不写。

---

## 5. Tier-1 用量记账（`admin/stats.ts`，148 行）

```rust
const EMPTY_STATS: KeyStats = KeyStats { requests:0, success:0, failed:0, client_errors:0,
                                         last_error_at: None, last_error_kind: None };
struct StatsDelta { requests:u64, success:u64, failed:u64, client_errors:u64,
                    last_error_at: Option<i64>, last_error_kind: Option<String> }
const ZERO_DELTA: StatsDelta = /* 同上全零 */;
```

**`is_zero_delta(d)`（`stats.ts:16-21`）—— 按值判，不按引用判**（TS 里 `d === ZERO_DELTA` 是一行永远为假的死代码）：
`requests==0 && success==0 && failed==0 && client_errors==0 && last_error_at.is_none()`。

**`normalize_stats(raw)`（`stats.ts:23-44`）—— 逐字段补零而不是整块丢弃**：
整块丢弃会让一次坏写把已经攒了几天的计数清零，**而面板上看不出发生过这件事**。
数字规则：`if v.is_finite() && v >= 0 { trunc(v) } else { 0 }`；
`last_error_at` 只收有限数字，否则 `None`；`last_error_kind` 只收字符串，否则 `None`。
⚠️ 形参是 `unknown` **不是** `KeyStats | undefined`：真实输入来自 `JSON.parse`。
**写成后者就是在签名上撒谎**——证据是测试当时必须靠 `as unknown as KeyStats` 强转才写得出「存储被写坏」那几格，
而**需要强转才能表达的输入，正说明签名把它排除在外了**。
**Rust 侧**：`fn normalize_stats(raw: &Option<serde_json::Value>) -> KeyStats`。

**`max_stats(a, b)`（`stats.ts:46-74`）—— 逐字段取较大者**。计数只会涨，所以「取大」等价于「绝不回退」。
它是 `pending_stats.base` 唯一的更新方式，同时处理两个方向：
① 调用方交上来的 `prev` 比存储**旧**（`dispatch` 把未合并的 next 写回自己的视图）；
② 快照过 TTL 之后带回**别的副本写得更高**的值。**一个 `max` 同时处理这两个方向，且不需要知道是哪一种。**
⚠️ **它对 `last_error_kind` 不可交换，这是有意的**：判据是 `b.last_error_at > a.last_error_at`（**严格大于**），
所以两条错误**同毫秒**时保留 `a` 的 kind，`max_stats(x,y)` 与 `max_stats(y,x)` 会给出不同的 kind。
计数那四项是真正的 max（可交换），**只有这一项不是**。
取严格大于是因为「同毫秒」下没有任何依据判定谁更新，而稳定地保留左侧（=已有基线）比让顺序决定结果更可预期。

**`with_outcome(rec, outcome, now, err_kind)`（`stats.ts:76-101`）—— 记一次终态**：

```rust
let s = normalize_stats(&rec.stats);
KeyStats {
    requests: s.requests + 1,
    success:  s.success + (outcome == Success) as u64,
    failed:   s.failed  + (outcome == Failed) as u64,
    client_errors: s.client_errors + (outcome == ClientError) as u64,
    last_error_at:   if outcome == Failed { Some(now) } else { s.last_error_at },
    last_error_kind: if outcome == Failed { err_kind } else { s.last_error_kind },
}
```
⚠️ **只碰 `stats`，一个调度字段都不动** —— 它要在 `apply_success` / `apply_strike` / `apply_evict` 之后叠加。
⚠️ **`ClientError`（上游 4xx 直通）不记 `last_error`**：那是客户端发错了请求，
把它记成这把 key 的「最近错误」会让运维去查一把其实没问题的 key。

**`stats_delta(prev, next)`（`stats.ts:103-116`）—— 计数一律取非负差**：
快照比存储旧时算出负增量，合并回去就是把别人的计数抹掉。
`last_error_*` 的判据同 `max_stats`（严格大于才带过来，否则 `None`）。

**`add_delta(a, b)`（`stats.ts:118-128`）** / **`apply_delta(base, d)`（`stats.ts:130-142`）** / 
**`sum_stats(list)`（`stats.ts:144-148`）**：直加，`last_error_*` 同一条「严格大于」规则。

---

## 6. 协议层（`protocol/`，6 文件 953 行）

**基本形态**：四条对外协议 → 统一转成「内部规范请求」→ 一律 `dispatch({ path: "/chat/completions" })`
→ 上游是 **OpenAI 兼容体** → 再按对外协议转回去。

```rust
struct InternalRequest {                 // 四条协议 toInternalRequest 的公共产物
    model: String,
    messages: Vec<Message>,              // { role: String, content: String } —— **内部规范格式只有纯文本**
    max_tokens: Option<u32>,
    stream: bool,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stop: Option<Vec<String>>,
}
```

### 6.1 🔴 三档参数裁定表（`request-shape.ts:116-145`）—— **三条协议共用这一张**

上游是 OpenAI 兼容体，所以判据是「**这一格在 OpenAI chat 请求体里有没有同名同义的位置**」：

| 档 | 处置 | 成员 |
|---|---|---|
| **① 透传** | 映射成本一行 | `temperature`；`top_p`（Gemini 的 `topP`）；`stop`（Anthropic 的 `stop_sequences`、Gemini 的 `stopSequences`） |
| **② 400 —— 上游那份请求体里根本没有这一格** | `UnsupportedParamError(field, "上游的 OpenAI 兼容请求体里没有这一格")` | Anthropic 的 `top_k`；Gemini 的 `generationConfig.topK` |
| **③ 400 —— 传得过去，但会让「响应」变成本网关转换不了的形状** | `UnsupportedParamError(field, why)` | `tools` / `tool_choice`（Anthropic、Responses）、`toolConfig`（Gemini）：三条协议的工具 schema 与 OpenAI 的不同构，而三个响应转换器全都只读 `choices[0].message.content`，上游真回 `tool_calls` 的话下游拿到的是**空正文**；Gemini 的 `candidateCount > 1`（对应 OpenAI 的 `n`，而三条转换器只转第一条 candidate ⇒ **客户端付了 n 份的钱只拿得到一份**） |

⚠️ **① 档不做取值范围校验**：实测 `temperature: 99` / `top_p: 5` 上游照样 200。
**上游自己不判，网关这层替它判就是替上游说了一句它没说的话。**

⚠️ **`candidateCount: 1` 放行**：它与不传逐字等价，为它报 400 只是在骂人。

⚠️ **这张表是「今天做得到什么」，不是「客户端不该要什么」**：哪天真的实现了工具调用的双向转换，
`tools` 就该从 ③ 挪到 ①，**而不是**在这里悄悄放行。

🔴 **为什么必须 400 而不是静默丢掉**（`request-shape.ts:101-114`，2026-09-10 实测缺陷）：
从前三条协议把 `tools` / `tool_choice` / `top_k` 这些格**一个字都不往下带**，而同一台网关的
`/v1/chat/completions` 是整包透传。实测最难看的一幕：同一份带 `tools` + `tool_choice:"required"` 的请求，
走 OpenAI 那条上游返回**真的工具调用**（`finish_reason:"tool_calls"`、`arguments {"city":"Paris"}`），
走 `/v1/messages` 返回 **200**、正文是模型自己吐的 `<tool_call><tool_call>…` 乱码、
`stop_reason` 还写着 `end_turn` ——**网关一句提示都没有**。
⇒ **做不到就明说，别假装做到了。**

### 6.2 错误类型体系（`request-shape.ts:30-145`）

```rust
enum RequestShapeError {          // 路由层只需要**一个** catch —— 少一个 catch 就少一处会漏
    Invalid(String),              // InvalidRequestError（基类）
    UnsupportedContent { block_type: String },   // extends Invalid
    UnsupportedParam { field: String, why: String },  // extends Invalid
}
// 一律 400
```

🔴 **为什么 `InvalidRequestError` 值得单独立一个类**（`request-shape.ts:30-36`）：
**OpenAI 官方 SDK 对 5xx 默认自动重试（默认 2 次）**，于是一个**永远修不好**的客户端错误会被放大成 3 倍请求。
上游那层 CF 约 2 次快请求就回 1015，而**限流额度是整个网关共享的——一个客户端的 bug 能把所有人的通道打死。**

**它解决的实测缺陷**（`request-shape.ts:4-18`，2026-09-10 新加坡 Docker 验收发现）：

| 请求体 | 从前 | 现在 |
|---|---|---|
| `{"model":"…"}`（漏写 `messages`） | **500「网关内部错误」** | 400 说清哪个字段 |
| `[]` / `"hello"` / `null`（合法 JSON、非对象） | **500** | 400 |

🔴 **「漏写 messages」不是构造出来的畸形输入，是真实客户端很容易犯的错。**
用户看到「网关内部错误」会以为网关挂了来报障，而不是自己少传了字段。

**三个守卫函数**：

```rust
fn require_object(req: &Value, what: &str) -> Result<&Map>;   // request-shape.ts:50-55
//   null 与数组都要挡：typeof null === "object"，两者都能通过朴素判断然后在下一行属性访问上抛
//   报文：「{what}必须是一个 JSON 对象」
fn require_array(v: &Value, field: &str) -> Result<&Vec<Value>>;  // request-shape.ts:63-68
//   **空数组也拒**：messages: [] 送到上游只会被上游以另一种措辞拒掉，白烧一次共享的限流额度
//   报文：「{field} 必须是一个非空数组」
fn require_string(v: &Value, field: &str) -> Result<&str>;    // request-shape.ts:71-76
//   报文：「{field} 必须是一个非空字符串」
```

**`UnsupportedContentError`（`request-shape.ts:78-99`）—— 它从来就不是 Anthropic 专有的**：
内部规范格式只有纯文本，`image` / `inlineData` / `input_image` / `tool_use` 这些块无法无损转换。
原实现是**静默过滤掉非文本块**——客户端发了图片却得到一个只看了文字的回答，**既无从察觉也无从排查**。
⚠️ **实测（2026-09-10）**：同一张 8×8 的 PNG 走 `/v1beta/models/…:generateContent` 与 `/v1/responses`
都是 **HTTP 200 + 一段模型没看见图编出来的答案**（gemini 那条真打回来的正文逐字是 `NO_IMAGE_RECEIVED`），
而走 `/v1/messages` 是 400。**同一台网关对同一件事给三种行为**，而那两条给的正是被判定为「不可接受」的那一种。
报文：「不支持的内容块类型: {block_type}（本网关仅支持 text）」。
报文（param）：「不支持的生成参数: {field}（{why}）」。

### 6.3 SSE 原语（`protocol/sse.ts`，132 行）

**`frame_end(buf)`（`sse.ts:12-46`）—— 一帧的边界 = 一个空行，`\n\n` 与 `\r\n\r\n` 两种都认**：

```rust
fn frame_end(buf: &str) -> Option<(usize /*idx*/, usize /*len*/)> {
    let lf   = buf.find("\n\n");
    let crlf = buf.find("\r\n\r\n");
    match (lf, crlf) {
        (_, Some(c)) if lf.is_none() || c < lf.unwrap() => Some((c, 4)),
        (Some(l), _) => Some((l, 2)),
        _ => None,
    }
}
```
⚠️ **为什么必须认 CRLF**：网关自己发的是 `\n\n`，但**上游不归我们管**——上游或中间任何一层反代把行尾
改写成 CRLF 时，只认 `\n\n` 的话永远找不到边界，于是这条流会一路攒到上游关闭。
**失败形态是「退化成一次性 + 全流无界缓冲」，不是「一条都读不出来」**（实测），
而**中途断流时收尾那次 flush 根本不跑 ⇒ 已经该交出去的负载 100% 丢失。**

⚠️ **只认这两种，不认裸 `\r`**（EventSource 规范里 CR 单独也算换行）：本仓至今没有见过只发裸 CR 的上游，
而多认一种就要多一条没有真实样本支撑的分支。**如实登记，不假装它是完备的 SSE 实现。**

⚠️ **取两者里靠前的那一个**，不是「先找 LF 找不到再找 CRLF」：同一段缓冲里两种行尾混着出现
（换代理、断点续传）时，后者会把中间整段当成一帧，**帧边界就错位了**。

⚠️ **行内那个 `\r` 由既有的 `.trim()` 吃掉，不许再加第二次 trim** —— 那会把负载正文里合法的前后空白也一起吃掉。

**`extract_payloads(block)`（`sse.ts:1-10`）**：逐行；只看 `data:` 开头的；`line[5..].trim()`；
`== "[DONE]"` ⇒ 立刻返回 `done=true`（且已收集的照常交出）；空串跳过。

**`parse_sse_stream(body, signal)`（`sse.ts:48-101`）**：

```
reader = body.get_reader(); decoder = utf8_incremental; buf = ""
// signal 用来带外中断一次正阻塞在 read() 上的读取
loop {
    (done, value) = reader.read().await;  if done { break }
    buf += decoder.decode(value, stream=true);
    while let Some((idx, len)) = frame_end(&buf) {
        let block = buf[..idx]; buf = buf[idx+len..];
        let found = extract_payloads(block);
        for p in found.payloads { yield p }
        if found.done { return }
    }
}
// 流可能不以完整的空行结尾结束（连接中断、非 [DONE] 式终止、代理截断都可能发生）
buf += decoder.flush();
for p in extract_payloads(&buf).payloads { yield p }   // 否则最后一个事件会被静默丢弃
```

🔴 **`signal` 存在的理由（`sse.ts:48-61`）—— Rust 侧要读懂这条，因为它是 JS 特有病的说明**：
不能指望调用方对异步生成器调用 `.return()` 来打断它：`.return()` 会排在已经在飞行中的 `.next()` 后面
（AsyncGenerator 内部按 FIFO 处理请求），而这次 `.next()` 内部的 `await reader.read()` 在上游没有更多数据、
也没有关闭连接之前永远不会自己 resolve，于是 `.return()` **也永远轮不到执行**。
处置是**直接对 reader 调 `cancel()`**：按 Streams 规范它会让所有挂起中的 `read()` 立即以 `{done:true}` 结算，
并进一步中止底层的 HTTP 请求。
**Rust 侧**：用 `tokio::select!` + `CancellationToken` 或直接 `drop` 掉 `Response` 即可，
**这条 JS 特有的坑不必移植，但「客户端断连必须真的释放上游连接」这条语义必须保留。**

**`sse_event(event, data)`（`sse.ts:103-106`）**：
`format!("data: {}\n\n", json(data))`，`event` 非空时前面加 `event: {event}\n`。

**`to_sse_stream(gen, on_cancel)`（`sse.ts:108-132`）**：拉模式；`cancel` 时先同步调 `on_cancel()`，
再 `gen.return()`。Rust 侧对应 `futures::Stream` + `Drop` 时翻转 `CancellationToken`。

### 6.4 OpenAI（`protocol/openai.ts`，40 行）—— **最薄的一条**

**这条协议的请求体与响应体网关都不解析**（`protocol-catalog.ts:158-166`）：
路由不传 `expect_json` ⇒ `dispatch` 走 `sanitize(res)` **原样搬运**，从头到尾没有解析过。
usage 确实在响应里、确实到得了客户端（实测过），**只是网关没读它**。

**模型清单（`openai.ts:17-33`）—— 写死，不是运行时从上游拉回来的**：

```rust
const MODELS: [&str; 12] = [
    // 对话（**排在最前**：MODEL_CATALOG[0] 是验活 handler 与集成示例卡的默认模型）
    "agnes-2.0-flash", "agnes-2.5-flash", "agnes-2.5-pro",
    "agnes-2.5-pro-alpha", "agnes-2.5-pro-beta", "agnes-3.0-flash",
    // 图片
    "agnes-image-2.1-flash", "agnes-image-2.0-flash", "agnes-image-2.5-flash",
    // 视频
    "agnes-video-v2.0", "agnes-video-2.5", "agnes-video-2.5-flash",
];
fn model_list_response(created: i64) -> Value {   // openai.ts:35-40
    json!({ "object": "list", "data": MODELS.map(|id| json!({ "id": id, "object": "model", "created": created, "owned_by": "agnes2api" })) })
}
```
⚠️ **顺序不是随手排的**；⚠️ **必须与 `MODEL_CATALOG` 逐条同序一致**（加模型要两边一起加）。
⚠️ **写死是刻意的**：面板的模型表与 Playground 靠它做静态渲染；改成动态拉取要先回答
「上游不可达时画什么、缓不缓存」两个问题。上游此刻真有哪些模型是另一个问题，走 `upstream-models.rs` 的差集呈现。

### 6.5 Anthropic（`protocol/anthropic.ts`，194 行）

**请求整形（`anthropic.ts:49-68`）**：

```rust
let o = require_object(req, "请求体")?;
require_string(o.get("model"), "model")?;
require_array(o.get("messages"), "messages")?;
// 生成参数三档，**判在压平之前**：这一档是纯字段判断，比逐块压平便宜，也让报文只说一件事
if o.contains_key("tools")       { return Err(UnsupportedParam("tools", "本网关不转换工具调用")); }
if o.contains_key("tool_choice") { return Err(UnsupportedParam("tool_choice", "本网关不转换工具调用")); }
if o.contains_key("top_k")       { return Err(UnsupportedParam("top_k", "上游的 OpenAI 兼容请求体里没有这一格")); }
let mut messages = vec![];
let system = flatten(req.system)?;     // system 可能是 string **或内容块数组**
if !system.is_empty() { messages.push(Message { role: "system", content: system }); }
for m in req.messages { messages.push(Message { role: m.role, content: flatten(m.content)? }); }
InternalRequest { model, messages, max_tokens: req.max_tokens, stream: req.stream == Some(true),
                  temperature: req.temperature, top_p: req.top_p, stop: req.stop_sequences }
```

⚠️ **`system` 必须同时接受字符串与内容块数组**（`anthropic.ts:20-25`）：官方允许两种，
**所有开启 prompt caching 的 SDK 都是用数组形式发的**。原实现把它标成 string 并直接塞进 messages，
数组就会以裸数组的形式发给上游（content 不是字符串），上游只能报错。

**`flatten(content)`（`anthropic.ts:39-47`）**：字符串直接返回；数组逐块，
`p.type != "text"` ⇒ `UnsupportedContentError(p.type)`，否则拼 `p.text.unwrap_or("")`。

**`STOP_REASON`（`anthropic.ts:70-89`）—— 上游 `finish_reason` → Anthropic `stop_reason`**：

```rust
"stop" => "end_turn",  "length" => "max_tokens",  "content_filter" => "refusal",  _ => "end_turn"
```
⚠️ **`content_filter` 映到 `refusal`，不是 `stop_sequence`**（2026-09-10 实测缺陷）：
从前映成 `stop_sequence`，而本文件两条出口都把配套的 `stop_sequence` 字段写死成 `null`
——**一个说「命中了停止词」、一个说「没有停止词」，自相矛盾**。
后果是上游内容过滤/拒答被下游读成「正常因停止词结束」：**不触发重试、不触发告警，
做审计的下游把「被过滤」记成「正常结束」。**
⚠️ 而且经本网关「真的命中停止词」这件事**从来就发生不了**——`stop_sequences` 直到本轮才开始往上游转。

**非流式响应（`anthropic.ts:91-106`）**：

```json
{ "id": "msg_{openai.id ?? \"unknown\"}", "type": "message", "role": "assistant", "model": "<请求里的 model>",
  "content": [{ "type": "text", "text": "<choices[0].message.content ?? \"\">" }],
  "stop_reason": "<STOP_REASON 映射>", "stop_sequence": null,
  "usage": { "input_tokens": "<usage.prompt_tokens ?? 0>", "output_tokens": "<usage.completion_tokens ?? 0>" } }
```

**流式（`anthropic.ts:108-194`）—— 事件序列**：

```
message_start          ← 本地合成 messageId = "msg_" + uuid_v4
content_block_start    ← index 0, { type: "text", text: "" }
  （这两条**必须在读取上游之前产出**，否则客户端要等到上游有数据甚至结束才能看到第一个字节，
    这条流就退化成了「攒完再吐」——与本函数存在的意义相悖）
content_block_delta ×N ← { type: "text_delta", text: <上游 choices[0].delta.content> }（**非空才发**）
content_block_stop     ← index 0
message_delta          ← { delta: { stop_reason, stop_sequence: null }, usage: { input_tokens, output_tokens } }
message_stop
```

* **`message_start.message.usage` 里那两个 0 是「此刻还不知道」，不是「永远是 0」**（`anthropic.ts:133-139`）。
  真实值在收尾的 `message_delta.usage` 里给（**真 Anthropic 也是在那里给累计 output_tokens 的**）。
* 🔴 **`message_delta.usage` 必须是真实值，不是硬编码 0**（`anthropic.ts:184-188`，实测缺陷）：
  从前这里写死 0，而非流式路径证明这两个数网关明明拿得到 ⇒ 任何靠流式 usage 做计费/配额/统计的下游读到的
  永远是 0。**0 比缺字段更坏 —— 它长得像一个真值。**
  取值来源：上游把 usage 放在**末块**（实测：`{"choices":[{"delta":{}}],"usage":{...}}`，
  **不传 `stream_options.include_usage` 也照给**），边读边攒。
* 🔴 **上游中途断掉必须说出来**（`anthropic.ts:164-178`，实测缺陷）：从前这里没有 try，
  响应头一旦发出（`res.ok` 已为 true），上游中途报错/断流时生成器**静默退出**，照常补一个 `message_stop`
  并声称 `stop_reason: "end_turn"` ⇒ **客户端把「被截断」当成「正常说完」**。
  现在发一条 `error` 事件后 `return`：
  `{ "type": "error", "error": { "type": "api_error", "message": "上游流式响应中断，本次回答不完整" } }`
  ⚠️ **只报这一句，不回显 `err.message`**：它可能带上游 URL 与栈帧。
* 解析失败的上游块（`JSON.parse` 抛）⇒ `continue`（跳过这一行）。

### 6.6 Gemini（`protocol/gemini.ts`，226 行）

**Part 是按键区分的联合体**（`gemini.ts:8-13`）：带正文的那一支是 `{ text }`，其余是
`{ inlineData }` / `{ fileData }` / `{ functionCall }` / …

**`parts_text(parts)`（`gemini.ts:33-51`）**：
`if let Some(t) = p.text { out += t } else { return Err(UnsupportedContent(第一个非 text 的键名)) }`。
⚠️ **块类型取的是「除 text 之外的第一个键」**：Gemini 的 part 没有 `type` 格，
报文里要说得出是哪一种块，只能报键名。一个键都没有的空对象报 `"unknown"`。
⚠️ **不再求值成空串**（2026-09-10 实测缺陷）：从前是 `parts.map(p => p.text ?? "").join("")`，
`inlineData` / `fileData` / `functionCall` 统统蒸发。

**请求整形（`gemini.ts:55-88`）**：
`require_object` → `require_array(contents)` →
`tools` / `toolConfig` ⇒ ③ 档 400 → `generationConfig.topK` ⇒ ② 档 400 →
`generationConfig.candidateCount` 非 `None` 且 `!= 1` ⇒ ③ 档 400。
`systemInstruction` ⇒ `role: "system"`；`contents[].role == "model"` ⇒ `"assistant"`，缺席 ⇒ `"user"`。
`max_tokens = generationConfig.maxOutputTokens`，`temperature` / `top_p = topP` / `stop = stopSequences`。
⚠️ **model 从路径来（`/v1beta/models/{model}:generateContent`），不在请求体里。**

**`FINISH`（`gemini.ts:53`）**：`"stop" => "STOP"`，`"length" => "MAX_TOKENS"`，`"content_filter" => "SAFETY"`，`_ => "STOP"`。

**非流式响应（`gemini.ts:90-107`）**：

```json
{ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "<…>" }] },
                   "finishReason": "<FINISH 映射>", "index": 0 }],
  "modelVersion": "<model>",
  "usageMetadata": { "promptTokenCount": p, "candidatesTokenCount": c, "totalTokenCount": p + c } }
```

**流式（`gemini.ts:109-195`）—— 每个非空 delta 一帧，末尾一个终帧**：

```
（无 event 名，纯 data:）{ candidates:[{ content:{role:"model",parts:[{text}]}, index:0 }], modelVersion }  ×N
终帧：{ candidates:[{ content:{role:"model",parts:[]}, finishReason:<FINISH>, index:0 }],
        modelVersion, usageMetadata:{ promptTokenCount, candidatesTokenCount, totalTokenCount } }
```

🔴 **这条流从前没有终帧：`finishReason` 与 `usageMetadata` 一次都不出现**（`gemini.ts:115-137`，实测缺陷）。
官方 `google-genai` 2.22.0 回放线上真吐的那串字节（零上游请求）：`text` 完整，
而 `last.candidates[0].finish_reason` 与 `last.usage_metadata` **都是 `None`**。
⚠️ **`finishReason` 是客户端判断「说完了 / 撞了 MAX_TOKENS / 被 SAFETY 拦了」的唯一信号**——
少了它，一个被截断的半截回答和一个完整回答在客户端看来**逐字节不可区分**。
⚠️ **「上游没给」这条辩护是堵死的**：同一网关 `/v1/chat/completions` 原样透传的流式末尾真实字节里
含 `"finish_reason":"length"` 与 `"usage"` ——**上游在流里明确给了，是这条协议自己丢的。**

**断流那一档（`gemini.ts:163-177`）**：发一帧 `finishReason: "OTHER"` 的终帧（`parts: []`，无 usageMetadata）。
⚠️ **刻意不发 `promptFeedback.blockReason`**：`promptFeedback` 说的是「**这条提示词**被拦了」，
那是一句关于**请求**的断言，而这里发生的是**响应**中途断了。用它等于替上游说一句它没说过的话。
`OTHER` 是 Gemini 现行取值里「因为别的原因停下了」那一档，**不多说也不少说**。

**`GET /v1beta/models`（`gemini.ts:197-226`）**：

```json
{ "models": [ { "name": "models/<id>", "displayName": "<id>",
                "supportedGenerationMethods": <chat 才是 ["generateContent","streamGenerateContent"]，否则 []> } ] }
```
⚠️ **`supportedGenerationMethods` 按形态给**（2026-09-10 实测缺陷）：从前三个图片模型与三个视频模型
也被声明成支持那两条方法，而它们真正的端点是 `POST /v1/images/generations` 与 `POST /v1/videos` 的两段式。
它是 Gemini 协议里**机器可读**的「这个模型能干什么」，照着它渲染下拉框的客户端会把视频模型列成可对话模型；
运维选中发一次对话 ⇒ **白烧一把 key + 一次全网关共享的限流额度**。
⚠️ **媒体模型仍然列出来，只是方法为空数组，不是从清单里删掉**：
`/v1/models` 与 `/v1beta/models` 交的是同一份模型全集，做模型发现的客户端只认其中一条 ——
少列一个模型对那个客户端就是「这个模型不存在」。**空数组说的正是实话。**（12 这个数被判据钉着。）

### 6.7 Responses（`protocol/responses.ts`，216 行）

**`TEXT_PARTS`（`responses.ts:22-27`）**：只有 `input_text`（用户侧）与 `output_text`（把上一轮回答喂回去）带正文。
其余（`input_image` / `input_file` / `refusal` / …）一律 `UnsupportedContentError`（2026-09-10 实测缺陷，
从前 `input_image` / `input_file` **直接蒸发**，网关照回 200）。

**请求整形（`responses.ts:47-67`）**：
`require_object` → `require_string(model)` →
⚠️ **`input` 两种合法形态（字符串 / 非空数组）都要放行**，所以只在它不是字符串时才 `require_array` →
`tools` / `tool_choice` ⇒ ③ 档 400。
`instructions` ⇒ `role: "system"`；`input` 是字符串 ⇒ 单条 `role: "user"`；否则逐条。
`max_tokens = max_output_tokens`。

**非流式响应（`responses.ts:69-87`）**：

```json
{ "id": "resp_{openai.id}", "object": "response", "model": "<model>",
  "status": "<finish_reason == 'length' ? 'incomplete' : 'completed'>",
  "output": [{ "type": "message", "id": "msg_{openai.id}", "role": "assistant", "status": "completed",
               "content": [{ "type": "output_text", "text": "<…>", "annotations": [] }] }],
  "usage": { "input_tokens": i, "output_tokens": o, "total_tokens": i + o } }
```

**流式（`responses.ts:89-216`）—— 🔴 官方 SDK 能用的最小事件序列，一条都不许少**：

```
response.created           seq=0  { response: { id, object:"response", model, status:"in_progress", output: [] } }
response.output_item.added seq=1  { output_index: 0, item: { type:"message", id:<itemId>, status:"in_progress", role:"assistant", content: [] } }
response.content_part.added seq=2 { item_id, output_index:0, content_index:0, part: { type:"output_text", text:"", annotations:[] } }
response.output_text.delta ×N     { item_id, output_index:0, content_index:0, delta:<str>, logprobs: [] }
response.output_text.done         { item_id, output_index:0, content_index:0, text:<全文>, logprobs: [] }
response.content_part.done        { item_id, output_index:0, content_index:0, part: part(全文) }
response.output_item.done         { output_index:0, item: item("completed", [part(全文)]) }
response.completed                { response: { id, object:"response", model, status:"completed", output: [item("completed", [part(全文)])] } }
```
（每条都带 `type` 与**全流单调递增**的 `sequence_number`，本地自己数、不依赖上游。）

🔴 **缺陷定位（`responses.ts:102-137`，2026-09-10 本地回放定案，零上游请求）**：
从前 `gen()` 只发三类事件（`created` / 若干 `output_text.delta` / `completed`），delta 上没有 `item_id`，
`completed.response` 里**没有 `output`**。把线上真吐的那串字节原样喂给官方 `openai` 3.11.0：
* `client.responses.stream(...)` ⇒ **`IndexError: list index out of range`**，抛在
  `openai/lib/streaming/responses/_responses.py` 的 `output = snapshot.output[event.output_index]` 那一行，
  **栈里全是 openai 包的文件**——用户第一反应是「我 SDK 装坏了」，排查方向整个被带偏；
* 对照组 `client.responses.create(stream=True)` 裸迭代**是通的**。
⇒ **SDK 的累积器靠 `response.output_item.added` 建出 `snapshot.output[0]`、
靠 `response.content_part.added` 建出 `output.content[0]`，而网关一条都不发。**
而 `stream()` 正是 OpenAI 文档里 Responses 流式的推荐写法，也是 Agents SDK 一类框架的底座。

⚠️ **`sequence_number` / `item_id` / `logprobs` 三格不是装饰**：SDK 的 `handle_event()` 在 delta 与 done
两档上**直接读这三个属性**去重建它自己的事件，缺一个就是一次 `AttributeError`（同一次回放里实测过）。

⚠️ **`responseId` 与 `itemId` 是两个不同的 uuid**（`responses.ts:90-96`）：官方事件里 `item_id` 指的是
**这一项**，下游按它把增量归到某一项上。

⚠️⚠️ **`completed` 里仍然不带 `usage`，这是一条被裁定过的取舍、不是漏的**（`responses.ts:132-137`）：
官方 `Response.usage` 本来就是可选格，缺它不会让 SDK 崩。**要动它得先动那一段裁定，别在这里顺手加。**

**断流那一档（`responses.ts:174-192`）**：发 `response.failed` 之后**不再发 `completed`**，
官方 SDK 的 `get_final_response()` 会明确抛「没收到 completed」——**一次响亮的失败，好过一份看起来完整的残缺回答**。

```json
{ "type":"response.failed", "sequence_number":N,
  "response": { "id","object":"response","model","status":"failed",
                "output": [ item("incomplete", [part(已攒到的文本)]) ],
                "error": { "code":"upstream_error", "message":"上游流式响应中断，本次回答不完整" } } }
```
⚠️ **只报这一句，不回显异常的 message**（可能带上游 URL 与栈帧）。

---

## 7. 配置体系（`config.ts` / `config-provenance.ts` / `config-errors.ts` / `admin/config-validate.ts`）

### 7.1 优先级：**env > 存储 > 内置默认值**，唯一实现在 `load_config_with_provenance`

🔴 **不允许在面板层另写一套来源推导——那必然与 `load_config` 漂移**（`config-provenance.ts:15-30`）。
漂移的形态很具体：`num()` 里还有一条「存储值非法 ⇒ 字段级降级回默认值」的分支。
面板照着「有 env 就显示 env，否则显示存储」推一遍，**在那条降级分支上就会说假话**——
面板显示存储里那个非法值是「生效值」，而网关实际在用默认值。
**两份实现之间的差恰好落在最需要面板说实话的那一格上。**

⇒ `load_config()` 退化成薄封装，**唯一做的事是把 `source` 丢掉**（`config.ts:155-165`）：

```rust
async fn load_config(env, storage, logger, opts) -> Result<GatewayConfig> {
    Ok(load_config_with_provenance(env, storage, logger, opts).await?.config)
}
```
⚠️ **不许在这里补任何一条「顺手的」判断。** 只要这个函数体里出现第二条优先级规则，
面板（走 `source`）与网关（走这里）就有了两份实现。

### 7.2 `GatewayConfig` 与默认值（`config.ts:16-93`、`config-provenance.ts:257-273`）

```rust
struct GatewayConfig {
    gateway_token: String,           // 唯一 fail-closed 字段
    agnes_base_url: String,          // 默认 "https://apihub.agnes-ai.com/v1"
    upstream_timeout_ms: i64,        // 默认 8000       （首字节）
    upstream_sync_timeout_ms: i64,   // 默认 120_000    （同步端点，见 §3.5）
    max_strikes: u32,                // 默认 3
    cooldown_rate_limit_ms: i64,     // 默认 60_000     （429）
    cooldown_payment_ms: i64,        // 默认 3_600_000  （402）
    cooldown_strike_ms: i64,         // 默认 1_800_000  （strike 满）
    pool_cache_ttl_ms: i64,          // 默认 60_000     （0 = 关；**建 app 时读一次**）
    pool_touch_interval_ms: i64,     // 默认 21_600_000 （0 = 关；**建 app 时读一次**）
    registrar: RegistrarConfig,
    degraded: bool,                  // **装载的产物，不是旋钮**
    usage_stats_enabled: bool,       // 默认 false；**建 app 时读一次**
}
```
⚠️ `upstream_sync_timeout_ms = 120_000` 的理由（`config-provenance.ts:265-266`）：实测图片生成约 12 秒；
留出「慢 key 比快 key 慢 3~4 倍」的余量，取 2 分钟。**它只在同步端点上生效，不影响对话的快速甩慢 key。**

**`degraded`（`config.ts:45-61`）—— 本次装载有没有降级**。它有一条完整的三级消费链
（响应体 → 前端窄化成 `bool | null` → `=== true` 时显示红色横幅）。
🔴 **「保存了却没生效」是这个项目最高频的用户困惑，不让它可见就等于让面板撒谎。**

**`usage_stats_enabled`（`config.ts:62-92`）—— 默认 false，且「关」必须是零成本**（全局约束）：
关闭时**不建累加器、不挂中间件、一次 `storage.put` 都没有**。
两条理由（**都在 v0.4.0 换过说法**）：
① `FileStorage` 每次 put 都**重写整个 `store.json`**，多一条写路径就是实打实的磁盘写放大，
   而它与 key 池的状态回写写的是同一个文件、走同一条写队列；
② sink 构造时 `last_flush_at = now()`，**存活不足一个落盘间隔（默认 2 小时）的进程一个字都存不下**
   ——频繁重启、崩溃重启循环、或者一天里滚动更新好几次的部署都够得着。
   **开着它会得到一份看起来完整、实际残缺的数字。**
⚠️ **它不在 `EDITABLE`（面板改不了），但仍然在 `ENV_LOCK_MAP` 里**，两件事各有各的理由：
* 不进 `EDITABLE`：设置页本期没有它的入口——进了就会得到一份「说能改、却没有任何地方能改」的字段清单；
* 进 `ENV_LOCK_MAP`：它**存储里就能改**。不进表的话 `GET /admin/api/config` 会对这个字段返回一份
  **自相矛盾**的四元组（`stored: true` / `env: null` / `effective: false`），**那是撒谎**，与「今天有没有消费者」无关。

**布尔字段的判据一律是 `== "true"`**（`config.ts:120-124`、`config-provenance.ts:549-555`），
**不是 `!!` 也不是「非空即真」**：`USAGE_STATS_ENABLED=0` / `=false` 必须是关，
那是运维写下「我不要它」时最自然的两种写法。
⚠️ **布尔不走 `num()`，所以没有「存储值非法 ⇒ 字段级降级」这一支**：布尔没有非法值，
`String(非布尔) != "true"` 一律落到 false，**那正是「默认关」该有的失败方向**（关是零成本，读不懂时必须往关的方向倒）。

### 7.3 `num()` —— 数值字段的取值 + 降级（`config-provenance.ts:277-347`）

```rust
fn num(env, env_name, field_name, stored: Option<&Value>, fallback: i64,
       logger, min: i64 /* 默认 1 */, flags: Option<&mut Flags>) -> Result<i64, ConfigRefusal> {
    if let Some(raw) = env.get(env_name) {
        let n = parse_number(raw);
        // **环境变量的非法值继续 fail-fast**：那是部署时错误，运维必须立刻看得见，
        // 而且它不可能是面板写坏的——面板永远碰不到环境变量
        if !is_int_at_least(n, min) {
            return Err(ConfigRefusal(format!("环境变量 {env_name} 必须是不小于 {min} 的整数: {raw}")));
        }
        return Ok(n);
    }
    if let Some(s) = stored.filter(|v| !v.is_null()) {
        if !is_int_at_least(s, min) {
            // **存储的非法值改为字段级降级**：抛错的后果是进程 exit(1) 进入重启循环，
            // 而且**没有面板可以进去改回来**；一次误操作把网关砖掉是不可接受的
            logger.warn("config.invalid", "存储中的配置值非法，本字段回落到默认值",
                        fields { field: field_name, source: "stored", raw, fallback });
            if let Some(f) = flags { f.degraded = true; }
            return Ok(fallback);
        }
        return Ok(s);
    }
    Ok(fallback)
}
fn is_int_at_least(n, min) -> bool { n.is_integer() && n >= min }
```

**`min` 默认 1 的理由**（`config-provenance.ts:294-300`）：`UPSTREAM_TIMEOUT_MS=-1` 会让超时立刻触发，
一个请求就能把整池打成 strike；**`MAX_STRIKES=0` 更糟**——`strikes >= max_strikes` 在第一次失败时即成立
（`1 >= 0`），**等于跳过了整个容错机制**。故只校验「是有限数」是不够的。
**只有 `pool_cache_ttl_ms` / `pool_touch_interval_ms` 传 `min = 0`**：对它们来说 0 是「关闭」这个有意义的取值，
是用户的逃生口。

⚠️⚠️ **注册机子树的 `pos_int()` 对 env 侧的非法值降级不抛，与这里正相反，那是刻意的例外**
（`config-provenance.ts:314-319`、`registrar/config.ts:144-157`）。
**判据不是「哪种更严格」，而是这个子系统有没有安全的降能模式**：
注册机停跑 = 池子不再自动补，转发照常；而这里这些旋钮（超时、strike 阈值、冷却时长）**一个都关不掉**
——回落默认值就是**静默改掉转发行为**，那比拒绝启动更坏。
**⇒ 两边的不一致是有理由的取舍，不是疏漏。要改先推翻这条理由。**

### 7.4 `degradeOnUnreadable` —— 两个调用身份要相反的行为（`config.ts:132-153`）

| 调用身份 | 传什么 | 理由 |
|---|---|---|
| `ConfigHolder` 的 `Refreshable::load`（热实例每 TTL 一次） | **`false`（严格：如实抛）** | 这条路径上 `Refreshable::reload()` 本来就有**严格更好**的兜底：抛错时原样保留上一份合法快照 |
| `create_config_holder` 的 `prime()`（冷启动）+ 入口的 `build_tend_deps` | **`true`** | 这里没有「上一份合法快照」可退，抛出去的后果是**进程直接起不来（容器重启循环）**，而 `GATEWAY_TOKEN` 通常就在环境变量里，存储那份只是覆盖层 |

🔴 **热路径传 true 的后果（评审实测复现）**：一次瞬时读抖动会把面板保存的配置**静默**换成内置默认值
——而且不是几十秒的抖动，是剩下的一整天：`max_strikes` 9→3、`cooldown_strike_ms` 7,777,000→1,800,000、
**`registrar.enabled` true→false（补池被静默关掉）**。

### 7.5 唯一保留的 fail-closed：`gateway_token`（`config-provenance.ts:521-526`）

```rust
let gateway_token = env.GATEWAY_TOKEN.or(as_string(stored.gatewayToken));
if gateway_token.is_none() { return Err(ConfigRefusal("缺少 GATEWAY_TOKEN，网关无法启动")); }
```
**message 逐字不动**：入口的 `main().catch` 打的就是它，五语言部署文档的故障排查条目引的也是这句原文。

**`ConfigRefusal` 这个专用异常**（`config-errors.ts:113-136`）：它与「配置有问题」不是一回事。
逃出 `build_app` 的**非** `ConfigRefusal` 异常按定义就是代码 bug。
⚠️ v0.4.0 之后只剩一个入口，而且它不分流：`main().catch` 打 `err.message` + `exit(1)`，两类异常同样让进程起不来
——**那是 Docker 形态正确的 fail-fast，一个字都不该改**（容器起不来是运维立刻看得见的信号）。
**它今天还剩什么用**：注册机那几条端点靠它把「运维配错」与「代码 bug」分开报；
`load_config_with_provenance` 的字段级降级也靠它区分「拒绝服务」与「降级继续跑」。
⚠️ **它不进任何未鉴权响应体。** 未鉴权路径上回的 `reason` 一律是**固定枚举串**，永不由 `err.message` 派生
——**配置细节一个字节都不到未鉴权调用方。**

### 7.6 逃生口 `RESET_CONFIG=1`（`config-provenance.ts:494-500`）

存储里的 config 键被写坏到连降级都救不回来时，`RESET_CONFIG=1` **完全忽略它**。
**只忽略不删**——删了用户就再也拿不回原值了。

### 7.7 存储读不出来的处置（`config-provenance.ts:501-516`）

```rust
match storage.get(CONFIG_KEY).await {
    Ok(v) => stored_raw = v.unwrap_or(json!({})),
    Err(e) => {
        if !degrade_on_unreadable { return Err(e); }     // 热路径：如实抛
        storage_unreadable = true;                        // 冷启动：降级到 env + 默认值
        logger.error("config.storage_unreadable",
            "读取存储中的 config 键失败，本次装载只用环境变量与内置默认值（面板里保存的配置本次不生效）",
            fields { err });
    }
}
let flags = Flags { degraded: storage_unreadable };
```
**与「存储非法值字段级降级」同一条原则**：存储**读不出来**比存储值非法更轻，不该得到更重的处置。

### 7.8 `FIELD_EXPOSURE` —— 哪些字段是凭据（`config-provenance.ts:79-135`）

🔴 **标错一格 = 明文出网。**

```
gatewayToken: secret          agnesBaseUrl: public
upstreamTimeoutMs / upstreamSyncTimeoutMs / maxStrikes / cooldown*Ms
  / poolCacheTtlMs / poolTouchIntervalMs / degraded / usageStatsEnabled: public
registrar.{enabled, channel, targetKeys, mintBatch, tendIntervalMs, codeTimeoutMs,
           mintDelayMinMs, mintDelayMaxMs, maxDomainAttempts, tokenName,
           agnesPlatformUrl, blocked}: public
registrar.yyds:    { baseUrl: public, apiKey: secret }
registrar.moemail: { baseUrl: public, apiKey: secret }
```

⚠️ **`registrar.agnesPlatformUrl` 是 public，但它是「注册凭据的去向」**（`config-provenance.ts:122-126`）：
改成自己的服务器就能**收走每次注册的邮箱 + 密码 + 验证码**。所以它照常走四元组，
但在面板上必须折进「高级」折叠区 + 红色警告 + 二次确认，不放主表单。

⚠️ **这张表的边界，明写，不许被读成过头**（`config-provenance.ts:85-96`）：
**它保证「有人做了一个决定」，不保证「那个决定是对的」。** 把一把新密钥标成 `public` 照样编译通过、照样泄漏。
**⇒「新增配置字段时确认 `FIELD_EXPOSURE` 那一格」是一条点名勾选的评审项。**
⚠️ **不要用关键词启发式（`/token|key|secret/i`）去兜底**——那是一张手写词表，正是本仓已登记为脆弱的判据。

⚠️ **一条已知盲点（`config-provenance.ts:66-71`）**：TS 里数组类型也满足 `extends object`，会被当成「对象字段」递归。
`GatewayConfig` 今天没有数组字段；将来加了要补一条分支。
**这正是 `registrarBlocked` / `registrarNotices` 不进 `GatewayConfig` 的理由**——
进 `GatewayConfig` 的只有 `registrar.blocked` 这一格**标量**。
**Rust 侧**：类型系统不同，但「每个叶子必须显式表态」这条纪律照旧——用一张手写的
`const FIELD_EXPOSURE: &[(&str, Exposure)]` + 一格「配置结构体字段数与表条数相等」的单测。

### 7.9 `ENV_LOCK_MAP` —— 「面板改了也不生效」的唯一依据（`config-provenance.ts:137-205`）

26 条映射（env 变量名 → 面板字段路径）：

```
GATEWAY_TOKEN→gatewayToken            AGNES_BASE_URL→agnesBaseUrl
UPSTREAM_TIMEOUT_MS→upstreamTimeoutMs UPSTREAM_SYNC_TIMEOUT_MS→upstreamSyncTimeoutMs
MAX_STRIKES→maxStrikes                COOLDOWN_RATE_LIMIT_MS→cooldownRateLimitMs
COOLDOWN_PAYMENT_MS→cooldownPaymentMs COOLDOWN_STRIKE_MS→cooldownStrikeMs
POOL_CACHE_TTL_MS→poolCacheTtlMs      POOL_TOUCH_INTERVAL_MS→poolTouchIntervalMs
USAGE_STATS_ENABLED→usageStatsEnabled
REGISTRAR_ENABLED→registrar.enabled
REGISTRAR_CHANNEL→registrar.channel   REGISTRAR_PRIMARY→registrar.channel   ← **两个名字映到同一路径**
TARGET_KEYS→registrar.targetKeys      MINT_BATCH→registrar.mintBatch
TEND_INTERVAL_MS→registrar.tendIntervalMs   CODE_TIMEOUT_MS→registrar.codeTimeoutMs
MINT_DELAY_MIN_MS→registrar.mintDelayMinMs  MINT_DELAY_MAX_MS→registrar.mintDelayMaxMs
MAX_DOMAIN_ATTEMPTS→registrar.maxDomainAttempts
REGISTRAR_TOKEN_NAME→registrar.tokenName    AGNES_PLATFORM_URL→registrar.agnesPlatformUrl
YYDS_BASE_URL→registrar.yyds.baseUrl        YYDS_API_KEY→registrar.yyds.apiKey
MOEMAIL_BASE_URL→registrar.moemail.baseUrl  MOEMAIL_API_KEY→registrar.moemail.apiKey
```

⚠️ **`REGISTRAR_FALLBACK` 刻意不在这张表里**（`config-provenance.ts:188-190`）：它已经不锁任何字段，
留着就是让面板对一个不存在的字段声称「被环境变量锁定」。装载器仍然读它（为了产出一条迁移提示），
**「被读到」与「锁定了某个字段」是两件事。**

**少了注册机那批名字的后果很具体**（`config-provenance.ts:143-149`）：`docker-compose` 里写了 `TARGET_KEYS=30`，
面板上那一格既不置灰也不说明，运维改成 20、**保存成功、生效值永远是 30，重启也不会好**。

**`env_locked_fields(env)`（`config-provenance.ts:242-255`）**：
判据是「**这个键在 env 里存在**」，不是「值合法」——`MAX_STRIKES=` 这种空串同样会走进 `num()` 的 env 分支
（然后抛错），面板必须显示它是锁定的，否则用户会一直改存储、一直没效果。**结果要去重**。

🔴 **逆表必须存数组，不是 `Object.fromEntries`**（`config-provenance.ts:207-224`）：
两个 env 名映到同一字段路径时，逆表会塌成「后写的赢」——于是一台设了 `REGISTRAR_PRIMARY` 的部署
会被面板告知「是 `REGISTRAR_CHANNEL` 锁的」，**运维去 compose 里 grep 什么都搜不到**。
`env_name_for_field(field, env)`（`config-provenance.ts:226-240`）：按 **env 里实际存在的那一个**挑，
都不存在时报第一个（正式名）。

### 7.10 `FieldSource` 四元组 / 二元组（`config-provenance.ts:349-381`、`:558-586`）

```rust
enum FieldSource {
    Public { stored: Value,            // 存储里那个 config 键在这条路径上的**原始值**，没有窄化、没有默认值兜底
             env: Option<String>,      // 环境变量的**原始字符串**
             effective: Value,         // 本次装载真正在用的值
             locked_by: Option<String> /* "env:<NAME>" */ },
    Secret { configured: bool, hint: Option<String>, locked_by: Option<String> },
}
```

🔴 **凭据字段绝不走四元组**（`config-provenance.ts:356-360`）：照字面实现「每个字段四元组」
就是把 `gatewayToken` 的明文放进 `stored` 与 `effective` 两个字段。
设计花整节论证 `ADMIN_TOKEN` 绝不能等于 `GATEWAY_TOKEN`，全部理由是
**「不让管理面变成网关口令的 reveal 端点」——四元组照抄就是把那个端点自己造出来。**

**`hint`（`config-provenance.ts:448-456`）—— 末 4 位，短于 5 位一律 `None`**：
`hint` 存在的意义是让运维认出「我配的是哪一把」，而对一把 4 位的口令，末 4 位就是全部
——**那不叫提示，那叫 reveal**。

⚠️ **`configured` 读的是「存储原件 + env」，不是生效模型**（`config-provenance.ts:565-570`）：
`registrar_from_env` 的结尾在关着时让 `yyds` / `moemail` 恒为 `null` ——照生效模型报的话，
用户「**先填凭据、后开开关**」这个最自然的顺序下，面板会说「未配置」，**逼他再填一遍**。

**路径清单从 `FIELD_EXPOSURE` 走出来，不另写一份**（`config-provenance.ts:386-392`、`:458-476`）：
判「是不是叶子」的依据是**值是不是字符串**（`"public"` / `"secret"`），不是「路径深度」
——后者要么写死层数、要么再手写一份清单，两条都会漂。

### 7.11 写入侧校验 `validate_config_patch`（`admin/config-validate.ts`，697 行）

**它今天的身份已经退了一档**（`config-validate.ts:14-34`）：装载器全函数化之后它**不再是**
「防止面板把网关砖掉」的唯一防线，而是 ① **写入侧预判**（别让人保存一份注册机跑不起来的配置）
+ ② **面板文案的码源**。整条装载路径今天只剩**一个**抛点：两边都没有 `gatewayToken`。

**为什么规则不与 `load_config` 共用一份代码**（`config-validate.ts:36-59`）：试过「干跑一次装载，抛了就是非法」，
它更省代码，但**给不出逐字段错误码**（今天更甚：装载器压根不抛了，干跑什么都测不出来）。
设计要的是 `400 { errors: [{ field, code, params }] }` —— **逐字段、机器可读、能映射五语言**。
⇒ **取舍明写：规则在这里是第二份实现，代价是它可能与装载器漂移。**
用两件事把代价压住：① 可编辑字段清单与 `FIELD_EXPOSURE` 逐条对账；
② 一格「防漂：validate 放行的，装载器必须装载得起来」+ 一格「双向等价：`config_load_blockers` 的
`field:code` 集合恒等于装载器的 `blockers`」。

**`patch` 用扁平点分路径，不是嵌套对象**（`config-validate.ts:205-214`）：
「缺席 = 不改」这条语义**只有扁平形态表达得清楚**——嵌套形态下 `{ registrar: { targetKeys: 5 } }`
到底是「只改 targetKeys」还是「把整个 registrar 换成只有一个字段的对象」，取决于合并规则写在哪一层，
而两种读法都有人会按 —— **一次读错就是把用户的两条通道凭据整段抹掉。**

**`EDITABLE` 表（`config-validate.ts:97-146`）**，26 条，`Spec` 五种：

```rust
enum Spec { Int { min: i64 }, Url, Text, Bool, ChannelOrNull, Secret }
gatewayToken: Secret                       agnesBaseUrl: Url
upstreamTimeoutMs / upstreamSyncTimeoutMs / maxStrikes / cooldown*Ms: Int{min:1}
poolCacheTtlMs / poolTouchIntervalMs: Int{min:0}       // **0 = 关闭，是逃生口不是越界值**
registrar.enabled: Bool                    registrar.channel: ChannelOrNull
registrar.{targetKeys,mintBatch,tendIntervalMs,codeTimeoutMs,
           mintDelayMinMs,mintDelayMaxMs,maxDomainAttempts}: Int{min:1}
registrar.tokenName: Text                  registrar.agnesPlatformUrl: Url
registrar.yyds.baseUrl: Url                registrar.yyds.apiKey: Secret
registrar.moemail.baseUrl: Url             registrar.moemail.apiKey: Secret
```
⚠️ **`degraded` 不在这里，这是有意的**：它是**装载的产物**，不是旋钮。
把它做成可写会让面板能「把红色横幅关掉」，**而横幅要报告的那件事一点没变**。

**流程（`config-validate.ts:256-408`）**：

```
1. patch 不是对象 ⇒ 400 [{ field: "", code: "unknown_field" }]
2. 逐字段：
   a. 不在 EDITABLE ⇒ unknown_field（**拼错的字段名一律 400，不静默丢弃**：
      `{maxStrikess: 9}` 在宽松实现下是一次「保存成功、什么都没发生」）
   b. 被 env 锁定 ⇒ locked_by_env { env: <按 env 里实际存在的名字报> }
      ⚠️ **拒绝而不是「写下去但不生效」**：写下去的后果是面板显示保存成功、四元组里 stored 真的变了、
         而 effective 纹丝不动 —— 运维会以为是缓存没刷，去等那 90 秒，然后再等一次
   c. Secret：见下
   d. 其余走 check_leaf
3. 逐字段有错 ⇒ **不跑跨字段规则**（一个 targetKeys:"abc" 会让「min 不大于 max」拿到无意义的操作数）
4. 跨字段阶段：**只拒这次补丁新引入的 blocker**（见下）
5. 顺手规整旧的主备键（migrate_stored_registrar），**这两个路径不许进 changed**
6. ok { next: <整份要写回存储的 config>, changed: <排序后的路径清单> }
```

**Secret 分支（`config-validate.ts:301-347`）—— 顺序：空白 → 字符集 → 长度 → 相同性**：

```rust
if value.is_none() { continue; }                       // 缺席 = 不改
if !value.is_string() { err(not_a_string); continue; }
let token = value.trim();
if token.is_empty() { continue; }                      // **空串 = 不改（不是清空）**
if value != token { err(whitespace_padded); continue; }
if !SENDABLE.is_match(value) { err(not_sendable); continue; }   // SENDABLE = ^[\x20-\x7e]*$
if field == "gatewayToken" {
    if value.len() < MIN_GATEWAY_TOKEN_LENGTH /* 24 */ { err(too_short { min: 24 }); continue; }
    if Some(value) == ctx.admin_token { err(same_as_admin_token); continue; }
}
```
🔴 **判据是 `trim() == ""` 而不是 `== ""`**（`config-validate.ts:309-313`，评审发现）：一次「粘了几个空格」的误操作
在 `== ""` 下会被**收下并落盘**，而 `if (!gatewayToken)` 对 `"   "` 为**真**（非空字符串）⇒ 一次都不 fail-fast，
面板还显示 `configured: true`。**实测：`PUT {"gatewayToken":"   "} → 200`、落盘 `"   "`、
原口令被抹掉、所有下游用户从此 401。**
🔴 **`whitespace_padded`**：HTTP 头值在传输层被 trim，而存储里不会 ⇒ 带空白的口令客户端**永远送不出来**，
症状是「口令明明是对的却一直 401」。**而首尾空格在面板上根本渲染不出来。**
🔴 **`same_as_admin_token`**：把它写成相等 ⇒ `adminAuth` 的每请求复查立刻把整个管理面判成 503，
**而改回去的那条 PUT 也是 503 ⇒ 面板把自己锁死**（实测过）。
**`MIN_GATEWAY_TOKEN_LENGTH = 24`**（`config-validate.ts:69-81`）：与 `ADMIN_TOKEN_MIN_LENGTH` 同一个数、
理由逐字相同——**本网关没有任何登录限速，因此口令熵就是唯一的防线，下限不是建议值**。
⚠️ **只对 `gatewayToken` 生效，不对两条通道的 `apiKey`**：那两把是**上游签发**的，长度不由本网关决定。

**`check_leaf`（`config-validate.ts:415-445`）**：

```
Int  : 不是整数 ⇒ not_an_integer；< min ⇒ below_min { min }
Url  : 非字符串 ⇒ not_a_string；空串 ⇒ empty；非 http(s) ⇒ not_a_url
Text : 非字符串 ⇒ not_a_string；空串 ⇒ empty；> MAX_TEXT_LENGTH(200) ⇒ too_long { max }
Bool : 非布尔 ⇒ not_a_boolean
ChannelOrNull : null 合法（「不选」是正当取值）；不是 yyds/moemail ⇒ not_a_channel
```
**`is_http_url`（`config-validate.ts:230-245`）—— 不用宽松的 URL 解析直接放行**：
标准 URL 解析认 `javascript:` 与 `file:`，而这几个字段全都会被拿去发请求
（`agnesBaseUrl` 是转发目标、`agnesPlatformUrl` 是**注册凭据的去向**、两条 `baseUrl` 是邮箱服务）。
**必须显式检查 `scheme` 是 `http` 或 `https`。**

🔴 **跨字段阶段：只拒这次补丁「新引入」的 blocker**（`config-validate.ts:359-383`）：

```rust
let before: HashSet<String> = config_load_blockers(&stored, env).iter().map(|b| format!("{}:{}", b.field, b.code)).collect();
errors.extend(config_load_blockers(&next, env).into_iter().filter(|b| !before.contains(&format!("{}:{}", b.field, b.code))));
```
**判据是「补丁前后两份 blocker 清单的差」，而不是「补丁之后还有没有 blocker」。**
第一版拿的是后者 ⇒ 那份配置**本来就**坏掉时，运维想改一个无关字段（`PUT {maxStrikes: 7}`）会被 400 拒
——而他手上正拿着一份装不起来的配置，**最需要的恰恰是一步一步修回来**。
⚠️ **它不会把砖机场景放回来**：本来就存在的 blocker 维持原样（没有新增伤害），
而**任何一条新引入的**（把注册机打开却不填凭据、把最后一把网关口令清掉）照旧当场 400。
⚠️ **诚实限定**：`config_load_blockers` 本身不完备，所以这道闸也随之不完备。

**`config_load_blockers(stored, env)`（`config-validate.ts:447-496`）**：
`gatewayToken` 两边都没有 ⇒ `gateway_token_required`；再加 `cross_field_errors`。

🔴 **它是两条评审发现的收口点**（`config-validate.ts:450-462`）：在它之前，
`configClearSecretHandler` 里只有一条 `nowMissing` 而且**只判 `gatewayToken`**；
同构的「清掉一条在链上的通道凭据」一个字都没写 ⇒ 那条路径上 `put` 先发生、随后回读抛、被兜成 **500**：
**面板说「保存失败」，而那把凭据已经被删掉了。**
更坏的是**没有出路**（实测）：清完之后 `PUT` 关掉注册机 / 重新填这把 key / 换主通道**全是 500**，
`GET /admin/api/config` 也是 500，**而干跑 validate 回 200** ——干跑说「你这个补丁合法」，真跑 500。
⇒ **仍然不许拿「blockers 为空」当「装得起来」的判据**。正确的判据是「**存储读得出来吗**」。

**`cross_field_errors`（`config-validate.ts:498-558`）—— 每一条都对应装载器产出的一条 blocker**：

```
① mintDelayMin > mintDelayMax ⇒ delay_min_gt_max { min, max }
   ⚠️ **这一条不受 enabled 门控**（与装载器逐字一致）
   ⚠️ **比的是生效值，不是存储原件**：原判据「两边都是 number 且 min > max」漏掉了两整类：
      (a) env 里只给了 MINT_DELAY_MIN_MS、存储缺席（生效 max 走内置取值）；
      (b) 存储里写了非法值（生效值是**默认值**，不是那个非法值）
② 以下受 enabled 门控（enabled 判据：env.REGISTRAR_ENABLED 缺席才看 stored.enabled === true）：
   通道值写错了 ⇒ not_a_channel { raw }     **else if** 压根没选 ⇒ channel_required
      （**写成 else if 是有意的**：两句不同的话同时说出来只会让运维以为有两处要改）
   **只对选中的那一条产 blocker**：
      moemail 且 baseUrl 取不到 ⇒ channel_credentials_missing { channel }
      apiKey 取不到 ⇒ channel_credentials_missing { channel }
      （YYDS 的 baseUrl 有内置取值、MoeMail 没有 —— 这是两条通道之间**唯一**的不对称，
        而它是一句事实：一条是地址固定的公共服务，一条是自建服务，**不是排名**）
```
**取值规则 `pick(env_raw, stored_raw)`（`config-validate.ts:564-571`）—— 与装载器逐字同形**：
「env 优先、**空串算没写**」。`MOEMAIL_API_KEY=` 这种写法在 compose 里极常见，
把它当成「配了一把空 key」会让两边给出不同答案。
**通道候选四级，顺序即优先级**（`config-validate.ts:573-597`）：
`REGISTRAR_CHANNEL > REGISTRAR_PRIMARY > registrar.channel > registrar.primary`，
**某一级存在但值非法就到此为止，不往下穿透**（否则新名字写错会静默落到旧名字上）。
⚠️ **不能借道 `pick()`**：那个函数把「非字符串」也归成「没写」，而存储里的 `channel: 123`
是**写了个不认识的值**（装载器对它报 `not_a_channel`），两者的文案与处置都不同。

**`clear_secret(stored, field)`（`config-validate.ts:667-684`）—— 显式清空一把凭据**：
只允许 `SECRET_FIELDS` 里那三把；返回删掉那条路径之后的整份 config。
⚠️⚠️ **清掉 `gatewayToken` 而环境变量里也没有时，下一次冷启动会 fail-closed**，
而已经在跑的实例因为 `Refreshable` 保留上一份合法快照**不会当场停摆**——
**所以这件事在面板上是看不见的，直到下一次重启/回收。**
**本模块不拦它**：这是一条显式动作（专门的端点 + 面板二次确认 + 红色警告），
而「把存储里那把泄漏的口令删掉」是一个正当且必须存在的能力。
拦住它的代价是：**环境变量提供口令的部署（最常见的形态）想清掉存储里那份多余的旧口令时无路可走。**

### 7.12 `CONFIG_ERROR_CODES`（`config-errors.ts:35-111`）—— 21 条，**数组是真源、类型是派生**

```
unknown_field / locked_by_env / not_an_integer / below_min / not_a_string / not_a_boolean
empty / too_long / not_a_url / not_a_channel / channel_required / delay_min_gt_max
channel_credentials_missing
— 下面三条**不是错误，是不拦人的通知**（RegistrarLoad.notices）—
legacy_channel_key / legacy_channel_env / legacy_fallback_ignored
gateway_token_required / whitespace_padded / not_sendable / too_short
same_as_admin_token / config_unloadable
```

⚠️⚠️ **第一版是手写联合 + 测试里一份 `satisfies` 镜像，那条护栏实测是假的**（`config-errors.ts:38-49`）：
`satisfies` 只做**单向可赋值检查**——它保证镜像里每一项都是合法的码，**不保证每一个码都在镜像里**。
给联合加一个新码而不补文案 ⇒ `tsc exit=0`、测试 34 passed、i18n 检查 exit=0，**零信号**；
而反向（从联合里删一个）确实报错。**删得住、加不住。**
后果是：后端加错误码 ⇒ 面板查不到文案 ⇒ **把裸码显示给运维**。
⇒ 改成**数组是真源、类型是派生**，测试直接遍历数组。
**Rust 侧**：用 `strum::EnumIter` 或一个 `const ALL: &[ConfigErrorCode]` + 一格遍历它的单测。

⚠️ **那三条 notice 进这张表，是因为面板选文案走的是同一条路**（`config-errors.ts:69-75`）。
**不许因此把它们当 blocker 用**：blocker 的意思是「注册机本次不启动」，这三条恰恰配着「注册机照常跑」。

🔴 **`legacy_fallback_ignored` 的文案必须点名到具体通道，并说清最咬人的场景**（`config-errors.ts:80-86`）：
一台部署的主通道凭据早已失效、**一直靠备通道在铸 key**，升级后产出会归零，而面板每一格都显示「已配置」。
**文案一旦写软成「备通道已弃用」，这就变成一次静默的生产事故。**

**`config_unloadable`（`config-errors.ts:97-108`）—— 「这份配置构造不出来，但说不出是哪一格」**：
⚠️ 它的射程在装载器全函数化之后收窄了一大截。保留它是因为「说不出哪一格」这件事本身必须有一个如实的表达，
**不编一个具体字段出来**；具体原因走事件板块。

---

## 8. admin 子系统（`admin/`，14 文件 3,906 行）

### 8.0 ⚠️⚠️ 本仓有两个都叫 key 的东西，方向相反，别读混（`api-keys.ts:6-13`）

| | 上游 key 池 | 对外 API 密钥 |
|---|---|---|
| 谁的 | **我们持有的**，拿去向 Agnes 证明身份 | **我们签发的**，别人拿来向我们证明身份 |
| 存储 | `key:<id>` 逐条 + `pool:index` | 单个顶层键 `apikeys` |
| 端点 | `/admin/api/keys` | `/admin/api/apikeys` |
| 明文 | **必须存**（要拿去用） | 见下（原本不存，后被用户拍板改成存） |

**路径刻意不共用**：`/admin/api/keys` 已经是上游池的公开契约，改它等于破坏已发布的 API。
代价是两个名字很像，**缓解手段是文案与文档，不是去改既有路径**。

### 8.1 对外 API 密钥（`admin/api-keys.ts`，459 行）

```rust
const APIKEY_SECRET_PREFIX: &str = "sk-";   // api-keys.ts:40 —— **我们自己发的常量，不是从明文里存下来的一段**
const API_KEY_SECRET_BYTES: usize = 16;     // api-keys.ts:50  = 128 bit
const API_KEY_HINT_LENGTH: usize = 4;       // api-keys.ts:53
const APIKEY_MAX: usize = 200;              // api-keys.ts:64
const API_KEY_NAME_MAX: usize = 64;         // api-keys.ts:67

struct ApiKeyRecord {                       // api-keys.ts:78-162 —— **七个字段**
    id: String,            // 12 位十六进制，**独立随机、不由密钥派生**
    name: String,          // 允许重名
    secret: Option<String>,// 🔴 明文（见下）
    hash: String,          // 明文的 SHA-256，64 位十六进制小写 —— 验证依据
    hint: String,          // 明文末 4 位
    disabled: Option<bool>,// 缺席 = 启用
    created_at: i64,
    expires_at: Option<i64>,  // None = 不过期
}
struct ApiKeyTable { version: u64, keys: Vec<ApiKeyRecord> }   // api-keys.ts:164-181，version 从 0 起
```

**`API_KEY_SECRET_BYTES = 16` 的理由**（`api-keys.ts:42-50`）：它是**均匀随机**的（不是人选的口令），
128 位在离线爆破面前已经远超任何可行算力，而每多 16 字节就是每一条 `Authorization` 头多 32 个字符。
**这个数同时决定了「无盐单轮 SHA-256 是正确选择」那条论证的前提，改小它之前先读那段。**

**`APIKEY_MAX = 200`**（`api-keys.ts:55-64`）：整张表是**一个**存储值，每条记录约 200 B ⇒ 200 把 ≈ 40 KB。
再大就该考虑分片，**而分片会把「一次刷新 = 1 次 get」这条本设计最要紧的性质破坏掉**。
**超了就 400，不静默截断。**

🔴🔴 **`secret` 字段：一次由用户拍板的、以安全性换便利性的取舍（2026-09-10）**（`api-keys.ts:93-111`）。
本仓从第一天起的姿态是「我们签发、只用来验证别人的这一族**根本不需要**可逆存」，
那条论证**在技术上依然成立**。改成存明文之后：
* **面板一旦被打穿，所有客户端密钥的明文一次性泄漏**（从前泄漏的只是摘要，而 128 位均匀随机 + SHA-256 是不可反推的）；
* 存储介质从「不含可直接使用的凭据」变成「含」，**备份与快照的处置级别要跟着升**。
用户已被明确告知这两点并坚持要这个功能，**这里如实记下代价，不粉饰**。
⚠️ **可选字段，别当成必然存在**：升级之前签发的记录**没有**这一格——它们的明文在当初那次 201 响应之后
就不存在于世界上了，任何代码都变不出来。面板对这一档要照实说「此密钥签发于升级前，无明文可显示」，
**不许用空串或掩码冒充明文**。

**`hash` —— 为什么是无盐、单轮 SHA-256，而不是 bcrypt / scrypt**（`api-keys.ts:129-139`）：
**这不是偷懒，是这一族的正确选择，别把「密码要用慢哈希」这条常识套上来。**
① 密钥是 128 位均匀随机（我们生成的，不是人选的口令）⇒ **没有字典可爆破**，慢哈希想抵抗的那件事在这里不存在；
② 慢哈希**不可查表**：每请求都要拿明文对全表逐条算一遍，200 把就是 200 次 KDF ——
   它跑在转发热路径上、与四条协议抢同一个事件循环，**几十毫秒的同步 CPU 是不可接受的**。
   单轮摘要则是**算一次、查一次表**；
③ 无盐是可查表的前提，而它在这里不损失什么：盐防的是「同一个口令在两处的摘要相同」，
   而这一族里两把密钥相同的概率是 2⁻¹²⁸。
**Rust 侧**：`sha2::Sha256`，**不许改用 argon2/bcrypt**，除非先推翻上面三条。

**`id` 不用自增号**（`api-keys.ts:79-89`）：自增要在表里维护一个 `next_id`，而那个数一旦被覆写就会**退号**，
退号 = **新密钥继承前任的身份**（用量桶、事件日志里的那一行）。随机 id 天然不复用、零状态。
**代价是没有 `#001` 这种好念的编号** ⇒ 面板上的行号由 `seq` 现算，**而 `seq` 不是身份**。
**不由密钥派生**是安全要求：id 会进 URL、进事件日志，那些地方不该携带密钥的任何函数值。

**`version` 是乐观并发，不是 CAS**（`api-keys.ts:164-176`）：写之前回读一次（那一次 get 本来就要付），
调用方手上的版本与读回来的对不上 ⇒ **409**。
⚠️ **它不是 CAS。** 存储没有 CAS，读到写之间仍有一个窗口；这条机制把窗口从「整个人类操作时长」
压到「一次读写之间」，**残余窗口确实存在，别在任何一处把它说成「不会丢更新」。**

**纯函数清单**：

```rust
fn digest(secret: &str) -> String;                       // api-keys.ts:183-190 —— **全仓唯一一份**
fn hint_of(secret: &str) -> String;                      // api-keys.ts:192-195 —— 末 4 位
fn mask_of(hint: Option<&str>) -> String;                // api-keys.ts:197-207
//   `sk-••••••••` + 末四位；**hint 读不出来时画一根破折号「—」，不画一串光秃秃的圆点**
//   （一串圆点会被读成「配了一把很短的口令」，而真相是「这条记录里那一格坏了」）
//   ⚠️ **不复用 key-view 的 mask_key()**：那个要前 5 位，而我们**不存前 5 位**，也不打算存
fn is_api_key_disabled(r) -> bool { r.disabled == Some(true) }   // api-keys.ts:209-212
fn is_api_key_expired(r, now) -> bool { r.expires_at.map_or(false, |e| now >= e) }  // api-keys.ts:214-220
//   **边界取 `now >= expires_at`**：那一刻本身已经在期外
fn is_api_key_usable(r, now) -> bool { !disabled && !expired }   // api-keys.ts:222-225 —— 鉴权路径唯一判据
```

**分档（`api-keys.ts:227-241`）—— 顺序即优先级：`disabled > expired > active`**。
**停用排在过期前面**：停用是**人做过的一个决定**，而过期是时间到了。
一把既被停用又已过期的密钥在面板上该说「已停用」——那是运维要找的那条线索；
说「已过期」会让他以为只要把日期往后挪就能恢复。

🔴 **`find_by_digest`（`api-keys.ts:243-267`）—— 常数时间比较，且遍历全表不在命中处提前 return**：

```rust
fn find_by_digest<'a>(keys: &'a [ApiKeyRecord], d: &str) -> Option<&'a ApiKeyRecord> {
    let mut found = None;
    for r in keys {
        let hit = constant_time_equal(&r.hash, d);
        found = if hit { Some(r) } else { found };   // 三元是**赋值**不是短路控制流
    }
    found
}
```
两条性质分别挡两维泄漏：
* 摘要本身已经抹平「与某把密钥前多少字节相同」那一维；
* **表内位置**那一维靠「不短路」挡：在命中处 return 会让耗时随「命中的是第几条」变化，
  于是**一个手里已经有一把有效密钥的人能量出别人那把排在自己前面还是后面**。
⚠️ 循环里出现任何 `break` / `return` / `&&` / `||` / `?:`（作为控制流）都是回归。
**Rust 侧**：不许用 `.iter().find()`；必须是全量循环 + 无分支赋值。

**`constant_time_equal`（`admin/constant-time.ts:1-35`）**：

```rust
fn constant_time_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() { return false; }      // 长度本身会泄漏，这是标准取舍
    let mut diff = 0u32;
    for (x, y) in a.bytes().zip(b.bytes()) { diff |= (x ^ y) as u32; }
    diff == 0
}
```
⚠️⚠️ **常数时间这个性质无法用单元测试证明，原因是它根本不在返回值里**（`constant-time.ts:8-19`）：
被测的性质是「**耗时**不随输入而变」，而测试只能断言返回值。把整个函数换成 `a == b`、
或把循环体改成提前 return，**返回值逐点相同**（已实测：两条变异全套测试照样绿）。
不写计时断言是因为在 CI 上测不准（纳秒量级 vs 毫秒量级噪声，**写出来只会是一条随机红绿的用例，那比没有更糟**）。
**⇒ 这一条明确不由测试保证，而由评审保证。**
长度泄漏是标准取舍：被它比较的两侧都是随机串（管理口令 ≥24 位、摘要恒 64 位十六进制）。
**Rust 侧建议**：用 `subtle::ConstantTimeEq`，但**保留「长度不同立刻 false」这条**（与 TS 逐字一致）。

**视图与查询**：

```rust
struct ApiKeyView { id, name, seq: usize, masked: String, hint: String,
                    bucket: ApiKeyBucket, disabled: bool /* **恒是布尔、恒存在** */,
                    created_at: i64, expires_at: Option<i64> }
// api-keys.ts:293-314 —— 排序：created_at 升序、id 破平；**返回顺序必须与 seq 一致**
//   （否则面板上会出现 #7 / #2 / #19）
fn api_key_counts(views) -> { all, active, disabled, expired }   // api-keys.ts:316-331
//   **判据只看 v.bucket**，不再各自重算一遍 —— 重算就是同一条优先级规则的第二份实现
//   ⚠️ **没有 pending 那一档**（kiro2api 那边有）：惰性激活本期不做，**画一张恒为 0 的卡就是撒谎**
fn matches_api_key_query(v, q) -> bool   // api-keys.ts:333-345
//   **只匹配名称、掩码的可见部分与 id，绝不匹配摘要**
//   匹配摘要等于把「没有 reveal 端点」这条保证降级成一个慢速预言机
const API_KEY_SORTS: [&str; 3] = ["new", "old", "name"];   // api-keys.ts:347-356
fn sort_api_key_views(views, sort)   // api-keys.ts:358-370
//   **new / old 一律拿 seq 破平**：同一毫秒签发的两把在两次刷新之间必须落在同一个位置
```

⚠️ **`ApiKeyRecord` 只有七个字段；kiro2api 那边是十一个，砍掉的四个各有各的理由**（`api-keys.ts:69-76`）：
`spendingLimit` / `limitUnit` 做不出跨实例正确的上限；`durationDays` / `activatedAt` 的惰性激活要**在鉴权热路径上写存储**；
`boundCredentialIds` 在本仓**没有对应概念**。
**它们不是「留了个恒为空的格」，是这个结构里根本没有那几格。**
👉 **P2 面板对标 kiro2api 时，这四格是明确的「不做」清单，别照抄 UI 把空格子画出来。**

**校验（`api-keys.ts:372-405`）**：

```rust
enum ApiKeyNameProblem { NameNotAString, NameEmpty, NameTooLong }
fn check_api_key_name(v) -> Option<Problem> {
    if !v.is_string() { return Some(NameNotAString); }
    if v.trim().is_empty() { return Some(NameEmpty); }   // **先判空再判长**：空名字报「太长」会把人指向反方向
    if v.len() > API_KEY_NAME_MAX { return Some(NameTooLong); }
    None
}
enum ApiKeyExpiryProblem { ExpiresNotANumber, ExpiresInThePast }
fn check_api_key_expires_at(v, now) -> Option<Problem> {
    if v.is_null() { return None; }                       // null 是**合法输入**（不过期），不是「没填」
    if !v.is_finite_integer() { return Some(ExpiresNotANumber); }
    if v <= now { return Some(ExpiresInThePast); }        // ⚠️ **`<= now` 而不是 `< now`**
    None
}
```
**过去的时刻当场拒**：签发一把生下来就过期的密钥不是一个有意义的形态，
而它在面板上长得与「签发成功」一模一样（列表里多一行「已过期」），**运维会以为自己按错了什么**。
⚠️ `<= now` 与 `is_api_key_expired` 的 `>= expires_at` **必须是同一条边界**，
否则会造出一个「创建时合法、下一行就过期」的洞。

**表的窄化（`api-keys.ts:407-459`）—— 三档，不许合并成两档**：

```rust
enum ApiKeyTableRead { Absent, Ok(ApiKeyTable), Invalid }
```
* `Absent`（存储里压根没有这个键）：**这是一份合法快照**（还没签发过任何密钥），holder 要按 TTL 缓存它；
* `Invalid`（读得到、但结构不认）：**绝不当空表**——当空表的后果是**全部客户 401**，
  而**下一次面板写会把幸存记录整份覆掉，不可逆**。
`is_api_key_record` 逐字段窄化（`api-keys.ts:412-426`）；坏一条 = 整张表不认。
**不做逐条抢救**（kiro2api 那边有 `salvage_api_keys`）（`api-keys.ts:442-450`）：本仓这张 blob 只有面板一个写者、
没有「运维手工编辑」这条正当来源，而坏字节本身留在存储里没有被任何一条路径覆盖过
（读路径零 put、写路径读到 `Invalid` 一律 **409** 拒绝落盘）⇒ **人工捞回随时可做。**
**抢救逻辑的代价是一整条「部分正确」的语义，收益在本仓为零。**

### 8.2 上游 key 的面板投影（`admin/key-view.ts`，162 行）

```rust
fn mask_key(key: &str) -> String {       // key-view.ts:30-33 —— **全仓唯一的一份实现**
    if key.len() <= 10 { "…".into() } else { format!("{}…{}", &key[..5], &key[key.len()-4..]) }
}
const BUCKETS: [&str; 4] = ["disabled", "evicted", "cooling", "fresh"];   // key-view.ts:35
fn key_bucket(r, now) -> Bucket {        // key-view.ts:53-58 —— **顺序即优先级，与 pool_health 同序**
    if is_disabled(r) { Disabled } else if r.evicted { Evicted }
    else if r.cooldown_until > now { Cooling } else { Fresh }
}
```
**绝不返回原值**：返回原值的「掩码」比没有掩码更糟，**调用方会以为它安全了**。
`key_bucket(...) == Fresh` **当且仅当** `is_available(...)`（有判据钉着）。
**Rust 侧注意**：`&key[..5]` 对多字节 UTF-8 会 panic ——`is_importable_key` 保证导入的是 ASCII，
但**铸号那条路存进去的可疑 key 可能不是**（见 §4.5）。必须用 `chars().take(5)` / `char_indices()`。

```rust
struct KeyView {                         // key-view.ts:60-106
    id, masked, seq: usize, bucket, added_at, last_used_at: Option<i64>,
    cooldown_until: i64, cooldown_reason: Option<String>, evicted_reason: Option<String>,
    strikes: u32,
    disabled: bool,                      // **恒是布尔、恒存在**
    note: Option<String>,                // **恒存在、恒是 string|null**
    stats: KeyStats,                     // normalize 过的，**近似值**
}
fn to_key_views(records, now) -> Vec<KeyView>   // key-view.ts:117-136
//   排序：added_at 升序、id 破平；**返回顺序必须与 seq 一致**
fn bucket_counts(views) -> { all, fresh, cooling, evicted, disabled }  // key-view.ts:138-151（判据只看 bucket）
fn matches_query(v, q) -> bool           // key-view.ts:153-162
//   **只匹配 id 与掩码后的可见部分，绝不匹配明文 key**
```

🔴 **返回顺序不是「顺手排一下好看」，分页的正确性建立在它上面**（`key-view.ts:108-116`）：
传入顺序来自 `pool:index`，而索引**顺序无语义**（对账会拿 `list()` 的结果整体重排它）。
只算 `seq` 却按传入顺序返回，后果有三层：
① 面板上出现 `#7 / #2 / #19`；
② 一次索引对账之后整个列表重排，而什么都没增删；
③ **分页在两次请求之间不稳定，翻页会重复或漏掉条目。**

🔴 **`note` 的渲染要求**（`key-view.ts:96-101`）：它是本结构里第一个「**运维自由输入、又会被投影到面板上**」的字段，
**渲染必须走 `textContent`，绝不许拼进 innerHTML**。其余字段要么是后端算出来的枚举、要么是数字、要么是掩码，
**只有它整串来自请求体**。后端刻意不做转义（JSON 响应体的字段边界由 JSON 语法本身保证，转义是渲染层的事）
——**所以这句话必须在渲染那一侧被执行，别以为后端已经洗过了。**

🔴 **`matches_query` 绝不匹配明文**：匹配明文等于把「没有 reveal 端点」这条保证降级成一个慢速预言机
——**提交一段猜测、看返回条数，就能逐字猜出整把 key。**

### 8.3 事件环与写预算（`admin/event-ring.ts`，312 行 + `admin/event-entry.ts`，82 行）

```rust
const EVENT_RING_SIZE:           usize = 100;       // event-ring.ts:50  单个分片键最多留多少条，超了丢**最旧**的
const EVENT_FLUSH_MIN_INTERVAL_MS: i64 = 60_000;    // event-ring.ts:70
const EVENT_WRITES_PER_DAY:      u32 = 12;          // event-ring.ts:95
const EVENT_WINDOW_MS:           i64 = 3_600_000;   // event-ring.ts:104  时间窗 = 1 小时
const EVENT_SLOTS:               usize = 2;         // event-ring.ts:112
const EVENT_WINDOW_RETAIN:       i64 = 24;          // event-ring.ts:120  ⇒ 24 小时可见历史
const EVENT_TTL_MARGIN_MS:       i64 = EVENT_WINDOW_MS;   // event-ring.ts:179
const DAY_MS:                    i64 = 86_400_000;
```

**架构裁定：去掉索引，改用「时间窗 + 槽位」的分片键空间**（`event-ring.ts:1-47`）。
读路径不再需要 `list()` 也不需要索引，候选键由 `candidate_keys()` **纯计算**得出。
「索引读改写丢更新」因此**根除**；代价改成「**同槽位真并发覆盖**」——两个副本在同一个「窗口+槽位」上
落盘时互相覆盖对方那一批（最后写的赢），**不是「永久不可达」，而是「丢一次交错，下一轮各自继续写自己的」**。

🔴 **「无上限增长」那条前两版都只做对了一半**（`event-ring.ts:12-34`）：
* 第一次：`candidate_keys()` 最初没有钳位 `after`，**一个陈旧游标能让候选键数随「游标陈旧度」线性增长、无上界**
  ——这是「无上限增长」换了一根轴复发（旧轴是「部署年龄」，新轴是「游标陈旧度」，
  而**「游标冻结、系统长期安静」恰恰是本项目最常见的稳态**）。**评审实测 `after=0` 时单次请求打出约 99 万次 get。**
* 第二次：`window` 是绝对纪元小时数、单调递增，**读路径钳位不等于「存储里实际驻留的键数有界」**。
  第一版对策是「每次成功落盘后顺手 delete 滚出保留期的那个窗口键」，
  **这个方案被下一轮评审逐场景实测推翻**：它只在「同一槽位恰好每 24 个窗口落盘一次」这种规律节奏下才命中；
  **稀疏落盘（gap 超过保留期，例如每 25 小时或每周才落一次）清理率直接归零**；
  生产 `shardId` 是随机 uuid，各副本各清自己那个槽位，清理率再打对折。
  **这不是「漏了一个 corner case」，是「有界性绑在了落盘节奏上」这个方案本身在结构上就错了。**
  **⇒ 现在的修法是 TTL**：把有界性变成**存储自己的性质**，与落盘节奏、槽位随机性、进程是否被结束**全部无关**。

```rust
fn window_index(at: i64) -> i64 { at.div_euclid(EVENT_WINDOW_MS) }        // event-ring.ts:123-125
fn slot_of(shard_id: &str) -> usize {                                     // event-ring.ts:127-136
    let mut h: i32 = 0;
    for b in shard_id.chars() { h = h.wrapping_mul(31).wrapping_add(b as i32); }  // **必须是 i32 回绕**
    (h.rem_euclid(EVENT_SLOTS as i32)) as usize
}
fn shard_key(window: i64, slot: usize) -> String { format!("event:{window}:{slot}") }   // event-ring.ts:158-160
fn event_expires_at(at: i64) -> i64 {                                     // event-ring.ts:181-183
    (window_index(at) + EVENT_WINDOW_RETAIN) * EVENT_WINDOW_MS + EVENT_TTL_MARGIN_MS
}
```
⚠️ **`slot_of` 必须确定性**：同一个进程的连续多次落盘要稳定落在同一把 key 上，
**环形语义才有意义**——落在不同 key 上就退化成每次只有一条。
⚠️ **`h * 31 + charCode` 在 JS 里靠 `| 0` 截成 32 位有符号**；Rust 侧必须用 `i32::wrapping_*`，
用 `u64` 会得到**不同的槽位分布**（那不影响正确性，但会让升级时同一个 shardId 换槽 ⇒ 丢一批已攒的事件）。
⚠️ **TTL 余量取一整个 `EVENT_WINDOW_MS`**（`event-ring.ts:162-178`）：判定过期用的时钟与算出 `expiresAt`
时用的时钟不保证逐毫秒同步；没有余量的话一个恰好卡在窗口边界的请求会读到 `null`
（本身不是错误，但**没有必要制造这种边界竞态**）。

🔴 **`candidate_keys(now, after)`（`event-ring.ts:185-229`）—— 「零索引」的全部依据**：

```rust
fn candidate_keys(now: i64, after: Option<i64>) -> Vec<String> {
    let now_window = window_index(now);
    let floor = now_window - EVENT_WINDOW_RETAIN + 1;        // **这条钳位是承重的**
    let from_window = match after { None => floor, Some(a) => max(floor, window_index(a)) };
    let mut keys = vec![];
    for w in from_window..=now_window { for s in 0..EVENT_SLOTS { keys.push(shard_key(w, s)); } }
    keys
}
```
* `after == None`（面板首次加载 / 清空视图后重新拉 / 直接调 API 不带 `after`）：回看 24 个窗口（**冷读**）；
* `after` 有值（增量轮询）：只看 `after` 所在窗口到当前窗口——通常 1 个，跨窗口边界那一刻 2 个。
* **钳位之后返回长度对任何 `after` 值（含负数、含极端未来值）都不超过 `24 × 2 = 48`。**
  这个函数自身现在是这条不变量**唯一**需要守住的地方；HTTP 层额外拒绝负数只是让语义更干净，**不是必要条件**。
* 返回顺序：窗口升序、同窗口内槽位升序（顺序本身没有语义，只是让候选键列表确定性）。

⚠️ **「全仓事件白名单清一色是错误诊断事件」那半句已经被推翻**（`event-ring.ts:204-210`）：
Key 写端点会产出 `key.added` / `key.restored` / `key.disabled` / `key.deleted`，其中两条是 info 级。
**后半句（本段真正依赖的那半）没有变**：那四条**人点一下才发生**，没有人操作的部署仍然是 0，
「游标长期冻结」仍然是最常见的稳态。

```rust
fn append_ring<T: Clone>(cur: &[T], add: &[T], size: usize) -> Vec<T> {   // event-ring.ts:231-241
    let merged = [cur, add].concat();
    if merged.len() > size { merged[merged.len()-size..].to_vec() } else { merged }
}
fn truncated_count(cur_len: usize, add_len: usize, size: usize) -> usize { // event-ring.ts:243-254
    (cur_len + add_len).saturating_sub(size)
}
```
🔴 **`truncated_count` 存在的理由（评审发现）**：`append_ring` 本身会静默丢弃超限的最旧条目，
而 `dropped` 计数器原来**只在内存缓冲溢出时计数**——分片键攒满 100 条之后，**每一次落盘都在这里静默丢弃**
（是任何有持续流量部署的稳态），**而面板此时显示 `dropped: 0`**。

```rust
fn merge_shards<T: HasTs>(shards: &[Vec<T>], limit: usize) -> Vec<T> {    // event-ring.ts:256-278
    // 按 ts 降序（最新在前），同 ts 用 (shard_idx, seq) **稳定破平**，截到 limit
}
```
**不依赖排序算法天然稳定**：显式的 `(shard, seq)` 二级键让排序结果与引擎/版本无关。
不这样做的话面板每次轮询顺序都可能在跳。

**写预算（`event-ring.ts:280-312`）**：

```rust
struct WriteBudget { day_start: i64, used: u32 }
const FRESH_BUDGET: WriteBudget = WriteBudget { day_start: 0, used: 0 };   // 0 只是哨兵起点
fn in_window(b: &WriteBudget, at: i64) -> bool { at >= b.day_start && at - b.day_start < DAY_MS }
fn can_write(b: &WriteBudget, at: i64, per_day: u32) -> bool {
    (if in_window(b, at) { b.used } else { 0 }) < per_day
}
fn consume(b: &WriteBudget, at: i64) -> WriteBudget {
    if in_window(b, at) { WriteBudget { day_start: b.day_start, used: b.used + 1 } }
    else { WriteBudget { day_start: at, used: 1 } }
}
```
⚠️ `at < b.day_start`（时钟回拨）与 `at - b.day_start >= DAY_MS`（正常跨天）**都**判定为「不在窗口内」
——两者都按「新的一天」处理，**与本仓其余处的 `age < 0` 立刻恢复同一套语义**（回拨立刻恢复可写，
不会把预算冻结到「回拨量走完」才解封，也不会缓存出负的 `used`）。

🔴 **`EVENT_FLUSH_MIN_INTERVAL_MS = 60_000` 挡的是一根杠杆**（`event-ring.ts:52-70`）：
设计原本的「满 20 条或距上次 flush > 60s」这条 size 规则被去掉了，因为它把写入速率的控制权交给了调用方
——白名单里的 `admin.login_failed` 是**任何未鉴权请求都能触发**的（零凭据零限速），
于是攻击者可以按 20 条一批地驱动存储写。**这正是本仓一直不肯给出去的那根杠杆**：
反复发失败登录就能驱动写，**把 DoS 面从「猜口令」扩大到「拖垮 key 池的状态回写」**。
（`FileStorage` 每次 put 都重写整个 `store.json`，与 key 池的状态回写共用同一个文件、同一条写队列。）
⚠️ **评审那条「写预算全局失守」的另一半**：只有这条闸不够——冷启动那一刻 `last_flush_at === null`，
第一次 `maybe_flush()` 跳过这条闸直接写，**等于每次进程冷启动送一次零门槛写**。
修法：`last_flush_at` 初值给 `now()` 而不是 `null`。

🟡 **`EVENT_WRITES_PER_DAY = 12` 如实登记**（`event-ring.ts:72-94`）：这个数是照着「1,000 次/天的 9.6%」挑的，
**那个分母已经不存在**。换句话说**今天它偏保守，但没有人重新算过它该是多少**。
要调它得先定一个新的分母（磁盘写放大能接受到什么程度），那是一次独立的取舍。
**这条闸只能保证「单个副本不超发」**——单个副本无法知道别的副本用了多少预算（没有 CAS、没有跨副本协调）。
预算用完时**不写**，缓冲区继续接（满了丢最旧的并计数），并让端点如实报 `budgetExhausted`。
**Console 那一路一条都不丢**，排障能力不受影响。

**事件条目的窄化（`event-entry.ts`，82 行）—— 判据只有一条：`ts` 是有限数字**：

```rust
struct EventEntry { ts: i64,                    // **唯一被校验过的字段**
                    level: Value, event: Value, msg: Value, corr: Value, fields: Value }  // 其余一律未校验
fn is_event_entry(v: &Value) -> bool { v.is_object() && v["ts"].is_finite_number() }
fn narrow_shard(raw: &Value) -> (Vec<EventEntry>, usize /*malformed*/) {
    if !raw.is_array() { return (vec![], 0); }   // **null/undefined 是完全正常的路径，计 0**
    let entries = filter(is_event_entry);
    (entries, raw.len() - entries.len())
}
```
**这不是「校验得不够」，是一条刻意划出来的界线**（`event-entry.ts:24-50`）：

| 档 | 处置 | 理由 |
|---|---|---|
| **结构性不可用**（`ts` 缺失/非数字/NaN/±Inf） | **丢** | 这类条目无法排序（比较器恒回 NaN）、无法当游标 ⇒ **面板的游标永远推不动**，稳态读吞吐从 70,560 涨到 276,480 次/天。**这条只在默认的「全部级别」档位下成立**（按级别筛选时它们被滤掉、游标立刻恢复），**而默认档恰恰是最常用的一档** |
| **显示不好看**（`level` 不在四档、`event`/`msg` 缺失、`fields` 不是对象） | **留** | 这些只影响某一列怎么渲染，**丢掉它们就是在一次上游异常之后把整段证据清空** |

⚠️ **`level` 后端一个字都不动**：归一化已经由前端那一份在跑。**代价明写**：畸形 `level` 的条目
在「按级别筛选」时选不中，只在「全部级别」下可见——**已知限制，登记不修**。
⚠️ **`malformed` 恒为 0 正是它的价值**：`src/` 里没有任何路径能产出畸形条目，所以它是那几条用例
**在生产环境里的对照组**——运维手改存储、值损坏、`store.json` 被写坏时，
**它是唯一能把「面板少了几条事件」和「本来就没有事件」区分开的信号**。

### 8.4 Tier-2 时间序列统计（`admin/usage-stats.ts`，809 行）—— **默认关，且「关」必须是零成本**

**存储形态：日键 + 值里装 24 个小时槽**（`usage-stats.ts:1-47`）。

🔴 **两条设计上的分叉，各有各的理由，不许照抄事件板块**：
* **有界性用 TTL，不用「Cron 压实 + 删分片」**：压实那一半正是事件分片第一版被推翻的形态
  （有界性依赖「落盘节奏恰好规律」，稀疏落盘或多副本各写各的随机分片时**清理率能跌到 0**），
  而 Cron 压实还额外依赖「Cron 按表达式可靠触发」——**官方没有文档化任何触发保证**。
  两个前提叠在一起，没有一端立得住。**⇒ 不做压实、不 delete。**
* **但「日键 + 值里装小时槽」这一半必须保留，第一版把它一起换掉是过度类比**：
  **事件分片的值是「这一小时里发生的那些条目」，日历上互不重叠，所以必须按小时分键；
  而用量的值是「累计计数」，一次写就能把当天全部 24 个小时槽一起覆盖掉。**
  按小时分键的代价算过：2 小时落盘间隔下每次 flush 恒写 2–3 个键（而预算只扣 1 格），
  真实写量 192–288/天、最坏一行 944–1040；读 30 天要 `720 × 2 = 1440` 次 get。
  **换回日键之后这两条同时消失：一次落盘恒 1 个 put（跨 UTC 零点那次 2 个），读 30 天 `30 × 2 = 60` 次 get。**

> **一句话记住这个形状为什么长这样：写路径按「天」分键所以便宜，读路径按「天」取键所以扇出小，
> 而「小时」这一维住在值里面、不额外花键。**

```rust
const USAGE_DAY_MS:               i64 = 86_400_000;   // usage-stats.ts:129
const USAGE_SLOTS:                usize = 2;          // usage-stats.ts:138  与 EVENT_SLOTS 相同
const USAGE_DAY_RETAIN:           i64 = 30;           // usage-stats.ts:190
const USAGE_WRITES_PER_DAY:       u32 = 13;           // usage-stats.ts:210  **= 12 次落盘 + 1**
const USAGE_FLUSH_MIN_INTERVAL_MS: i64 = 7_200_000;   // usage-stats.ts:267  = 2 小时
const USAGE_KEY_PREFIX:           &str = "usage:";    // usage-stats.ts:303
const USAGE_MODEL_KEY_MAX_LEN:    usize = 64;         // usage-stats.ts:341
const USAGE_MODEL_MAX_KEYS:       usize = 32;         // usage-stats.ts:372
const USAGE_APIKEY_MAX_KEYS:      usize = APIKEY_MAX + 1;  // usage-stats.ts:410 = 201
const USAGE_MASTER_BUCKET:        &str = "master";        // usage-stats.ts:429
const USAGE_UNATTRIBUTED_BUCKET:  &str = "unattributed";  // usage-stats.ts:452
const USAGE_OTHER_BUCKET:         &str = "__other__";     // usage-stats.ts:472
const USAGE_TTL_MARGIN_MS:        i64 = USAGE_DAY_MS;     // usage-stats.ts:565
```

```rust
struct UsageBucket {                     // usage-stats.ts:69-94  —— **没有原始延迟数组**（明确放弃 P95）
    requests: u64, success: u64, errors: u64,
    tokens_in: u64, tokens_out: u64,     // **只有非流式、且协议路由传了 expect_json 的那三条**才有
    streaming_requests: u64,             // 流式单列一栏，让 token 的缺口可见
    latency_sum: u64, latency_count: u64,
}
struct UsageDayShard {                   // usage-stats.ts:96-127
    shard_id: String, day: i64 /* floor(at / 86400000)，**不是日期字符串** */, updated_at: i64,
    total: UsageBucket,
    hours:      HashMap<String /*"00".."23"*/, UsageBucket>,
    by_model:   HashMap<String, UsageBucket>,
    by_protocol:HashMap<String, UsageBucket>,
    by_api_key: HashMap<String, UsageBucket>,
}
```
⚠️ **`day` 用整数日序号，不是 `YYYY-MM-DD`**：整数比对不会有时区歧义，也不必在读路径上反复解析。
⚠️ **`latency_*` 只累计成功请求**（`usage-stats.ts:76-86`，评审发现）：第一版注释写「只统计有意义的那些」，
**而代码对失败请求同样 +1** ——注释与代码互相矛盾，两边都没有用例。
后果不是措辞问题：**一个 8 秒超时和一个 30 毫秒的 401 都会进平均延迟，面板上那个「平均延迟」就不是任何真实的东西。**
⇒ **裁定：只有 `ok` 的请求进 `latency_sum` / `latency_count`。**

🔴 **`USAGE_WRITES_PER_DAY = 13` —— 那个 `+1` 不是余量**（`usage-stats.ts:192-210`）：
它是**跨 UTC 零点的那一次落盘要写两个键**（前一天一个、当天一个）。
第一版取 12，于是**每 24 小时恰好有一次 put 被预算拒绝**——数据不丢（那天的 dirty 不清，下一轮补上），
**但「尾巴 ≤2 小时」这句对外承诺变假**（那一次的尾巴是 ≤4 小时）。**评审用 3 天仿真确认它是确定性发生的，不是边角。**
⚠️ **这个数数的是 `put` 次数，不是 flush 次数**：第一版的不变量数的是 flush，而当时的键形状让一次 flush 写 2–3 个键
⇒ 真实写量是它算出来的 2–3 倍，**而不变量照样绿**。

🔴 **`USAGE_FLUSH_MIN_INTERVAL_MS = 7_200_000` 与预算是一条乘法关系，没有自由度**（`usage-stats.ts:212-267`）：
要让一整天都被覆盖，必须 **`间隔 × (预算 − 1) >= 一天`**（那个 `−1` 是跨零点写两个键）。
预算由写配额定死（13）⇒ `86_400_000 / 12 = 7_200_000`，**两边恰好相等，没有余量，这是刻意的**。
* **设计原值 300s 对应的预算是 288，× M=8 = 2,304 次/天 = 免费档的 230%**，不可行；
* 而**保留 300s 却把预算压到 12 的话，每天只有头 60 分钟被覆盖，之后整天不写**
  ——**写量合格而数据从中午起就是假的，比第一种更糟。**

**代价（一条，不是两条）**：未落盘的尾巴最长等于这个间隔。它的两种表现是同一个事实：
① 面板上「今天」的数字最多旧 2 小时；② 存活不足一个间隔的实例攒的计数随实例消失。
⚠️ **②不是「默认关」的独立理由**——第一版这么写过，那是循环论证：
「活不满一个落盘间隔的实例存不下」在 `last_flush_at = now()` 下恒真，**而让它变严重的正是这个间隔取值本身**。
**默认关的理由只有一条：统计吃掉写配额会连带打死 key 池的状态回写。**

**运维可经 `USAGE_FLUSH_INTERVAL_MS` 覆盖它，但判据是「存储有没有写配额」，不是运行时名字**：
* **没有写配额的存储**（FileStorage / Docker）：任意正整数放行，**并且不再设每天的写预算**
  ——留着它的话调到 300s 的结果是「头 65 分钟写满 13 次、之后整天不写」；
* **有写配额的存储**：预算恒为 13，而间隔必须满足那条乘法关系，破了就**启动即抛**并给出最小可用值。

⚠️ **「相同」只到默认值为止，别写成「两边行为一个字节都不差」**：没设这个变量时两边的**落盘间隔**逐字相同，
但**预算那道闸本来就只有「有写配额」的一侧才有**——同样 20 个待落盘的日，文件存储一次写 20 个键、KV 写 13 个。
**那不是分叉，那就是这个设计本身。**

```rust
fn usage_day_index(at: i64) -> i64 { at.div_euclid(USAGE_DAY_MS) }            // usage-stats.ts:269-271
fn usage_hour_of(at: i64) -> String {                                        // usage-stats.ts:273-277
    format!("{:02}", (at - usage_day_index(at) * USAGE_DAY_MS) / 3_600_000)   // UTC 小时，两位补零
}
fn usage_slot_of(shard_id: &str) -> usize { /* 与 slot_of 算法逐字相同，只换常量 */ }  // usage-stats.ts:291-295
fn usage_day_key(day: i64, slot: usize) -> String { format!("usage:{day}:{slot}") }   // usage-stats.ts:305-307
fn usage_expires_at(day: i64) -> i64 {                                       // usage-stats.ts:567-569
    (day + USAGE_DAY_RETAIN) * USAGE_DAY_MS + USAGE_TTL_MARGIN_MS
}
```
⚠️ `usage_slot_of` 的**散布性判据不许只用一个样本**：`usage_slot_of("s1")` 恰好是 0，
写死成 `return 0` 时它不可观测（**评审实测 ESCAPED**）。

🔴 **`usage_candidate_keys(now, from, to)`（`usage-stats.ts:571-639`）—— 返回键数对任意入参都 `<= 30 × 2 = 60`**：

```rust
fn usage_candidate_keys(now_ms: f64, from_ms: f64, to_ms: f64) -> Vec<String> {
    if !now_ms.is_finite() { return vec![]; }                     // **护栏 1**
    let newest = usage_day_index(min(to_ms, now_ms));
    let oldest_allowed = usage_day_index(now_ms) - USAGE_DAY_RETAIN + 1;
    let oldest = max(usage_day_index(from_ms), oldest_allowed);
    let mut keys = vec![]; let (mut d, mut n) = (oldest, 0);
    while d <= newest && n < USAGE_DAY_RETAIN {                   // **护栏 2：结构性计数闸**
        for s in 0..USAGE_SLOTS { keys.push(usage_day_key(d, s)); }
        d += 1; n += 1;
    }
    keys
}
```
**两道护栏是这条不变量的一部分，不是可有可无的防御性代码**：
1. `!now_ms.is_finite()` ⇒ 返回空。`now = ±∞` 时 `d++` 对 `±∞` 是**恒等操作**，循环不终止；
2. `n < USAGE_DAY_RETAIN` 计数闸。**只有第 1 道不够**——`1e300` 是有限数，而 `usage_day_index(1e300)`
   已远超 `2^53`，那里 `d++` 失去精度、**原地打转**。
   ⚠️ **逐项实测（两组只差一个 `from_ms`）**：

   | 入参 | `oldest` | 第 0 步 `d+1 == d` | 去掉计数闸 |
   |---|---|---|---|
   | `(1e24, 0, 1e24)` | `11574074074074044` | **true** | **不停** |
   | `(1e24, 1e24, 1e24)` | `11574074074074074` | false | 停（1 步） |
   | `(1e300, 0, 1e300)` | `1.157…e292` | **true** | **不停** |
   | `(1e300, 1e300, 1e300)` | `1.157…e292` | **true** | **不停** |

   成因：该量级上相邻可表示浮点数的间距是 2，`d + 1` 落在两者正中间，**靠「就近舍入、平局取偶」决定往哪边倒**。
   ⇒ **「会不会卡住」是入参相关的、肉眼推不出来的**，这正是护栏要做成**结构性计数闸**的理由。
   **失效形态是「不返回」，不是「数字变大」** —— 第一版宣告了「恒 ≤ 60」却没有这两道护栏，
   **评审的第一次探针直接把引擎跑成 OOM。**
   👉 **Rust 侧用 `i64` 就没有浮点精度问题，但 `now_ms` 来自外部时仍要挡非有限值 / 溢出，
   而那个计数闸是零成本的保险，照留。**

⚠️ **夹逼的理由是上面那条上界，不是「反正已经过期」**（`usage-stats.ts:606-616`）：
TTL 刻意多留了一整天余量，`usage_expires_at(d)` 保留到第 `d+31` 天零点，而 `oldest_allowed = 今天 − 30 + 1`
⇒ **`今天 − 30` 那一天的键在任何时刻都还活着，却永远不会被这个函数取到。**
那份余量落在 `(0, 86_400_000]` 区间内、**日初满格、日末趋近 0（最坏只剩 1 ms），但恒 `> 0`**。
**别照「反正过期了」去把那天加回来——那会直接打破 60 这条上界。**

⚠️⚠️ **`USAGE_DAY_RETAIN = 30` 与那 60 次扇出的诚实登记**（`usage-stats.ts:140-190`）：
原仓查过官方 limits 页，**结论不是一个干净的数**：Workers 把子请求分两行（免费档 50 / 1,000），
**而那一页从头到尾没有定义什么叫「internal services」，也没有说 KV 绑定调用算哪一行**；
KV 自己那页写着「Operations/Worker invocation: 1,000」。
⇒ 按 KV 那页 60 次占 6%；按 50 那行读，**60 就是超的**。
**两页对不上 ⇒ 当作「没有平台承诺」，不臆测。**
⚠️⚠️ **控制端已裁定：这个 30 保留，而「60 次是安全的」这句话在任何注释或文档里都不许出现。**
读端点必须**失败得诚实**：读不出来显示 `—`，**不许 500、更不许把半份数据当成全份**
（判据：读 30 天时第 47 次 get 抛错 ⇒ 整条仍然是 200，`days` 是 `null`，绝不把读到的那 46 个当成全份）。

**有界性：三维上界（`usage-stats.ts:309-410`）**

🔴 **`USAGE_MODEL_MAX_KEYS = 32` 不是防御性代码，是有界性本身**：`by_model` 的键**完全由客户端控制**
（模型名来自请求体），每个不同的串就是一个新 `UsageBucket`，而那份 map **永不清理、每次落盘整份覆写进一个键**。
没有上界的三个后果：
① 长驻进程的内存无上界；
② **单值上限撞上之后 `put` 抛错 ⇒ 那天永远 dirty ⇒ 每个落盘间隔重试一次、白烧预算**，
   而这一天的数据再也写不出去；
③ 读路径要把最多 60 个这样的分片合并。

🔴 **`USAGE_APIKEY_MAX_KEYS = 201` —— 「键不是外部可控的」推不出「它有界」**（`usage-stats.ts:374-410`）：
那 200 是**当下**的上限，而**日桶是历史累计**：同一个 UTC 日里「签发 → 用一次 → 删掉」反复操作，
一天之内出现过的不同 id 数**可以远超 200**。
⚠️ **不拿「写配额自然会拦住它」当有界性的依据**：那是把本模块的有界性建在**另一个子系统的配额**上，
**而文件存储那一侧根本没有写配额——那一侧一句话就穿了。**

⚠️ **`by_protocol` 刻意不设上界**：那一维的值来自四条路由里的字面量，**客户端碰不到它**。`hours` 同理（恒 24 格）。
⇒ **收尾结论拆成两句：外部可控的只有 `by_model`；而需要上界的是 `by_model` 与 `by_api_key` 两维。**

```rust
fn bound_usage_key(existing: &HashMap<String, UsageBucket>, raw: &Value, max_keys: usize) -> String {
    let k: String = safe_string(raw).chars().take(USAGE_MODEL_KEY_MAX_LEN).collect();
    if existing.contains_key(&k) { return k; }            // **先截长度、再判格数**
    if existing.len() >= max_keys { return USAGE_OTHER_BUCKET.into(); }
    k
}
```
⚠️ **第三个参数 `max_keys` 刻意没有默认值**（`usage-stats.ts:544-552`）：两维的上界是两个不同的数（32 与 201），
写成有默认值的话，`by_api_key` 那一侧漏传就会**静默按 32 计** ⇒ 一个有 33 把密钥的部署里，
第 33 把之后的用量全部并进「其它」，**而面板上那看起来完全正常**。
（同源教训：`can_write` 的 `per_day` 有默认值 12，`UsageSink` 必须显式传 13 ——
**漏传不会有任何编译期信号，它会静默按 12 计，而 12 恰好就是那个被判定有缺陷的值。**）
⚠️ **「先截长度、再判格数」是承重的**：`existing.contains_key` 必须在满桶判断之前
——否则同一个模型会因为 map 满了而在两次请求之间被分到两个不同的格子，**计数直接错位**。
（评审实测：删掉那句早返回**全量 2182 全绿**，因为那格用例里 200 个模型名各只出现一次、满桶之后从不复用旧键。）
⚠️ **上界是 `USAGE_MODEL_MAX_KEYS + 1`，多出来的一格是 `__other__` 自己。**

```rust
fn safe_string(raw: &Value) -> String {      // usage-stats.ts:520-542
    if let Value::String(s) = raw { return s.clone(); }
    // JS 侧 String(x) 本身会抛（`{"toString":1,"valueOf":1}`），兜底回 "[unstringifiable]"
    to_display_string(raw).unwrap_or_else(|| "[unstringifiable]".into())
}
```
⚠️⚠️ **护栏只有这一道，就在本文件里；四条协议路由上都不许再加第二道**（`usage-stats.ts:320-339`，
**本任务最贵的一次教训**）：上一版为了「冗余」在路由的 `record` 闭包里各加了一个 `String(...)`，
并写下「少任何一道就等于多一条 500」。**那句话两个方向都被实测证伪**（三种组合全都是 503）。
**而它的代价是实打实的**：那两行在 `record` 闭包体里，而 `recordUsage()` 的「sink 缺席就 return」**在它之后**
⇒ `{"model":{"toString":1,"valueOf":1}}` 让 `String()` 自己抛，**连 Tier-2 关着的部署也被打成 500**
——一次为修 A 加的防御，**把影响面从「开了统计的人」扩大成了「所有部署」**，
正面违反「关是零成本」。
**⇒ 判据记下来：防御要加在「只有开着才跑」的那一侧。**
⭐ **另一条形状**：`safe_string` 上方原本写着「删掉会让网关 500」——
**那句话在写下的那一刻是真的，却被同一个提交里给 `record()` 加的 try/catch 变成了假的。**
今天它不再是「防 500」的最后一道，但它是「**别丢计数**」的唯一一道。
👉 **加一处 try/catch 会改变别处注释的真假 —— 加完要回头查谁在依赖那个前提。**

**已知边界，如实登记**（`usage-stats.ts:505-518`）：
* 客户端可以把 `model` 直接填成 `__other__` 去抢占那个溢出格 —— 后果**只是「其它」那一格里混进了一个真实模型**，
  总数一条不丢、上界照旧成立。**不为它换一个「客户端猜不到的」键名：那是把正确性建在一个秘密上。**
* `"[unstringifiable]"` 同理。
* `USAGE_MASTER_BUCKET = "master"` 是**伪 id，不是那张表里的一行**（`usage-stats.ts:412-429`）：
  真实子密钥 id 是 **12 位十六进制**，而 `"master"` 有 6 个字符、带着 `m`/`s`/`t` 三个十六进制里没有的字母
  ⇒ **两者的取值集合不相交**。（**手工编辑过的存储里放得进一个名字就叫 `master` 的记录**，
  后果是那一把的用量与主口令合成一格——总数不丢、上界照旧。）
  ⚠️ **主口令那一格不享受任何优待**：一天之内先有 201 个不同的子密钥 id 落过桶、主口令的第一条请求才到，
  那一条会并进 `__other__`。**「主口令不可删不可停用」说的是它不在那张表里，不是「它在统计里有一个永久保留的桶」。**
* `USAGE_UNATTRIBUTED_BUCKET = "unattributed"`（`usage-stats.ts:431-452`）：**今天它结构上不可达**
  （鉴权中间件在放行前写进归属，而四条协议路由只能经它进来）。
  **那为什么还要有它**：另外两种兜底都在撒谎——归到 `master` ⇒ 把别人的流量算到主口令头上
  （而归属正是这一维存在的全部理由）；干脆不记 ⇒ `total` 与 Σ`by_api_key` 从此对不上，**而那是静默的**。
  ⇒ **留一个看得出是兜底的桶名。它一旦在面板上出现，说的就是「有一条路绕过了鉴权中间件」——那是要人去查的。**

**桶运算（`usage-stats.ts:641-669`）**：

```rust
fn add_to_bucket(b: &UsageBucket, o: Outcome) -> UsageBucket {
    UsageBucket {
        requests: b.requests + 1,
        success:  b.success + o.ok as u64,
        errors:   b.errors + !o.ok as u64,
        tokens_in: b.tokens_in + o.tokens_in,  tokens_out: b.tokens_out + o.tokens_out,
        streaming_requests: b.streaming_requests + o.stream as u64,
        latency_sum:   b.latency_sum   + if o.ok { o.latency_ms } else { 0 },   // **只在 ok 时累计**
        latency_count: b.latency_count + o.ok as u64,
    }
}
```

**合并读回来的分片（`usage-stats.ts:671-750`）**：

```rust
struct Merged { by_day, hours, by_model, by_protocol, by_api_key, total, shards: usize, malformed: usize }
```
⚠️ **入参是 `&[Value]` 不是 `&[UsageDayShard]`，且逐条窄化**：真实输入来自存储。
**`malformed` 是给面板看的：静默丢弃就是撒谎。**
⚠️⚠️ **返回值里那几个 map 在 JS 侧必须是 `Object.create(null)`（无原型）**，这是一条给调用方的硬契约：
键来自客户端填的模型名，普通 `{}` 上 `__proto__` 会让那一条**彻底消失**、`toString` 会让那一格**算成一堆 NaN**，
**两种都让落盘分片的 `total` 与 Σ`by_model` 对不上**。
**Rust 侧**：`HashMap<String, _>` 天然没有原型链，**这条 JS 特有的坑不必移植**，
但「`__proto__` / `toString` / `hasOwnProperty` / `constructor` 这四个名字必须原样出现在响应里」这条**行为**要保留。
⚠️ **`hours` 是跨天求和的，调用方必须传单日区间**（`usage-stats.ts:704-709`）：
多日区间下 `hours["13"]` 是「区间里所有天的 13 点之和」（一条 hour-of-day 剖面），**不是某一天的 13 点**。
**单日下钻必须只把那一天的分片喂进来。**（`by_day` 不受影响，它按 `sh.day` 分了键。）

**逐字段窄化（`usage-stats.ts:752-809`）**：
* `num(v)`：有限、`>= 0` ⇒ `trunc`，否则 0；
* ⚠️ **`narrow_bucket` 要单独拒掉数组**：`typeof [] === "object"` 且 `[]` 是 truthy，
  只判 `typeof` 的话 `{ day: 5, total: [] }` 会拿到一个**全 0 的桶**、被判成**合法分片**记进 `shards`
  ——**一个畸形分片伪装成合法的，比被丢掉更糟：它还会把 `shards` 那个数一起变假。**
* ⚠️ **连带变化**：`hours: { "13": [] }` 从「记成一个全 0 桶」变成「`"13"` 这个键静默消失」
  （分片本身仍算合法，`malformed` 不加）。方向是无害的（少一个假的 0 好过多一个假的 0），
  **但它确实是一次静默丢弃**：`malformed` 今天只数**分片级**的畸形，**不数桶级的**。
* `narrow_shard`：`total` 窄化不出来 ⇒ `None`；`day` 不是有限数 ⇒ `None`；
  `shard_id` 非字符串 ⇒ `""`；`by_api_key` 缺席 ⇒ 空 map（**存量分片没有这一格，这是实情不是缺陷**）。

### 8.5 手动补池护栏（`admin/tend-guard.ts`，296 行）

```rust
const MANUAL_GUARD_KEY:        &str = "registrar_manual_guard";  // tend-guard.ts:47
const MANUAL_TEND_COOLDOWN_MS: i64 = 600_000;                    // tend-guard.ts:61  = 10 分钟
const MANUAL_TENDS_PER_DAY:    u32 = 24;                         // tend-guard.ts:102
struct ManualGuard { day: i64, used: u32, cooldown_until: i64 }  // tend-guard.ts:104-116
```

🔴 **冷却与预算合成一把键，但两个字段各管各的、互不借用**（`tend-guard.ts:1-34`）：
* **读法 A（失效）**：冷却与预算共用同一把键、同一个 `expiresAt`。冷却是 10 分钟 ⇒ 这把键每 10 分钟蒸发一次
  ⇒ `used` 随之归零 ⇒ **日预算闸永远走不到耗尽**；
* **读法 B（本文件）**：一把键装 `{ day, used, cooldownUntil }`，`day` 判跨天、`cooldownUntil` 判冷却。
⚠️ **原注释曾写「读法 A 在结构上不存在——把缺陷做成不可表达」，那是假的，评审实证**：
`Storage.put` 的第三参是**可选**的，读法 A 距离现状只有一个实参，编译全绿、写起来还很自然。
**做成的是「可检测，且检测点唯一」，不是「不可表达」** —— 而**「唯一」这件事本身是个风险**：
删掉那一格判据，读法 A 就重新变成一个静默缺陷。

**`MANUAL_TEND_COOLDOWN_MS = 600_000` 挡的是连点**（`tend-guard.ts:49-61`）：
一次手动补池最多消耗 `MINT_BATCH` 个临时邮箱，而两条通道的活跃邮箱名额是有限的。
⚠️ **两个上限数字都不许写进面板文案**：**它们都不是常数**（一个与账号档位绑定，一个是可被实例覆盖的上游默认值）。

🟡 **`MANUAL_TENDS_PER_DAY = 24` 如实登记**（`tend-guard.ts:63-102`）：
当初的算式是照着「1,000 次写/天」算的（720 + 416 = 1,136 > 1,000 ⇒ 直接打穿），**那个分母没了**。
**护栏本身留着**：「一次点击就是几遍全量重写」与它挡的连点是同一件事。
🟡 **但 24 这个数今天没有推导它的分母**，要调它得先定一个新的分母。
**一次手动补池的非铸造写侧成本**：固定 3 次 put（护栏键 1 + 抢锁 1 + `tend:history` 1）
+ 两笔条件写（域名台账、退避键各最多 1）⇒ **上界 5 次 put**，再加 1 次 delete（释放锁）；
每铸出一把 key 再加 2 次 put（记录 + 索引），而铸造那部分**自限**。
（**实测下界是 3**：成功一轮 = 3 次 put + 1 次 delete。）

```rust
fn day_index(now: i64) -> i64 { now.div_euclid(DAY_MS) }        // tend-guard.ts:118-121
fn day_ends_at(now: i64) -> i64 { (day_index(now) + 1) * DAY_MS }  // tend-guard.ts:123-129  ← 429 里的 resetAt
fn narrow_manual_guard(raw: &Value) -> Option<ManualGuard> {    // tend-guard.ts:131-148
    // **任一字段读不得就整条返回 None** ⇒ 当成「今天还没点过」放行一次
}
fn used_today(cur: Option<&ManualGuard>, now: i64) -> u32 {     // tend-guard.ts:150-161
    cur.filter(|g| g.day == day_index(now)).map_or(0, |g| g.used)
}
```
⚠️ **`narrow_manual_guard` 的失败方向是刻意的**：被外部写坏一把键的代价是**多放行一轮补池**（有存储锁与冷却兜着），
而反过来（当成已耗尽）会**把「立即补池」这颗按钮永久锁死，运维在面板上看不出任何原因**。
⚠️ **`used_today` 提成具名函数是为了让两个消费者用同一处判据，不是为了好看**（`tend-guard.ts:154-161`）：
`check_manual_tend`（消费名额）与 `manual_tend_quota`（只看不动）曾经各写一遍，
**而两份的分歧只在跨天那一瞬间可观测**——面板上显示「今天还剩 24 次」、点下去却拿到「今天的额度用完了」。
**分歧点越窄，越不会有人想到去测它。**

```rust
struct ManualTendQuota { used: u32, remaining: u32, per_day: u32, reset_at: i64,
                         cooldown_until: Option<i64>, retry_after_ms: Option<i64> }
fn manual_tend_quota(cur, now) -> ManualTendQuota   // tend-guard.ts:189-200 —— **一次写都不产生、一格名额都不消费**
```
⚠️ **`remaining` 的口径与 `check_manual_tend` 的 ok 支差 1，这是刻意的**：
那边给的是「点完这一次之后还剩几次」，这边给的是「现在还剩几次」。
**两个数字都对，但读错就会在面板上少显示一次。**
⚠️ **`cooldown_until` / `retry_after_ms` 成对给，且只在真的冷却中才非空**：
不在冷却中时两个都是 `None` —— 给一个「已经过去的时刻」会让面板渲染出一个**恒为 0 的倒计时**，
而那与「现在就能点」长得不一样却是同一件事。
⚠️ **`per_day` 也要给**：面板要说「还剩 19 次（共 24 次）」，而不是只说 19。

```rust
enum ManualTendVerdict {
    Ok { next: ManualGuard, remaining: u32, reset_at: i64, cooldown_until: i64, retry_after_ms: i64 },
    Denied { reason: "write_budget_exhausted", remaining: u32, reset_at: i64, retry_after_ms: i64 },
    Denied { reason: "manual_cooldown", remaining: u32, reset_at: i64, retry_after_ms: i64, cooldown_until: i64 },
}
fn check_manual_tend(cur: Option<&ManualGuard>, now: i64) -> ManualTendVerdict {   // tend-guard.ts:269-296
    let today = day_index(now); let reset_at = day_ends_at(now);
    let used = used_today(cur, now);
    let remaining = MANUAL_TENDS_PER_DAY.saturating_sub(used);
    if used >= MANUAL_TENDS_PER_DAY {                 // **写预算排在冷却前面，顺序有意义**
        return Denied { reason: "write_budget_exhausted", remaining: 0, reset_at, retry_after_ms: reset_at - now };
    }
    if let Some(g) = cur { if now < g.cooldown_until {
        return Denied { reason: "manual_cooldown", remaining, reset_at,
                        retry_after_ms: g.cooldown_until - now, cooldown_until: g.cooldown_until };
    }}
    let cooldown_until = now + MANUAL_TEND_COOLDOWN_MS;
    Ok { next: ManualGuard { day: today, used: used + 1, cooldown_until },
         remaining: remaining - 1, reset_at, cooldown_until, retry_after_ms: MANUAL_TEND_COOLDOWN_MS }
}
```
⚠️ **写预算排在冷却前面，顺序有意义**（`tend-guard.ts:251-259`）：两条同时不满足时报冷却，
运维会等 10 分钟再点一次，然后撞上预算耗尽 —— **报的那句话得是他照着能改的那一句**，
而「今天的额度用完了、到 UTC 零点恢复」与「再等 8 分钟」是两种完全不同的处置。
⚠️ **`reason` 是机器可读判别字段**：面板靠它选一句五语言文案，**靠解析 message 的中文是不行的**。
🔴 **「什么时候能再点」一律成对给：一个绝对时刻 + 一个相对时长**（`tend-guard.ts:209-221`）。
评审抓到的不对称：`202` 只给绝对的、`429` 只给相对的 ⇒ **同一个倒计时两种时钟基准**。
**口径定死：相对量用来做倒计时（免疫客户端时钟偏差），绝对时刻用来显示「几点恢复」。
绝不让客户端自己拿本地时钟去减。** 所以 ok 那一支也带 `retry_after_ms`（恒等于 600_000，
**显式给出比让调用方自己推更不容易错**）。
🔴 **如实登记的残留**（`tend-guard.ts:264-268`）：`cooldown_until` 不跟着跨天一起清（两个字段互不借用）。
**时钟往回拨 1 小时时，冷却会比 10 分钟多挡约 50 分钟。** 方向是 fail-safe 的（少写、不多写），
而让跨天顺手清冷却就等于把 `day` 的职责借给了 `cooldown_until` —— **那正是读法 A 的错处，只是方向反过来。**

### 8.6 管理接口错误码（`admin/admin-errors.ts`，210 行）

**28 条码，全小写下划线，一个点都不许有**（`admin-errors.ts:66-128`）。

```
信封级：admin_unavailable / admin_unauthorized / bad_json / body_not_an_object / unknown_field
Key 池四条写端点：key_not_found / must_disable_first / not_a_boolean / note_not_a_string
                 / note_too_long / empty_patch / keys_not_a_string_array / too_many_import_keys
                 / not_a_bulk_op / ids_not_a_string_array / too_many_bulk_ids
对外 API 密钥五条：apikey_not_found / name_not_a_string / name_empty / name_too_long
                 / expires_not_a_number / expires_in_the_past / too_many_apikeys
                 / version_not_a_number / stale_write / apikeys_unreadable
```
⚠️⚠️ **这个文件里一个字的用户文案都不许有。** 文案住在五语言字典里。
⚠️⚠️ **命名是全小写下划线，不是风格偏好，是实测踩出来的**（`admin-errors.ts:56-64`）：
第一版写成 `keys.notFound` 那种点分形态，而 i18n 检查脚本的判据是「命名空间前缀锚定的引号对」
——它把 `"keys.notFound"` 当成一次 i18n key 引用，**十条码逐条报「引用了字典里没有的 key」（实测 EXIT=1）**。

**为什么是「后端回码 + 前端查字典」而不是「后端按 `Accept-Language` 返回」**（`admin-errors.ts:11-19`）：
1. 后端按 `Accept-Language` 返回，等于把**一整份五语言字典搬进后端**，于是同一批文案有两份真源；
2. **面板的语言是用户在面板里选的**（存在 localStorage 里），不一定等于浏览器的 `Accept-Language`
   ⇒ 后端那条路在「浏览器是 en 而用户把面板切到 ja」时给出的仍然是错的语言；
3. **码是闭集、可被机器守；散文不是。**

⚠️⚠️ **边界（如实写明，别当成全称承诺）**（`admin-errors.ts:21-45`）：本表只覆盖**面板真的会把后端 `message`
画到屏幕上**的那一族。**网关业务口**（鉴权中间件、gemini 路由、media 路由、dispatcher）
对 API 客户端**仍然只说中文**，**条数刻意不写**（当初的需求书点的是「四条」，而现扫是**六处**）。
**本期不改**：那是四协议对外响应体的一部分，改它就是改已经写进五语言 API.md 的对外契约；
而 API 客户端没有「用户选定的语言」，只能走 `Accept-Language`，正是上面第 1 条论证过不该做的那件事。
它们**全部**伴随一个语言无关的 `type` / HTTP 状态码。
⇒ **作为一笔仍然欠着的账如实登记，不许在任何地方说成「五语言破口已全部了结」。**
⚠️ **不许顺手给网关那一族也加码**——半做会造出第三种状态。

**`ADMIN_ERROR_PARAMS`（`admin-errors.ts:132-201`）—— 每个码在响应体里带哪几格 params**：

```
unknown_field: [fields]      not_a_boolean: [field]     note_too_long: [max]
too_many_import_keys: [max]  not_a_bulk_op: [ops]       too_many_bulk_ids: [max]
name_too_long: [max]         too_many_apikeys: [max]    stale_write: [expected, actual]
其余全部: []
```
⚠️ **`params` 里永不许出现中文**：它会被插进 ja/ko 的字典串里，**等于把中文又漏回屏幕上**。
⚠️ **`params` 里永不回显请求体里字段的「值」**，但 `unknown_field` 的 `fields`
**会逐字回显调用方送来的字段名，这是刻意的**。
`stale_write` 的两个数都是**我们自己算出来的版本号**，`expected` 虽来自请求但已被窄化成非负整数，
**不是一段原样搬运的调用方文本**。

### 8.7 补池历史（`admin/tend-history.ts`，139 行）

```rust
const TEND_HISTORY_SIZE: usize = 50;   // tend-history.ts:25 —— Cron 每 30 分钟一轮 ⇒ 约 25 小时的补池历史
const TEND_HISTORY_KEY: &str = "tend:history";
enum TendTrigger { Cron, Manual }
struct TendRecord { /* TendResult 全部字段 */ + trigger: TendTrigger }   // tend-history.ts:16-18
```
⚠️ **`TendRecord` 用「继承」而不是把 `TendResult` 的字段抄一遍**：抄一遍就会漂。
**Rust 侧**：`#[serde(flatten)] result: TendResult` + `trigger`。

**`crashed_tend_record()`（`tend-history.ts:41-66`）—— 这一轮抛错了，给它造一条如实的记录**：

```rust
TendRecord { skipped: false, available: 0, attempted: 0, minted: 0, minted_by_channel: {},
             failures: vec![Failure { reason: "round_crashed", channel }],
             at, primary_channel: channel, duration_ms, trigger }
```
🔴 **防住的真实故障**：`tend_once` 一抛，`record_round` 整个被跳过，而 catch 里原来是**裸 console.error**、
进不了事件缓冲 ⇒ flush 首行 `buffer.is_empty()` 就 return。
**实测结果是 `{"events": [], "history": null}` ——面板上这一轮什么都没有，与「注册机根本没跑」逐字节不可区分。**
⚠️ **它也证伪了「`tend:history` 每轮汇总一定在」那句话**：那句话只对**跑完**的轮次成立。
`tend:history` 免疫的是**槽位碰撞**，不是丢失更新、更不是崩轮。
🔴 **三个数一律 0 且 `skipped: false`** —— **`skipped` 有且只有一个含义（`config.enabled == false`），
拿它表示「崩了」就是伪造。**

**窄化（`tend-history.ts:78-139`）—— 判据比事件条目严，这是刻意的**：
事件条目里 `level`/`msg`/`fields` 承载的是**上游来的证据**，一条只有 `ts` 的事件仍然是证据。
`TendRecord` 不一样——它是**一张定长表的一行**，每个字段全部由本仓自己的代码一次性写出，
面板把它渲染成一行数字汇总。**一行里 `minted` 坏掉的记录不是「不完整的证据」，是一行读不得的数，
显示它比丢掉它更容易误导运维。**
⇒ **逐字段校验，任一字段不合就整条丢掉并计数。**
`failures` 里的 `reason` **必须是联合成员，不只是字符串**：面板的失败归因渲染是 `switch` + 穷尽检查，
一个表外的 reason 会掉进 `default`，而 i18n 键也只对表里的成员齐全。
⚠️ **加字段时要做的那个决定，明写**（`tend-history.ts:84-88`）：加了必填字段之后，
**升级前写下的旧记录会因为缺这个字段被整条丢掉**（并计进 `malformed`）。
要么把新字段做成可选、要么接受最多 50 条历史在升级那一刻清零。
**编译期报错保证这个决定「会被做」，但不保证它被做对——这一条登记为已知盲点。**

### 8.8 协议目录（`admin/protocol-catalog.ts`，567 行）—— 面板四个消费者的单一真源

**它服务的四个消费者**：集成示例卡 / Playground / 模型表 / 单把 key 验活。

```rust
enum ProtocolId { OpenAI, Anthropic, Responses, Gemini }
enum Modality   { Chat, Image, Video }
const SAMPLE_PROMPT: &str = "ping";     // protocol-catalog.ts:40
struct ProtocolEntry {
    id: ProtocolId,
    label: &'static str,          // **不进 i18n**：它是协议的专名，翻译它只会制造歧义
    method: "POST",
    path_template: &'static str,  // **网关对外**的路径（客户端打这一条），`{model}` 是唯一占位符
    upstream_path: &'static str,  // **网关对上游**的路径，拼在 agnes_base_url 之后
    auth_header: "authorization" | "x-api-key" | "x-goog-api-key",
    stream_mode: "body" | "path",
    stream_key: &'static str,     // body 模式是字段名；path 模式是流式路径模板
    usage_path: Option<&[&str]>,  // 非流式响应里 usage 的位置
    stream_text_path: &[&str],    // **流式响应里「这一块新增的回答文本」在 SSE 数据行的哪一格**
    sample: fn(&str) -> Value,
}
```

| id | path_template | upstream_path | auth_header | stream | usage_path | stream_text_path |
|---|---|---|---|---|---|---|
| `openai` | `/v1/chat/completions` | `/chat/completions` | `authorization` | body `stream` | **`null`** | `["choices","0","delta","content"]` |
| `anthropic` | `/v1/messages` | `/chat/completions` | `x-api-key` | body `stream` | `["usage"]` | `["delta","text"]` |
| `responses` | `/v1/responses` | `/chat/completions` | `authorization` | body `stream` | `["usage"]` | `["delta"]` |
| `gemini` | `/v1beta/models/{model}:generateContent` | `/chat/completions` | `x-goog-api-key` | path → `/v1beta/models/{model}:streamGenerateContent` | `["usageMetadata"]` | `["candidates","0","content","parts","0","text"]` |

**sample 请求体**：
`openai`: `{model, messages:[{role:"user",content:"ping"}]}`；
`anthropic`: `{model, max_tokens:64, messages:[…]}`（**`max_tokens` 是 Anthropic 协议的必填项，少了上游会 400**）；
`responses`: `{model, input:"ping"}`；`gemini`: `{contents:[{role:"user",parts:[{text:"ping"}]}]}`（**不带 model**）。

⚠️ **`path_template` 与 `upstream_path` 是两个问题，第一版把两者混为一谈**（`protocol-catalog.ts:52-66`）：
前者回答「客户端怎么调这个网关」，后者回答「网关怎么调上游」。
**「今天四条都一样」不是省掉这个字段的理由**：单把 key 验活是这条知识的第二个消费者。
**绑住它的判据必须落在真实出站 URL 上**（不是比对本文件自己的两个字段——那是同义反复）。

⚠️ **`openai` 的 `usage_path: null` 不是「这条协议没有 usage」，是「网关这条路径不解析响应体」**
（`protocol-catalog.ts:158-163`）。usage 确实在响应里、确实到得了客户端（实测过），**只是网关没读它**。

⚠️⚠️ **`stream_text_path` 这四条不是照着协议规范抄的，是照着本网关真正吐出去的字节填的**
（`protocol-catalog.ts:102-109`）：四条里三条由本仓自己合成，只有 openai 是上游字节原样透传。
**照规范抄会在合成那三条上漂。** 绑住它的判据必须落在真 app 吐出去的 SSE 字节上。
* `anthropic` 的 `["delta","text"]`：同一条流里的 `message_delta` 事件也有一格 `delta`（装 `stop_reason`），
  它没有 `text` ⇒ 走到底取不到字符串 ⇒ 按「这一行不带正文」处理，**正是想要的**；
* `responses` 的 `["delta"]` **是字符串本身，不是一个对象**——与 anthropic 那条**同名不同形**。
  **这正是「照协议规范抄」会翻车的地方。**
* 数字段视为数组下标；走不到 / 走到的不是字符串就表示**这一行不带正文**，消费方按空串处理。
⚠️ **`stream_text_path` 是 `&[&str]` 不是 `Option`**：四条协议全都有正文增量，没有「这条协议流式没有正文」这一档。
**`null` 会和「路径写错了、取不到」在消费侧长成同一个空串。**

⚠️⚠️ **一条今天定不了案的边界，登记在此**（`protocol-catalog.ts:115-137`）：
**openai 是四条里唯一网关不解析响应体的一条**。真实上游若在流末发一块 usage（不少 OpenAI 兼容实现会发），
**那些字节会原样到达浏览器**——本字段对它们取不到正文、按「不带正文」静默跳过。
假上游下量不到，**需要一次真上游才能定案** ⇒ **不猜着加解析分支。**
面板那句话已改成只讲本面板自己的行为（「本面板在流式这一档不读 token 用量」），**与上游有没有 usage 无关、恒为真**。

**`MODEL_CATALOG`（`protocol-catalog.ts:224-345`）**：12 条，与 `MODELS` 逐条同序。
⚠️ **形态按模型名里的关键字分（image / video / 其余对话），但这条判法刻意没有被写成一个函数**
（`protocol-catalog.ts:243-248`）：形态决定这个模型走哪条端点，而端点是**逐条手写钉住的**。
**从名字推形态等于把一条上游命名约定当成本仓的控制流**——上游哪天发一个不带关键字的图片模型，
推导会静默把它归成对话模型并教出一条注定 4xx 的调法，**而没有任何东西会红**。
⚠️ **`endpoints` 逐条手写，不从 `PROTOCOLS` 生成**：生成会把「对话模型的 endpoints 与 PROTOCOLS 逐条一致」
那一格变成**同义反复**。

**媒体端点（`protocol-catalog.ts:420-484`）**：

| id | modality | op | method | path_template | upstream_path | task_slot |
|---|---|---|---|---|---|---|
| `image.generate` | image | generate | POST | `/v1/images/generations` | `/images/generations` | — |
| `video.create` | video | generate | POST | `/v1/videos` | `/videos` | — |
| `video.poll` | video | poll | GET | `/v1/videos/:id` | `/videos/:id` | `:id` |

⚠️ **媒体与对话分两张表，分界不是「对话 vs 媒体」这个分类癖，而是形状真的不同**
（`protocol-catalog.ts:67-72`）：对话四条有 `stream_mode` / `stream_text_path` / `usage_path` 三格媒体一格都用不上，
媒体三条有 `op` / `task_slot` 两格对话一格都用不上。合成一张表的代价是每一条都带着一半恒为 `null` 的字段，
**而恒为 null 的字段与「这一条没填」在消费侧长得一模一样。**
⚠️ **`video.poll` 的 `sample` 返回 `None` 不是「忘了填」**：narrow 那一侧据它决定这一条不走 `withPrompt()`，
而一个空对象会让它去找那句占位文本、找不到、**判成整份读不出来**。

```rust
const VIDEO_TASK_ID_RE: &str = r"^[A-Za-z0-9_-]{1,128}$";   // protocol-catalog.ts:365
const UPSTREAM_MODELS_PATH: &str = "/models";                // protocol-catalog.ts:499
fn with_task_id(template, slot, id) -> String    // protocol-catalog.ts:515-517 —— **split/join 而不是 replace**
fn endpoint_for(p, model, stream) -> String      // protocol-catalog.ts:520-523
```
🔴 **`VIDEO_TASK_ID_RE` 的由来**（`protocol-catalog.ts:347-364`）：原实现把 `:id` 直接拼进上游路径，
**已鉴权的客户端因此可以拿池中的真实上游 key 打上游的任意路径**
（`..%2F..%2Fadmin` 规范化后落到 `/admin`；`x%3Fsecret%3D1` 是查询参数注入）。
故先按白名单校验形状（不匹配直接 400），再 `encodeURIComponent` 做纵深防御。
⚠️ **`split`/`join` 而不是 `replace`**：后者会把替换串里的 `$&` 之类当成引用展开。
Rust 的 `str::replace` 没有这个问题，**但 `regex::Regex::replace` 有——不许用它。**
⚠️ **模型名不做 encode**：它已被 `MODEL_CATALOG` 限定成白名单。
⚠️ **这条字符集本身是一条未核实的假设**（登记在 `upstream-facts` 的 `video.taskIdCharset`）：
标识是**上游签发的**，本仓从没见过真上游签发的样本。真上游若用了 `.` `:` `+` `/` `=`，
**两段式视频接口对所有客户端结构性不可用。**
⇒ **文档化与错误文案可以改，`VIDEO_TASK_ID_RE` 一个字符都不许跟着放宽**
（放宽它会把早已解决的路径穿越 / 查询参数注入搬回来）。

**`video_task_id_shape(source)`（`protocol-catalog.ts:367-418`）—— 从正则源码派生「人读形状」**：
只认 `^[字符类]{下界,上界}$` 这一种写法，**认不出就抛，不返回一个「大概对」的串**。
⚠️ **连长度那两个数一起派生，不只是字符类那一段**：只派生字符类、把 `(1-128)` 写成字面量的话，
`{1,128}` 哪天改成 `{1,64}`，形状串一个字节都不变 ——**报文与五份文档会一起对读者说一个已经不成立的上界。**
⚠️ **它必须是纯 ASCII**：五份 API.md 逐字带着它，字符类哪天被放宽到收非 ASCII，
`en` / `ja` / `ko` 三份里会凭空出现一段中文。

### 8.9 上游模型清单（`admin/upstream-models.ts`，100 行）

```rust
const UPSTREAM_MODELS_MAX: usize = 500;    // upstream-models.ts:23
struct UpstreamModels { ids: Vec<String>, truncated: bool }
fn parse_upstream_models(payload: &Value) -> Option<UpstreamModels>   // upstream-models.ts:57-76
```
⚠️⚠️ **它不是 `protocol-catalog` 的替代品，两者回答的是两个问题**（`upstream-models.ts:8-14`）：
协议目录回答「**本网关**支持哪些协议、哪些端点、拿哪个模型名去调」；这里回答「**上游账号**此刻的模型清单长什么样」。
**拿上游的返回去替换目录，那四个消费者会立刻拿到一份没有端点、没有协议归属的清单** ——
**⇒ 并存，并且把差集明确标出来。**

**规则**：
* **读不出来是 `None`，不是空数组**：「上游一个模型都没有」与「这份响应我们看不懂」是两句话，
  **而一个空数组在屏幕上与前者长得一模一样**；
* 只认 `{ data: [{ id }] }` 这个形状。⚠️⚠️ **这个形状是推断出来的，不是实测出来的**
  ——手上没有一把有效的 Agnes key，**上游 200 的真实报文一次都没验过**；
* `data` 里混进的非对象 / 没有 `id` / `id` 不是字符串的**逐条跳过**
  ——这一条与本仓自己的目录「一条坏的就整份判成读不出来」**刻意相反**（那份是本仓自己的缺陷，必须整份报警；
  这份是别人家的清单，多一条我们不认识的记录不该让整个功能失效）；
* ⚠️⚠️ **「逐条跳过」有一个必须堵住的尽头：一条都没抽出来。**
  `data` 非空而 `ids` 为空时，逐条跳过的终局是一个空数组，**而空数组在屏幕上说的是「上游这次一个模型都没回」
  ——那是一句关于上游的事实，可我们手上只有一句关于自己的话。**
  ⇒ 这一档回 `None`。**判据是「一条都没抽出来」而不是「跳过了几条」**（跳掉两条还留下三条时那三条是真的）。
  **`data` 为空数组时不走这里**——那一档才是真的一句上游事实；
* 去重（`seen`）；超过 500 ⇒ `truncated = true` 并 break。**截断的那一半由 `truncated` 如实交代，不静默丢**
  ——静默丢会让运维以为上游就这些模型。

**`diff_against_catalog(ids)`（`upstream-models.ts:78-100`）—— 两个方向都要给**：
* `only_upstream`：目录该不该补一条（补之前得先给它填协议归属与端点）；
* `only_catalog`：本仓正在承诺一个上游此刻不认的模型名，**那是运维照着目录会踩空的那一半**。
⚠️ **`only_catalog` 不许被读成「目录写错了」**：上游按账号发模型，一把权限较窄的 key 拉回来的清单本来就会短。
**所以呈现那一侧的措辞该是「这次上游没回」，不是「不存在」。**

### 8.10 上游事实登记表（`admin/upstream-facts.ts`，202 行）—— **一张「诚实度」的机器形态**

**它解决的是循环取证**（`upstream-facts.ts:1-8`）：本仓对「真实 Agnes 上游」的形状知识，
今天几乎全部来自**我们自己写的那份假上游**。拿它当上游依据是**循环取证**：
我们先写下一句响应体，再回头把它当成「上游给的正是这一格」。
**这件事此前只散在注释里，而注释不会红——这张表就是那句话的机器形态。**

```rust
struct UpstreamFact { id, subject, assumed, status: "assumed" | "verified",
                      source: String, anchor_file, anchor, doc_sections: Vec<String>,
                      doc_hints: HashMap<Lang, String> }
```
⚠️ **`status` 只许在有本仓之外的依据时才写 `verified`**：`source` 必须以 `https://` 或 `docs/` 开头，
**且不许以 `tests/` 开头**。

**今天三条事实，全部 `assumed`**：
1. `video.taskIdSlot` —— 建任务响应把任务标识放在哪一格；
2. `video.taskIdCharset` —— 上游签发的任务标识落在哪个字符集里；
3. `openai.streamTrailingUsage` —— openai 协议的流式响应末帧带不带 usage（`source: "无"`）。

**它能做到什么 / 做不到什么，明写**（`upstream-facts.ts:37-42`）：
**能**：让「这条事实的依据强度」不再只是一句人写的散文。
**不能**：**它一个字节都不知道上游到底长什么样。它验的是登记的诚实，不是事实本身。**

⚠️ **`assumed` 那一栏只写相对句，不复述被指向的那份真源的内容**（`upstream-facts.ts:24-30`）：
上一版逐字复述了真源那张槽表的第一格长什么样，**把真源改掉之后本表两个测试文件一格都没红**
——一份没有任何机器管的第二知识，**真源一动它就静静变假**。

👉 **P1/P2/P3 每碰一次上游相关的假设，都要回来更新这张表；铸出第一把真 key 之后，
这三条该当场重验并把 `status` 改掉。**

---

## 9. 其余两个小模块

### 9.1 存储可写性健康检查（`storage-health.ts`，139 行）

**存在的理由（真机验证暴露）**（`storage-health.ts:4-17`）：Docker 部署用 `./data:/app/data` 绑定挂载时，
宿主目录的属主（通常 uid 1000）会盖过镜像里 `chown app:app` 的结果，容器内的 app 用户（uid 100）因此写不进去。
此时 `/health` 仍然返回 200——**它压根不碰存储**——容器被报告为 healthy，
**而所有 API 调用都返回 `pool_empty`、导入 key 报 EACCES。**
**这是「CI 绿 + 镜像绿 ≠ 能跑」里最坏的一类：静默失败。**

```rust
struct StorageStatus { writable: bool, checked_at: Option<i64> }
// 初始状态取「可写」这一乐观值：它表示的是「尚未观测到任何写失败」
```
**实现上刻意不在健康检查里写盘**：状态由两处更新——
① 启动时的一次性探测（`probe_writable`）；② 运行期每一次**真实**的写操作（`watch_storage` 包一层）。
⇒ `/health` 只是读一个内存里的布尔值，**30 秒一次的 HEALTHCHECK 不会产生任何写入**。

⚠️ **只观测 `put` / `delete`**（`storage-health.ts:47-53`）：读操作（`get`/`list`）在数据目录只读时照样成功，
**用它们判断可写性会得出「健康」的错误结论——真机上正是读得到、写不进。**
异常一律**原样抛出，不吞**：调用方的错误处理路径完全不变。

**`probe_writable`（`storage-health.ts:98-135`）**：

```rust
if let Err(e) = storage.put(HEALTH_PROBE_KEY, json!({"at": now()}), None).await { return Some(e); }
if let Err(e) = storage.delete(HEALTH_PROBE_KEY).await {
    logger.error("storage.probe_cleanup_failed", "健康探针键清理失败（不影响「存储可写」的结论）", ...);
    health.record(true, now());   // 按 put 的结论纠正回来
}
None
```
🔴 **可写性的结论只看 `put`**：`delete` 只是「不留痕迹」的收尾，**它失败并不能推翻「刚刚确实写进去了」这个事实**。
原实现把两步一起 try，于是「写得进、删不掉」会被误报成 degraded——**而那种情况下探针键还留在存储里，
两个后果同时发生。**
**探测失败不阻止进程启动**：让容器起来并把 `/health` 报成不健康，**比崩溃重启循环更容易排障**
——运维能直接 curl 到原因，也仍然会在 `docker ps` 里看到 unhealthy。
⚠️ `HEALTH_PROBE_KEY = "health:probe"` 与 `key:` 前缀不冲突，**因此即使删除那一步失败，也不会被 `all()` 当成一把 key。**

### 9.2 补池调度循环（`tend-scheduler.ts`，99 行）—— **自重排的 setTimeout 递归，不是 setInterval**

🔴 **`setInterval` 的周期在创建时定死，没有任何 API 能改一个已存在 interval 的周期**（`tend-scheduler.ts:1-9`）。
后果是 `TEND_INTERVAL_MS` 成了面板「保存」**唯一一处确定会骗人**的字段
——其余配置项每一轮都会重读、每一轮都生效，只有它不行。

```rust
const MAX_SAFE_TIMER_DELAY_MS: i64 = 2_147_483_647;   // tend-scheduler.ts:54
async fn tick(deps) {
    let mut next_ms = last_good_interval_ms;
    // **一定重排**（Rust 侧用 loop + defer 语义或显式 scopeguard）
    let _ = catch(deps.run_once()).map_err(|e| { let _ = catch(|| deps.on_error("run", e)); });
    match catch(deps.read_interval_ms()) {
        Ok(v)  => { next_ms = clamp(v); last_good_interval_ms = next_ms; }
        Err(e) => { let _ = catch(|| deps.on_error("read_interval", e)); }
    }
    deps.set_timer(tick, next_ms);   // **必须在 finally / 无条件路径上**
}
tick();   // **立即跑一轮**：否则冷启动后要空等满一个间隔（默认 30 分钟）才开始补池
```

🔴 **`MAX_SAFE_TIMER_DELAY_MS` 的钳制不是洁癖**（`tend-scheduler.ts:42-53`）：
延迟参数用 32 位有符号整数表示；超过这个值时 `node:timers` **既不抛错也不钳到上限**，
而是**静默把延迟当成 `1`**（已实测：`setTimeout(fn, 5_000_000_000)` 实际约 1ms 后就触发）。
**后果方向和直觉相反**：把 `TEND_INTERVAL_MS` 设成一个超大值，效果不是「补池几乎永不触发」，
而是**只受一次 `read_interval_ms` IO 往返约束的紧循环**——每一轮都读一次配置、可能触达邮箱/Agnes
（`in_flight` 挡的是并发，挡不住高频触发）。
👉 **Rust 的 `tokio::time::sleep` 没有这个坑（`Duration` 最大约 68 年），
但「上界钳制」这条照留，因为它同时防住了 `i64` 溢出与 `Duration::from_millis` 的负值 panic。**

🔴 **`set_timer` 必须在无条件路径上，不能只是「写在最后一行」**（`tend-scheduler.ts:63-67`）：
位置性保证挡不住将来有人在中间插一段 `return`/`throw`；**`finally` 是唯一能让「重排」在这个函数的
任何提前退出路径上都成立的结构化手段。**
🔴 **`on_error` 自己抛错也必须被就地吞掉，绝不能因此不重排**（`tend-scheduler.ts:31-39`、`:72-78`）：
上报 Sentry、写审计日志、无监听器时同步抛的 `EventEmitter.emit('error')` 都是真实场景。
后果不是「补池永久停止」而是**整个网关进程崩溃**（Node 20+ 默认 `--unhandled-rejections=throw`），
**转发能力也一起没了**。
🔴 **`read_interval_ms` 抛错时沿用上一次已知的合法值**：链条断了就是「面板一次误操作永久停掉补池」，
**比补池失败严重得多**。
⚠️ **`read_interval_ms` 不能用 `build_tend_deps` 拿**：那个函数在注册机未启用时返回 `None`，
而**定时器必须在关闭状态下也继续存在**（否则启动时关着就再也打不开）。
⚠️ **`set_timer` 自己抛错是唯一真正无法自救的失败点**——调用方必须保证这个函数不抛。

---

## 10. 注册机（`registrar/`，10 文件 2,704 行）—— 🔴 **P3 才移植，本轮只做规格，不实现**

> 本节只写「规则是什么」，不写 Rust 骨架。P3 动手前请连同本节与原仓 `registrar/` 各文件的注释再读一遍。

### 10.1 顶层流程

```
定时轮（默认 30 分钟）/ 面板「立即补池」
  → 抢补池锁 → tend_once()
      1. config.enabled == false            ⇒ skipped: true，一次上游请求都不发
      2. available = 池里 counts_toward_target 的把数
         need = target_keys - available；need <= 0 ⇒ 直接返回（**走不到收尾**，见 10.5）
      3. **退避闸**：还在 registrar:backoff 窗口里 ⇒ 整轮跳过（failures: [upstream_backoff]）
      4. rounds = min(need, mint_batch)
         轮开头：读域名台账一次 + provider.list_domains() 一次
      5. for i in 0..rounds:
           delay = if i == 0 { 0 } else { mint_delay_min + rand()*(max-min) }
           **轮级墙钟预算判据**（只有手动轮传 round_budget_ms）
           sleep(delay); attempted += 1
           candidates = select_domains(ledger, all_domains, now, max_domain_attempts, rand, used_this_round, rejected_this_round)
           out = mint_one(...)
           成功 ⇒ repo.add(key)（**照存不误**）；可疑 ⇒ 另记 key_suspicious
           失败 ⇒ 按 reason 决定要不要 abort_round
      6. **收尾（finally）**：域名台账最多写 1 次、退避键按需写 1 次
      7. minted > 0 ⇒ reconcile_after_mint()
```

### 10.2 配置（`registrar/config.ts`，615 行）

```rust
struct RegistrarConfig {
    enabled: bool,
    channel: Channel,          // yyds | moemail —— **二选一，没有主备、没有自动降级**
    target_keys: u32,          // 默认 20
    mint_batch: u32,           // 默认 5
    tend_interval_ms: i64,     // 默认 1_800_000
    code_timeout_ms: i64,      // 默认 120_000
    mint_delay_min_ms: i64,    // 默认 60_000   🟢 **实测下界**
    mint_delay_max_ms: i64,    // 默认 90_000   🟡 抖动上界，不是实测
    max_domain_attempts: u32,  // 默认 1        🟢 实测 + 推导
    token_name: String,        // 默认 "auto"
    agnes_platform_url: String,// 默认 "https://platform-backend.agnes-ai.com"
    yyds: Option<ChannelCreds>, moemail: Option<ChannelCreds>,
    blocked: bool,             // **装载的产物，不是旋钮**
}
// YYDS 的 baseUrl 有内置取值 "https://maliapi.215.im"；MoeMail 没有（自建服务）
```
🟢 **`mint_delay_min_ms = 60_000` 是量出来的下界**（`registrar/config.ts:109-116`）：
≥60 秒间隔时同一出口连续四次发码全部成功，**第七次才撞上应用层限流**；
**无间隔连发时第三次就被上游前置的边缘限流挡下**。
⚠️ **这个数是观测不是承诺**：来自单一出口、单日样本，换出口或换时段可能完全不同。**所以它留在可配那一侧。**
🟢 **`max_domain_attempts = 1`**（`registrar/config.ts:122-130`）：有了域名台账之后，
一个名额里的**第二个**域名期望收益为负（多烧一格限流预算，而暖机后成功率提升趋近 0）；
而「一个名额里连打好几次」正是实测中触发边缘限流的那个形态。
**域名轮换没有消失，只是从「一个名额之内」搬到「两个名额之间」。**

🔴 **`channel` 从 `primary` + `fallback` 改名成 `channel`，改名不是换措辞**（`registrar/config.ts:14-26`）：
**只要类型、存储键、补丁路径、错误码里还留着「主」这个字，内部就还是主备，界面上把第二个下拉藏起来只是又一个谎。**

🔴 **装载器模块级零 `throw`（唯一豁免是消费方护栏 `require_channel`）**（`registrar/config.ts:59-70`）：
注册机是**可选子系统**，它缺凭据不该让转发、`/health`、面板一起死。
今天抛错的结果是**容器起不来**。
⇒ 装载器**只产出 `blockers`，启不启动由消费方 gate**。
⚠️ **`blocked` 为真时 `enabled` 一个字都不改**：「运维明明打开了，面板却说未启用」是另一种撒谎。
**真话是「已启用 · 本次没跑起来」。** 消费方一律**先判 `enabled` 再判 `blocked`**。
⚠️ **`blocked` 可以在 `enabled == false` 时为真**（`delay_min_gt_max` 不受门控）。

**`RegistrarLoad { config, blockers, notices }`（`registrar/config.ts:59-91`）—— 两个集合不许合并**：
blocker = 「注册机本次不启动」；notice = **不拦人、但必须说出来**，恰恰配着「注册机照常跑」。
⚠️ **notices 是状态，不是事件**：走 `GET /admin/api/config`（零写）带出去、渲染成常驻横幅。
做成事件的两条硬理由各自单独成立：
① 装载器每 30 秒刷一次 ⇒ 每实例每天约 2880 次装载，**而事件环只有 100 格，
   它会在运维升级完来查问题的那一刻把诊断挤出去**；
② 补池每轮都重新装载一次，装载期的无条件 warn 会把「健康的一轮零事件零写」变成
   「每轮至少一条事件 ⇒ 跨过刷新窗后每轮一次 put」。

**`pos_int()`（`registrar/config.ts:138-181`）—— 非法值两侧都降级，不抛**：
🔴 **这是本轮改动里唯一一处主动放弃的 fail-fast**。它与 `num()` 对 env 的策略**故意不一致**，
理由见 §7.3（有没有安全的降能模式）。
⚠️ **不产出 blocker**：字段回落之后注册机照样跑得起来。
**代价明写**：运维**不能再靠「容器崩了」发现部署笔误**，只能靠面板红横幅与事件。
⚠️ 事件名取 `config.invalid` 而不是 `registrar.*`：面板与文档里「字段级降级」这件事只有一条事件名。
代价是它拿不到 `[registrar]` 前缀，`fields.field` 里的 `registrar.` 路径前缀是唯一的归属线索。

**`resolve_channel()`（`registrar/config.ts:230-271`）—— 候选四级，本级判死就不往下穿透**：
`REGISTRAR_CHANNEL > REGISTRAR_PRIMARY > registrar.channel > registrar.primary`。
🔴 **`??` 链会让 `REGISTRAR_CHANNEL=yydss` 静默穿透成别名或存储里那条通道**
——运维写错一个字母，网关拿另一条通道去跑，**一句话都不说**。
**「缺席」的判据四级同源**：`undefined` / `null` / **空串**都算没写
（env 侧的空串 `REGISTRAR_CHANNEL=` 在 compose 里极常见）。

**`migrate_stored_registrar()`（`registrar/config.ts:276-322`）—— 旧主备键的唯一真源**：
读路径拿 `notices`，写路径拿 `next`。**两处共用这一份，不许各写一份**
——这一条一旦分叉，**面板上的横幅会与存储里的实际键说两句不同的话**。
规整动作只有两步：`channel` 缺席时把 `primary` 的值抬上来；然后删掉 `primary` 与 `fallback`。
⇒ **运维在面板上保存任意一次设置，旧键消失、横幅随之消失。**
⚠️ **`fallback == 本次生效的那条通道` 时一条 notice 都不产**：那份配置从前就被「备通道等于主通道」那条 blocker
拦着、**从来没有生效过**，说「丢弃了一条降级路径」是假话。
⚠️ **已知边界**：`fallback` 写的是一个非法值时**只删键、不产 notice**（那条通道一把 key 都没铸过）。

**`creds()`（`registrar/config.ts:329-374`）—— 缺什么就产出哪一条 blocker，不抛、也不发明取值**。
🔴 **两条通道都解析凭据，但只有选中那条的缺凭据产 blocker**（`registrar/config.ts:564-590`）：
只给「选中那条」解析的话，未选中通道的 `cfg.yyds` / `cfg.moemail` 恒为 `None`，
而面板的 `channelConfigured()` 判据正是「这一格非 null」。三个消费者会连锁失真：
`/status` 的 `configured` 变成假话、「测试通道」对未选中那条恒 409、「自动注册」指定它也恒 409。
**于是「想在切换前先测一下另一条」必须先切过去保存 —— 鸡生蛋，而那恰恰是「二选一」模型下最核心的工作流。**
⚠️ **`if enabled` 这道门保留**：关着的注册机不解析任何凭据。
**代价（关着时两张卡都显示「未配置」）是今天就有的，如实登记为已知缺口。**

**三条启动期告警（只 warn、不产 blocker）**：
1. `registrar.interval_shorter_than_worst_round`（`registrar/config.ts:468-499`）：
   `worst_round_ms = mint_batch × code_timeout_ms + max(0, mint_batch - 1) × mint_delay_max_ms`。
   ⚠️ **间隔那一项是后来补上的**：默认间隔从几秒改成几十秒之后，它不再是可以忽略的尾巴
   （默认配置下占 360 秒，而整轮最坏是 960 秒）。**漏掉它，这条 warn 就会在一份真的会重叠的配置上保持沉默。**
2. `registrar.domain_attempts_costly`（`registrar/config.ts:501-511`）：`max_domain_attempts > 2` 时。
   **这条 warn 是「调大它的代价」唯一说得出口的地方**——面板上它只是一个数字输入框。
3. `registrar.attempt_exceeds_worker_budget`（`registrar/config.ts:513-562`）：
   `worst_attempt_ms = code_timeout_ms + max(0, max_domain_attempts - 1) × mint_delay_max_ms > 780_000` 时。
   **今天真实的后果**：一轮补池没有任何东西会把它截断，而补池锁的有效期是 900 秒
   ⇒ **这一轮可能跑过自己那把锁的有效期**，锁一过期，下一轮触发或另一个共卷副本就会在它还活着的时候并发开跑。
   🟡 **事件名里那个 `worker` 是一处已知残留，刻意没改**：它是五语言文档逐字给运维的 grep 锚点，
   改名必须**源码与五份文档同一次改完**。建议的新名是 `registrar.attempt_exceeds_round_budget`。

🔴 **`worst_attempt_ms` / `worst_round_ms` 这份口径散在多处，改一处就得全改**
（那张点名表**只写在 `registrar/types.ts` 的 `SCHEDULED_ROUND_BUDGET_MS` 上方一处**，
因为它曾被抄成四份、四份点名的集合互相不一致 ——**照任一份走都会漏掉一个文件**）：
`registrar/types.ts` 的 `SCHEDULED_ROUND_BUDGET_MS`、`registrar/config.ts` 的告警、
`registrar/tender.ts` 的 `worstAttemptMs`、`wire.ts` 传给「立即补池」的预算，外加五语言文档的散文。

**`require_channel(cfg)`（`registrar/config.ts:598-615`）—— 本模块唯一的 `throw` 豁免**：
**它不在装载路径上**——装载器全函数化说的是「一份坏配置不该让网关起不来」，
而这里是**消费方护栏**：走到这里还没有通道，说明某个消费者跳过了 `enabled` / `blocked` 两道 gate，
**那是代码 bug，必须响。**

### 10.3 时间常量（`registrar/types.ts`，150 行）

```rust
const REGISTRAR_REQUEST_TIMEOUT_MS:   i64 = 15_000;   // types.ts:22  注册链上**每一个** HTTP 请求
const SCHEDULED_ROUND_WALL_CLOCK_MS:  i64 = 900_000;  // types.ts:37  = 补池锁 TTL
const SCHEDULED_ROUND_BUDGET_MS:      i64 = 780_000;  // types.ts:88  = 上面的 87%，留约 120 秒余量
// ── 手动补池专用一族（**与上面那族不是一回事，不许合并**）──
const MANUAL_MINT_BATCH:          u32 = 1;            // types.ts:109
const MANUAL_MAX_DOMAIN_ATTEMPTS: u32 = 1;            // types.ts:120
const MANUAL_CODE_TIMEOUT_MS:     i64 = 60_000;       // types.ts:129
const MANUAL_ROUND_BUDGET_MS:     i64 = 70_000;       // types.ts:150
```

🔴 **`REGISTRAR_REQUEST_TIMEOUT_MS` 为什么必须有**（`types.ts:7-22`）：注册机的所有耗时预算都建立在
「每个请求都会及时返回」这个**未言明的前提**上。**一次挂起的请求就能把单轮拖过锁的 TTL
→ 下一轮在上一轮还活着的时候开跑 → 两轮同时占邮箱名额**；容器这时被重启（`docker compose up -d` / OOM kill）
则正在铸的那个邮箱的 `finally` 不会执行 → **邮箱泄漏 → 悄悄耗尽活跃邮箱配额**。

**`SCHEDULED_ROUND_WALL_CLOCK_MS = 900_000` 的上下界**（`types.ts:24-37`）：
* **上界**：必须**明显短于**补池间隔（默认 30 分钟）。锁是这么用的：进程被硬杀时 `finally` 一行都不跑，
  锁只能靠自然过期放开——TTL 取到间隔的一半，**代价上界就是最多跳过一轮**；
* **下界**：必须盖得住一轮真实最坏耗时，否则**一把还活着的锁会被下一轮读成空的，两轮同时开跑**。

🔴 **`SCHEDULED_ROUND_BUDGET_MS` 今天没有运行期消费者，只剩启动期交叉校验这一个用途**（`types.ts:39-51`）：
`tend_once` 的 `round_budget_ms` 是可选参数，**只有手动那一轮传**，定时轮一直不传。
**别把它读成「运行期真的会截断」——今天不会。**
那 120 秒余量是留给注册链上那几个 15 秒请求超时、以及准备阶段的（`loadDomainLedger` + `listDomains`）。
**把它们也算进判据是行不通的**——理论最坏本来就高于 900 秒，那样会变成一次尝试都不敢开始。

🔴 **手动那一族的来历（Cloudflare 平台日志原话，不是推断）**（`types.ts:90-108`）：
面板「立即补池」回 202 之后把整轮交给 `ctx.waitUntil`，而平台在**响应结束后约 30 秒**就把它取消掉，
**实测 3/3 复现**。取消不抛异常 ⇒ 两层 `try/catch/finally` 一个都不执行 ⇒
不写补池历史、不发事件、锁泄漏到自然过期 ⇒ **点一次按钮把注册机整个停摆一刻钟。**
处置是**换载体**——端点自己 `await` 这一轮再返回。
⚠️ **平台前提虽然没了，这一族仍然留着**：换完之后这一轮的载体就是**这条 HTTP 请求本身**，
于是它的墙钟上限变成「浏览器 / 反向代理愿意等多久」，**那是一个比定时轮小一个量级的数**。
**⇒ 手动一族不是定时轮那一族的一个副本，两者今天仍然不许合并。**

🔴 **`MANUAL_MAX_DOMAIN_ATTEMPTS = 1` 不是「保守一点」**（`types.ts:111-120`）：
它让 `worst_attempt_ms` **退化成常量**。不压它的话，运维把 `MAX_DOMAIN_ATTEMPTS` 调成 2，
`worst_attempt_ms` 就跳到 150 秒 > 预算 ⇒ 预算判据当场判「这一次尝试开不起来」
⇒ **按钮变成永远铸不出 key 的诚实空转，而且空转得毫无征兆。**
🔴 **`MANUAL_ROUND_BUDGET_MS = 70_000` 不是耗时上界，别读成上界**（`types.ts:131-150`）：
在 `MANUAL_MINT_BATCH = 1` 之下它**只被判一次**，语义是「这一次尝试开不开得起来」。
唯一的硬约束是**必须严格大于 `worst_attempt_ms`（= 60 秒）**。
⚠️⚠️ **原注释「多出来的 10 秒留给 `elapsedMs`」是错的，而它错得很贵**：
判据里的 `elapsedMs` 从前是从**整轮开头**算起的，中间隔着 `listDomains()` 这类**单请求就允许 15 秒**的准备动作
——**10 秒的余量根本不够，上游邮箱服务慢一次这颗按钮就诚实空转**，还打出一条指向 `CODE_TIMEOUT_MS` 的
错误处置（而手动轮的 `code_timeout_ms` 已经被压到 60 秒，调它没有用）。
⇒ **处置不是把这个数抬到 85 秒**（那只是把同一条赌注的赔率改一改），
而是**把预算判据的起点挪到准备阶段之后**（见 10.5）。

### 10.4 跨轮退避（`registrar/backoff.ts`，248 行）

🔴 **为什么非有它不可**（`backoff.ts:1-16`）：上游那两层限流的惩罚窗口都比「一轮补池」长得多，
**而窗口里每打一次请求就把窗口续一次**。从前撞上限流之后只在一次尝试内换个域名接着打，
于是一轮里的每一次请求都还在窗口里、每一次都把窗口续上——**打得越多，恢复得越晚**。
光靠「本轮立刻中止」还不够：Cron 是雷打不动地每 `TEND_INTERVAL_MS` 来一轮，
窗口比补池间隔长的时候，**每一轮都会去续一次窗口，正是同一个缺陷换了个尺度重演**。
**⇒ 退避必须跨轮，而且必须指数。**

```rust
enum BackoffKind { Edge, App, Cluster }
struct BackoffState { until: i64, kind: BackoffKind, since: i64, hits: u32 }
const EDGE_BACKOFF_MS:    i64 = 900_000;      // backoff.ts:85   🟢 实测 + 余量
const APP_BACKOFF_MS:     i64 = 1_800_000;    // backoff.ts:95   🔴 **没有任何实测支撑，是猜的**
const CLUSTER_BACKOFF_MS: i64 = 1_800_000;    // backoff.ts:120  🔴 同上
const BACKOFF_MAX_MS:     i64 = 14_400_000;   // backoff.ts:128  = 4 小时封顶（≈8 轮）
```
🟢 **`EDGE_BACKOFF_MS = 900_000`**：实测那一层的惩罚窗口约 14 分钟才退净，且窗口内每打一次就续一次。
向上取整再加 1 分钟余量。⚠️ **这个数是观测不是承诺**。
🔴 **`APP_BACKOFF_MS` / `CLUSTER_BACKOFF_MS` 没有任何实测支撑，是猜的，不许在别处被写成实测结论**：
我们只知道「≥60 秒间隔下第 7 次会触发」，**窗口有多长我们没量过**。
取一个补池周期，依据只有一条：**我们不知道它多长，所以至少跳过下一轮。**
⚠️ **第一次撞时它很可能一轮都跳不过去**：它恰好等于 `TEND_INTERVAL_MS` 的内置取值，
而窗口判据是 `until > now` 这一处**严格**比较 ⇒ 下一轮准点到达时窗口刚好算过去了。
**压住请求量靠的是指数那一段（30min → 1h → 2h → 4h 封顶），不是第一格。**
🟡 **`BACKOFF_MAX_MS` 封顶的取舍**：超过这个长度基本可以确定是**出口 IP 被长期限了**，
那该让运维在面板上看见并去换出口，**而不是让程序无限等下去**。

🔴🔴 **三档 `kind` 的证据类型不同，别把它们读成「三层限流」**（`backoff.ts:28-50`）：
* `edge` / `app` —— 上游**回话里的字眼**落进了词表。这两档说的是「撞的是哪一层限流」；
* `cluster` —— **我们的词表一个字都没命中**，证据是**这一轮的形状**。
  **它存在的唯一理由是「分类器读错了的那一档」**：上游改了限流文案 ⇒ 真限流被逐条读成域名屏蔽
  ⇒ 前两档一个都不会产生。

🔴🔴 **别把 `cluster` 读成「上游没说过限流」，那句话在它自己要治的场景里就是假的**：
上游改了限流文案的时候，上游**明明说了一句限流的话**（判据夹具里逐字是 `Slow down, mate.`），
只是**我们的词表没认出来**。这两句话不是一回事，**而混同它们的代价是实的**：
面板横幅曾经据此逐字写着「上游一句限流的话都没说」，**等于在最该提示「可能是上游改了限流文案」的那一刻
反过来否掉这个方向**。
⇒ 这一档成立时**两种可能都还开着**，面板文案与文档**必须把两个方向都摆出来**。

**`hits` 数的是「这一串」里撞了几次**（`backoff.ts:56-73`）。
⚠️ **「连续」指的是「连续几轮零产出」，不是「连续几次请求」**：一轮里既铸出了 key 又撞了限流时
**重新起一串**（`hits` 回到 1），因为那一轮上游明明还在给我们发号 —— **它不是「越限越死」的那种形态**。
**`since` 同时是「这是哪一串」的身份**：只在**重新起一串**时推到当前时刻，`merge_backoff` 靠比它大小分辨串。

```rust
fn in_backoff(s: Option<&BackoffState>, now: i64) -> bool { s.map_or(false, |s| s.until > now) }
fn retry_after_ms(s, now) -> Option<i64>    // 不在窗口里时是 None（口径逐字照 manual_tend_quota）
fn streak_broken(prev, now) -> bool { prev.is_none() || now - prev.until > BACKOFF_MAX_MS }
fn next_backoff(prev, kind, now) -> BackoffState {
    let streak = if streak_broken(prev, now) { None } else { prev };
    let hits = streak.map_or(0, |s| s.hits) + 1;
    let base = match kind { Edge => EDGE, App => APP, Cluster => CLUSTER };   // **穷尽查表，不是三元链**
    let factor = 2u64.pow(min(hits - 1, 30));     // 先夹后乘：hits 很大时 2**(hits-1) 会溢出
    let span = min(base * factor as i64, BACKOFF_MAX_MS);
    BackoffState { until: now + span, kind, since: streak.map_or(now, |s| s.since), hits }
}
```
⚠️ **`BASE` 写成穷尽的查表而不是三元链**：`BackoffKind` 再加一档时这里会在编译期报错，
**而三元链只会安静地把新档落进 `else` 那一支**。Rust 的 `match` 天然穷尽，✅。
⚠️ **`streak_broken` 刻意复用 `BACKOFF_MAX_MS` 当阈值，不新增第四个常量**（`backoff.ts:157-174`）：
合法的一串里，两次撞之间最多隔一个「上一段窗口 + 一个补池间隔」，而窗口本身封顶就是它。
**它治的是一条真实的驻留路径**：池子填满之后 `tend_once` 在 `need <= 0` 那里就 return 了，
**根本走不到收尾** ⇒ 一把 `hits` 很高的退避键会一直留在存储里；
等池子再耗干时**第一次撞限流就直接跳到封顶那一档**。

🔴 **`merge_backoff`（`backoff.ts:206-235`）—— 丢更新在这把键上最疼**：
丢掉 `until` 等于退避窗口凭空消失 ⇒ 继续打 ⇒ 每打一次续一次窗口 ⇒ **正好回到本次要修的那个缺陷**。
所以**窗口本身一律取更保守的那个**：`until` 取大的，`kind` 跟着胜出的 `until` 走。
⚠️⚠️ **`hits` 不能照着 `until` 那条「取大的」办，这是一条实测出来的坑**：
`next_backoff` 重新起一串时写回来的 `hits` 是 1，而存储里那份旧的可能是 5 ——
**无脑取大就把「重新起一串」在落盘这一层原地撤销掉，而 `tend_once` 那一侧的判据（注入的假 saveBackoff）照样全绿：
一份行为在两个地方各说各话。**
⇒ **判据是 `since`**：它只在重新起一串时才前进，所以 `since` 更大的那一份说的就是更新的那一串，
`hits` / `since` **整对**跟着它走；两份 `since` 相同才是同一串，那时才取 `hits` 更大的。

**`narrow_backoff` 的失败方向与台账相反，这是刻意的**（`backoff.ts:130-137`）：
台账读坏了当成「什么都没记住」是安全的（多探几次），**而退避读坏了当成「还在退避中」会让注册机静默停摆**。
这里 fail-safe 的方向是**放行**，代价明写：一把被写坏的退避键 = 多打一轮。

⚠️ **退避时长刻意做成常量、不加 env 旋钮**（`backoff.ts:237-248`）：加一个旋钮要连带
`.env.example` + provenance + `EDITABLE` + 别名表 + 面板一格 + 五语言文档表 + 测试网格。
⚠️ **同样没有加的：跨轮令牌桶。** 它确实能挡住「Cron 一轮 + 两次手动补池在半小时里叠出十几次发码请求」
这种叠加，但要付一整圈门禁，而退避键 + 整轮中止已经把主路径堵死了。
**如实登记为已知缺口，不是「已经防住了」。**

### 10.5 补池一轮（`registrar/tender.ts`，889 行）

**`TendFailureReason`（`tender.ts:16-93`）—— 13 个成员，运行期表 + 编译期穷尽双向钉住**：

```
domain_blocked_all / upstream_error / code_timeout / register_failed / login_failed
/ key_failed / provider_error / network_error / rate_limited      ← mintOne 的产物
provider_missing   ← 选中的通道压根没构造出 provider，是**接线错误**
round_crashed      ← **整轮抛出了异常**
key_suspicious     ← **铸出来了，但那串 key 材料本身可疑**
upstream_backoff   ← **这一轮还在退避窗口里，一次都没开始**
```

🔴 **`key_suspicious` 是本表唯一「不是失败」的成员**（`tender.ts:38-56`）：这一轮的 `minted` 照常 +1、
`mintedByChannel` 照常记账，因为**上游那边账号是真的建出来了、一个临时邮箱是真的花掉了，说 0 是伪造**。
它出现在 `failures` 里是因为 `failures` 是这份结果里**唯一**能逐条带 `channel` 说明「这一轮有什么不对劲」的通道，
**而运维必须在补池历史那一行上看得见它。**

🔴 **`round_crashed` / `upstream_backoff` 的共同理由**（`tender.ts:26-37`、`:57-72`）：
**「这一轮根本没跑」不该靠合读 `attempted === 0 && failures[0]` 去推**——面板的补池历史上这一行
必须能自己说清发生了什么。
🔴 **绝不许改用 `skipped: true` 表示它们**：**`skipped` 有且只有一个含义（`config.enabled == false`），
拿它表示别的就是伪造。**
⚠️ **`upstream_backoff` 不区分撞的是哪一层限流**：两档都归到既有的 `rate_limited`，
层级由退避键的 `kind` 与事件带出去。**代价如实登记**：只看补池历史那一行分不出上一次撞的是哪一层。

**`TendResult`（`tender.ts:132-208`）**：

```rust
struct TendResult {
    skipped: bool,                     // **有且只有一个含义：config.enabled == false**
    available: u32,                    // 本轮开始时**占着 target_keys 名额**的 key 数
    attempted: u32, minted: u32,
    minted_by_channel: HashMap<String, u32>,
    failures: Vec<Failure>,            // { reason, channel }
    at: i64,                           // 这一轮**开始**的时刻（不是结束时刻）
    primary_channel: String,           // 这一轮用的那条通道；skipped 时记空串
    duration_ms: i64,
}
```
⚠️ **`available` 不要读成「能打上游的 key 数」**（`tender.ts:134-148`）：**被管理员停用的、
以及正在冷却的 key 都计入这个数**，而这两种恰恰都是不能打上游的。
**整池 3 把全在冷却时这个数仍然说 3，实际能服务的是 0。**
**这一栏等于「池子里没被剔除的把数」。** 名字没改是因为它已经落进 `tend:history` 持久化、也进了运维日志。
**给建补池历史板块的人：这一栏不许按「可用」渲染。**
⚠️ **`primary_channel` 名字里的 `primary` 是历史格式留下的**，今天系统里已经没有主备了；
**名字不改**：它已经逐字持久化进 `tend:history`（改名会让升级前的历史条目被整条判成 malformed 丢掉），
而且它写在五语言 API.md 的响应示例里——**那是已发过 tag 的公开仓的响应体契约**。
⚠️ **`minted_by_channel` 今天与 `minted` + `primary_channel` 等价**（二选一之后一轮只可能有一条通道产出）。
**保留它的唯一理由是存量历史条目的形状。** 没铸出来的通道**不出现在表里**（不是记 0）。
🔴 **`{}` 有五个产出者，消费方必须靠别的字段把它们分开**（`tender.ts:164-176`）：

| `{}` 的来源 | 怎么认出来 |
|---|---|
| 注册机关着 | `skipped == true` |
| 健康轮，缺口 `need <= 0` | `!skipped && attempted == 0 && failures.is_empty()` |
| 整轮抛错 | `attempted == 0 && failures` 里是 `round_crashed` |
| 退避窗口内整轮跳过 | `attempted == 0 && failures` 里是 `upstream_backoff` |
| 尝试了但全失败 | `attempted > 0 && !failures.is_empty()` |

**⇒ `skipped` + `attempted` + `failures` 三个字段合读才分得清。别拿 `minted_by_channel` 的空表去推断。**

**`summarize_failures()`（`tender.ts:210-226`）**：聚合成 `yyds:register_failed×3 moemail:code_timeout×1`。
**两个入口的收尾日志必须给出同一份口径**，否则排障方式就得按形态写两套。

🔴 **`round_started_at` 挪到准备阶段之后，这是一条修复不是整理**（`tender.ts:421-453`）：
预算判据（`elapsed + delay + worst_attempt > round_budget`）问的是**「还开不开得起一次完整的尝试」**。
而从整轮开头算起时，`elapsed` 里装着 `repo.all()` / `load_backoff()` / `load_domain_ledger()` /
`provider.list_domains()`，**其中 `list_domains` 单请求就允许 15 秒**。
手动轮预算 70 秒、`worst_attempt` 恒 60 秒 ⇒ **留给 `elapsed` 的只有 10 秒**
⇒ **上游邮箱服务挂起一次（15 秒 > 10 秒），i=0 那次判定就不通过** ⇒ 整颗按钮诚实空转，
而且空转时打出的是 `registrar.round_budget_impossible`：「这是配置问题不是瞬时状况，请调小 CODE_TIMEOUT_MS」
——**一条确切而错误的处置**。**人恰恰是在池子出问题时才会去点那颗按钮，也就是最可能撞上这一格的时刻。**
⇒ 起点挪到准备之后，i=0 时 `elapsed ≈ 0`，判据退化成 `worst_attempt > round_budget` ——**一个纯配置量**。
⚠️ **代价明写（不是零成本）**：准备阶段不再计入预算 ⇒ Cron 那一轮的实际墙钟最坏比 780 秒多出一段准备时间
（**本来就是那份预算留了余量的东西**，780 + 余量仍在 900 秒之内）。

**预算判据的两条分支（`tender.ts:466-493`）**：
* `attempted == 0` ⇒ `error` 级 `registrar.round_budget_impossible`：
  「这不是『这轮差一点』而是『这个配置在本运行时下永远铸不出 key』，每一轮都会原地打转」。
  **走 error 且措辞必须与下面那条区分开**：`failures` 是空的、`minted < attempted` 是 `0 < 0` 为假，
  两个入口的归因日志一条都不会打，**这里是唯一能说破它的地方**。
* 否则 ⇒ `warn` 级 `registrar.round_budget_exhausted`（提前收尾，剩余名额留给下次调度）。
🔴 **关键在于「不开始」而不是「跑到一半停下」**：被从中间砍断时 `mint_one` 的 `finally` 不会执行，
**邮箱就漏了**；主动提前收尾则每一次尝试都完整走完。
**代价只是这一轮少铸几把，下个调度周期会接着补，使用者不需要调任何参数。**

**失败处置的 switch（`tender.ts:561-636`）—— 只决定「要不要中止整轮」，不再决定「换不换通道」**：

| reason | 处置 |
|---|---|
| `provider_error` / `network_error` / `code_timeout` / `domain_blocked_all` / `register_failed` / `login_failed` / `key_failed` | 本次名额作废，本轮的下一个名额照常开始（先睡 `mint_delay_min..max`） |
| `rate_limited` | 🔴 **立刻中止整轮** + 记退避窗口 |
| `upstream_error` | **立刻中止整轮**（Agnes 后端整体故障，继续本轮只会在故障期间制造更多注定失败的请求） |

⚠️⚠️ **两条通道是二选一，一条通道失败绝不会去碰另一条**（`tender.ts:561-580`，本轮的行为变更）：
从前 `provider_error` 与 `code_timeout` 会降级到备通道；现在与其余几支处置完全一样。
**没有跨轮退避、也不新增**：失败不改变下一轮的时间（固定的定时器），轮内节流已有两层。
**加自动退避等于偷偷把「二选一」变成「二选一 + 自适应调度」，运维在面板上看到的补池节奏会与配置对不上。**
⚠️ **变坏的地方点名写出来**：从前「这条通道收不到验证码」还有一次换通道的机会；
现在同一条通道收不到验证码 = **这一轮乃至这一天一把都铸不出来**，直到运维自己去面板换通道。
**故障形态从「补池变慢但还在出 key」变成「补池零产出、池子慢慢耗干、几小时到几天后以 pool_empty 503 炸出来」
——把一个自愈的故障换成了一个要人管的故障。这是「二选一」的固有代价，不是实现缺陷。**
⚠️ **switch 本身必须留着**：它还担着 `upstream_error ⇒ abort_round` 与穷尽性断言。

🔴 **`rate_limited` 那一支：这一轮铸出过 key 就重新起一串**（`tender.ts:598-611`）：

```rust
let next = next_backoff(if minted > 0 { None } else { backoff }, out.limit_kind, now());
```
从前这里无条件传 `backoff`，于是「前 4 把成功、第 5 把撞限流」这种**默认参数下最常见的一轮**
照样把 `hits` 一路推上去：连着几轮都长这样 ⇒ 15min → 30min → 1h → 2h → 4h(封顶)，
**而每一轮其实都在正常出 key**。**指数退避要治的是「越打越死」，不是「出着 key 顺带撞了一次上限」。**
⚠️ **`minted` 在这一行已经是这一轮的终值**：本支紧跟着 `abort_round = true`。

**铸出来之后（`tender.ts:538-557`）**：

```rust
await repo.add(out.key);      // **照存不误**：刻意不加校验、也不 trim()
minted += 1; *minted_by_channel.entry(channel).or_insert(0) += 1;
if !is_importable_key(&out.key) {
    failures.push(Failure { reason: KeySuspicious, channel });
    logger.error("registrar.minted_key_suspicious",
        "上游发回来的 key 含有不可打印字符或空白，已照常存进池子，但它多半每次被选中都会让转发失败\
         （拼进 authorization 头时会抛 TypeError）；请在面板上停用或删除它",
        fields { channel, key_length: out.key.len() });   // **不带明文**：只报长度与通道
}
```
🔴 **在这里拒收 = 销毁凭据 = 数据丢失**（见 §4.5 那张两条路处置对照表）。
⚠️ **`is_importable_key` 此前只挂在面板导入这条「人点一下」的路径上，而稳态下 key 进池子的主路径是这一行**
——不对称是登记过的，处置不同是刻意的，**但"不报"从来不是选项**。

**收尾 `finish_round()`（`tender.ts:697-856`）—— 放在 `finally` 里是刻意的**：
整轮抛错时（`round_crashed` 那一档）**退避截止时刻同样必须落盘**，否则一次崩轮就把「别再打了」这条结论丢掉。
**整段自己包一层 catch** —— 收尾出错不该掩盖正在飞的那个异常。

⚠️ **「结论没变就一次 put 都不发」是写配额账的一根轴**：Cron 每 30 分钟一轮，
稳态下台账的结论不再变动 ⇒ 这把键的写次数是 **0** 而不是 48 次/天。
（判据：一轮铸 5 把，`registrar:domains` 只被 put 一次；退避键一次都不写。）

🔴 **`cluster` 那一档的形状证据，两条，取并集**（`tender.ts:728-776`）：
```rust
let cluster_shape = committed.discarded > 0 || round_all_rejected;
if to_save.is_none() && minted == 0 && cluster_shape {
    to_save = Some(next_backoff(backoff, Cluster, now()));
}
// round_all_rejected = rejected.len() > 0 && rejected.len() == probed.len() && probed.len() == listed.len()
```
① `committed.discarded > 0` = 钳位生效 = **同一轮里 ≥2 个域名被判成「域名被屏蔽」**；
② `round_all_rejected` = **上游列出来的域名这一轮一个不落全试过了，而且全被判成「域名被屏蔽」**。

🔴🔴 **② 是补上来的，它治的是一条实测出来的、对整类部署完全失效的漏洞**：
**只配了一个邮箱域名的部署永远凑不满 ① 要的那 2 个**，于是上游一改限流文案，
那种部署每一轮都打满 `mint_batch` 次注定失败的发码请求、**一个退避键都不写**。
**实测（测试替身、内置值、20 轮）**：逐轮请求数恒为 5、`minted` 恒为 0、退避 20 轮全是 `null`
⇒ 稳态 **240 次/天**；接上 ② 之后同一份夹具是 `[5,5,0,5,0,0,0,5,0,0,0,0,0,0,0,5,0,0,0,0]`
⇒ 稳态 **约 30 次/天**（封顶那一档每 8 轮打一次 × 5 次/轮，48 轮/天）。
⚠️ **② 真正多接住的只有「上游只给出一个域名」那一种**（≥2 个域名时 ① 早就成立）。
写成更一般的形状，只是因为**这一档的语义本来就该是「这一轮的候选全军覆没」**，
而不是「凑够两个」——**「凑够两个」是钳位的口径，被顺手借用成了退避的口径，那次借用就是这条漏洞的来源。**
⚠️ **「一个不落全试过」这半句是承重的，不许省**：`journal` 里只有拿到过可分类回话的域名。
省掉它的话，「池子快满、这一轮只开了 1 个名额、而它恰好撞上一个真被拉黑的域名」也会记退避。
⚠️ **不并进钳位本身**：钳位管的是「学不学这条域名结论」，改它会让单域名部署再也学不到 `blocked`、
`registrar.domain_blocked` 那条带着上游原话的事件再也发不出来 —— **而横幅恰恰要运维去翻那条原话。**
⚠️ **`minted == 0` 这个前提是承重的，不是保险起见**：这两条证据在「上游真的成批拉黑了好几个域名」时同样会成立，
而那一档**这一轮照样铸得出 key**（`rejected_this_round` 把被拒过的跨档排到最后）。
**那时上游明明还在给我们发号，记退避等于自己把补池按停。**

🔴 **代价登记（可证不是推断）：钳位生效的那一支拿不到全部上游原话**（`tender.ts:786-806`）：
`commit_journal` 里 `applied` 在钳位生效时把**全部** `blocked` 判定滤光 ⇒ **`newly_blocked` 可证恒为空**
⇒ 那条 `registrar.domain_blocked` **一条都发不出来**。
而 `domain_verdicts_discarded` 的 fields **一个字的上游原话都不带**。
⚠️ **这一支上唯一带得出上游原话的是 `registrar.known_good_domain_rejected`，而它只覆盖台账里已知能用的那些域名**
——冷启动、或者上游这一轮新列出来的域名，**一句原话都没有**。
⇒ **面板横幅与五语言文档都按支分开写，不许说成「去翻 domain_blocked 就能分辨」。**

**收尾发的三类事件**：`registrar.domain_verdicts_discarded`（钳位生效）/
`registrar.round_all_domains_rejected`（单域名部署那一支，**照那条事件的名字发出去就是假话，所以另起一条**）/
`registrar.domain_blocked`（**只有第二跳判死才记**）。

**退避键的最终落盘（`tender.ts:849-855`）**：

```rust
if let Some(s) = to_save { deps.save_backoff(s).await?; }
else if minted > 0 && backoff.is_some() {
    // 铸出来了 ⇒ 上游现在认我们 ⇒ 把那把陈旧的退避键清掉，指数从头数
    // **只有键真的存在时才写**：否则每一轮成功补池都要白付一次 put
    deps.save_backoff(None).await?;
}
```

🔴 **`reconcile_after_mint()`（`tender.ts:858-889`）—— 本轮真的铸出 key 之后再对一次账**：
`index_add` 是**没有 CAS 的读-改-写**，而边缘读缓存可能让它读到「对账刚修好之前」的那份索引
⇒ **同一轮里刚被捡回来的孤儿转眼又成了孤儿**。
**连带后果比「索引脏了」严重得多**：孤儿不被 `all()` 看到 ⇒ `available` 偏小 ⇒ 缺口被高估 ⇒ **超额补铸**，
而每铸一把都是一次真实的 Agnes 建号，同时撞注册风控与两条通道各自的活跃邮箱上限。
**补铸出来的 key 还会再次触发同一条覆盖，一轮一轮地滚。**
这一步**不消灭那个窗口**（没有 CAS 就消灭不了），只把它从「一整轮」压到「一轮之内」。
**失败只记一条 warn 就算了**：索引残留是 fail-safe 的，让它把一轮**已经成功铸出了 key** 的 tend 变成异常，
只会让入口打出误导性的「补池失败」，**而实际上 key 已经落盘了**。

**`rejected_this_round(journal)`（`tender.ts:671-695`）**：从本轮观测本子现算，**同一个域名以最后一条观测为准**
（先被拒、后来又成功过的，这一轮不该再被让到后面——口径与 `commit_journal` 的折叠逐字相同）。
**不落盘、不跨轮**：下一轮从空集重新开始 ——**一个好域名不可能被它永久降权。**

**`used_this_round`（`tender.ts:386-402`）—— 装的是「派出去的候选」而不是「真的打过的域名」，是刻意的取舍**：
🔴 **为什么非有它不可**：台账**轮内不更新**（落盘统一在收尾），而 `select_domains` 对「已知 ok」那一档
是按 `at` 的全序排序 —— `at` 轮内不变 ⇒ 默认 `MAX_DOMAIN_ATTEMPTS = 1` 时
**一轮 5 个名额确定性地全落在同一个域名上**（6 分钟内 5 个账号挂在同一个域名下），
而那一档的说明逐字写着「LRU 轮换，别把一个好域名打成上游风控的焦点」。
**轮换从前只发生在轮与轮之间，而「打成焦点」这件事发生的正是轮内那几次连续注册。**
要精确知道「真的打过哪几个」就得改 `MintOutcome` 的形状 —— 那是两个 case 都要动的公开结构。
**多标几个的代价只有一条：档内多转几格，而档与档的优先级一格都不动。**

### 10.6 单次铸号（`registrar/mint.ts`，388 行）

```rust
enum MintOutcome {
    Ok { key: String },
    RateLimited { limit_kind: BackoffKind, marker: Option<String> },
    Failed { reason: "domain_blocked_all" | "upstream_error" | "code_timeout" | "register_failed"
                    | "login_failed" | "key_failed" | "provider_error" | "network_error" },
}
```

**主循环**：对 `candidates` 逐个：
```
i > 0 ⇒ **必须真的等一下**（sleep(mint_delay_min + rand()*(max-min))）
        🔴 从前这里是裸 continue（零间隔连打），而实测的边缘限流正是被「无间隔连发」触发的：
           一个全新出口连发到第 3 次就开始 429
create_mailbox(domain) 失败 ⇒ warn registrar.create_mailbox_failed，换下一个域名（**不记域名结论**）
created_any = true
try {
    r = send_code(agnes, mailbox.address)
    verdict = classify_send_code(r.status, r.body)
    rate_limited_edge/app ⇒ 🔴 **当场 return RateLimited，不换域名、不 sleep、不记任何域名结论**
    domain_blocked  ⇒ message = upstream_message(r.body, address)
                       若 is_known_good(ledger, domain, now) ⇒ warn registrar.known_good_domain_rejected（**只诊断，不改控制流**）
                       record_verdict(journal, domain, "blocked", message); continue
    unreadable      ⇒ warn registrar.send_code_unclassified；**不产生任何域名判定**，换下一个
    upstream_error  ⇒ saw_upstream_error = true; warn registrar.send_code_bad_status; continue
    2xx ⇒ record_verdict(journal, domain, "ok")     ← **干净可靠的证据，无条件覆盖回 ok**
    code = poll_code(mailbox, code_timeout_ms)
    code.is_none() ⇒ warn registrar.code_timeout；return Failed(code_timeout)
    password = random_password(rand)
    reg = register(...);  !reg.ok ⇒ warn registrar.register_rejected; return Failed(register_failed)
    lo  = login(...);     lo.token.is_none() ⇒ warn registrar.login_no_token; return Failed(login_failed)
    created = create_key(token, token_name);  key.is_none() ⇒ warn registrar.key_not_returned; return Failed(key_failed)
    return Ok { key }      // **账号密码到此为止，不返回也不持久化**
} catch(err) {
    warn registrar.network_error；**一条域名结论都不记**（它说明的是「这次请求没走通」，
    不是「上游怎么看这个域名」。把它接进台账等于用噪声污染证据）
    return Failed(network_error)
} finally {
    delete_mailbox(mailbox)    // **用完即删**，失败只记一条 warn（不掩盖已产出的返回值或异常）
}
```
**循环结束后的归因优先级**（`mint.ts:366-388`）：
`!created_any` ⇒ `provider_error`（**通道级失败**：凭据失效、活跃邮箱配额耗尽、邮箱服务挂了）；
否则 `saw_upstream_error` ⇒ `upstream_error`；否则 `domain_blocked_all`。
**限流不在这里——它当场 return，根本走不到这一行。**
⚠️ **`msg` 里不许出现「可降级到备通道」那半句**（`mint.ts:371-375`）：它会渲染进面板事件板块、
进容器 stdout、进事件下载。**二选一之后永远不会自动切通道，照着旧文案说一遍，
运维就会「等它自己切过去」而实际上永远不会切 —— 那是把一个要人管的故障说成了自愈的故障。**

🔴🔴 **`domain_blocked` 那一支曾经有第二道保险，它被拆掉了，理由是实测出来的一条死锁**（`mint.ts:185-226`）：
那道保险是「已知 ok 的域名回 400 就当成出口限流、当场 return、一条域名结论都不记」。
在**已知好域名真的被上游拉黑**时：
① 一条结论都不记 ⇒ 台账里那条 `ok` 的 `at` **永远不刷新**；
② 当场 return ⇒ 同一次尝试里后面的候选一个都不试；
③ `tender` 据此中止整轮并记指数退避。
而 `select_domains` 档内按 `at` 升序 ⇒ 那个 `at` 冻住的坏域名**每一轮都排第一**
⇒ 每轮「打它一次 → 判成限流 → 停整轮 → 退避翻倍」，**一把 key 都出不来，直到 `OK_TTL_MS`（7 天）把那条 ok 过期掉才自愈**。
**实测探针：连着 6 轮 minted 全 0，而台账里另外三个好域名一次都没被派出去。**
🔴 **拆掉它没有丢掉「防误判」**：`commit_journal` 的**两跳规则**本来就在防同一件事，
外加一轮最多学 1 条的钳位、以及排序永不 filter。**保险与两跳规则重复，却额外制造了上面那把死锁。**
⚠️ **代价如实登记（三条，后两条是量出来的）**：
① 一个好域名挨一次误判会从候选第一档掉到「待复查」那一档；
② 🔴 **「上游改了限流文案」那一档的请求量从 1 次/轮涨到 `mint_batch` 次/轮**，
   而那道保险还在时它写退避、拆掉之后一个退避键都不写 ⇒ 处置接在 `finish_round` 的 `cluster` 那一档；
③ 🔴 **接上处置之后仍然比那道保险还在时贵**：封顶后**约 30 次/天**；保险还在时**约 6 次/天**；
   接处置之前**约 240 次/天**。**30 不是 6** —— 换来的是那把 `at` 冻住的死锁不会回来。
**三条加起来仍比 7 天零产出便宜得多，但它不是零成本。**

**`step_message(status, body, address, secrets)`（`mint.ts:79-110`）—— 注册链后三步的诊断，两档**：
* **非 2xx** ⇒ 上游拒了，正文是它的裁决词 ⇒ 走 `upstream_message`（抹邮箱地址 + 抹**本步在场的已知凭据** + 抹 `sk-` 片段 + 截断 + 回查）；
* **2xx 却没解析出目标字段** ⇒ 🔴 **只说字段名，一个值都不说**。这一档的正文**极可能就装着那把令牌/key 本身**
  （「上游把字段改名了」正是它要说的话），而运维在这一档需要的恰恰只有字段名：
  **看到 `data.token_id` 他就知道名字变了。**
  正文不是 JSON 对象时落回上一档——那种正文里不会有我们没认出来的令牌。

**为什么这三步非要有它**（`mint.ts:82-89`）：**这三步是最贵的失败**——走到这里时临时邮箱已经真的建出来、
验证码已经真的向 Agnes 要过一次（吃掉的是全网关共享的限流预算），**上游那边甚至可能已经把账号建好了**。
而上一版的三条 warn 只有 `address`——**四种真因在面板上长得一模一样，处置却完全不同。**

### 10.7 域名台账（`registrar/domain-ledger.ts`，686 行）

```rust
const DOMAIN_LEDGER_KEY: &str = "registrar:domains";   // domain-ledger.ts:47
const DOMAIN_LEDGER_CAP: usize = 512;                  // domain-ledger.ts:60
const OK_TTL_MS:    i64 = 604_800_000;                 // domain-ledger.ts:68  = 7 天   🟡 没有实测支撑
const BLOCK_TTL_MS: i64 = 86_400_000;                  // domain-ledger.ts:77  = 24 小时 🟡 没有实测支撑
const BODY_SNIPPET_MAX: usize = 512;                   // domain-ledger.ts:82
struct DomainEntry { s: "ok" | "blocked", at: i64, n: u32 }
struct DomainLedger { v: 1, updated_at: i64, total: Option<u32>, entries: HashMap<String, DomainEntry> }
```
**第三态 `unknown` 用「不在 `entries` 里」表示**（`domain-ledger.ts:84-89`）——
**少一个能撒谎的字段，也省掉「`s: "unknown"` 与不在表里哪个优先」这种没人会去测的分歧。**
⚠️ **`n` 数的是轮不是观测条数**（`domain-ledger.ts:95-101`）：`commit_journal` 先把同一个域名在这一轮里的
多条观测折叠成一条，所以一轮之内最多 +1。**这条不是文风，是「两跳 = 两轮」的全部依据。**
**`DOMAIN_LEDGER_CAP = 512`**：**键空间有界不等于值有界**——这把键恒为 1 把，
但它的**值**是按上游 `list_domains()` 的返回长起来的。512 的由来：上游今天返回 374 个域名，留出余量。
⚠️ **这是别人家服务的当前取值、不是常数，所以它不进面板文案**
——把一个「当前取值」印在面板上，运维会把它当成自己这套部署的事实。
🟡 **`BLOCK_TTL_MS` 刻意取 24 小时而不是更长**：一条错记录的寿命上界越短越好，**而代价极小**
——过期之后它重新进候选，最多多花 1 次发码请求；真黑名单域名会被两跳规则立刻压回去。
⚠️ **`total` 是上一轮记下来的观测值，不是现打一次 `list_domains()` 问出来的**：
`/status` 今天一次上游请求都不发，**为了凑一个总数破掉这条性质，等于给面板加了一颗每几秒就打一次上游的按钮**。
**取不到就如实回 `None`。**

🔴🔴 **两种 400 必须分开，而分开的判据是启发式的，这句话不许被改写成事实**（`domain-ledger.ts:16-30`）：
上游用 `400` 同时表达「这个域名被屏蔽了」与「你这个出口发得太频繁了」，**正文是唯一的区分线索**，
而正文的措辞是别人家服务的实现细节。
⇒ 判死走的是**负向匹配**（「不是限流就算屏蔽」），**因此必然会误判**。
**三层东西压着这个误判，缺一层就变成静默的系统性记错**：
① 判死要两跳（`n >= 2`）**而且两跳必须来自两轮**；
② `commit_journal` 的**一轮最多学 1 个域名**的钳位（**与文案无关**）；
③ `select_domains` 全序排序**永不 filter** ⇒ 判死的域名照样在候选里，只是排最后。
⚠️ **我们没有见过「域名真被屏蔽」那条 400 的正文长什么样。这是知识空白，不是代码事实。**
真机抓到一条之后应该把判死改成**正向**匹配。

```rust
const RATE_LIMIT_MARKERS: [&str; 6] = ["too many", "rate limit", "ratelimit", "too frequent", "频繁", "次数过多"];
fn classify_send_code(status: u16, body: &str) -> SendCodeClass {      // domain-ledger.ts:201-218
    if status == 429 || status == 403 { return RateLimitedEdge; }      // **403 并进 edge**
    if status == 400 {
        let t = body.trim();
        if t.is_empty() { return Unreadable; }
        let lower = t.to_lowercase();
        return if RATE_LIMIT_MARKERS.iter().any(|m| lower.contains(m)) { RateLimitedApp } else { DomainBlocked };
    }
    if (200..300).contains(&status) { Ok } else { UpstreamError }
}
fn edge_marker(body: &str) -> Option<&str> { body.contains("1015").then_some("1015") }   // domain-ledger.ts:220-228
```
⚠️ **词表是启发式，不是协议**：它命中 ⇒ 判限流（**安全**）；它不命中 ⇒ 判域名屏蔽（**有风险**）。
⚠️ **`403` 并进 `edge`**：它在本仓从前是一条**死分支**（撞到的实测全是 429 与 400），
但并进来比留一条「睡 5 秒接着打」的路安全——**后者正是把惩罚窗口一次次续上的那个形态**。
⚠️ **`edge_marker`（实测形态 `error code: 1015`）只进事件字段，不参与任何判定**：
判据是状态码 429，而这一串是别人家边缘网关的实现细节。**让它参与判定就等于把一条外部实现细节焊进控制流。**
⚠️ **`unreadable` 不是「域名被屏蔽」的同义词**：判死走的是负向匹配，而负向匹配至少要先有一段正文可读。
**这一档不产生任何域名判定。**

**脱敏（`domain-ledger.ts:236-344`）**：

```rust
const UNSAFE_UPSTREAM_MESSAGE: &str = "<上游正文里仍有邮箱地址或凭据，已整段丢弃>";
const ADDRESS_PLACEHOLDER: &str = "<邮箱地址>";
const SECRET_PLACEHOLDER:  &str = "<凭据>";
const CREDENTIAL_LIKE: &str = r"sk-[A-Za-z0-9_-]{6,}";   // 与 dispatcher 逐字同源
fn upstream_message(body: &str, address: &str, secrets: &[String]) -> String {
    let mut redacted = body.to_string();
    if !address.is_empty() {
        redacted = redacted.replace(address, ADDRESS_PLACEHOLDER)
                           .replace(&url_encode(address), ADDRESS_PLACEHOLDER);
    }
    for s in secrets { if s.is_empty() { continue; }   // **空串必须跳过**
        redacted = redacted.replace(s, SECRET_PLACEHOLDER).replace(&url_encode(s), SECRET_PLACEHOLDER); }
    redacted = CREDENTIAL_RE.replace_all(&redacted, "sk-***").into();
    let out = body_snippet(&redacted);
    // **回查打在真正会被写出去的那一段上**：替换本身也可能拼出新的一处命中
    let hunted: Vec<_> = once(address).chain(secrets).filter(|s| !s.is_empty()).collect();
    if hunted.iter().any(|s| out.contains(s) || out.contains(&url_encode(s))) { UNSAFE_UPSTREAM_MESSAGE.into() }
    else { out }
}
```
🔴 **顺序是「先脱敏、后截断」，反过来会在边界上漏出地址前缀**（`domain-ledger.ts:269-273`）：
从前是先截断再替换：地址正好跨在 512 那一刀上时，**前半截留在正文里**，而回查查的是**完整**地址、
查不出来 ⇒ 一个 `u0@x` 这样的前缀原样进事件。**泄漏量小，但那句「替换完再回头查一遍」的纪律在那一档从前是没做到的。**
🔴 **纪律：替换完再回头查一遍，还命中就整段丢掉。**
**「我们没想到的编码形态」的后果因此是「少说一句话」，不是「多漏一个地址」。**
⚠️ **代价明写**：回查只查**完整地址**（含 URL 编码形态），**不查本地部分**（形如 `u0` 的短串，
回查它会把一大堆本来无害的正文整段丢掉）。**这是有意取舍，不是遗漏。**
⚠️ **空串必须跳过**：JS 里 `"".split("")` 会把整条正文拆成单字符，而 `includes("")` 恒为真
——不跳过的话**一个空 secret 就能让每一条诊断都变成 `UNSAFE_UPSTREAM_MESSAGE`**。
（Rust 的 `str::replace(s, "")` 行为不同，但**「空 secret 让回查恒真」这条照样成立**。）

**`body_field_names(body)`（`domain-ledger.ts:314-344`）**：顶层 + `data` 下一层的字段名，最多 20 个；
非 JSON 对象（含数组、字符串、数字）一律 `None`。
🔴 **只走两层、只取 20 个**：再深就等于把整份响应结构画出来，而这条诊断只需要认出「名字变了」。

**`select_domains(...)`（`domain-ledger.ts:346-473`）—— 🔴 全序排序 + 取前 N，绝不是 filter**：

```rust
// 排序键（从最前到最后）：rejected(跨档) → tier → spent(档内) → order → 域名字典序
tier: 1 = ok 且未过 OK_TTL_MS（按 at 旧→新，LRU 轮换）
      2 = 不在表里的、以及 ok 已过期的（按洗牌名次，**真正的未知**）
      3 = blocked 但已过 BLOCK_TTL_MS、或 n == 1 的「可疑」（按 at 旧→新）
      4 = blocked 且 n >= 2 且未过期（列表末尾，按 at 旧→新）
rejected: 本轮已被上游明确拒过 ⇒ 排到**全表最后**（**跨档，压过 tier**）
spent:    本轮已经派出去过几次（**只影响档内次序**）
limit:    夹到至少 1 —— 返回空数组会让这一次尝试连一个域名都没有
```
🔴 **「只要 `all_domains` 非空、`limit >= 1`，返回值就非空」是结构性回答**：
最坏情况（全表被判死）退化成台账出现之前的随机轮换，**而不是「一个候选都挑不出来 ⇒ `attempted` 恒为 0
⇒ 注册机静默停摆」**。
🔴 **`spent` 用次数而不是布尔**：布尔在名额数多于域名数时会退化成「p q r p p」
（第四个名额之后大家都是「派过」，次序又塌回 `at`），**而次数给出的是真正的轮转「p q r p q」**。
🔴 **`spent` 只在档内生效，绝不跨档**：跨档的话「只有一个已知 ok 域名 + 一堆判死域名」这种台账
**会在第二个名额上把判死的那些顶到前面** —— 那正是台账存在的理由要挡掉的事。
🔴 **`rejected` 必须跨档**：`commit_journal` 的钳位在「同一轮里两个已知 ok 的域名同时被拉黑」时
会把两条结论**整体作废** ⇒ 台账一个字都不变 ⇒ 下一轮的排序与这一轮**逐字节相同**
⇒ 每一轮都把全部名额喂给那两个坏域名，**补池归零直到那两条 ok 过 7 天**。
**光靠档内轮换救不了它：那两个域名自己就是第一档，档内怎么转都轮不到第二档的域名。**
⚠️ **两者敢不敢跨档的判据**：`spent` 表达的是「派出去过」（**噪声**），
`rejected` 表达的是「上游这一轮当着面拒了它」（**证据**）。
⚠️ **同档内 `at` 相同时以域名字典序兜底**：输入顺序本身是上游返回的顺序 —— **那不是我们能断言的东西。**

**`commit_journal(...)`（`domain-ledger.ts:506-606`）**：

```rust
// ① 一轮之内同一个域名只留最后一条（保留首次插入的次序）
let folded: IndexMap<String, Observation> = journal.observations.iter().map(|o| (o.domain.clone(), o)).collect();
// ② **折叠之后再算钳位**
let blocked_domains: HashSet<_> = folded.values().filter(|o| o.verdict == Blocked).map(|o| &o.domain).collect();
let clamp = blocked_domains.len() >= 2;         // 🔴 **一轮最多学 1 条 blocked**
let applied = folded.values().filter(|o| !(clamp && o.verdict == Blocked));
// ③ 逐条合并：方向相同 ⇒ n += 1；方向变了 ⇒ n = 1；at = now
//    s == blocked && n >= 2 && !was_dead ⇒ 进 newly_blocked（**只有第二跳判死才记事件**）
// ④ 值的有界性：超过 CAP ⇒ 按 (at, 域名) 升序删掉最旧的那些
// ⑤ dirty = changed || (total 非 None 且与旧值不同)；**dirty 为假时调用方一次 put 都不许发**
```
🔴 **「一轮最多学 1 条」是本设计里唯一与上游文案无关的防线**（`domain-ledger.ts:522-536`）。
判据取自实测形态：**限流发作是「一轮里连着好几个域名全挂」**，而有台账之后正常的域名屏蔽是零星的。
⚠️⚠️ **这一格是那条防线的唯一检测点**——删掉它，「上游改了限流文案 ⇒ 好域名被成批判死」就重新变成静默的
（形态与 `tend-guard` 登记的「读法 A 的唯一检测点」逐字同源）。
🔴 **两跳 = 两轮，所以同一个域名在这一轮里的多条观测先折叠成一条**：
不折叠的话「两跳」在**一轮之内**就走得完（钳位数的是**域名数**，而 `n` 从前是按**观测条数**累加的
⇒ 同一个域名在一轮里被拒两次 ⇒ 钳位不触发、`n` 直接到 2 ⇒ **一轮之内从 unknown 判死并发出事件**）。
⚠️ **`discarded` 数的是折叠之后的域名数，不是观测条数。**
⚠️ **第一跳刻意不记事件**：事件环只有 100 格，冷启动一轮就能产出好几条，
**第一跳也记会在最该看诊断的那一刻把诊断挤出环。**
**学得慢完全可以接受：我们要的是记住好域名，不是记全所有坏的。**

**`merge_domain_ledger(cur, next)`（`domain-ledger.ts:608-632`）**：同名域名取 `at` 更大的那个，
`total` 取新的那份。**丢一条域名结论只是下一轮重学（便宜）**，所以合并只把后果从「整份覆盖」降到「逐域名取更新的那一条」。

**`narrow_domain_ledger` 的失败方向**（`domain-ledger.ts:125-133`）：
任一字段读不得 ⇒ **丢掉那一条**；整个对象读不得 ⇒ 返回**空台账**。
也就是「什么都没记住，多探几次」，**绝不返回「全被屏蔽」**——后者会让注册机一把 key 都铸不出来，
**而面板上看不出任何原因。**

### 10.8 Agnes 站点四步（`registrar/agnes.ts`，195 行）

```
GET  {platform_url}/api/verification?email=<urlencoded>&purpose=register   headers: x-user-language: zh-CN
POST {platform_url}/api/user/register   body { email, password, password_confirm: password, code }   x-user-language: zh
POST {platform_url}/api/user/login      body { username: email, password }                           x-user-language: zh
POST {platform_url}/api/token           body { name }   headers: authorization: Bearer <token>       x-user-language: zh-CN
每一步：content-type + accept = application/json；signal = AbortSignal.timeout(REGISTRAR_REQUEST_TIMEOUT_MS)
```
⚠️ **`platform_url` 与网关转发用的 `agnes_base_url` 不是同一个服务。**
🔴 **四步统一返回 `{ status, body: String }`，一步都不许直接 `.json()`**（`agnes.ts:40-47`）：
直接 `json()` 的那一版把**正文本身**吃掉了，解析失败时只剩一个 `null`，
**而「上游回了什么」正是调用点唯一能拿去归因的东西**。
⚠️ **正文在这里不截断、不脱敏**：那是调用点的职责（**它才知道这一步有哪些凭据在场**）。
**本函数只负责把证据完整交出去。**
🔴 **上一版这三步分别返回 `bool` / `Option<String>` / `Option<String>` —— 状态码与正文在函数边界就被丢光了**
（`agnes.ts:65-79`），于是三条事件只写得出一句「Agnes 注册被拒」，**四种真因在面板上长得一模一样**。
**`login` 的 token 在四个可能位置之一，四处都要认**（`agnes.ts:118-154`）：
`data.access_token` / `data.token` / 顶层 `access_token` / 顶层 `token`。
⚠️ **`token == None` 有两种形态，靠 `status` 分**：非 2xx（上游拒了）与 2xx 却没认出令牌（上游改了字段名）。
**非 JSON 正文（网关超时/维护页会以 200 返回）按取不到令牌处理，不抛。**
`create_key` 同理认 `data.key` / 顶层 `key`。
**`random_password(rand)`（`agnes.ts:186-195`）**：字母表 `abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789`
（**去掉了易混字符 l/o/O/I/0/1**）取 14 位，**末尾固定加 `Q!` 满足复杂度要求**。
**账号密码到此为止，不返回也不持久化。**

### 10.9 验证码提取（`registrar/code.ts`，160 行）—— 🔴 三条正则每个量词都必须有上界

```rust
const MAX_GAP: usize = 200;         // code.ts:47  class 属性值 / 关键词到下一段数字
const MAX_TAG_GAP: usize = 400;     // code.ts:50  **标签剩余的全部属性** —— 不许复用 MAX_GAP
const MAX_WS: usize = 32;           // code.ts:53  `>` 与验证码之间的排版空白
const MAX_BODY_LEN: usize = 1024 * 1024;   // code.ts:93
const MAX_SUBJECT_LEN: usize = 1024;       // code.ts:94
```
**实测表（构造性输入，非理论推断）**（`code.ts:8-25`）：

| 模式 | 64 KiB | 1 MiB |
|---|---|---|
| `[^"']*verification[^"']*["']` | 820 ms | **约 215 秒** |
| `[^>]*>`（载荷需多个 `class=` 起始位置） | 382 ms | **约 139 秒** |
| `[^\d]*\b(\d{6})\b` | 约 85 ms | 约 22 秒 |
| **全部限长后** | <5 ms | **约 150 ms** |

**这是同步 CPU，会把整个事件循环（含四个协议的转发）堵死。**

🔴 **上界取多少必须逐段按真实数据论证，不能把一个数机械地套到所有段上**（`code.ts:27-46`）：
**这个文件已经在同一个坑里栽过两次**（第一次漏了 `[^>]*` 没限，第二次把 200 直接套到 `[^>]` 上 ⇒ 真实模板取到错码）。
* `[^"']{0,200}` —— 段内容是 **class 属性值**，真实模板几十字符；
* `[^\d]{0,200}` —— 这段**本来就跨不过任何数字**（`padding:10px` 里的 10 就能截断它）；
* `[^>]{0,400}` —— 段内容是**标签剩余的全部属性**，与上面完全不同的长度分布：
  真实营销模板一个 `style=` 就能到 200+，**实测两个「正常但不极端」的模板已经吃到 175/179 字符**；
* `\s{0,32}` —— 真实 Agnes 模板是 `>143770<`（零空白），但格式化过的模板是换行 + 缩进；
  **实测「换行 + 12 空格」这种普通缩进在 `{0,8}` 下就会失配。**

🔴 **超界的后果不是返回 `None`，而是返回一个看起来合理的错码**：快路径失配后会落到兜底的
「全文第一个六位数」，**正文里只要在验证码之前出现过任意一个六位数（订单号 / 工单号 / 活动 ID）
就会被当成验证码提交上去**——白白烧掉一个补池名额和一个临时邮箱，Agnes 那边还会注册失败。
**所以这些上界宁可留足余量，代价只是几十毫秒。**

**三级匹配（`code.ts:139-160`）**：

```
0. body 为空 ⇒ None
1. 先抹掉 CSS 十六进制颜色 `#[0-9a-fA-F]{6}\b` ⇒ 空格（避免 #123456 被当成验证码）
2. TAGGED_CODE（**只在 body 上跑，不含 subject**）:
   class=["'][^"']{0,200}verification[^"']{0,200}["'][^>]{0,400}>\s{0,32}(\d{6})\s{0,32}<
   ⚠️ **数字必须是该元素的直接文本** —— 这个严格性是判别器，不是缺陷：
      verification-title 这类容器不含数字时匹配失败，引擎会自动前进到下一个候选容器（verification-code）
      **放宽成 [\s\S]*? 会让第一个开标签吞掉全文，后面的容器再无机会**
3. KEYWORD_ANCHORED_CODE（在 `subject + " " + body` 上跑，大小写不敏感）:
   (?:验证码|verification[\s-]?code|auth[\s-]?code)[^\d]{0,200}\b(\d{6})\b
   ⚠️ **两侧 \b 缺一不可**：`[^\d]` 吃不进数字但吃得进字母（abc887766 会误命中），
      右边界防止从时间戳/工单号这类长数字串里切出前六位
4. 兜底：`\b(\d{6})\b`（定长量词，线性扫描，1 MiB 实测约 4 ms）
```

🔴 **`bounded_for_scan(s, max)`（`code.ts:96-119`）—— 截断点落在一串数字中间会凭空造出伪词边界**：
例如尾部是「订单号 1234567890」，切在第 6 位后变成「订单号 123456<EOF>」，末尾就是词边界，
`\b(\d{6})\b` **直接命中并返回一个错误的验证码**。
⇒ 只要真的发生了截断，就把结尾那段数字**整个丢掉**（换成空格，免得把前后文粘到一起）。
⚠️ **补一个字符不足以消掉这个伪边界**——空格前面照样是词边界，**必须把残缺的数字本身去掉**。
⚠️ **刻意不用 `/\d+$/` 这样的正则**：保留的前缀里若有长数字串而结尾不是数字，引擎会退化成 O(n²)
——**实测 128 KiB 就要 35 秒**。这里从截断点往回走一次即可，线性，1 MiB 全数字的最坏输入实测约 5 ms。

🔴 **`MAX_BODY_LEN = 1 MiB` 而不是更小**（`code.ts:77-93`）：Agnes 的验证码邮件走 HTML 模板，
一旦内嵌 base64 logo（动辄 100 KB+）且图片排在验证码元素之前，几十 KiB 的上限会被图片整个吃掉
——**`extract_code` 恒返回 `None`、`poll_code` 一路空转到超时、注册机 100% 静默失效，
而单测里的正文都是短字符串，全绿。**
**提高上限的前提是所有量词都有上界。别在没读那段的情况下往回调这个值，也别在这个文件里新增无界量词。**

**`normalize_body(v)`（`code.ts:121-137`）**：真机实测 YYDS 详情响应里的 `html` 是**数组**（元素数 1）、
`text` 是字符串。原代码的模板插值对单元素数组碰巧等价，**但多段 HTML 时会用逗号拼接**
（**逗号会直接贴到数字上，影响 `\b` 边界判定**）。⇒ 显式处理数组形态，用**换行**拼接。

**Rust 侧移植提示**：`regex` crate 是**线性时间**的（无回溯），上面那张 O(n²) 表在 Rust 下不成立。
**但三个上界照留**：① 它们同时是「取到错码」那条缺陷的防线（与回溯无关）；
② `MAX_BODY_LEN` / `bounded_for_scan` 的伪词边界问题在任何引擎下都成立。

### 10.10 出站脱敏（`registrar/url.ts`，285 行 + `registrar/fetch.ts`，48 行）

**`fetch_channel`（`fetch.ts`）—— 注册机所有出站请求的唯一出口**：

🔴 **`fetch` 有两种失败，它们此前的待遇完全不同**（`fetch.ts:7-18`）：
* **发得出去、上游回非 2xx** ⇒ 调用方自己拼 `http_fail_message`，地址是脱敏的；
* **压根没发出去**（URL 带 userinfo、DNS 失败、TLS 错误、超时中止）⇒
  **运行时抛的那个 Error 直接穿出去，它的 message 里可能带着完整的原始 URL**，一路进事件板块与容器 stdout。
⇒ **脱敏必须落在「发请求这件事」上，而不是落在「非 2xx」那一支上。**
**做成一个出口而不是在每个调用点各写一次 try/catch，是因为后者的失败形态是「下一个人新加的那一处忘了包」，
而那种遗漏不会有任何东西变红。**
⚠️ **`method` 从 `init` 里取，不另开一个参数**：另开一个就多了一处能与真正发出去的方法对不上的地方。

**脱敏的射程（`url.ts:16-23`）—— 盲区在这里登记，文档里不许说成「已脱敏」**：
只挡三处：**userinfo（用户名/口令）、查询串、片段**。
**凭据写在路径段里挡不住**——形如 `https://host/t/<token>/v1` 的地址会**原样**打进日志。
**这是已知缺口，不是遗漏**：路径段里哪一段是凭据没有可靠判据，
而那条「不要用关键词启发式去兜底」的禁令逐字管着这种场合。

⚠️ **要防的不是面板**（`url.ts:5-15`）：`registrar.yyds.baseUrl` 标的是 `public`，
`GET /admin/api/config` 今天就把它明文返给已鉴权的管理员；事件板块同样在鉴权之后。
**「事件板块没鉴权」是假的，别把它写成理由。** 真正的理由有两条：
* `GET /admin/api/events/download` 的**设计用途就是被导出**——粘进 issue、贴进聊天窗口。
  **导出的那一刻它就离开了鉴权边界；**
* 事件同时进容器 stdout，**那是另一个信任域**。

```rust
const UNPARSEABLE_URL: &str = "<不是一个可解析的 URL>";        // url.ts:43
const UNSAFE_MESSAGE:  &str = "<原始错误消息里仍有凭据成分，已整段丢弃>";   // url.ts:87
fn redact_url(raw: &str) -> String {          // url.ts:45-78 —— **幂等**
    // 解析失败 ⇒ UNPARSEABLE_URL（**不回落原串**）
    // userinfo 任一非空 ⇒ 两者清空，主机名前留 `***@` 这个**可见标记**
    // 查询串非空 ⇒ 整段换成 `?<redacted>`（**不做逐参数白名单**）
    // 片段非空   ⇒ 换成 `#<redacted>`
}
fn redact_in_message(msg: &str, url: &str) -> String {   // url.ts:100-139
    let secrets = url_secrets(url);      // username / password / search[1..] / hash[1..]
    if secrets.is_none() { return UNSAFE_MESSAGE; }      // **解析不开 ⇒ 无条件整段丢弃**
    let out = msg.replace(url, &redact_url(url));        // **replace_all 而不是只换第一处**
    if secrets.iter().any(|s| out.contains(s)) { UNSAFE_MESSAGE } else { out }
}
```
🔴 **`redact_url` 解析失败时刻意不回落成原样输出**（`url.ts:34-43`）：解析不开的串照样可能含凭据
——env 侧的 `YYDS_BASE_URL` / `MOEMAIL_BASE_URL` **只过 `as_non_empty`，一道 URL 校验都不走**，
运维粘错半截带口令的串是完全可能的。
**而「它压根不是一个 URL」本身就是一条完整的诊断结论。**
**留 `***@` 标记而不是静默删掉**：运维如果真把凭据塞进了 baseUrl，**他需要知道我们抹了它**。
**留 `?<redacted>` 占位而不是直接删**：`…/domains?x=1` 与 `…/domains` 在日志里必须读得出区别。
**片段那条不是洁癖**：baseUrl 带 `#` 时，适配器里 `${baseUrl}/v1/domains` 追加的那段路径整个掉进 fragment，
**实际打的是原来的 pathname —— 这个占位符是那条失败形态唯一的线索。**

🔴 **`redact_in_message` 守的洞（实测出来的，不是推断）**（`url.ts:104-115`）：
baseUrl 带 userinfo 时，undici 在**构造 Request 的那一步**就抛 `TypeError`，而它的 message 里带着**完整的原始 URL**
——本机 Node v24.15.0 实测逐字：
`Request cannot be constructed from a URL that includes credentials: https://user:pass@…`
**于是口令原样进事件板块、进容器 stdout。适配器里 `if (!r.ok)` 那一支根本没跑到，脱敏被整个绕过。**
🔴 **它凭什么敢说自己挡住了：不是靠「把认识的形态替换掉」，是靠一条后置条件**——
替换完之后再回头查一遍，只要还有任何一段凭据留在输出里，**整段丢掉**。
⇒ **「我们没想到的形态」的后果是少说一句话，不是多漏一把口令。**
反过来写（只做替换、不做回查）时，一个被运行时重新编码过的 URL 就能整串漏出去，**而门禁全绿。**
⚠️ **另一半代价**：口令恰好是一个在别处也会自然出现的短串（`1`、`a=1`）时，
回查会命中一句**本来无害**的消息并把它整段丢掉。**这是刻意选的方向——假阳性只让人少看到一句话，
假阴性会把口令贴进 issue。**
⚠️ **`replace_all` 而不是只换第一处**：undici 那句话里 URL 出现一次，别的运行时可能出现两次
（「构造 X 失败」+「原始输入 X」）。

**四个消息模板（`url.ts:141-285`）—— 统一形态 `<通道> <动作>失败: <原因> (<方法> <脱敏地址>)`**：

| 函数 | 用于 | reason |
|---|---|---|
| `http_fail_message` | 发得出去、上游回非 2xx | `HTTP {status}` |
| `http_fail` | 同上，但**另把状态码原样挂在 Error 上** | 同上 |
| `transport_fail_message` | **请求压根没发出去** | `redact_in_message(cause, url)` |
| `body_fail` | **上游答了 2xx、正文却读不出来** | `响应正文不是 JSON: {redact_in_message(cause, url)}` |

🔴 **`http_fail` 与 `http_fail_message` 必须是同一个工厂**（`url.ts:186-207`）：
「消息里写着 404、属性上挂着 403」是这类改动最容易长出来的假话，**而它只会在两条路各产一半时出现**。
⚠️⚠️ **绝不许用正则从 `err.message` 里抠 `HTTP (\d+)`**：那是一张手写启发式，
消息模板是给人看的、会被翻译会被改写，**而它一改，抠数字的那一头静默地开始答错，门禁全绿**。
🔴 **`transport_fail_message` 与 `body_fail` 没有状态码，也不许伪造一个**：
`0` / `502` 那种兜底值会让调用方把「没连上」读成「上游回了话」。**没有就是没有。**
`body_fail` 也**刻意不挂 `status: 200`**：上游确实回了 2xx，但挂上去会让消费方把它读成
「上游回了话、这是它的裁决」，**而这一档真正要说的是正文读不出来**。
⚠️ **`method` 是一个参数而不是写死的 `GET`**：`create_mailbox` 打的是 POST，
**写死会让日志报一个它没发过的方法——那是新的一句假话。**
⚠️ **URL 拼进的是 Error 的 message，不是一个新的结构化字段，代价明写**（`url.ts:163-171`）：
**收益**——两处只搬运 `err.message` 的调用点一个字都不用改就同时带上了地址；
**代价**——地址因此是消息串的一部分，**面板不能按它筛选或聚合**。**这是有意取舍，不是遗漏。**
🔴 **`body_fail` 存在的理由是一次实测出来的假话**（`url.ts:254-263`）：
上一版这条路径上**一个模板都没有**，适配器在 2xx 之后直接 `.json()`，抛出来的是裸 `SyntaxError`，
**而它的 message 里一个地址都没有**（实测逐字：`error="Unexpected token '<', \"<html><bod\"... is not valid JSON"`）
⇒ 面板那句「事件里那条失败信息带着它实际请求的那个地址」**在这一支上是假的**。
**⇒ 处置不是把那句话删掉，而是让这条路真的带上地址。**

---

## 11. 常量总表（Rust 侧照抄，改任何一个先回到本文对应小节读理由）

| 常量 | 值 | 出处 | 依据强度 |
|---|---|---|---|
| `UPSTREAM_TIMEOUT_MS` 默认 | 8_000 | `config-provenance.ts:263` | 设计取值 |
| `UPSTREAM_SYNC_TIMEOUT_MS` 默认 | 120_000 | `config-provenance.ts:266` | 🟢 实测（图片生成 11.99s）× 余量 |
| `MAX_STRIKES` 默认 | 3 | `config-provenance.ts:267` | 设计取值 |
| `COOLDOWN_RATE_LIMIT_MS` 默认 | 60_000 | `config-provenance.ts:268` | 设计取值 |
| `COOLDOWN_PAYMENT_MS` 默认 | 3_600_000 | `config-provenance.ts:269` | 设计取值 |
| `COOLDOWN_STRIKE_MS` 默认 | 1_800_000 | `config-provenance.ts:270` | 设计取值 |
| `SYNC_ATTEMPT_SHARE` | 2 | `dispatcher.ts:330` | 🟢 论证（5% 硬失败 vs 池大小×预算） |
| `REDACT_CARRY` | 256 | `dispatcher.ts:150` | 论证（远大于任何真实密钥长度 + 内存硬上界） |
| `DEFAULT_POOL_CACHE_TTL_MS` | 60_000 | `keypool-repo.ts:26` | 配额账 |
| `DEFAULT_POOL_TOUCH_INTERVAL_MS` | 21_600_000 | `keypool-repo.ts:27` | 配额账 |
| `INDEX_WRITE_RETRY_MS` | 300_000 | `keypool-repo.ts:182` | 🟡 必须 ≫ cache TTL；今天是功能性死代码 |
| `READ_PATH_LIST_BACKOFF_MS` | 600_000 | `keypool-repo.ts:205` | 🟢 配额账（≤144 次/天/副本） |
| `LIST_FAIL_FAST_RETRY_MS` | 60_000 | `keypool-repo.ts:235` | 论证（一次抖动不粘 10 分钟） |
| `LIST_FAIL_ESCALATE_AFTER` | 3 | `keypool-repo.ts:238` | 论证（滤抖动 / 不高频重试） |
| `MAX_KEY_LENGTH` | 1024 | `keypool-repo.ts:260` | 论证（挡手滑，不是挡攻击） |
| key id 长度 | SHA-256 前 8 字节 = 16 hex | `keypool-repo.ts:247-250` | — |
| `APIKEY_SECRET_PREFIX` | `"sk-"` | `api-keys.ts:40` | — |
| `API_KEY_SECRET_BYTES` | 16（128 bit） | `api-keys.ts:50` | 🟢 论证 |
| `API_KEY_HINT_LENGTH` | 4 | `api-keys.ts:53` | — |
| `APIKEY_MAX` | 200 | `api-keys.ts:64` | 🟢 论证（≈40 KB，再大要分片） |
| `API_KEY_NAME_MAX` | 64 | `api-keys.ts:67` | — |
| `MIN_GATEWAY_TOKEN_LENGTH` | 24 | `config-validate.ts:81` | 🟢 论证（无限速 ⇒ 熵是唯一防线） |
| `MAX_TEXT_LENGTH` | 200 | `config-validate.ts:67` | — |
| `EVENT_RING_SIZE` | 100 | `event-ring.ts:50` | — |
| `EVENT_FLUSH_MIN_INTERVAL_MS` | 60_000 | `event-ring.ts:70` | 🟢 论证（挡匿名驱动写的杠杆） |
| `EVENT_WRITES_PER_DAY` | 12 | `event-ring.ts:95` | 🟡 **分母已不存在，没人重算过** |
| `EVENT_WINDOW_MS` | 3_600_000 | `event-ring.ts:104` | 权衡 |
| `EVENT_SLOTS` | 2 | `event-ring.ts:112` | 权衡 |
| `EVENT_WINDOW_RETAIN` | 24 | `event-ring.ts:120` | ⇒ 24 小时可见历史 / 冷读 48 键 |
| `EVENT_TTL_MARGIN_MS` | = `EVENT_WINDOW_MS` | `event-ring.ts:179` | 时钟偏差余量 |
| `USAGE_SLOTS` | 2 | `usage-stats.ts:138` | 同 `EVENT_SLOTS` |
| `USAGE_DAY_RETAIN` | 30 | `usage-stats.ts:190` | ⇒ 读扇出上界 60；**「60 安全」不许出现在任何文字里** |
| `USAGE_WRITES_PER_DAY` | 13 | `usage-stats.ts:210` | 🟢 12 次落盘 + 1（跨零点写两键） |
| `USAGE_FLUSH_MIN_INTERVAL_MS` | 7_200_000 | `usage-stats.ts:267` | 🟢 `86_400_000 / 12`，**无余量，刻意** |
| `USAGE_MODEL_KEY_MAX_LEN` | 64 | `usage-stats.ts:341` | 有界性 |
| `USAGE_MODEL_MAX_KEYS` | 32 | `usage-stats.ts:372` | 有界性 |
| `USAGE_APIKEY_MAX_KEYS` | 201 | `usage-stats.ts:410` | `APIKEY_MAX + 1` |
| `USAGE_TTL_MARGIN_MS` | = `USAGE_DAY_MS` | `usage-stats.ts:565` | 时钟偏差余量 |
| `MANUAL_TEND_COOLDOWN_MS` | 600_000 | `tend-guard.ts:61` | 挡连点（邮箱名额） |
| `MANUAL_TENDS_PER_DAY` | 24 | `tend-guard.ts:102` | 🟡 **分母已不存在** |
| `TEND_HISTORY_SIZE` | 50 | `tend-history.ts:25` | ⇒ 约 25 小时补池历史 |
| `UPSTREAM_MODELS_MAX` | 500 | `upstream-models.ts:23` | 有界性 |
| `VIDEO_TASK_ID_RE` | `^[A-Za-z0-9_-]{1,128}$` | `protocol-catalog.ts:365` | 🔴 **未核实的假设**，但**不许放宽** |
| `SAMPLE_PROMPT` | `"ping"` | `protocol-catalog.ts:40` | — |
| `MAX_SAFE_TIMER_DELAY_MS` | 2_147_483_647 | `tend-scheduler.ts:54` | 🟢 实测（超界静默变 1ms） |
| **注册机** | | | |
| `TARGET_KEYS` 默认 | 20 | `registrar/config.ts:105` | — |
| `MINT_BATCH` 默认 | 5 | `registrar/config.ts:106` | — |
| `TEND_INTERVAL_MS` 默认 | 1_800_000 | `registrar/config.ts:107` | — |
| `CODE_TIMEOUT_MS` 默认 | 120_000 | `registrar/config.ts:108` | — |
| `MINT_DELAY_MIN_MS` 默认 | 60_000 | `registrar/config.ts:116` | 🟢 **实测下界**（单出口单日样本） |
| `MINT_DELAY_MAX_MS` 默认 | 90_000 | `registrar/config.ts:121` | 🟡 抖动上界，非实测 |
| `MAX_DOMAIN_ATTEMPTS` 默认 | 1 | `registrar/config.ts:130` | 🟢 实测 + 推导 |
| `REGISTRAR_TOKEN_NAME` 默认 | `"auto"` | `registrar/config.ts:131` | — |
| `AGNES_PLATFORM_URL` 默认 | `https://platform-backend.agnes-ai.com` | `registrar/config.ts:132` | — |
| YYDS baseUrl 默认 | `https://maliapi.215.im` | `registrar/config.ts:133` | 公共服务地址固定 |
| `REGISTRAR_REQUEST_TIMEOUT_MS` | 15_000 | `registrar/types.ts:22` | 🟢 论证（控制面小 JSON） |
| `SCHEDULED_ROUND_WALL_CLOCK_MS` | 900_000 | `registrar/types.ts:37` | 🟢 上下界都有论证 |
| `SCHEDULED_ROUND_BUDGET_MS` | 780_000 | `registrar/types.ts:88` | = 87%，留 120 秒余量 |
| `MANUAL_MINT_BATCH` | 1 | `registrar/types.ts:109` | 🟢 事故驱动 |
| `MANUAL_MAX_DOMAIN_ATTEMPTS` | 1 | `registrar/types.ts:120` | 🟢 让 `worst_attempt` 退化成常量 |
| `MANUAL_CODE_TIMEOUT_MS` | 60_000 | `registrar/types.ts:129` | 定时轮的一半 |
| `MANUAL_ROUND_BUDGET_MS` | 70_000 | `registrar/types.ts:150` | 必须 > 60_000 |
| `EDGE_BACKOFF_MS` | 900_000 | `registrar/backoff.ts:85` | 🟢 实测约 14 分钟 + 余量 |
| `APP_BACKOFF_MS` | 1_800_000 | `registrar/backoff.ts:95` | 🔴 **猜的，没有实测** |
| `CLUSTER_BACKOFF_MS` | 1_800_000 | `registrar/backoff.ts:120` | 🔴 **猜的，没有实测** |
| `BACKOFF_MAX_MS` | 14_400_000 | `registrar/backoff.ts:128` | 🟡 取舍（超过就该换出口） |
| `DOMAIN_LEDGER_CAP` | 512 | `registrar/domain-ledger.ts:60` | 上游今天 374 个域名 + 余量 |
| `OK_TTL_MS` | 604_800_000 | `registrar/domain-ledger.ts:68` | 🟡 没有实测支撑 |
| `BLOCK_TTL_MS` | 86_400_000 | `registrar/domain-ledger.ts:77` | 🟡 没有实测支撑；**刻意取短** |
| `BODY_SNIPPET_MAX` | 512 | `registrar/domain-ledger.ts:82` | 挡一张边缘挑战页冲光事件环 |
| `MAX_GAP` / `MAX_TAG_GAP` / `MAX_WS` | 200 / 400 / 32 | `registrar/code.ts:47,50,53` | 🟢 **逐段按真实模板实测** |
| `MAX_BODY_LEN` / `MAX_SUBJECT_LEN` | 1 MiB / 1024 | `registrar/code.ts:93,94` | 🟢 论证（base64 logo） |

---

## 12. 存储窄化方向表（**逐处不同，不许统一成一种**）

| 数据 | 整体读不出来 | 单条/单字段坏 | 方向的理由 |
|---|---|---|---|
| `pool:index` | 当「索引缺失」⇒ 回落 `list()` 并重建 | **就地剔掉那一个元素** | 一条脏数据把整个索引作废 ⇒ 每次 `all()` 都 list 一遍 |
| `key:<id>` | 那一条被 filter 掉（幽灵索引项） | `stats` 逐字段补零 | **整块丢弃会让一次坏写把攒了几天的计数清零，而面板上看不出发生过** |
| `config` | 冷启动降级到 env+默认；**热路径如实抛** | 字段级降级回默认值 + `degraded=true` | 抛错 ⇒ 重启循环，**而且没有面板可以进去改回来** |
| `apikeys` | 🔴 **`Invalid`，绝不当空表** | 坏一条 = 整张表不认 | 当空表 ⇒ **全部客户 401**，且下一次面板写会把幸存记录整份覆掉，不可逆 |
| `event:*` | 非数组 ⇒ 空分片，`malformed` **计 0** | `ts` 非有限数 ⇒ 丢；其余一律留 | `null` 是完全正常的路径；**丢 level/msg = 在一次上游异常之后把整段证据清空** |
| `usage:*` | 非对象/`total` 坏/`day` 坏 ⇒ 整条 `malformed` | 单个桶坏 ⇒ 那个键静默消失 | **数组必须单独拒掉**，否则畸形分片会伪装成合法的并把 `shards` 一起变假 |
| `tend:history` | 非数组 ⇒ 空 | **逐字段校验，任一不合整条丢** | 它是**定长表的一行**：一行里 `minted` 坏掉的记录**显示比丢掉更容易误导运维** |
| `registrar_manual_guard` | — | **任一字段读不得 ⇒ 整条 `None`**（= 今天还没点过） | 多放行一轮（有锁与冷却兜着）好过**把按钮永久锁死而看不出原因** |
| `registrar:backoff` | — | **任一字段读不得 ⇒ `None`**（= 放行） | 当成「还在退避」会让注册机**静默停摆**；放行的代价只是多打一轮 |
| `registrar:domains` | **整个对象读不得 ⇒ 空台账** | 单条读不得 ⇒ **丢那一条** | 「什么都没记住，多探几次」；**绝不返回「全被屏蔽」** |

---

## 13. 事件名全表（Rust 侧一个字都不许改，它们是运维的 grep 锚点）

```
pool.load_failed          pool.commit_failed        pool.stale_write_dropped
pool.index_backfilled     pool.index_bootstrapped   pool.index_repaired
pool.index_unreadable     pool.index_write_failed
pool.index_reconcile_failed               🟡 Rust 侧新增：TS 那里是 console.error，无事件名
config.invalid            config.storage_unreadable
storage.probe_cleanup_failed
key.added  key.restored  key.disabled  key.deleted        （面板写端点，其中两条 info 级）
admin.login_failed                                        （**任何未鉴权请求都能触发**）
registrar.config_ignored  registrar.interval_shorter_than_worst_round
registrar.domain_attempts_costly
registrar.attempt_exceeds_worker_budget   🟡 名字里的 worker 是已知残留，改名必须源码+五份文档同一次改完
registrar.backoff_read_failed  registrar.backoff_skipped  registrar.rate_limited
registrar.round_budget_impossible  registrar.round_budget_exhausted
registrar.ledger_write_failed  registrar.post_mint_reconcile_failed
registrar.config_reload_failed            🟡 Rust 侧新增：TS 那里是 console.error，无事件名
registrar.round_skipped_locked            🟡 Rust 侧新增：TS 那里锁在 handler 判、直接回 409
registrar.domain_ledger_read_failed  registrar.list_domains_failed
registrar.domain_verdicts_discarded  registrar.round_all_domains_rejected  registrar.domain_blocked
registrar.known_good_domain_rejected  registrar.send_code_unclassified  registrar.send_code_bad_status
registrar.create_mailbox_failed  registrar.no_mailbox_on_any_domain  registrar.delete_mailbox_failed
registrar.code_timeout  registrar.register_rejected  registrar.login_no_token  registrar.key_not_returned
registrar.network_error  registrar.minted_key_suspicious
registrar.channel_test_failed （http 层）
```

⚠️ **`fields` 只放标量**（`config-errors.ts:31`、`logger.ts`）。
⚠️ **凡是把上游正文放进 `fields.message` 的，一律先过 `upstream_message` / `step_message` 脱敏。**
⚠️ **`registrar.minted_key_suspicious` 只报 `channel` + `key_length`，不带明文。**

---

## 14. Rust 移植的几处「JS 病不必移植 / 但语义必须保留」对照

| TS 里的处置 | 它防的是什么 | Rust 侧 |
|---|---|---|
| `parse_sse_stream` 的 `signal` + `reader.cancel()` | AsyncGenerator 的 `.return()` 排在飞行中的 `.next()` 后面永远轮不到 | ✅ 不必移植这条机制；**但「客户端断连必须真的释放上游连接」的语义要保留**（`select!` + `CancellationToken` 或 drop `Response`） |
| `mergeDayShards` 的 `Object.create(null)` | `__proto__` 让一条静默消失、`toString` 让桶算成 NaN | ✅ `HashMap` 天然没有原型链；**但「这四个名字必须原样出现在响应里」这条行为断言要保留** |
| `safeString()` 兜 `String(x)` 自己抛 | `{"toString":1,"valueOf":1}` | ✅ 不必移植；**但「别丢计数」这条要保留**（非字符串 model 要有一个确定的桶名，不能 panic 也不能丢） |
| `boundedForScan` 防 O(n²) 回溯 | 正则灾难性回溯 | ⚠️ `regex` crate 线性、无此问题；**但那三个上界与「伪词边界」那条照留**（与引擎无关） |
| `clampToMaxDelayMs` | `setTimeout` 超 i32 静默变 1ms | ⚠️ `tokio::sleep` 无此坑；**但上界钳制照留**（防 `i64` 溢出 / `Duration` panic） |
| `c.json` 丢掉 `undefined` 字段 | 面板分不清「没停用」和「读不出来」 | ⚠️ serde 的 `skip_serializing_if` 会造出同样的问题；**视图层字段一律实打实的 `bool` / `Option<String>` 且不许 skip** |
| `maskKey` 的 `key.slice(0,5)` | — | 🔴 **Rust 的 `&s[..5]` 对多字节 UTF-8 会 panic**；铸号那条路存进去的可疑 key 可能不是 ASCII ⇒ 必须 `chars().take(5)` |
| `slot_of` 的 `h * 31 + c \| 0` | — | 🔴 **必须 `i32::wrapping_*`**；用 `u64` 会得到不同槽位分布 ⇒ 升级时同一 shardId 换槽 ⇒ 丢一批已攒的事件 |
| `withTaskId` 用 `split`/`join` | `replace` 会把 `$&` 当引用展开 | ⚠️ `str::replace` 没问题；**`regex::Regex::replace` 有 ⇒ 不许用它** |

---

## 15. Gaps —— 本轮**没弄清 / 有风险 / 要主线裁**的

1. 🔴 **`admin/protocol-catalog.ts` 的 1-26 行与 257-344 行（`MODEL_CATALOG` 的 12 条 entry 字面量）
   我是靠 grep 拿到结构、没有逐行通读**。P2 抄模型表时**必须回原文件对一遍 `endpoints` 那几行**，
   本文 §8.8 那张表只给了 `PROTOCOLS` 与 `MEDIA_ENDPOINTS`，`MODEL_CATALOG[].endpoints` 没有逐条抄。
2. 🔴 **`src/http/` 与 `src/adapters/` 完全没读**（不在本轮任务范围）。本文多处引用它们
   （`wire.ts` 的 `buildTendDeps`、`apikey-store.ts` 的 `issueSecret`、`usage-sink.ts`、`logger-store.ts`、
   `tend-lock.ts`、`middleware/auth.ts`、`admin/auth.ts` 的 `SENDABLE` 与 `ADMIN_TOKEN_MIN_LENGTH`、
   两个 mailbox 适配器的活跃邮箱上限）**只是转述 core 侧注释里对它们的描述，没有一手核对**。
   P1 接线前要单独抽一轮。
3. 🟡 **`dispatcher` 里的模块级 `cursor` 在 Rust 侧到底放哪**：本文建议 `AppState` 里的 `AtomicUsize`
   （与 TS 逐字同语义），**但那意味着面板仍然不许展示「下一把 key / 轮换顺序」**。
   要不要借移植的机会把它做成全局一致的分布式游标，**是一次行为变更，需要主线裁。**
4. 🟡 **两个 🟡 分母已不存在的常量**（`EVENT_WRITES_PER_DAY = 12`、`MANUAL_TENDS_PER_DAY = 24`）
   在 Rust + Docker-only 形态下要不要重算？原仓自己登记着「要调它得先定一个新的分母」。
   **本轮照抄，没有重算。**
5. 🟡 **`FileStorage` 的形态本身没读**（在 `src/adapters/`）。本文多处引用「每次 put 都重写整个 `store.json`」
   这条性质作为一堆常量的依据 —— **Rust 侧如果换成 sled / redb / 追加写日志，那些常量的依据全部需要重估**。
   **这是一个值得主线裁的分叉点：换存储引擎会让本文至少 8 处论证失效（全部是「写放大」那一族）。**
6. 🟡 **`upstream-facts` 那三条 `assumed` 至今没有真上游依据**。
   P3 铸出第一把真 key 之后，`video.taskIdSlot` / `video.taskIdCharset` / `openai.streamTrailingUsage`
   要当场重验。**在那之前，`VIDEO_TASK_ID_RE` 一个字符都不许放宽。**
7. 🟡 **`api-keys` 的 `secret` 明文落盘那一条**：本文如实记了代价（§8.1）。
   **它是用户拍板的，不是技术结论** —— Rust 版要不要沿用，主线应当再确认一次
   （沿用的话，备份/快照的处置级别要跟着升，这一条要写进部署文档）。
8. 🟡 **`constant_time_equal` 的「不由测试保证、由评审保证」在 Rust 侧怎么落**：
   建议用 `subtle` crate（编译器不会优化掉），但**「长度不同立刻 false」这条与 TS 逐字一致的行为要保留**，
   而 `subtle::ConstantTimeEq` 对不等长切片的处理需要显式写清楚。**这一条需要在 code review checklist 里点名。**
9. 🟡 **`ADMIN_ERROR_CODES` 里那笔「仍然欠着的账」**：网关业务口（六处）对 API 客户端**只说中文**。
   Rust 版要不要一并改成码 + 前端查字典？**原仓明确裁定「本期不改」且「不许顺手给网关那一族也加码」**
   （半做会造出第三种状态）。**移植时照旧不改，但这笔账要转写进新仓的 README / CHANGELOG。**
10. 🟡 **`usage-stats` 那条「读扇出 60 次」的平台承诺问题在 Docker-only 下已经不成立**
    （没有子请求上限），但 **60 次整份 `store.json` 的 `read_file` 这件事还在**。
    **要不要把 `USAGE_DAY_RETAIN` 或读端点的形态改掉，是一次独立取舍。**
