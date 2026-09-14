//! 转发热路径。🔴 **全仓唯一一条到上游的链路。**
//!
//! 四条协议路由与三条媒体端点都是它的适配器：它们负责「请求体长什么样、响应体怎么成型」，
//! **选哪把 key、怎么记账、失败了换谁** 一律在这里。
//! 各协议自己拉一条链路的后果很好认：「OpenAI 那条能用、Anthropic 那条选 key 的行为不一样」。
//!
//! # 这个文件里四件互相独立的事
//!
//! 1. **出站响应头白名单**（§3.1）—— 上游响应头一律不原样转发；
//! 2. **MIME 钳制**（§3.2）—— 不让这个源上出现攻击者影响得了的同源文档；
//! 3. **错误体流式脱敏**（§3.3）—— `sk-` 那一族，**流式改写，不许先读完再替换**；
//! 4. **主循环与两档超时**（§3.5/§3.6/§3.7）。

use crate::adapters::now_ms;
use crate::core::config::GatewayConfig;
use crate::core::errors::{ThrownError, UpstreamAction, classify_status, classify_thrown};
use crate::core::keypool::{
    LbMode, apply_cooldown, apply_evict, apply_strike, apply_success, is_disabled, pool_health,
    select,
};
use crate::core::stats::{Outcome, with_outcome};
use crate::core::types::{KeyRecord, PoolPolicy};
use crate::pool::repo::KeyPoolRepo;
use crate::ports::fetcher::{BodyStream, FetchError, Fetcher, OutboundRequest, OutboundResponse};
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::server::errors::{DispatchError, FailReason, GatewayError};
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── 1. 出站响应头白名单 ─────────────────────────────────────────────────────

/// 🔴 **只有这四个头会被转发给客户端。**
///
/// 三条理由，逐条都是买过单的：
/// ① `set-cookie` / `www-authenticate` / `authorization` 带的是**上游的凭据语义**，
///    透传等于把它们落到网关自己的域上；
/// ② 上游的 `x-*` 内部头是不受控的泄漏面（**实测 `x-upstream-internal` 会原封不动到达客户端**）；
/// ③ 池子每次请求都可能换一把 key，**逐 key 的限流/配额头对客户端只是误导**。
///
/// **不含 `content-length` / `content-encoding`**：响应体在这里被重新包装
/// （出站客户端已解压，见 `Cargo.toml` 里那三个压缩 feature 的注释），沿用会与实际字节不符。
/// 🔴 Rust 侧同样必须**重建**这两个头，不许 `headers.clone()`。
///
/// **含 `content-disposition`**：媒体路由承诺「上游返回什么就原样转发响应体」，
/// 剥掉它会让浏览器端下载丢失文件名。
///
/// **不含 `accept-ranges` / `content-range`**：网关不支持 range（不解析、不转发 `Range`），
/// 放行只会让客户端发起注定失败的 range 请求 —— 属于误导而非帮助。
pub const SAFE_RESPONSE_HEADERS: [&str; 4] = [
    "content-type",
    "cache-control",
    "retry-after",
    "content-disposition",
];

// ── 2. MIME 钳制 ───────────────────────────────────────────────────────────

/// 会被浏览器当成「同源可执行 / 可渲染文档」的 MIME。命中 ⇒ 改写成 `application/octet-stream`。
///
/// **为什么是 deny-list 而不是 allow-list**：媒体路由承诺「上游返回什么就原样转发」，
/// allow-list 会把没预料到的媒体类型（新的图片 / 视频容器）打成下载，那是真的功能退化。
/// **边界写清楚**：deny-list 永远列不全，目标是把「同源文档」这一类关掉，
/// 不是做完备的 MIME 治理。
///
/// **为什么需要它**：面板的 `script-src 'self'` 隐含前提是「本源只有面板脚本」，
/// 而 `GET /v1/videos/{id}` 是一条接受 `?key=` 的 GET 路由、content-type 逐字透传上游
/// ⇒ 这个源上可以出现一个攻击者影响得了的同源文档，
/// 而 `ADMIN_TOKEN` 就存在这个 origin 的 localStorage 里
/// （**localStorage 的作用域是 origin 而不是 path**）。
///
/// ⚠️ **全局 `nosniff` 挡不住这一半**：它只否掉「按内容嗅探出一个更危险的类型」，
/// 不阻止浏览器渲染一个**显式声明**为 `text/html` 的响应。
///
/// ⚠️ **`multipart/x-mixed-replace` 在列，它是 deny-list 最经典的那条绕法**：
/// 外层类型本身无害，而 Firefox 会按**内层每个 part 自己的 `Content-Type`** 渲染。
const DOCUMENT_MIME: &str = r"(?i)^\s*(text/html|application/xhtml\+xml|image/svg\+xml|text/xml|application/xml|text/javascript|application/javascript|application/x-javascript|application/ecmascript|text/ecmascript|multipart/x-mixed-replace)\b";

fn document_mime_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(DOCUMENT_MIME).expect("DOCUMENT_MIME 是字面量，编译不失败"))
}

/// 钳制一个 content-type。
#[must_use]
pub fn clamp_content_type(v: &str) -> String {
    if document_mime_re().is_match(v) {
        "application/octet-stream".to_string()
    } else {
        v.to_string()
    }
}

// ── 3. 错误体流式脱敏 ───────────────────────────────────────────────────────

/// **只认 `sk-` 开头那一族**：上游与本网关签发的对外密钥都是这个前缀，
/// 而普通英文错误文案里几乎不会出现它。
///
/// ⚠️ **刻意不做更宽的匹配**（例如「任何 20 位以上的十六进制」）：错误体里本来就常带
/// request id、trace id、模型指纹，把那些一并打码会让运维**丢掉排障线索**。
/// **宁可漏掉一种没见过的凭据格式，也不要把有用的错误体变成一片星号。**
const CREDENTIAL_LIKE: &str = r"sk-[A-Za-z0-9_-]{6,}";

/// 块边界可能正好落在密钥中间 ⇒ 必须把「可能才写了一半的那一段」留到下一块再判。
/// 256 远大于任何真实密钥长度，同时给内存一个硬上界。
const REDACT_CARRY: usize = 256;

const MASK: &str = "sk-***";

fn credential_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(CREDENTIAL_LIKE).expect("CREDENTIAL_LIKE 是字面量，编译不失败"))
}

/// 打码一段已成型的文本。
///
/// ⚠️ **必须用 `NoExpand`**：`Regex::replace` 会把替换串里的 `$&` 当引用展开。
/// 今天的 `MASK` 里没有 `$`，但这条纪律钉在调用点上，
/// 免得哪天有人把 MASK 改成 `[redacted:$&]` 之类的东西 —— 那会把凭据原样吐回来。
#[must_use]
pub fn redact_text(s: &str) -> String {
    credential_re()
        .replace_all(s, regex::NoExpand(MASK))
        .into_owned()
}

/// 增量 UTF-8 解码器。
///
/// 🔴 **不可省**：一个多字节字符被切在两个 TCP 块之间是常态而不是边角情况。
/// 直接每块 `from_utf8_lossy` 会把那个字符变成两个 `U+FFFD`，**而且是静默的**。
///
/// ⚠️ **如实登记一处重复**：`core::protocol::sse` 里有一份同样的私有实现。
/// 它没有对外导出，而本模块不属于 `core`（不许反向依赖也不该去改 `core`）。
/// **两份必须同时存在的理由只有可见性这一条** —— `core` 那边哪天把它 `pub` 出来，
/// 这里应当整体删掉改成引用，而不是继续各自演化。
#[derive(Debug, Default)]
struct Utf8Incremental {
    pending: Vec<u8>,
}

impl Utf8Incremental {
    fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // SAFETY 之外的等价写法：前 `valid` 个字节已被 `from_utf8` 判为合法。
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or(""));
                    match e.error_len() {
                        // 真的是坏字节 ⇒ 吐一个替换字符往前走，绝不卡住整条流。
                        Some(bad) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..valid + bad);
                        }
                        // 只是没读完 ⇒ 剩下的留给下一块。
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// 收尾：把残留的坏字节交代掉。**不许丢**。
    fn flush(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let s = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        s
    }
}

/// 尾部那几个字节**有没有可能是下一块里那把凭据的头**（`sk-` 写了一半）。
///
/// 🔴 **不可省**：`rfind("sk-")` 只认得**完整**的前缀，而块边界照样可能切在它正中间
/// —— `...api key s` | `k-Ab12...` 两块各自都找不到 `sk-` ⇒ 各自整块 emit
/// ⇒ 客户端收到**完整明文**。
///
/// 返回要留到下一块的字节数（0 / 1 / 2）。`sk-` 三个字符都是 ASCII，
/// 所以按字节比较不会切坏多字节字符。
fn dangling_prefix_len(s: &str) -> usize {
    let b = s.as_bytes();
    if b.ends_with(b"sk") {
        2
    } else if b.ends_with(b"s") {
        1
    } else {
        0
    }
}

/// 流式脱敏。
///
/// 🔴 **必须是流式改写，不许 `res.text().await` 之后再替换。**
/// 第一版就是那么写的，被判据当场抓住 —— 一个**永不关闭**的响应体会让 `text()` 挂死。
/// 真实上游会关闭错误体，但**半开连接不会** ——
/// 那是拿一条可用性风险去换一条泄漏防护。
fn redact_stream(body: BodyStream) -> impl futures_core::Stream<Item = Result<Bytes, FetchError>> {
    async_stream::stream! {
        let mut decoder = Utf8Incremental::default();
        let mut carry = String::new();
        let mut body = body;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => { yield Err(e); return; }
            };
            let mut text = std::mem::take(&mut carry);
            text.push_str(&decoder.push(&chunk));

            // 从尾部往回找最后一个 `sk-`：它后面那一段可能才写了一半。
            // 找不到完整前缀时还要看一眼**前缀本身有没有被切开**（见 `dangling_prefix_len`）。
            let cut = match text.rfind("sk-") {
                Some(idx) if idx >= text.len().saturating_sub(REDACT_CARRY) => idx,
                _ => text.len() - dangling_prefix_len(&text),
            };
            let (emit, rest) = text.split_at(cut);
            if !emit.is_empty() {
                yield Ok(Bytes::from(redact_text(emit)));
            }
            carry = rest.to_string();

            // **兜底**：`sk-` 后面跟着几千个合法字符的东西不是凭据，
            // 别让 carry 无界增长（那就是把「有上界的内存」换成了「攻击者控制的内存」）。
            if carry.len() > REDACT_CARRY * 4 {
                yield Ok(Bytes::from(redact_text(&carry)));
                carry.clear();
            }
        }
        let tail = format!("{carry}{}", decoder.flush());
        if !tail.is_empty() {
            yield Ok(Bytes::from(redact_text(&tail)));
        }
    }
}

/// 白名单重建响应头 + MIME 钳制。
fn safe_headers(headers: &[(String, String)]) -> Vec<(HeaderName, HeaderValue)> {
    let mut out = Vec::new();
    for (k, v) in headers {
        let lower = k.to_ascii_lowercase();
        if !SAFE_RESPONSE_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        let value = if lower == "content-type" {
            clamp_content_type(v)
        } else {
            v.clone()
        };
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(lower.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            out.push((name, val));
        }
    }
    out
}

fn build_response(status: u16, headers: &[(String, String)], body: Body) -> Response {
    let mut res = Response::new(body);
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (k, v) in safe_headers(headers) {
        res.headers_mut().insert(k, v);
    }
    res
}

// **成功路径没有对应的 `sanitize()` 函数**，这是有意的：
//
// 成功那一支交给调用方的是「白名单筛过的头 + **未经改写的字节流**」
// （见 [`Dispatched::Ok`] 与 [`filtered_headers`]），因为各条协议还要在上面
// 补自己的 content-type、或者把字节流转成另一种协议的事件序列。
//
// 🔴 **成功路径不脱敏**：逐块改写会破坏流式透传（UTF-8 边界要重排、carry 会引入延迟），
// 而成功响应里不含上游的错误文案。脱敏只发生在错误体那一侧。

/// **错误路径**：白名单重建响应头 + **流式脱敏**正文。
fn sanitize_error(res: OutboundResponse) -> Response {
    build_response(
        res.status,
        &res.headers,
        Body::from_stream(redact_stream(res.body)),
    )
}

// ── 4. 超时档位 ────────────────────────────────────────────────────────────

/// 两档超时。**判据是「上游的第一个字节什么时候才可能到达」，不是端点的名字。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutProfile {
    /// `upstream_timeout_ms`（默认 8000）。**流式**对话（慢 key 在这里被快速甩掉）；
    /// 视频轮询 `GET /videos/{id}`（不是流式，但只查一次任务状态，同样是快接口）。
    FirstByte,
    /// `upstream_sync_timeout_ms`（默认 120_000）。图片生成、视频建任务、
    /// **以及非流式对话** —— 上游要把整段回答生成完才发响应头，与图片生成是同一种延迟语义。
    ///
    /// 🔴 它必须与 `FirstByte` 那档分开：实测直连上游 `/images/generations`
    /// 首字节耗时 **11.99 秒**。用 8 秒去卡它的结果是**图片生成 100% 失败**，
    /// 且每次请求把池中每把 key 各记一次 strike ——
    /// **三次请求就能把整个池子打进 30 分钟长冷却，连对话一起拖死。**
    Sync,
}

/// 同步档把总预算切成几份。
///
/// 同时满足两个互相拉扯的要求：
/// ① 客户端最坏只等**一个**预算（不能是「池大小 × 预算」）；
/// ② 一把挂起的 key（TCP 连得上但永不响应）不能吃掉整个请求 ——
///    否则 20 把 key 的池子里只要有 1 把挂起，就有 **5% 的请求硬失败**，且没有任何自愈通路。
///
/// **代价明写**：若单次调用真有可能超过预算的一半，
/// 就该把 `UPSTREAM_SYNC_TIMEOUT_MS` 设成「单次最坏耗时的两倍以上」。
pub const SYNC_ATTEMPT_SHARE: i64 = 2;

// ── 5. 池子不可用时的 503 ───────────────────────────────────────────────────

/// 池子里一把可用的都没有时回什么。
///
/// **四条 reason 的排序判据**：成因可能同时有三类（冷却 / 停用 / 剔除），
/// 而 `reason` 只有一个位置 ⇒ **取「最先能让池子重新可用的那一类」**，
/// 因为运维读到 reason 之后要做的正是那件事：
/// **冷却会自己好（等）→ 停用是他自己关的（点一下）→ 剔除要换 key（买）**。
/// 精确的三个数一律在 `message` 里给全，reason 只负责把人指向该做的那件事。
///
/// **`all_disabled` 不复用 `all_evicted`，理由是物理不是口味**：
/// `all_evicted` 对运维的含义是「凭据都失效了，去换 key」，
/// 而这里的真相是「是你自己在面板上关的」。
#[must_use]
pub fn unavailable(records: &[KeyRecord], now: i64) -> DispatchError {
    let h = pool_health(records, now);
    if h.total == 0 {
        return DispatchError::new(FailReason::PoolEmpty, "key 池为空，请先导入 key");
    }
    // 还有可用的 key ⇒ 走到这里只可能是「每一把都试过、上游都失败」。
    if h.fresh > 0 {
        return DispatchError::new(
            FailReason::UpstreamError,
            "已尝试池中每一把 key，上游均返回失败；key 本身仍可用",
        );
    }
    if h.cooling > 0 {
        // ⚠️ **`!is_disabled(r)` 这个过滤条件是承重的**：被停用的 key 的 `cooldown_until`
        //    通常是 0（它根本没在冷却），漏掉它会让 `min` 取到 0 ⇒ `Retry-After: 1`
        //    ⇒ 客户端每秒重试一次，而那把 key 永远不会自己回来。
        //    这一段与 `pool_health` 的 `cooling` 计数**必须筛出同一批记录**。
        let earliest = records
            .iter()
            .filter(|r| !r.evicted && !is_disabled(r))
            .map(|r| r.cooldown_until)
            .min()
            .unwrap_or(now);
        let secs = ((earliest - now) as f64 / 1000.0).ceil().max(1.0);
        // ⚠️ **两处条件拼接是刻意的**：无条件拼上「0 把被管理员停用」
        //    会让每一条既有的 503 都多出一句废话。`disabled == 0` 时本函数的输出与旧版逐字相同。
        let disabled_clause = if h.disabled > 0 {
            format!("{} 把被管理员停用、", h.disabled)
        } else {
            String::new()
        };
        return DispatchError::cooling(
            format!(
                "全部 key 暂不可用：{} 把冷却中（到期自动恢复）、{}{} 把已永久剔除",
                h.cooling, disabled_clause, h.evicted
            ),
            secs as u64,
        );
    }
    // 到这里只剩 disabled 与 evicted 两类，**都不会自动恢复 ⇒ 一律不带 `Retry-After`**。
    if h.disabled > 0 {
        let evicted_clause = if h.evicted > 0 {
            format!("、{} 把因凭据失效被永久剔除", h.evicted)
        } else {
            String::new()
        };
        return DispatchError::new(
            FailReason::AllDisabled,
            format!(
                "全部 {} 把 key 均不可用且不会自动恢复：{} 把被管理员手工停用（在管理面板上重新启用即可）{}",
                h.total, h.disabled, evicted_clause
            ),
        );
    }
    DispatchError::new(
        FailReason::AllEvicted,
        format!(
            "全部 {} 把 key 均因凭据失效被永久剔除，不会自动恢复，请更换 key",
            h.total
        ),
    )
}

/// 同步档预算耗尽。
///
/// ⚠️ 文案**刻意不断言**「这是超时预算问题而非 key 故障」：原实现这么写，
/// 而在「key 对应的上游会话被挂起」时它是一句假话 ——
/// 会把运维引向调大 `UPSTREAM_SYNC_TIMEOUT_MS`，只会让挂起挂得更久。
/// **这里只陈述事实，并把两种可能都摆出来。**
#[must_use]
pub fn sync_budget_exhausted(tried: usize, budget_ms: i64) -> DispatchError {
    DispatchError::new(
        FailReason::UpstreamTimeout,
        format!(
            "同步端点用尽了 {budget_ms} 毫秒的总预算：已尝试 {tried} 把 key，均未在各自的尝试预算内收到上游响应。\
可能是上游整体变慢或预算配小（可调大 UPSTREAM_SYNC_TIMEOUT_MS），也可能是这几把 key 对应的上游会话被挂起；本次未惩罚任何 key"
        ),
    )
}

// ── 6. 主循环 ──────────────────────────────────────────────────────────────

/// 出站方法。**刻意只有两个**：上游今天只有 POST 与 GET 两种形态，
/// 开一个 `String` 会让「拼错一个方法名」变成运行期才发现的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// 一次转发要什么。
pub struct DispatchArgs {
    /// 上游路径（拼在 `agnes_base_url` 后面），如 `/chat/completions`。
    pub path: String,
    pub method: Method,
    pub body: Option<Value>,
    /// 🔴 **档位由调用方显式传入，不在这里按 `stream` 自动推断**：
    /// 视频轮询是「非流式的快接口」这唯一的例外。
    pub timeout: TimeoutProfile,
    /// 上游 200 但响应体不是 JSON 时算失败（记 strike、换下一把）。
    ///
    /// 它的理由：放任非 JSON 冒泡到路由里的 `res.json()`，结果是一个纯文本 500，
    /// 而且**这把持续吐 HTML 错误页的 key 不会被记账、会被无限轮到**。
    pub expect_json: bool,
}

/// 转发的依赖。
pub struct DispatchDeps<'a> {
    pub repo: &'a KeyPoolRepo,
    pub fetcher: &'a dyn Fetcher,
    /// 🔴 **一份快照**（请求开头取一次）。不是 getter、更不是装配期展开的一堆字段。
    pub config: &'a GatewayConfig,
    pub logger: &'a dyn Logger,
    pub lb_mode: LbMode,
    /// 轮询游标。**每进程一份**，与 TS 的模块级游标逐字同语义。
    pub cursor: &'a AtomicUsize,
    /// 粘滞模式「当前这把」的 id。
    pub sticky: &'a Mutex<Option<String>>,
    /// 用量写端。**关着时是 `None`，那时这一层零成本。**
    ///
    /// 🔴 **它接在这里而不是四条路由上，是刻意的。**
    /// 逐路由各调一次 `record` 的失败形态是**「下一个人新加的那一条忘了调」**，
    /// 而那种遗漏不会有任何东西变红 —— 面板上只会安静地少一截数字。
    /// `dispatch` 是四协议**唯一的**出口，接在这里之后那种遗漏**在结构上不存在**。
    pub usage: Option<&'a crate::server::usage_sink::UsageSink>,
    /// 这次请求归属到哪一把对外密钥（来自鉴权中间件）。
    pub api_key_id: Option<String>,
}

/// 转发结果。
pub enum Dispatched {
    /// 上游 2xx。头已按白名单重建 + MIME 钳制。
    Ok {
        status: u16,
        /// **已经过白名单筛选**的上游响应头（调用方还要往里加 SSE 的 content-type）。
        headers: Vec<(String, String)>,
        body: OkBody,
    },
    /// 一个**已经成型、可以直接返回**的响应：上游 4xx 直通、503 / 504 / 502。
    Ready(Response),
}

pub enum OkBody {
    /// 未解析的响应体流（原样搬运 / 交给协议层转换）。
    Stream(BodyStream),
    /// `expect_json` 的路由：已确认是 JSON。
    Json(Value),
}

/// 读整段响应体的上界。超过就当「上游行为异常」，不再往内存里灌。
///
/// 只作用在 `expect_json` 的路由上（流式那条从不走这里）。
const MAX_JSON_BODY_BYTES: usize = 8 * 1024 * 1024;

/// 把一把 key 从**本请求的选择视图**里摘掉。
///
/// 🔴 **同一次请求的重试循环内绝不重复打同一把 key**（规格 05 §B6「跨 key 故障转移」，
/// 裁定：用 kiro 的显式排除，比靠游标推进更难写错）。
///
/// 没有它的后果是实测过的：`LbMode::Priority` + 3 把健康 key + 上游对第一把返 500
/// ⇒ 三次尝试**全打在同一把上**，另两把零尝试，客户端拿到那个 500 而它们本来能成功；
/// 那把 key 还在**一个请求之内**从 0 攒满 strike 进 30 分钟冷却。
/// 根因是 `apply_strike` 之后它仍然 `is_available`（`is_available` 不看 strikes），
/// 于是 `select_sticky` 每轮又把它还回来。
/// 连带地，503 那句「已尝试池中每一把 key」也是靠这条才成立。
///
/// ⚠️ **粘滞语义只被收窄到「跨请求」这一层**：`deps.sticky` 照常记着最后选中的那把，
/// 下一个请求仍然从它开始 —— 那才是 `Priority` 存在的理由。
///
/// ⚠️ **借 `is_available` 的 `disabled` 那一条否决来标记**，为的是让下标与 `records`
/// 逐位对齐：换成「把已试过的从数组里删掉」会让 [`select_key`](crate::core::keypool::select_key)
/// 返回的游标（那是**传进去那个切片**里的下标）指到别人身上。
///
/// 🔴 这份视图**只喂给 `select`，绝不进 `commit`**：它要是被当成 `prev` 写下去，
/// 这个假的 `disabled` 就成了一次真实的停用。
///
/// **代价老实写**：每个请求多克隆一份池子。`repo.all()` 本来就已经交出一份同样大小的
/// 浅拷贝快照，这是第二份 —— 换来的是「一次请求打死一把 key」这条实测事故不再发生。
fn mask_as_tried(r: &mut KeyRecord) {
    r.disabled = Some(true);
}

/// 🔴 **全仓唯一一条到上游的链路。**
///
/// # 用量记在这里，而且**只在这一层**
///
/// 🔴 **包一层、在唯一的出口记一笔**，而不是在下面那个函数的每个 `return` 前各记一次 ——
/// 后者的失败形态是「**某一条提前 return 忘了记**」，而那种遗漏不会有任何东西变红：
/// 面板上只会安静地少一截数字，**而「用量是 0」与「用量没记上」在屏幕上长得一模一样**。
pub async fn dispatch(deps: &DispatchDeps<'_>, args: DispatchArgs) -> Dispatched {
    // 关着时**零成本**：不取时钟、不取模型名、一个累加器都不碰。
    let Some(sink) = deps.usage else {
        return dispatch_inner(deps, args).await;
    };

    let started = now_ms();
    // ⚠️ **模型名与协议在发出去之前取** —— `args.body` 之后会被搬走。
    let model = args
        .body
        .as_ref()
        .and_then(|b| b.get("model").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let protocol = protocol_of(&args.path);
    // ⚠️ **「是不是流式」由超时档位判**，而不是再去猜一次请求体：
    // 那个档位本来就是调用方按「这条要不要流」显式传的（视频轮询是唯一的例外，
    // 它是非流式的快接口，走 FirstByte）。
    let stream = matches!(args.timeout, TimeoutProfile::FirstByte) && protocol != "media";

    let out = dispatch_inner(deps, args).await;

    let (ok, tokens_in, tokens_out) = match &out {
        Dispatched::Ok { body, .. } => match body {
            // ⚠️ **只有解析过响应体的那几条才有 token** —— 其余记 0，
            // 而那个缺口由 `streamingRequests` 那一栏**可见地**说出来。
            OkBody::Json(v) => {
                let u = v.get("usage");
                let n = |k: &str| {
                    u.and_then(|u| u.get(k))
                        .and_then(Value::as_i64)
                        .filter(|x| *x >= 0)
                        .unwrap_or(0)
                };
                (true, n("prompt_tokens"), n("completion_tokens"))
            }
            OkBody::Stream(_) => (true, 0, 0),
        },
        // ⚠️ `Ready` 装的是**已经成型的失败响应**（4xx 直通、503/504/502）。
        Dispatched::Ready(_) => (false, 0, 0),
    };

    sink.record(
        &crate::server::usage_sink::UsageOutcome {
            ok,
            stream,
            latency_ms: now_ms().saturating_sub(started),
            tokens_in,
            tokens_out,
            model,
            protocol,
            api_key_id: deps.api_key_id.clone(),
        },
        started,
    );
    out
}

/// 上游路径 ⇒ 协议名。
///
/// ⚠️ **这一维刻意不收上界**（见用量写端）：它的取值是这几条路由里的字面量，
/// 外部碰不到。
fn protocol_of(path: &str) -> &'static str {
    if path.starts_with("/messages") {
        "anthropic"
    } else if path.starts_with("/responses") {
        "responses"
    } else if path.contains("generateContent") || path.starts_with("/models") {
        "gemini"
    } else if path.starts_with("/images") || path.starts_with("/videos") {
        "media"
    } else {
        "openai"
    }
}

async fn dispatch_inner(deps: &DispatchDeps<'_>, args: DispatchArgs) -> Dispatched {
    let policy = PoolPolicy::from_config(deps.config);
    let start = now_ms();

    // 1. 一次快照，之后整轮都用它。
    let mut records = match deps.repo.all(start).await {
        Ok(r) => r,
        Err(e) => {
            // 🔴 存储读不出来**不是**「池子是空的」。对外只说「网关内部错误」
            //    （这条路径未必鉴权过，真实原因只进日志）。
            deps.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    "pool.load_failed",
                    "装载 key 池失败，本次请求无法转发",
                )
                .with("err", e.to_string()),
            );
            return Dispatched::Ready(GatewayError::internal().into_response());
        }
    };
    if records.is_empty() {
        return Dispatched::Ready(unavailable(&records, start).into_response());
    }

    // 3. 同步档的两个预算。
    let sync_deadline = start.saturating_add(deps.config.upstream_sync_timeout_ms);
    let sync_attempt_budget = (deps.config.upstream_sync_timeout_ms / SYNC_ATTEMPT_SHARE).max(1);

    let mut last_error: Option<Response> = None;
    let mut timed_out_slots: Vec<usize> = Vec::new();
    // 本请求的**选择视图**：打过一把就把它从这里摘掉。见 [`mask_as_tried`]。
    let mut selectable = records.clone();

    for _ in 0..records.len() {
        // a. 这一次尝试给多少预算。
        let mut timeout_ms = deps.config.upstream_timeout_ms;
        if args.timeout == TimeoutProfile::Sync {
            let remaining = sync_deadline - now_ms();
            if remaining <= 0 {
                // 预算耗尽就别再发一个注定被立刻掐断的请求。
                break;
            }
            timeout_ms = sync_attempt_budget.min(remaining);
        }

        // b. 选 key。**在选择视图上选**：本请求内已经打过的那几把不在候选里。
        let current = deps.sticky.lock().clone();
        let Some(sel) = select(
            deps.lb_mode,
            &selectable,
            deps.cursor.load(Ordering::Relaxed),
            current.as_deref(),
            now_ms(),
        ) else {
            return Dispatched::Ready(nothing_answered(
                deps,
                &records,
                &timed_out_slots,
                last_error,
            ));
        };
        deps.cursor.store(sel.next_cursor, Ordering::Relaxed);
        *deps.sticky.lock() = Some(sel.current_id.clone());
        let slot = sel.index;
        mask_as_tried(&mut selectable[slot]);

        // c. 发请求。
        let req = build_request(deps.config, &args, &records[slot].key, timeout_ms);
        let sent = deps.fetcher.send(req).await;

        let res = match sent {
            // d. 发不出去 / 超时。
            Err(err) => {
                let thrown = match err {
                    FetchError::TimeoutBeforeHeaders => ThrownError::Timeout,
                    FetchError::Connect(_) | FetchError::BodyInterrupted(_) => ThrownError::Network,
                };
                let action = classify_thrown(thrown);
                let reason = match &action {
                    UpstreamAction::Strike { reason } => reason.clone(),
                    _ => "unknown".to_string(),
                };
                // ⚠️ 同步档下的超时**先不记账**，等到本轮真有一把 key 成功了再归因。见 §3.7。
                if args.timeout == TimeoutProfile::Sync && thrown == ThrownError::Timeout {
                    timed_out_slots.push(slot);
                    continue;
                }
                let updated = apply_strike(&records[slot], now_ms(), &policy, &reason);
                commit_outcome(
                    deps,
                    &mut records,
                    slot,
                    updated,
                    Outcome::Failed,
                    Some(&reason),
                )
                .await;
                continue;
            }
            Ok(res) => res,
        };

        // e. 拿到响应头 ⇒ 超时已由适配器在这一刻解除（响应体自由流动）。
        let action = classify_status(res.status, &policy);

        match action {
            UpstreamAction::Success => {
                if args.expect_json {
                    let status = res.status;
                    let headers = res.headers.clone();
                    match read_json(res).await {
                        None => {
                            let why = "upstream non-JSON body";
                            let updated = apply_strike(&records[slot], now_ms(), &policy, why);
                            commit_outcome(
                                deps,
                                &mut records,
                                slot,
                                updated,
                                Outcome::Failed,
                                Some(why),
                            )
                            .await;
                            last_error = Some(
                                DispatchError::new(
                                    FailReason::UpstreamBadBody,
                                    "上游返回了非 JSON 的成功响应",
                                )
                                .into_response(),
                            );
                            continue;
                        }
                        Some(body) => {
                            attribute_sync_timeouts(
                                deps,
                                &mut records,
                                &mut timed_out_slots,
                                slot,
                                &policy,
                            )
                            .await;
                            let updated = apply_success(&records[slot], now_ms());
                            commit_outcome(
                                deps,
                                &mut records,
                                slot,
                                updated,
                                Outcome::Success,
                                None,
                            )
                            .await;
                            return Dispatched::Ok {
                                status,
                                headers: filtered_headers(&headers),
                                body: OkBody::Json(body),
                            };
                        }
                    }
                }
                attribute_sync_timeouts(deps, &mut records, &mut timed_out_slots, slot, &policy)
                    .await;
                let updated = apply_success(&records[slot], now_ms());
                commit_outcome(deps, &mut records, slot, updated, Outcome::Success, None).await;
                return Dispatched::Ok {
                    status: res.status,
                    headers: filtered_headers(&res.headers),
                    body: OkBody::Stream(res.body),
                };
            }
            // g. 上游 4xx 直通：**客户端的错，不是这把 key 的错**（状态不变，只记一笔 Tier-1）。
            UpstreamAction::Passthrough => {
                let rec = records[slot].clone();
                commit_outcome(deps, &mut records, slot, rec, Outcome::ClientError, None).await;
                return Dispatched::Ready(sanitize_error(res));
            }
            // h. 401/403：凭据失效。
            UpstreamAction::Evict { reason } => {
                // ⚠️ **只丢弃这次的 401，不动 `last_error`**：上游 401/403 说的是
                //    「池里这把 key 失效了」，与客户端无关，**绝不能把上游的错误体透传出去**
                //    （凭据无效的错误体恰恰是各家 API 最爱回显 key 片段的地方）。
                //    先前某把 key 的真实上游错误（例如 500）仍然更有信息量，不该被抹掉。
                drop(res);
                let updated = apply_evict(&records[slot], &reason);
                commit_outcome(
                    deps,
                    &mut records,
                    slot,
                    updated,
                    Outcome::Failed,
                    Some(&reason),
                )
                .await;
                continue;
            }
            // i. 其余：冷却 / strike。上游正文**脱敏之后**留作 last_error。
            UpstreamAction::Cooldown { ms, reason } => {
                last_error = Some(sanitize_error(res));
                let updated = apply_cooldown(&records[slot], now_ms(), ms, &reason);
                commit_outcome(
                    deps,
                    &mut records,
                    slot,
                    updated,
                    Outcome::Failed,
                    Some(&reason),
                )
                .await;
            }
            UpstreamAction::Strike { reason } => {
                last_error = Some(sanitize_error(res));
                let updated = apply_strike(&records[slot], now_ms(), &policy, &reason);
                commit_outcome(
                    deps,
                    &mut records,
                    slot,
                    updated,
                    Outcome::Failed,
                    Some(&reason),
                )
                .await;
            }
        }
    }

    Dispatched::Ready(nothing_answered(
        deps,
        &records,
        &timed_out_slots,
        last_error,
    ))
}

fn build_request(
    cfg: &GatewayConfig,
    args: &DispatchArgs,
    key: &str,
    timeout_ms: i64,
) -> OutboundRequest {
    let mut headers = vec![("authorization".to_string(), format!("Bearer {key}"))];
    // ⚠️ GET **不带 content-type**：带了会让某些上游按有正文处理。
    if args.method == Method::Post {
        headers.insert(
            0,
            ("content-type".to_string(), "application/json".to_string()),
        );
    }
    OutboundRequest {
        method: args.method.as_str().to_string(),
        url: format!("{}{}", cfg.agnes_base_url, args.path),
        headers,
        body: args
            .body
            .as_ref()
            .and_then(|b| serde_json::to_vec(b).ok())
            .map(Bytes::from),
        timeout_ms,
    }
}

/// 只留白名单里那几个头（给调用方继续用的字符串形态）。
fn filtered_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    safe_headers(headers)
        .into_iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}

/// 把响应体读成 JSON。读不动 / 不是 JSON / 太大 ⇒ `None`。
async fn read_json(res: OutboundResponse) -> Option<Value> {
    let mut body = res.body;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.ok()?;
        if buf.len() + chunk.len() > MAX_JSON_BODY_BYTES {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).ok()
}

/// 回写一条记录 + 推进本请求内的视图。
///
/// 🔴 **落盘失败必须吞掉**：这里是成功分支上的最后一步，
/// 异常会把一次**已经成功的上游转发**变成 500。
///
/// 🔴 **无论落盘成不成都要推进本请求内的视图**：同一个请求里换下一把 key 时
/// `select` 读的就是这个数组。
async fn commit(deps: &DispatchDeps<'_>, records: &mut [KeyRecord], at: usize, updated: KeyRecord) {
    // 传上一份：`save()` 据此判断「这次改动是不是只动了可消除的字段」。
    if let Err(e) = deps.repo.save(&updated, Some(&records[at]), now_ms()).await {
        deps.logger.log(
            LogEntry::new(
                LogLevel::Error,
                "pool.commit_failed",
                "key 状态回写失败：本次请求的结果不受影响，但这一次调度记账丢了",
            )
            .with("id", updated.id.clone())
            .with("err", e.to_string()),
        );
    }
    records[at] = updated;
}

/// **分两步**（`apply_*` 先算调度状态、再叠 Tier-1）而不是合并：
/// 调度那套是「丢一次就是事故」的，而 Tier-1 是明说了会少计的近似值 ——
/// 两套语义混在一个函数里迟早互相拖累。
async fn commit_outcome(
    deps: &DispatchDeps<'_>,
    records: &mut [KeyRecord],
    at: usize,
    updated: KeyRecord,
    outcome: Outcome,
    err_kind: Option<&str>,
) {
    let with = with_outcome(&updated, outcome, now_ms(), err_kind);
    commit(deps, records, at, with).await;
}

/// 🔴 **只有当同一次请求里另一把 key 真的成功了，才把先前那些超时记到对应 key 头上。**
///
/// 这是能拿到的最强的对照：同一时刻、同一个上游、同一份预算，A 超时而 B 成功
/// ⇒ 上游当时是应答得了的，超时来自 A 自己（例如会话挂起）。
///
/// 反过来，整轮下来没有任何 key 成功时更可能是预算配小了或上游整体变慢
/// —— **那是配置 / 上游的问题，不该记到 key 头上**
/// （原实现正是在这里把一次图片请求变成「整池各记一次 strike」）。
async fn attribute_sync_timeouts(
    deps: &DispatchDeps<'_>,
    records: &mut [KeyRecord],
    timed_out_slots: &mut Vec<usize>,
    success_slot: usize,
    policy: &PoolPolicy,
) {
    let slots: Vec<usize> = timed_out_slots
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    for at in slots {
        // 同一把 key 先超时后成功：它刚刚自证还活着，**不惩罚**。
        if at == success_slot {
            continue;
        }
        let updated = apply_strike(&records[at], now_ms(), policy, "sync timeout");
        commit_outcome(
            deps,
            records,
            at,
            updated,
            Outcome::Failed,
            Some("sync timeout"),
        )
        .await;
    }
    timed_out_slots.clear();
}

/// 一圈下来谁都没答。
fn nothing_answered(
    deps: &DispatchDeps<'_>,
    records: &[KeyRecord],
    timed_out_slots: &[usize],
    last_error: Option<Response>,
) -> Response {
    // 有过一个真实的上游错误 ⇒ 它比任何一句网关自造的话都有信息量。
    if let Some(r) = last_error {
        return r;
    }
    if !timed_out_slots.is_empty() {
        let tried = timed_out_slots.iter().collect::<HashSet<_>>().len();
        return sync_budget_exhausted(tried, deps.config.upstream_sync_timeout_ms).into_response();
    }
    unavailable(records, now_ms()).into_response()
}

#[cfg(test)]
mod tests {
    // ── 用量：dispatch 是唯一出口 ────────────────────────────────────────

    fn sink_for(
        mem: &std::sync::Arc<crate::adapters::storage_test_doubles::MemoryStorage>,
    ) -> crate::server::usage_sink::UsageSink {
        crate::server::usage_sink::UsageSink::new(
            mem.clone(),
            std::sync::Arc::new(NullLogger),
            "t",
            crate::server::usage_sink::USAGE_FLUSH_DEFAULT_MS,
            0,
        )
    }

    async fn flushed(
        mem: &crate::adapters::storage_test_doubles::MemoryStorage,
        sink: &crate::server::usage_sink::UsageSink,
    ) -> Value {
        sink.flush(now_ms()).await;
        // ⚠️ **按真实时钟的日号去读** —— `dispatch` 记的那一笔用的是 `now_ms()`，
        // 而不是夹具里那个 0。按 0 去读会拿到 `None`，
        // 而那个 `None` 看起来与「压根没记」一模一样。
        let day = crate::admin::usage::usage_day_index(now_ms());
        mem.get(&crate::admin::usage::usage_day_key(day, sink.status(0).0))
            .await
            .unwrap()
            .expect("一笔用量都没落盘")
    }

    /// 🔴 **这一格钉的是「转发真的记了一笔用量」。**
    ///
    /// 变异对象：把 `dispatch` 外层那一圈拆掉（直接调 `dispatch_inner`）——
    /// 面板上会安静地显示 0，**而「用量是 0」与「用量没记上」在屏幕上长得一模一样**。
    #[tokio::test]
    async fn a_successful_forward_records_exactly_one_usage_entry() {
        let mem = std::sync::Arc::new(crate::adapters::storage_test_doubles::MemoryStorage::new());
        let sink = sink_for(&mem);
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: r#"{"usage":{"prompt_tokens":7,"completion_tokens":9}}"#,
        }]);
        let h = Harness::new();
        let mut d = h.deps(&repo, f.as_ref(), LbMode::Balanced);
        d.usage = Some(&sink);
        d.api_key_id = Some("ak-7".into());

        // `expect_json: true` ⇒ 走解析过响应体那一档，才有 token。
        let out = dispatch(&d, args(true)).await;
        assert!(
            matches!(out, Dispatched::Ok { .. }),
            "夹具前提：这一次该成功"
        );
        assert_eq!(sink.status(0).1, 1, "🔴 转发完成了却一笔用量都没记");

        let v = flushed(&mem, &sink).await;
        assert_eq!(v["total"]["requests"], 1);
        assert_eq!(v["total"]["success"], 1);
        assert_eq!(v["total"]["tokensIn"], 7);
        assert_eq!(v["total"]["tokensOut"], 9);
        assert_eq!(
            v["byApiKey"]["ak-7"]["requests"], 1,
            "归属没接上 —— 它会落进「未归属」那一格：{}",
            v["byApiKey"],
        );
    }

    /// 🔴 **关着时零成本** —— 不是「建好再用一个 `if` 拦住写」。
    #[tokio::test]
    async fn a_disabled_usage_sink_makes_the_forward_path_touch_nothing() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![],
            body: "{}",
        }]);
        let h = Harness::new();
        let d = h.deps(&repo, f.as_ref(), LbMode::Balanced);
        assert!(d.usage.is_none(), "夹具前提：默认不接用量");
        assert!(matches!(
            dispatch(&d, args(true)).await,
            Dispatched::Ok { .. }
        ));
    }

    /// 失败的那一次也要记 —— 否则「错误率」那张卡的**分母是假的**。
    #[tokio::test]
    async fn a_failed_forward_is_recorded_as_an_error_not_dropped() {
        let mem = std::sync::Arc::new(crate::adapters::storage_test_doubles::MemoryStorage::new());
        let sink = sink_for(&mem);
        let repo = pool(&["sk-one"]).await;
        // 4xx 直通 ⇒ `Ready`，也就是失败那一档。
        let f = fake(vec![Step::Reply {
            status: 400,
            headers: vec![],
            body: "{}",
        }]);
        let h = Harness::new();
        let mut d = h.deps(&repo, f.as_ref(), LbMode::Balanced);
        d.usage = Some(&sink);

        let _ = dispatch(&d, args(true)).await;
        let v = flushed(&mem, &sink).await;
        assert_eq!(v["total"]["requests"], 1);
        assert_eq!(v["total"]["errors"], 1);
        assert_eq!(v["total"]["latencySum"], 0, "失败请求不许进延迟");
    }

    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::core::config::resolve_config;
    use crate::core::config_provenance::EnvMap;
    use crate::ports::logger::NullLogger;
    use crate::ports::storage::{Storage, StorageError, StorageResult};
    use axum::body::to_bytes;
    use serde_json::json;
    use std::sync::Arc;

    // ── 白名单与钳制 ────────────────────────────────────────────────────────

    /// 🔴 上游的内部头**不许**到达客户端；白名单里那四个照常过。
    ///
    /// 回归对象：实测 `x-upstream-internal` 会原封不动到达客户端。
    #[test]
    fn only_the_four_whitelisted_headers_survive() {
        let up = vec![
            ("content-type".into(), "application/json".into()),
            ("cache-control".into(), "no-cache".into()),
            ("retry-after".into(), "3".into()),
            (
                "content-disposition".into(),
                "attachment; filename=a.png".into(),
            ),
            ("set-cookie".into(), "sid=secret".into()),
            ("www-authenticate".into(), "Bearer realm=up".into()),
            ("x-upstream-internal".into(), "leak".into()),
            // 🔴 这两个必须被剥掉：响应体在网关这里被重新包装过。
            ("content-length".into(), "999".into()),
            ("content-encoding".into(), "gzip".into()),
            // 网关不支持 range，放行只会让客户端发起注定失败的请求。
            ("accept-ranges".into(), "bytes".into()),
        ];
        let got: Vec<String> = safe_headers(&up)
            .iter()
            .map(|(k, _)| k.as_str().to_string())
            .collect();
        assert_eq!(
            got,
            vec![
                "content-type",
                "cache-control",
                "retry-after",
                "content-disposition"
            ]
        );
    }

    /// 🔴 上游的 content-type 不许变成一个**可渲染的同源文档**。
    ///
    /// 回归对象：`GET /v1/videos/{id}` 接受 `?key=`，而 `ADMIN_TOKEN` 就在这个
    /// origin 的 localStorage 里（localStorage 的作用域是 origin 而不是 path）。
    #[test]
    fn document_mime_types_are_clamped_to_octet_stream() {
        for bad in [
            "text/html",
            "TEXT/HTML; charset=utf-8",
            "  text/html",
            "image/svg+xml",
            "application/xhtml+xml",
            "application/javascript",
            // deny-list 最经典的那条绕法：外层无害，Firefox 按内层 part 渲染。
            "multipart/x-mixed-replace; boundary=x",
        ] {
            assert_eq!(clamp_content_type(bad), "application/octet-stream", "{bad}");
        }
        for ok in [
            "application/json",
            "text/event-stream",
            "image/png",
            "video/mp4",
            // 没预料到的新媒体容器必须照常放行（这就是选 deny-list 的原因）。
            "image/avif",
        ] {
            assert_eq!(clamp_content_type(ok), ok);
        }
    }

    // ── 脱敏 ────────────────────────────────────────────────────────────────

    /// `sk-` 那一族被打码，而 request id / trace id 这类排障线索**留着**。
    #[test]
    fn only_sk_prefixed_credentials_are_masked() {
        assert_eq!(
            redact_text("invalid key sk-abcdef1234567890 for req"),
            "invalid key sk-*** for req"
        );
        // ⚠️ 刻意不做更宽的匹配：把 trace id 一并打码会让运维丢掉线索。
        let keep = "request_id=6f1a2b3c4d5e6f708192a3b4c5d6e7f8 model=gpt-x";
        assert_eq!(redact_text(keep), keep);
    }

    /// 🔴 **流式脱敏：跨块的密钥也要被抓住。**
    ///
    /// 回归对象：块边界可能正好落在密钥中间，不留 carry 就会漏掉半截。
    #[tokio::test]
    async fn a_credential_split_across_chunks_is_still_masked() {
        let chunks: Vec<Result<Bytes, FetchError>> = vec![
            Ok(Bytes::from_static(b"bad key sk-abc")),
            Ok(Bytes::from_static(b"def1234567890 end")),
        ];
        let body: BodyStream = Box::pin(futures_util::stream::iter(chunks));
        let out = collect(redact_stream(body)).await;
        assert_eq!(out, "bad key sk-*** end");
    }

    /// 🔴🔴 **块边界切在 `sk-` 这三个字符正中间时，凭据照样不许漏出去。**
    ///
    /// 回归对象：carry 只在找到**完整** `sk-` 时才留尾巴 ⇒ `...api key s` | `k-Ab12...`
    /// 两块各自 `rfind("sk-") == None` ⇒ 各自整块 emit、carry 为空
    /// ⇒ 客户端收到**完整明文**。
    ///
    /// ⚠️ 上一格（切在前缀之后）**永远绿，抓不到这一类** —— 所以切点必须扫过前缀内部。
    #[tokio::test]
    async fn a_credential_split_inside_the_sk_prefix_is_still_masked() {
        let full = r#"{"error":"invalid api key sk-Ab12Cd34Ef56Gh78"}"#;
        let at = full.find("sk-").unwrap();
        // 前缀之前 / `s` 之后 / `sk` 之后 / 前缀之后，四个切点全扫一遍。
        for cut in [at, at + 1, at + 2, at + 3] {
            let (a, b) = full.split_at(cut);
            let chunks: Vec<Result<Bytes, FetchError>> = vec![
                Ok(Bytes::copy_from_slice(a.as_bytes())),
                Ok(Bytes::copy_from_slice(b.as_bytes())),
            ];
            let body: BodyStream = Box::pin(futures_util::stream::iter(chunks));
            let out = collect(redact_stream(body)).await;
            assert!(
                !out.contains("Ab12Cd34"),
                "切在第 {cut} 字节时凭据原文漏出去了：{out}"
            );
            assert!(out.contains("sk-***"), "切在第 {cut} 字节：{out}");
        }
    }

    /// 🔴 **carry 不许无界增长。**
    ///
    /// `sk-` 后面跟着几千个合法字符的东西不是凭据 —— 不兜底的话，
    /// 上游（或攻击者影响得了的上游文案）就能让网关按它的输入长度占内存。
    #[tokio::test]
    async fn the_carry_buffer_is_bounded() {
        let long = format!("sk-{}", "a".repeat(REDACT_CARRY * 8));
        let chunks: Vec<Result<Bytes, FetchError>> = vec![Ok(Bytes::from(long.clone()))];
        let body: BodyStream = Box::pin(futures_util::stream::iter(chunks));
        let out = collect(redact_stream(body)).await;
        // 它整体被当成一个凭据打了码（`CREDENTIAL_LIKE` 的量词是贪婪的），
        // 这里钉的是**不挂死、不无界攒着**这件事。
        assert!(out.len() < long.len());
    }

    /// 多字节字符被切在块边界上时**不许变成替换字符**。
    #[tokio::test]
    async fn a_multibyte_char_split_across_chunks_survives() {
        let s = "上游错误".as_bytes();
        let (a, b) = s.split_at(5); // 切在「游」中间
        let chunks: Vec<Result<Bytes, FetchError>> =
            vec![Ok(Bytes::copy_from_slice(a)), Ok(Bytes::copy_from_slice(b))];
        let body: BodyStream = Box::pin(futures_util::stream::iter(chunks));
        assert_eq!(collect(redact_stream(body)).await, "上游错误");
    }

    /// 🔴🔴 **错误体必须边流边改写，不许「先读完整个 body 再替换」。**
    ///
    /// 第一版就是 `res.text().await` 之后再 replace，被判据当场抓住 ——
    /// 一个**永不关闭**的响应体会让它挂死（真实上游会关闭错误体，**半开连接不会**）。
    ///
    /// ⚠️ 装置照抄 `core/protocol/anthropic.rs` 那一格：上游**先说一句话，然后永不结束**。
    /// 喂有限流的判据对这条性质是瞎的 —— `stream::iter` 自己会结束，缓冲版照样绿。
    #[tokio::test(start_paused = true)]
    async fn the_error_body_is_rewritten_while_it_streams_never_buffered_whole() {
        let body: BodyStream = Box::pin(
            futures_util::stream::iter(vec![Ok(Bytes::from_static(b"upstream exploded "))])
                .chain(futures_util::stream::pending()),
        );
        let res = sanitize_error(OutboundResponse {
            status: 500,
            headers: vec![("content-type".into(), "text/plain".into())],
            body,
        });
        let mut out = res.into_body().into_data_stream();
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), out.next())
            .await
            .expect("上游还没关，这一块就该已经到客户端了")
            .expect("流不该就这么结束")
            .expect("这一块不该是错误");
        assert_eq!(first, Bytes::from_static(b"upstream exploded "));
    }

    async fn collect(s: impl futures_core::Stream<Item = Result<Bytes, FetchError>>) -> String {
        futures_util::pin_mut!(s);
        let mut out = Vec::new();
        while let Some(c) = s.next().await {
            out.extend_from_slice(&c.unwrap());
        }
        String::from_utf8(out).unwrap()
    }

    // ── 503 那一族 ──────────────────────────────────────────────────────────

    fn rec(id: &str) -> KeyRecord {
        KeyRecord::fresh(id, format!("sk-{id}"), 0)
    }

    /// 空池 ⇒ `pool_empty`，**不带 `Retry-After`**。
    #[test]
    fn an_empty_pool_says_so() {
        let e = unavailable(&[], 0);
        assert_eq!(e.reason, FailReason::PoolEmpty);
        assert_eq!(e.retry_after_secs, None);
    }

    /// 🔴 **全池冷却时的 `Retry-After` 不许被「停用的 key」拉到 1 秒。**
    ///
    /// 回归对象：被停用的 key 的 `cooldown_until` 通常是 0，
    /// 漏掉 `!is_disabled` 过滤会让 `min` 取到 0 ⇒ `Retry-After: 1`
    /// ⇒ 客户端每秒重试一次**永远不会回来的那把 key**。
    #[test]
    fn retry_after_ignores_disabled_keys() {
        let cooling = KeyRecord {
            cooldown_until: 60_000,
            ..rec("a")
        };
        let off = KeyRecord {
            disabled: Some(true),
            cooldown_until: 0,
            ..rec("b")
        };
        let e = unavailable(&[cooling, off], 0);
        assert_eq!(e.reason, FailReason::AllCooling);
        assert_eq!(e.retry_after_secs, Some(60));
    }

    /// ⚠️ `disabled == 0` 时的文案与旧版**逐字相同**（不许多出一句「0 把被管理员停用」）。
    #[test]
    fn the_cooling_message_does_not_grow_a_useless_clause() {
        let e = unavailable(
            &[KeyRecord {
                cooldown_until: 1_000,
                ..rec("a")
            }],
            0,
        );
        assert_eq!(
            e.message,
            "全部 key 暂不可用：1 把冷却中（到期自动恢复）、0 把已永久剔除"
        );
    }

    /// 🔴 **`all_disabled` / `all_evicted` 一律不带 `Retry-After`**：
    /// 它们不会自愈，带了等于让客户端每秒重试一把永远不会回来的 key。
    #[test]
    fn non_self_healing_states_never_carry_retry_after() {
        let off = KeyRecord {
            disabled: Some(true),
            ..rec("a")
        };
        let e = unavailable(&[off], 0);
        assert_eq!(e.reason, FailReason::AllDisabled);
        assert_eq!(e.retry_after_secs, None);

        let gone = KeyRecord {
            evicted: true,
            ..rec("b")
        };
        let e = unavailable(&[gone], 0);
        assert_eq!(e.reason, FailReason::AllEvicted);
        assert_eq!(e.retry_after_secs, None);
    }

    /// 还有可用的 key 却走到这里 ⇒ 只可能是「每一把都试过、上游都失败」。
    #[test]
    fn a_healthy_pool_that_reaches_here_blames_the_upstream() {
        assert_eq!(
            unavailable(&[rec("a")], 0).reason,
            FailReason::UpstreamError
        );
    }

    // ── 主循环 ──────────────────────────────────────────────────────────────

    /// 可编排的假上游。
    struct FakeFetcher {
        /// 每次 `send` 按顺序取一个。
        script: Mutex<Vec<Step>>,
        seen: Mutex<Vec<OutboundRequest>>,
    }

    enum Step {
        Reply {
            status: u16,
            headers: Vec<(String, String)>,
            body: &'static str,
        },
        Fail(FetchError),
        /// 先真的睡一会儿再超时 —— **把同步档的总预算烧掉一截**，
        /// 好让下一次尝试的 `remaining` 小于 `sync_attempt_budget`。
        SlowTimeout {
            ms: u64,
        },
    }

    #[async_trait::async_trait]
    impl Fetcher for FakeFetcher {
        async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
            self.seen.lock().push(req);
            let step = {
                let mut s = self.script.lock();
                if s.is_empty() {
                    None
                } else {
                    Some(s.remove(0))
                }
            };
            match step {
                // 脚本用完 = 判据写错了（发了预期之外的一次上游调用）。
                // 让它以一个**可观察的失败**收场，而不是悄悄成功。
                None => Err(FetchError::Connect(
                    "脚本用完了：发生了预期之外的上游调用".into(),
                )),
                Some(Step::Fail(e)) => Err(e),
                Some(Step::SlowTimeout { ms }) => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    Err(FetchError::TimeoutBeforeHeaders)
                }
                Some(Step::Reply {
                    status,
                    headers,
                    body,
                }) => Ok(OutboundResponse {
                    status,
                    headers,
                    body: Box::pin(futures_util::stream::iter(vec![Ok(Bytes::from_static(
                        body.as_bytes(),
                    ))])),
                }),
            }
        }
    }

    fn fake(steps: Vec<Step>) -> Arc<FakeFetcher> {
        Arc::new(FakeFetcher {
            script: Mutex::new(steps),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn cfg() -> GatewayConfig {
        cfg_with(&[])
    }

    fn cfg_with(extra: &[(&str, &str)]) -> GatewayConfig {
        let mut env = EnvMap::new();
        env.insert("GATEWAY_TOKEN".into(), "t-abcdefghijklmnop".into());
        for (k, v) in extra {
            env.insert((*k).into(), (*v).into());
        }
        resolve_config(&env, None, false).unwrap().0
    }

    /// 每一次上游调用**实际带出去的那把 key**（判据要看的就是这个序列）。
    fn used_keys(f: &FakeFetcher) -> Vec<String> {
        f.seen
            .lock()
            .iter()
            .map(|r| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == "authorization")
                    .expect("出站请求必须带 authorization")
                    .1
                    .clone()
            })
            .collect()
    }

    async fn pool(keys: &[&str]) -> Arc<KeyPoolRepo> {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let repo = Arc::new(KeyPoolRepo::new(
            storage,
            Arc::new(NullLogger),
            60_000,
            21_600_000,
        ));
        for k in keys {
            repo.add(k, 0).await.unwrap();
        }
        repo
    }

    struct Harness {
        cursor: AtomicUsize,
        sticky: Mutex<Option<String>>,
        cfg: GatewayConfig,
        logger: NullLogger,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_cfg(cfg())
        }
        fn with_cfg(cfg: GatewayConfig) -> Self {
            Self {
                cursor: AtomicUsize::new(0),
                sticky: Mutex::new(None),
                cfg,
                logger: NullLogger,
            }
        }
        fn deps<'a>(
            &'a self,
            repo: &'a KeyPoolRepo,
            fetcher: &'a dyn Fetcher,
            mode: LbMode,
        ) -> DispatchDeps<'a> {
            DispatchDeps {
                repo,
                fetcher,
                config: &self.cfg,
                logger: &self.logger,
                lb_mode: mode,
                cursor: &self.cursor,
                sticky: &self.sticky,
                // 这一族判据钉的是**转发本身** —— 用量那一层由
                // `server::usage_sink` 的判据与下面那两格单独钉。
                usage: None,
                api_key_id: None,
            }
        }
    }

    fn args(expect_json: bool) -> DispatchArgs {
        DispatchArgs {
            path: "/chat/completions".into(),
            method: Method::Post,
            body: Some(json!({ "model": "m" })),
            timeout: TimeoutProfile::FirstByte,
            expect_json,
        }
    }

    async fn body_of(res: Response) -> String {
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// 成功路径：上游 200 ⇒ 原样搬运，并且**发出去的是 `Bearer <key>`**。
    #[tokio::test]
    async fn a_successful_call_carries_the_selected_key_and_passes_the_body_through() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: r#"{"ok":true}"#,
        }]);
        let h = Harness::new();
        let out = dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await;
        let Dispatched::Ok { status, body, .. } = out else {
            panic!("应该成功");
        };
        assert_eq!(status, 200);
        assert!(matches!(body, OkBody::Stream(_)));
        assert_eq!(
            f.seen.lock()[0]
                .headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .unwrap()
                .1,
            "Bearer sk-one"
        );
    }

    /// 🔴 **401 那一把被永久剔除，而它的错误体一个字节都不许透传。**
    ///
    /// 回归对象：凭据无效的错误体恰恰是各家 API 最爱回显 key 片段的地方。
    #[tokio::test]
    async fn a_401_evicts_the_key_and_its_body_is_never_forwarded() {
        let repo = pool(&["sk-bad"]).await;
        let f = fake(vec![Step::Reply {
            status: 401,
            headers: vec![],
            body: r#"{"error":"invalid key sk-bad0123456789"}"#,
        }]);
        let h = Harness::new();
        let out = dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await;
        let Dispatched::Ready(res) = out else {
            panic!("应该是一个成型的响应");
        };
        let text = body_of(res).await;
        assert!(!text.contains("sk-bad0"), "上游 401 的正文不许透传：{text}");
        assert!(text.contains("all_evicted"), "整池剔除之后该说 all_evicted");
        // 记录真的被剔除了。
        assert!(repo.all(0).await.unwrap()[0].evicted);
    }

    /// 🔴 **其余 4xx 直通，并且正文里的凭据片段被打掉。**
    #[tokio::test]
    async fn a_client_error_is_passed_through_with_credentials_redacted() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 400,
            headers: vec![("content-type".into(), "application/json".into())],
            body: r#"{"message":"bad request from sk-abcdef1234567890"}"#,
        }]);
        let h = Harness::new();
        let Dispatched::Ready(res) =
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await
        else {
            panic!("4xx 应该直通");
        };
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let text = body_of(res).await;
        assert!(text.contains("sk-***"), "凭据片段必须打码：{text}");
        assert!(!text.contains("abcdef123456"));
        // 4xx 是客户端的错 ⇒ **不许**记在这把 key 头上（状态不变）。
        let r = &repo.all(0).await.unwrap()[0];
        assert_eq!(r.strikes, 0);
        assert!(!r.evicted);
    }

    /// 429 ⇒ 进冷却、换下一把；第二把成功则整体成功。
    #[tokio::test]
    async fn a_rate_limited_key_cools_down_and_the_next_key_is_tried() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::Reply {
                status: 429,
                headers: vec![],
                body: "slow down",
            },
            Step::Reply {
                status: 200,
                headers: vec![],
                body: r#"{"ok":1}"#,
            },
        ]);
        let h = Harness::new();
        let out = dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await;
        assert!(matches!(out, Dispatched::Ok { .. }), "第二把应该成功");
        let cooling = repo
            .all(0)
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.cooldown_until > 0)
            .count();
        assert_eq!(cooling, 1, "恰好一把进了冷却");
    }

    /// 🔴 **上游 200 但正文不是 JSON**（只在 `expect_json` 的路由上）：
    /// 记 strike、换下一把，全部失败时 **502 `upstream_bad_body`**。
    ///
    /// 回归对象：放任它冒泡到路由里的 json 解析，结果是一个纯文本 500，
    /// 而且这把持续吐 HTML 错误页的 key **不会被记账、会被无限轮到**。
    #[tokio::test]
    async fn a_non_json_success_is_charged_to_the_key_and_ends_in_502() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: "<html>upstream error page</html>",
        }]);
        let h = Harness::new();
        let Dispatched::Ready(res) =
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(true)).await
        else {
            panic!("该走 502");
        };
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        assert!(body_of(res).await.contains("upstream_bad_body"));
        assert_eq!(repo.all(0).await.unwrap()[0].strikes, 1);
    }

    /// 池子里一把可用的都没有 ⇒ **一次上游都不发**。
    #[tokio::test]
    async fn a_fully_cooling_pool_does_not_touch_the_upstream() {
        let repo = pool(&["sk-one"]).await;
        let all = repo.all(0).await.unwrap();
        // ⚠️ 冷却时刻必须相对**真实墙钟**：转发热路径读的是 `now_ms()`，
        //    写一个小常数的话它在被读到的那一刻早就过期了 —— 这一格会静默失效。
        let cooling = KeyRecord {
            cooldown_until: now_ms() + 600_000,
            ..all[0].clone()
        };
        repo.save(&cooling, Some(&all[0]), 0).await.unwrap();

        let f = fake(vec![]);
        let h = Harness::new();
        let Dispatched::Ready(res) =
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await
        else {
            panic!("该是 503");
        };
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(f.seen.lock().is_empty(), "不许对着一把冷却中的 key 发请求");
    }

    /// 🔴 **`LbMode::Priority` 是粘滞的**：上一把还能用就继续用它，不换人。
    ///
    /// ⚠️ 这一格**不复用** balanced 的判据 —— 两种模式各有各的一组。
    #[tokio::test]
    async fn priority_mode_sticks_to_the_same_key() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
        ]);
        let h = Harness::new();
        for _ in 0..3 {
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Priority), args(false)).await;
        }
        let used: Vec<String> = f
            .seen
            .lock()
            .iter()
            .map(|r| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == "authorization")
                    .unwrap()
                    .1
                    .clone()
            })
            .collect();
        assert_eq!(used.len(), 3);
        assert!(
            used.iter().all(|u| u == &used[0]),
            "粘滞模式三次应该是同一把：{used:?}"
        );
    }

    /// 🔴🔴 **同一次请求的重试循环内绝不重复打同一把 key** —— 粘滞是**跨请求**的，
    /// 不是「一个请求里咬住一把不放」。
    ///
    /// 回归对象（实测）：3 把健康 key、`LbMode::Priority`、上游对第一把返 500 ⇒
    /// `used == ["sk-one","sk-one","sk-one"]`，另两把**零尝试**，客户端拿到那个 500,
    /// 而另两把本来能成功。根因是 `apply_strike` 之后那把 key 仍 `is_available`
    /// ⇒ `select_sticky` 又把它还回来。
    #[tokio::test]
    async fn priority_mode_fails_over_to_other_keys_within_one_request() {
        let repo = pool(&["sk-one", "sk-two", "sk-three"]).await;
        let f = fake(vec![
            Step::Reply {
                status: 500,
                headers: vec![],
                body: "boom",
            },
            Step::Reply {
                status: 500,
                headers: vec![],
                body: "boom",
            },
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
        ]);
        let h = Harness::new();
        let out = dispatch(&h.deps(&repo, f.as_ref(), LbMode::Priority), args(false)).await;
        assert!(
            matches!(out, Dispatched::Ok { .. }),
            "第三把是好的 ⇒ 这次请求就该成功，而不是把那个 500 交给客户端"
        );
        let used = used_keys(&f);
        assert_eq!(used.len(), 3);
        assert_eq!(
            used.iter().collect::<HashSet<_>>().len(),
            3,
            "同一次请求内不许重复打同一把 key：{used:?}"
        );
    }

    /// 🔴 **`upstream_error` 那句「已尝试池中每一把 key」必须是真话。**
    ///
    /// 回归对象：粘滞模式在一个请求里反复打同一把 key ⇒ 它从 0 攒满 strike 进长冷却、
    /// 另两把零尝试，而 503 还在跟运维说「每一把都试过了」。
    #[tokio::test]
    async fn the_upstream_error_503_only_claims_every_key_was_tried_when_it_really_was() {
        let repo = pool(&["sk-one", "sk-two", "sk-three"]).await;
        let f = fake(vec![
            Step::Fail(FetchError::Connect("dns".into())),
            Step::Fail(FetchError::Connect("dns".into())),
            Step::Fail(FetchError::Connect("dns".into())),
        ]);
        let h = Harness::new();
        let Dispatched::Ready(res) =
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Priority), args(false)).await
        else {
            panic!("三把全连不上 ⇒ 503");
        };
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let text = body_of(res).await;
        assert!(text.contains("已尝试池中每一把 key"), "{text}");
        let used = used_keys(&f);
        assert_eq!(
            used.iter().collect::<HashSet<_>>().len(),
            3,
            "说了「每一把都试过」就真的得每一把都试过：{used:?}"
        );
        // 而且一个请求不许把一把 key 从 0 攒满 strike 打进 30 分钟长冷却。
        for r in repo.all(0).await.unwrap() {
            assert_eq!(r.strikes, 1, "每把各记一笔，不是一把吃三笔");
            assert_eq!(r.cooldown_until, 0);
        }
    }

    /// `LbMode::Balanced` 则轮着来。
    #[tokio::test]
    async fn balanced_mode_rotates() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
        ]);
        let h = Harness::new();
        for _ in 0..2 {
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await;
        }
        let used: Vec<String> = f
            .seen
            .lock()
            .iter()
            .map(|r| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == "authorization")
                    .unwrap()
                    .1
                    .clone()
            })
            .collect();
        assert_ne!(used[0], used[1], "轮询模式两次应该换人");
    }

    /// GET **不带 content-type**（带了会让某些上游按有正文处理）。
    #[tokio::test]
    async fn a_get_request_carries_no_content_type() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![],
            body: "{}",
        }]);
        let h = Harness::new();
        dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                path: "/videos/abc".into(),
                method: Method::Get,
                body: None,
                timeout: TimeoutProfile::FirstByte,
                expect_json: false,
            },
        )
        .await;
        assert!(
            !f.seen.lock()[0]
                .headers
                .iter()
                .any(|(k, _)| k == "content-type")
        );
    }

    // ── 两档超时真的发出去了 ────────────────────────────────────────────────

    /// 🔴 **`FirstByte` 档发出去的是 `upstream_timeout_ms`（默认 8000）。**
    ///
    /// ⚠️ 期望值**手写字面量**，不许从配置或 `SYNC_ATTEMPT_SHARE` 现算 ——
    /// 现算的话把两档的配置字段互换一下这一格照样绿。
    #[tokio::test]
    async fn the_first_byte_profile_sends_the_short_budget() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![],
            body: "{}",
        }]);
        let h = Harness::new();
        assert_eq!(h.cfg.upstream_timeout_ms, 8_000, "默认值变了先回规格 §3.5");
        dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                timeout: TimeoutProfile::FirstByte,
                ..args(false)
            },
        )
        .await;
        assert_eq!(f.seen.lock()[0].timeout_ms, 8_000);
    }

    /// 🔴 **`Sync` 档单次尝试拿的是总预算的一半（默认 120_000 ⇒ 60_000）。**
    ///
    /// 回归对象正是 01 §3.5 记着的那次实测事故：`POST /v1/images/generations`
    /// 拿到 `8000/2 = 4000ms` 预算 ⇒ **图片生成 100% 失败**，且每次请求把池中每把 key
    /// 各记一次 strike，三次请求就把整池打进 30 分钟长冷却、连对话一起拖死。
    ///
    /// ⚠️ 期望值**手写字面量**：从 `SYNC_ATTEMPT_SHARE` 现算的话，
    /// 把它从 2 改成 1 这一格会跟着一起变，等于没测。
    #[tokio::test]
    async fn the_sync_profile_sends_half_of_the_long_budget() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Reply {
            status: 200,
            headers: vec![],
            body: "{}",
        }]);
        let h = Harness::new();
        assert_eq!(
            h.cfg.upstream_sync_timeout_ms, 120_000,
            "默认值变了先回规格 §3.5"
        );
        dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                timeout: TimeoutProfile::Sync,
                ..args(false)
            },
        )
        .await;
        assert_eq!(f.seen.lock()[0].timeout_ms, 60_000);
    }

    /// 🔴 **同步档的下一次尝试只能用**剩下的**预算（`.min(remaining)`）。**
    ///
    /// 它是「客户端最坏只等一个预算」那条承诺的全部实现：少了这一格，
    /// 两次尝试各拿满半个预算之外还能再拿一次，总时长就越过 `UPSTREAM_SYNC_TIMEOUT_MS`。
    ///
    /// ⚠️ 这一格用**真实墙钟**（`dispatch` 读的是 `now_ms()`）：把总预算配成 1200ms，
    /// 让第一把 key 睡掉 800ms，剩下的预算必然小于单次配额 600ms。
    #[tokio::test]
    async fn a_sync_retry_never_exceeds_the_remaining_budget() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::SlowTimeout { ms: 800 },
            Step::Fail(FetchError::TimeoutBeforeHeaders),
        ]);
        let h = Harness::with_cfg(cfg_with(&[("UPSTREAM_SYNC_TIMEOUT_MS", "1200")]));
        dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                timeout: TimeoutProfile::Sync,
                ..args(false)
            },
        )
        .await;
        let sent: Vec<i64> = f.seen.lock().iter().map(|r| r.timeout_ms).collect();
        assert_eq!(sent.len(), 2, "第二把还有预算，该发出去：{sent:?}");
        assert_eq!(sent[0], 600, "第一次拿满单次配额 = 总预算的一半");
        assert!(
            (1..=400).contains(&sent[1]),
            "第二次只能拿剩下的那 400ms 以内（坏行为是照样拿满 600）：{sent:?}"
        );
    }

    /// 🔴 **没有任何 key 成功时，同步档的超时不许记到 key 头上。**
    ///
    /// 回归对象：原实现在这里把一次图片请求变成「整池各记一次 strike」，
    /// 三次请求就能把整个池子打进长冷却。
    #[tokio::test]
    async fn sync_timeouts_are_not_charged_when_nothing_succeeded() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::Fail(FetchError::TimeoutBeforeHeaders),
            Step::Fail(FetchError::TimeoutBeforeHeaders),
        ]);
        let h = Harness::new();
        let Dispatched::Ready(res) = dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                timeout: TimeoutProfile::Sync,
                ..args(false)
            },
        )
        .await
        else {
            panic!("该是 504");
        };
        assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(body_of(res).await.contains("upstream_timeout"));
        for r in repo.all(0).await.unwrap() {
            assert_eq!(r.strikes, 0, "本次未惩罚任何 key");
        }
    }

    /// 🔴 **有一把成功了，先前那些同步档超时才被记到各自 key 头上。**
    ///
    /// 这是能拿到的最强对照：同一时刻、同一个上游、同一份预算，A 超时而 B 成功。
    #[tokio::test]
    async fn sync_timeouts_are_charged_once_another_key_succeeds() {
        let repo = pool(&["sk-one", "sk-two"]).await;
        let f = fake(vec![
            Step::Fail(FetchError::TimeoutBeforeHeaders),
            Step::Reply {
                status: 200,
                headers: vec![],
                body: "{}",
            },
        ]);
        let h = Harness::new();
        let out = dispatch(
            &h.deps(&repo, f.as_ref(), LbMode::Balanced),
            DispatchArgs {
                timeout: TimeoutProfile::Sync,
                ..args(false)
            },
        )
        .await;
        assert!(matches!(out, Dispatched::Ok { .. }));
        let struck = repo
            .all(0)
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.strikes > 0)
            .count();
        assert_eq!(struck, 1, "恰好那把超时的被记了一笔");
    }

    /// 非超时的失败（连不上）**照常当场记账**，不进归因队列。
    #[tokio::test]
    async fn a_connect_failure_is_charged_immediately() {
        let repo = pool(&["sk-one"]).await;
        let f = fake(vec![Step::Fail(FetchError::Connect("dns".into()))]);
        let h = Harness::new();
        let _ = dispatch(&h.deps(&repo, f.as_ref(), LbMode::Balanced), args(false)).await;
        assert_eq!(repo.all(0).await.unwrap()[0].strikes, 1);
    }

    /// 存储随时可以被冻成**只读**：`get` / `list` 照常，`put` / `delete` 一律失败。
    /// （先让夹具把 key 导进去，再冻。）
    struct FreezableStorage {
        inner: Arc<MemoryStorage>,
        frozen: std::sync::atomic::AtomicBool,
    }

    impl FreezableStorage {
        fn new(inner: Arc<MemoryStorage>) -> Self {
            Self {
                inner,
                frozen: std::sync::atomic::AtomicBool::new(false),
            }
        }
        fn freeze(&self) {
            self.frozen.store(true, Ordering::Relaxed);
        }
        fn is_frozen(&self) -> bool {
            self.frozen.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl Storage for FreezableStorage {
        async fn get(&self, k: &str) -> StorageResult<Option<Value>> {
            self.inner.get(k).await
        }
        async fn put(&self, k: &str, v: Value, e: Option<i64>) -> StorageResult<()> {
            if self.is_frozen() {
                return Err(StorageError::new("EROFS"));
            }
            self.inner.put(k, v, e).await
        }
        async fn delete(&self, k: &str) -> StorageResult<()> {
            if self.is_frozen() {
                return Err(StorageError::new("EROFS"));
            }
            self.inner.delete(k).await
        }
        async fn list(&self, p: &str) -> StorageResult<Vec<String>> {
            self.inner.list(p).await
        }
    }

    /// 🔴 **落盘失败了，本请求内的视图照样要推进。**（规格点名的 52 格之一。）
    ///
    /// 回归对象：把 `records[at] = updated` 挪进 `if let Ok(..)` ⇒ 存储只读时
    /// 每一把 key 在这一轮里的判定结果都留不下来 ⇒ 这一轮明明**每一把都被上游 401 判死**，
    /// 503 却回 `upstream_error`「key 本身仍可用」，把运维指向上游而不是凭据。
    #[tokio::test]
    async fn a_commit_that_could_not_be_persisted_still_advances_the_in_request_view() {
        let mem = Arc::new(MemoryStorage::new());
        let store = Arc::new(FreezableStorage::new(mem));
        let repo = KeyPoolRepo::new(store.clone(), Arc::new(NullLogger), 60_000, 21_600_000);
        repo.add("sk-one", 0).await.unwrap();
        repo.add("sk-two", 0).await.unwrap();
        // 此后任何一次回写都失败 —— 但这次请求已经知道的事实不许跟着丢。
        store.freeze();

        let f = fake(vec![
            Step::Reply {
                status: 401,
                headers: vec![],
                body: "invalid key",
            },
            Step::Reply {
                status: 401,
                headers: vec![],
                body: "invalid key",
            },
        ]);
        let h = Harness::new();
        let Dispatched::Ready(res) =
            dispatch(&h.deps(&repo, f.as_ref(), LbMode::Priority), args(false)).await
        else {
            panic!("两把都 401 ⇒ 503");
        };
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let text = body_of(res).await;
        assert!(
            text.contains("all_evicted"),
            "两把都在本请求内被判了凭据失效，503 就该这么说：{text}"
        );
        // 落盘确实失败了（存储里两条都还没被标成 evicted）—— 视图推进与落盘是两件事。
        for r in repo.all(0).await.unwrap() {
            assert!(!r.evicted, "存储只读 ⇒ 这次记账本来就落不了盘");
        }
    }
}
