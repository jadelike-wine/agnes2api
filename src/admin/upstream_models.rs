//! `GET /admin/api/upstream/models` —— **真打一次上游，问它此刻有哪些模型**
//! （02 §9.4，01 §8.9）。
//!
//! # ⚠️⚠️ 它**不是** [`crate::admin::models`] 的替代品，两者回答的是两个问题
//!
//! - 协议目录（`GET /admin/api/models`）：**本网关**支持哪些协议、哪些端点、
//!   拿哪个模型名去调 —— 每条都带着协议归属与端点表；
//! - 这一条：**上游这个账号**此刻的模型清单长什么样 —— 一串光秃秃的 id。
//!
//! 拿上游的返回去替换目录，目录那四个消费者（集成示例卡、Playground、模型表、
//! 连通性测试）会立刻拿到一份**没有端点、没有协议归属**的清单。
//! ⇒ **并存，并且把差集明确标出来**（[`diff_against_catalog`]）。
//!
//! # 🔴 两个方向的差集都要给，而且措辞不许含糊
//!
//! - `onlyUpstream`：上游有、目录没有 ⇒ 目录**该不该补一条**
//!   （补之前得先给它填协议归属与端点）；
//! - `onlyCatalog`：目录有、上游这次没回 ⇒ **本仓正在承诺一个上游此刻不认的模型名**，
//!   那是运维照着目录会踩空的那一半。
//!   ⚠️ **它不许被读成「目录写错了」**：上游按账号发模型，一把权限较窄的 key
//!   拉回来的清单本来就会短 ⇒ 呈现那一侧的措辞该是「这次上游没回」，不是「不存在」。
//!
//! # ⚠️⚠️ `{ data: [{ id }] }` 这个形状是**推断**的，不是实测的
//!
//! 手上没有一把有效的 Agnes key，**上游 200 的真实报文一次都没验过**。
//! ⇒ 认不出来的一律 `bad_payload`（**不是**一份空清单），
//! 见 [`parse_upstream_models`] 里那条「一条都没抽出来」的闸。

use crate::admin::keys::{ProbeAcquire, ProbeGuard, probe_guard};
use crate::admin::models::MODEL_CATALOG;
use crate::core::keypool::select_key;
use crate::ports::fetcher::{FetchError, OutboundRequest, OutboundResponse};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::errors::GatewayError;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

// ── 常量 ───────────────────────────────────────────────────────────────────

/// 注册用的子树内路径。🔴 `app.rs` 的 `.route()` 从这里取，不许手抄字面量。
pub const UPSTREAM_MODELS_SUBPATH: &str = "/upstream/models";

/// **对上游**的那条路径（拼在 `agnes_base_url` 之后）。
///
/// ⚠️ 与对外的 `/admin/api/upstream/models` 是两个东西，别读混：
/// 前者是我们去问上游，后者是面板来问我们。
pub const UPSTREAM_MODELS_UPSTREAM_PATH: &str = "/models";

/// 最多收多少个 id（01 §8.9，`upstream-models.ts:23`）。
///
/// 超过就 `truncated = true` 并停 —— **截断的那一半由 `truncated` 如实交代，
/// 不静默丢**：静默丢会让运维以为上游就这些模型。
pub const UPSTREAM_MODELS_MAX: usize = 500;

/// 读整份清单的字节上界。超过 ⇒ `body_incomplete`（我们**没把它读完**，这是实话）。
///
/// ⚠️ 与 `pool::dispatcher` 那个 8 MiB 同量级但**刻意更小**：那一条要兜住真实对话的
/// 响应体，这一条读的是一串模型名 —— 上游若吐出 1 MiB 的模型清单，
/// 那件事本身就该被当成「上游行为异常」，而不是往内存里灌。
const MAX_LIST_BODY_BYTES: usize = 1024 * 1024;

/// 护栏的 kind。
///
/// 🔴 **常量，不带任何后缀**（02 §10：`verify:<id>` / `channel:<ch>` 逐对象分槽，
/// 而 `"model-test"` / `"upstream-models"` 是两个常量）。
/// 理由与 [`crate::admin::models::MODEL_TEST_KIND`] 同源：这条端点每一次调用
/// 打的都是**同一个上游账号**，让它们互相挡才是扛上游边缘限流的那道闸
/// （上游约 2 次快请求就回 1015）。
pub const UPSTREAM_MODELS_KIND: &str = "upstream-models";

// ── 1. 解析上游那份清单（01 §8.9，纯函数）──────────────────────────────────

/// 上游此刻的模型清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpstreamModels {
    pub ids: Vec<String>,
    /// 超过 [`UPSTREAM_MODELS_MAX`] 被截断了没有。
    pub truncated: bool,
}

/// 从上游的响应体里抽模型 id。**读不出来是 `None`，不是空数组。**
///
/// # 🔴 `None` 与「空清单」是两句话
///
/// 「上游一个模型都没有」与「这份响应我们看不懂」在屏幕上长得**一模一样**
/// （都是一张空表），但前者是一句关于上游的事实，后者是一句关于我们自己的话。
/// ⇒ 只认 `{ data: [{ id }] }` 这个形状，认不出来一律 `None`（handler 报 `bad_payload`）。
///
/// # ⚠️ 坏记录**逐条跳过**，这与本仓自己的目录**刻意相反**
///
/// `data` 里混进的非对象 / 没有 `id` / `id` 不是字符串的，跳过就是了 ——
/// 这是**别人家**的清单，多一条我们不认识的记录不该让整个功能失效。
/// （本仓自己的目录是反过来的：一条坏的就整份判成读不出来，因为那是本仓自己的缺陷，
/// 必须整份报警。）
///
/// # ⚠️⚠️ 「逐条跳过」有一个必须堵住的尽头：**一条都没抽出来**
///
/// `data` 非空而一个 id 都没抽出来时，逐条跳过的终局是一个空数组 ——
/// 而空数组在屏幕上说的是「上游这次一个模型都没回」，那是一句关于上游的事实，
/// 可我们手上只有一句关于自己的话。⇒ 这一档回 `None`。
/// 🔴 **判据是「一条都没抽出来」而不是「跳过了几条」**：跳掉两条还留下三条时，
/// 那三条是真的。
/// ⚠️ **`data` 是空数组时不走这里** —— 那一档才是真的一句上游事实。
#[must_use]
pub fn parse_upstream_models(payload: &Value) -> Option<UpstreamModels> {
    let data = payload.get("data")?.as_array()?;

    let mut ids: Vec<String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut truncated = false;
    for item in data {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue; // 坏记录逐条跳过。
        };
        if id.is_empty() || !seen.insert(id) {
            continue; // 空串与重复都不进清单。
        }
        if ids.len() >= UPSTREAM_MODELS_MAX {
            truncated = true;
            break;
        }
        ids.push(id.to_string());
    }

    // 🔴 那道闸：`data` 里**有东西**却一条都没抽出来 ⇒ 这份响应我们看不懂。
    if ids.is_empty() && !data.is_empty() {
        return None;
    }
    Some(UpstreamModels { ids, truncated })
}

/// 与本仓目录的**双向**差集。**两个方向都现算**，不缓存、不预生成。
///
/// ⚠️ 顺序：`only_upstream` 按上游给的顺序，`only_catalog` 按目录顺序 ——
/// 两边各自的顺序都是它那一侧的事实，排序会把它抹掉。
#[must_use]
pub fn diff_against_catalog(ids: &[String]) -> (Vec<String>, Vec<String>) {
    let catalog: BTreeSet<&str> = MODEL_CATALOG.iter().map(|m| m.id).collect();
    let upstream: BTreeSet<&str> = ids.iter().map(String::as_str).collect();

    let only_upstream = ids
        .iter()
        .filter(|id| !catalog.contains(id.as_str()))
        .cloned()
        .collect();
    let only_catalog = MODEL_CATALOG
        .iter()
        .filter(|m| !upstream.contains(m.id))
        .map(|m| m.id.to_string())
        .collect();
    (only_upstream, only_catalog)
}

// ── 2. 线格式（🔴 一律结构体，不用 `json!` 宏 —— 键序是对外契约）─────────────

/// 失败的机器可读判别串。**面板按它选五语言文案，不解析中文 `message`。**
///
/// 🔴 **传输失败分三档，不许混成一句**（`admin-upstream-models` 那 18 格钉的就是它）：
/// 「根本连不上」`network_error` / 「响应头没来」`timeout` / 「正文没来」`body_incomplete`
/// —— 三者的处置方向完全不同（查网络 / 调超时 / 看上游是不是在中途掐流）。
mod reason {
    /// 池里一把能用的 key 都没有。**不是错误**，是一句要说清的话。
    pub const NO_KEY: &str = "no_key";
    /// 上游回了，但不是 2xx。
    pub const UPSTREAM_ERROR: &str = "upstream_error";
    /// 响应头到手了，正文没读全（中途断了 / 超上界）。
    pub const BODY_INCOMPLETE: &str = "body_incomplete";
    /// 正文读全了，但不是 JSON / 形状不认。
    pub const BAD_PAYLOAD: &str = "bad_payload";
    /// 响应头到达之前就超时了。
    pub const TIMEOUT: &str = "timeout";
    /// 根本没连上（DNS / TCP / TLS）。
    pub const NETWORK_ERROR: &str = "network_error";
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelsBlock {
    ids: Vec<String>,
    truncated: bool,
    /// 上游有、目录没有。
    only_upstream: Vec<String>,
    /// 目录有、上游这次没回。⚠️ **不是「不存在」**，见文件头。
    only_catalog: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    ok: bool,
    /// 上游的状态码。**头都没到手时是 `null`**，不是 0 —— 见 01 §1「诚实第一」。
    status: Option<u16>,
    latency_ms: i64,
    reason: Option<&'static str>,
    /// 🔴 **失败时恒 `null`，不是一份空清单**：空清单说的是「上游一个模型都没有」。
    models: Option<ModelsBlock>,
}

#[derive(Serialize)]
struct GatewayInner {
    r#type: &'static str,
    message: String,
}

/// 02 §4.4 的「handler 自拼」混合体：**网关信封 + 顶层 `reason`**。
#[derive(Serialize)]
struct WithReason {
    error: GatewayInner,
    reason: &'static str,
}

fn failed(status: Option<u16>, latency_ms: i64, reason: &'static str) -> Response {
    Json(Wire {
        ok: false,
        status,
        latency_ms,
        reason: Some(reason),
        models: None,
    })
    .into_response()
}

// ── 3. 端点 ────────────────────────────────────────────────────────────────

/// `GET /admin/api/upstream/models` —— **真打一次上游**。
///
/// 骨架与 [`crate::admin::models::test`] 同源（选 key 的判据是 `select_key()` 而不是
/// 「池子长度为 0」、护栏排在选 key 之后、零存储写）。
/// 下面只写**与那条端点不同的三处**。
///
/// ── 共同点：**护栏走 RAII 租约，不手写 `release`** ────────────────────────
///
/// ⚠️ **这一条曾经是「不同点 ⓪」，今天不是了** —— 三条探测端点已经全部收口到
/// [`crate::admin::keys::ProbeLease`]，drop 即放开。留着这一段是因为它记着**为什么**：
/// 手写配对漏掉的是「请求被取消」那一支（axum 直接 drop handler 的 future），
/// 而本端点的 kind 是常量 ⇒ 漏一次卡住的是**整颗按钮、对所有人、永久**。
/// 由 [`tests::a_cancelled_request_does_not_wedge_the_button_forever`] 钉住。
///
/// ── 不同点 ①：**它要读响应体** ────────────────────────────────────────────
///
/// 连通性测试把上游正文整个丢掉（那是「一个字节都不回」的物理保证）；
/// 这一条的产出**就是**正文里那串 id ⇒ 必须读。
/// 于是它比那一条多出一整档失败形态：**头到手了但正文没读全**
/// （[`reason::BODY_INCOMPLETE`]）。🔴 这一档不许并进 `network_error`。
///
/// ── 不同点 ②：**GET，没有请求体** ─────────────────────────────────────────
///
/// 也因此没有「这条端点不收任何选项」那道闸。
pub async fn upstream_models(State(st): State<AppState>) -> Response {
    upstream_models_with(&st, probe_guard(), &crate::adapters::now_ms).await
}

/// [`upstream_models`] 的本体，**护栏与时钟可注入**。
///
/// 理由与 [`crate::admin::models::test_with`] 逐字相同：护栏是**全进程单例**，
/// 而「连点被挡」「挡完之后能恢复」这两条性质在固定单例 + 真墙钟上不可观测。
pub async fn upstream_models_with(
    st: &AppState,
    guard: &ProbeGuard,
    now: &(dyn Fn() -> i64 + Sync),
) -> Response {
    let at = now();
    let records = match st.repo.all(at).await {
        Ok(v) => v,
        Err(e) => {
            // 🔴 **500 不许静默** —— 运维要能在事件流里看见它。
            st.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED,
                    "管理树读存储失败",
                )
                .with("op", "upstream.models")
                .with("error", e.to_string()),
            );
            return GatewayError::internal().into_response();
        }
    };
    // 🔴 **判据是 `select_key()` 而不是「池子长度为 0」**：一池全在冷却 / 全被停用时
    //    长度不为 0 而一把都用不了，那两种状态在这条端点上是同一件事。
    let Some((idx, _)) = select_key(&records, 0, at) else {
        // 空池不是错误，是一句要说清的话。**200，不是 500、也不是转圈。**
        // ⚠️ `latencyMs: 0` 而不是省略：面板那一格画的是耗时，缺了它会画成 NaN。
        return failed(None, 0, reason::NO_KEY);
    };
    let picked = &records[idx];

    // ⚠️ **护栏排在选 key 之后**：一次注定失败的调用不该消费一次最小间隔
    //    （那等于「点了一下、什么都没打，但接下来几秒不许再点」）。
    //
    // 🔴 **走 `acquire()` 拿一张租约，不手写 `release`。**
    // 手写配对只覆盖得到「上游回了」「上游报错」两条支，**漏掉第三条：请求被取消**
    // —— 客户端一断开，axum 直接 drop 这个 future，那一行 `release` 根本不执行，
    // 而本端点的 kind 是**常量** ⇒ 卡住的是整颗按钮，且这道闸不像最小间隔那道会自愈。
    // 租约的 drop 在「正常返回 / 提前 return / panic 展开 / 被取消」四种收场里都走到。
    let lease = match guard.acquire(UPSTREAM_MODELS_KIND, now()) {
        Ok(lease) => lease,
        Err(refused) => {
            let reason = refused.reason().expect("acquire 的 Err 支只可能是两种拒绝");
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(WithReason {
                    error: GatewayInner {
                        r#type: "rate_limit_error",
                        message: busy_message(refused).to_string(),
                    },
                    reason,
                }),
            )
                .into_response();
        }
    };

    let cfg = st.config.current();
    // 计时起点：**只读这一次**。
    let started_at = now();
    let sent = st
        .fetcher
        .send(OutboundRequest {
            method: "GET".to_string(),
            url: format!("{}{UPSTREAM_MODELS_UPSTREAM_PATH}", cfg.agnes_base_url),
            // ⚠️ 只带 `Authorization`：GET 没有请求体，`content-type` 是一句空话。
            headers: vec![(
                header::AUTHORIZATION.as_str().to_string(),
                format!("Bearer {}", picked.key),
            )],
            body: None,
            // 首字节档：这条端点只要一份小清单，给它同步档（默认 2 分钟）
            // 等于让面板挂两分钟。
            timeout_ms: cfg.upstream_timeout_ms,
        })
        .await;

    let res = match sent {
        Ok(res) => res,
        Err(e) => {
            // ⚠️ 这里**没有** `release`：租约随这次 `return` 一起 drop。
            //    「上游连不上」恰恰是运维最想连点重试的那一刻，漏放一次就再也点不动。
            return failed(
                None,
                now() - started_at,
                match e {
                    FetchError::TimeoutBeforeHeaders => reason::TIMEOUT,
                    FetchError::Connect(_) => reason::NETWORK_ERROR,
                    // 结构上到不了这里（`send` 回的是「头已到手」），但**认真分档**：
                    // 把它算成 `network_error` 正是这条端点存在的那个 18 格判据禁止的混档。
                    FetchError::BodyInterrupted(_) => reason::BODY_INCOMPLETE,
                },
            );
        }
    };

    let status = res.status;
    if !(200..300).contains(&status) {
        // ⚠️ **非 2xx 时正文直接丢掉**：上游的错误正文可能带着账号信息，
        //    而这条端点的产出只有「哪些模型」。
        drop(res);
        return failed(Some(status), now() - started_at, reason::UPSTREAM_ERROR);
    }

    let body = read_all(res).await;
    // 上游那一段到此为止 ⇒ **显式还闸**，别把它拖到序列化之后
    // （「最小间隔」量的是两次**发起**之间的距离，多占那一会儿是白占）。
    drop(lease);
    let latency_ms = now() - started_at;

    let Some(bytes) = body else {
        // 「正文没来」**自成一档**：头已经到手了，所以 `status` 是有值的 ——
        // 这一格与 `network_error`（status 为 null）的差别就是那两句话的差别。
        return failed(Some(status), latency_ms, reason::BODY_INCOMPLETE);
    };
    let Some(parsed) = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(parse_upstream_models)
    else {
        return failed(Some(status), latency_ms, reason::BAD_PAYLOAD);
    };

    let (only_upstream, only_catalog) = diff_against_catalog(&parsed.ids);
    Json(Wire {
        ok: true,
        status: Some(status),
        latency_ms,
        reason: None,
        models: Some(ModelsBlock {
            ids: parsed.ids,
            truncated: parsed.truncated,
            only_upstream,
            only_catalog,
        }),
    })
    .into_response()
}

/// 429 那两种拒绝的文案。理由同 [`crate::admin::models`] 那一处：
/// `message` 不是对外契约（`reason` 才是），措辞可以各写各的。
const fn busy_message(a: ProbeAcquire) -> &'static str {
    match a {
        ProbeAcquire::Ok => "",
        ProbeAcquire::InFlight => {
            "上一次拉取还没有返回，等它结束再试（这颗按钮每按一次都会真的打一次外网）"
        }
        ProbeAcquire::Cooldown => "两次拉取之间至少要隔一小段时间，请稍后再试",
    }
}

/// 把响应体读全。中途断了 / 超上界 ⇒ `None`（**这两种都是「没读全」**）。
async fn read_all(res: OutboundResponse) -> Option<Vec<u8>> {
    let mut body = res.body;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.ok()?;
        if buf.len() + chunk.len() > MAX_LIST_BODY_BYTES {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(buf)
}

// ── 判据 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::admin::keys::PROBE_MIN_INTERVAL_MS;
    use crate::server::app::build_router;
    use crate::server::test_support::{
        ScriptedFetcher, TestApp, UpstreamReply, state_with, state_with_storage,
    };
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";
    const KEY: &str = "sk-fixture-upstreammodels0123456789abcdef";

    /// 一套「池里有一把 key + 脚本化上游」的夹具。
    async fn app_with(script: Vec<UpstreamReply>) -> (AppState, Arc<ScriptedFetcher>) {
        let fetcher = ScriptedFetcher::new(script);
        let TestApp { state, .. } =
            state_with(Arc::new(MemoryStorage::new()), fetcher.clone()).await;
        state
            .repo
            .add(KEY, crate::adapters::now_ms())
            .await
            .expect("判据用的 key 必须放得进去");
        (state, fetcher)
    }

    /// 冻结的时钟：`latencyMs` 因此是确定的 0，而护栏的行为由**推进量**驱动。
    fn clock_at(ms: i64) -> impl Fn() -> i64 + Sync {
        move || ms
    }

    async fn body_of(res: Response) -> Value {
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&b)
            .unwrap_or_else(|_| panic!("响应必须是合法 JSON：{}", String::from_utf8_lossy(&b)))
    }

    /// 走**本体**（护栏与时钟可注入），不碰全进程那把单例。
    async fn call(state: &AppState, guard: &ProbeGuard) -> Value {
        body_of(upstream_models_with(state, guard, &clock_at(1_000)).await).await
    }

    // ── parse_upstream_models（01 §8.9）────────────────────────────────────

    /// 只认 `{ data: [{ id }] }` 这个形状，认不出来一律 `None`。
    #[test]
    fn a_shape_we_do_not_recognise_is_none_not_an_empty_list() {
        for payload in [
            json!({}),
            json!({ "data": null }),
            json!({ "data": {} }),
            json!({ "models": [{ "id": "a" }] }),
            json!([{ "id": "a" }]),
            json!("nope"),
        ] {
            assert!(
                parse_upstream_models(&payload).is_none(),
                "{payload} 该是 None（空数组在屏幕上是一句关于上游的假话）"
            );
        }
    }

    /// 🔴 **`data: []` 是一句真的上游事实**，它不许被并进「看不懂」那一档。
    #[test]
    fn an_empty_data_array_is_a_fact_about_the_upstream() {
        assert_eq!(
            parse_upstream_models(&json!({ "data": [] })),
            Some(UpstreamModels {
                ids: vec![],
                truncated: false
            })
        );
    }

    /// 坏记录**逐条跳过**，好的那几条留下。
    #[test]
    fn broken_records_are_skipped_one_by_one() {
        let v = json!({ "data": [
            { "id": "a" },
            "not-an-object",
            { "no-id": 1 },
            { "id": 42 },
            { "id": "" },
            { "id": "b" },
            { "id": "a" },
        ]});
        assert_eq!(
            parse_upstream_models(&v).unwrap().ids,
            vec!["a".to_string(), "b".to_string()],
            "坏的跳过、重复的去掉，剩下的两条是真的"
        );
    }

    /// 🔴🔴 **「逐条跳过」的尽头：一条都没抽出来 ⇒ `None`。**
    ///
    /// 回归对象：只写「逐条跳过」而不堵这个尽头的话，一份**完全不认识**的
    /// `data`（比如上游换成了 `{ data: [{ name: … }] }`）会被渲染成
    /// 「上游这次一个模型都没回」——一句我们没资格说的关于上游的话。
    #[test]
    fn skipping_everything_is_not_the_same_as_an_empty_upstream() {
        let v = json!({ "data": [{ "name": "a" }, { "name": "b" }] });
        assert!(
            parse_upstream_models(&v).is_none(),
            "一条都没抽出来 ⇒ 这份响应我们看不懂，不是「上游没有模型」"
        );
        // ⚠️ 判据是「一条都没抽出来」而不是「跳过了几条」：还剩一条就是真的。
        let v = json!({ "data": [{ "name": "a" }, { "id": "real" }] });
        assert_eq!(parse_upstream_models(&v).unwrap().ids, vec!["real"]);
    }

    /// 超过上界 ⇒ 截断 + **如实交代**，不静默丢。
    #[test]
    fn an_oversized_list_is_truncated_and_says_so() {
        let data: Vec<Value> = (0..UPSTREAM_MODELS_MAX + 7)
            .map(|i| json!({ "id": format!("m-{i}") }))
            .collect();
        let got = parse_upstream_models(&json!({ "data": data })).unwrap();
        assert_eq!(got.ids.len(), UPSTREAM_MODELS_MAX);
        assert!(got.truncated, "截断了就必须说");

        // 不到上界时不许说自己截断了。
        let data: Vec<Value> = (0..3).map(|i| json!({ "id": format!("m-{i}") })).collect();
        assert!(
            !parse_upstream_models(&json!({ "data": data }))
                .unwrap()
                .truncated
        );
    }

    // ── diff_against_catalog ───────────────────────────────────────────────

    /// 🔴 **两个方向都现算**，而且都不是空的那一半。
    #[test]
    fn the_diff_is_computed_in_both_directions() {
        let ids = vec![
            MODEL_CATALOG[0].id.to_string(),
            "brand-new-upstream-model".to_string(),
        ];
        let (only_upstream, only_catalog) = diff_against_catalog(&ids);
        assert_eq!(only_upstream, vec!["brand-new-upstream-model"]);
        assert!(
            !only_catalog.contains(&MODEL_CATALOG[0].id.to_string()),
            "上游回了的那条不许出现在 onlyCatalog 里"
        );
        assert_eq!(
            only_catalog.len(),
            MODEL_CATALOG.len() - 1,
            "目录里其余每一条都该在 onlyCatalog 里"
        );
    }

    /// 上游一条都没回时，**整份目录**都是「这次上游没回」。
    ///
    /// ⚠️ 这一格同时钉住「`only_catalog` 不许被读成目录写错了」那条措辞的前提：
    /// 一把权限较窄的 key 拉回来的清单本来就会短。
    #[test]
    fn an_empty_upstream_list_puts_the_whole_catalog_in_only_catalog() {
        let (only_upstream, only_catalog) = diff_against_catalog(&[]);
        assert!(only_upstream.is_empty());
        assert_eq!(only_catalog.len(), MODEL_CATALOG.len());
    }

    // ── 端点：六档结果（02 §9.4 那张表逐行）────────────────────────────────

    /// 🔴 **池里没可用 key ⇒ 200 + `no_key`，而且一次上游都不打。**
    ///
    /// ⚠️ 不是 500、不是转圈：这是一句要说清的话。
    #[tokio::test]
    async fn an_empty_pool_answers_no_key_without_touching_the_upstream() {
        let fetcher = ScriptedFetcher::new(vec![]);
        let TestApp { state, .. } =
            state_with(Arc::new(MemoryStorage::new()), fetcher.clone()).await;
        let guard = ProbeGuard::new();
        let v = call(&state, &guard).await;
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["reason"], "no_key");
        assert_eq!(v["status"], json!(null));
        assert_eq!(v["latencyMs"], json!(0));
        assert_eq!(
            v["models"],
            json!(null),
            "失败时 models 恒 null，不是空清单"
        );
        assert!(fetcher.seen().is_empty(), "空池不许打上游");

        // ⚠️ **护栏一格都不许被消费**：一次注定失败的调用不该让接下来几秒点不动。
        assert_eq!(
            guard.try_acquire(UPSTREAM_MODELS_KIND, 1_000),
            ProbeAcquire::Ok
        );
    }

    /// 成功那一档：`ok:true` + 双向差集 + 打的是 `{base}/models`、用的是池里那把 key。
    #[tokio::test]
    async fn a_successful_pull_reports_ids_and_both_diffs() {
        let (state, fetcher) = app_with(vec![UpstreamReply::json(json!({
            "data": [{ "id": MODEL_CATALOG[0].id }, { "id": "upstream-only-1" }]
        }))])
        .await;
        let guard = ProbeGuard::new();
        let v = call(&state, &guard).await;

        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["status"], json!(200));
        assert_eq!(v["reason"], json!(null));
        assert_eq!(
            v["models"]["ids"],
            json!([MODEL_CATALOG[0].id, "upstream-only-1"])
        );
        assert_eq!(v["models"]["truncated"], json!(false));
        assert_eq!(v["models"]["onlyUpstream"], json!(["upstream-only-1"]));
        assert!(
            v["models"]["onlyCatalog"]
                .as_array()
                .unwrap()
                .contains(&json!(MODEL_CATALOG[1].id)),
            "目录里没被上游回到的那些要在 onlyCatalog 里：{v}"
        );

        // 打的是哪一条、用的是哪把 key。
        let sent = fetcher.only();
        assert_eq!(sent.method, "GET");
        assert_eq!(
            sent.url,
            format!("{}/models", state.config.current().agnes_base_url)
        );
        assert!(
            sent.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("authorization")
                    && v == &format!("Bearer {KEY}")),
            "必须拿池里那把 key 去打：{:?}",
            sent.headers
        );
        assert!(sent.body.is_none(), "GET 不带请求体");
    }

    /// 上游非 2xx ⇒ **200** + `upstream_error` + 真实状态码。
    ///
    /// ⚠️ 回 200 不是在掩盖失败：这条端点的 4xx/5xx 留给**本网关自己**的失败，
    /// 「上游没正常回」是这份报告的一行内容。
    #[tokio::test]
    async fn a_non_2xx_upstream_keeps_its_status_code() {
        let (state, _) = app_with(vec![UpstreamReply {
            status: 401,
            headers: vec![],
            chunks: vec![axum::body::Bytes::from_static(
                b"{\"data\":[{\"id\":\"x\"}]}",
            )],
        }])
        .await;
        let v = call(&state, &ProbeGuard::new()).await;
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["status"], json!(401));
        assert_eq!(v["reason"], "upstream_error");
        assert_eq!(v["models"], json!(null), "非 2xx 的正文一律不解析");
    }

    /// 🔴🔴 **传输失败分三档，一档都不许混。**
    ///
    /// 回归对象：三档并成一句 `network_error` 之后，运维拿到的建议永远是「查网络」——
    /// 而「响应头没来」要调的是超时，「正文没来」要看的是上游中途掐流。
    /// ⚠️ **`status` 那一格是三档的分水岭**：头到手了才有状态码。
    #[tokio::test]
    async fn the_three_transport_failures_stay_three() {
        // ① 根本连不上。
        let (state, _) = app_with(vec![]).await; // 脚本空 ⇒ ScriptedFetcher 回 Connect
        let v = call(&state, &ProbeGuard::new()).await;
        assert_eq!(v["reason"], "network_error");
        assert_eq!(v["status"], json!(null), "连都没连上，哪来的状态码");

        // ② 响应头没来（超时）。
        struct TimingOut;
        #[async_trait::async_trait]
        impl crate::ports::fetcher::Fetcher for TimingOut {
            async fn send(&self, _r: OutboundRequest) -> Result<OutboundResponse, FetchError> {
                Err(FetchError::TimeoutBeforeHeaders)
            }
        }
        let TestApp { state, .. } =
            state_with(Arc::new(MemoryStorage::new()), Arc::new(TimingOut)).await;
        state
            .repo
            .add(KEY, crate::adapters::now_ms())
            .await
            .unwrap();
        let v = call(&state, &ProbeGuard::new()).await;
        assert_eq!(v["reason"], "timeout");
        assert_eq!(v["status"], json!(null));

        // ③ 头到手了，正文中途断了。
        struct Interrupted;
        #[async_trait::async_trait]
        impl crate::ports::fetcher::Fetcher for Interrupted {
            async fn send(&self, _r: OutboundRequest) -> Result<OutboundResponse, FetchError> {
                Ok(OutboundResponse {
                    status: 200,
                    headers: vec![],
                    body: Box::pin(futures_util::stream::iter(vec![
                        Ok(axum::body::Bytes::from_static(b"{\"data\":[")),
                        Err(FetchError::BodyInterrupted("上游中途掐了".into())),
                    ])),
                })
            }
        }
        let TestApp { state, .. } =
            state_with(Arc::new(MemoryStorage::new()), Arc::new(Interrupted)).await;
        state
            .repo
            .add(KEY, crate::adapters::now_ms())
            .await
            .unwrap();
        let v = call(&state, &ProbeGuard::new()).await;
        assert_eq!(
            v["reason"], "body_incomplete",
            "「正文没来」自成一档，不许并进 network_error"
        );
        assert_eq!(v["status"], json!(200), "头已经到手了 ⇒ 状态码必须在");
    }

    /// 读得全但不是 JSON / 形状不认 ⇒ `bad_payload`（**不是**空清单）。
    #[tokio::test]
    async fn an_unparsable_body_is_bad_payload_not_an_empty_list() {
        for reply in [
            UpstreamReply::raw(
                "application/json",
                vec![axum::body::Bytes::from_static(b"<html>")],
            ),
            UpstreamReply::json(json!({ "models": ["a"] })),
            UpstreamReply::json(json!({ "data": [{ "name": "a" }] })),
        ] {
            let (state, _) = app_with(vec![reply]).await;
            let v = call(&state, &ProbeGuard::new()).await;
            assert_eq!(v["reason"], "bad_payload", "实际：{v}");
            assert_eq!(v["status"], json!(200));
            assert_eq!(v["models"], json!(null));
        }
    }

    /// 🔴 **连点被挡：第二次 429 + `probe_cooldown`，而上游只被打了一次。**
    #[tokio::test]
    async fn a_second_click_is_refused_by_the_guard() {
        let (state, fetcher) = app_with(vec![UpstreamReply::json(json!({ "data": [] }))]).await;
        let guard = ProbeGuard::new();
        let first = upstream_models_with(&state, &guard, &clock_at(1_000)).await;
        assert_eq!(first.status(), StatusCode::OK);

        let second = upstream_models_with(&state, &guard, &clock_at(1_001)).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let v = body_of(second).await;
        assert_eq!(v["reason"], "probe_cooldown");
        assert_eq!(v["error"]["type"], "rate_limit_error");
        assert_eq!(fetcher.seen().len(), 1, "被挡下的那次不许真打上游");

        // 🔴 **过了最小间隔要能恢复**：`release` 漏在任何一条支上，这里都会
        //    恒 `probe_in_flight` 而不是放行 —— 那颗按钮从此再也点不动。
        let (state2, _) = app_with(vec![UpstreamReply::json(json!({ "data": [] }))]).await;
        let third =
            upstream_models_with(&state2, &guard, &clock_at(1_000 + PROBE_MIN_INTERVAL_MS)).await;
        assert_eq!(
            third.status(),
            StatusCode::OK,
            "护栏必须自愈，实际：{:?}",
            body_of(third).await
        );
    }

    /// 🔴🔴 **客户端断开（future 被取消）也不许把那颗按钮永久卡死。**
    ///
    /// 回归对象：`try_acquire` → `send().await` → `release` 这个形状只覆盖了
    /// 两条支（上游回了 / 上游报错），**漏掉第三条：请求被取消**。
    /// axum 在客户端断开时直接 drop handler 的 future ⇒ `release` 那一行根本不执行 ⇒
    /// `in_flight` 永远停在 `true`，而 `try_acquire` 第一句就是「在飞就拒」。
    /// ⚠️ **这条端点的 kind 是常量** ⇒ 卡住的不是某一把 key，是**整颗按钮**；
    /// 而最小间隔那道闸会自愈，这道不会 —— 只能重启进程。
    ///
    /// ⇒ 放开不许依赖执行流走到某一行，只许挂在 [`crate::admin::keys::ProbeLease`]
    /// 的 drop 上（drop 在「正常返回 / 提前 return / panic 展开 / 被取消」四种收场里都执行）。
    #[tokio::test]
    async fn a_cancelled_request_does_not_wedge_the_button_forever() {
        /// 永远不返回的上游：handler 会停在 `send().await` 上，正好是被取消的那一刻。
        struct Hangs;
        #[async_trait::async_trait]
        impl crate::ports::fetcher::Fetcher for Hangs {
            async fn send(&self, _r: OutboundRequest) -> Result<OutboundResponse, FetchError> {
                std::future::pending().await
            }
        }

        // ⚠️ **不碰全进程那把单例**：这一格要观察的是「卡没卡住」，
        //    用单例的话一旦真卡住就会污染同进程里其余判据。
        let guard: &'static ProbeGuard = Box::leak(Box::new(ProbeGuard::new()));
        let TestApp { state, .. } =
            state_with(Arc::new(MemoryStorage::new()), Arc::new(Hangs)).await;
        state
            .repo
            .add(KEY, crate::adapters::now_ms())
            .await
            .unwrap();

        let task =
            tokio::spawn(
                async move { upstream_models_with(&state, guard, &clock_at(1_000)).await },
            );
        // 等它真的占住那一格（占住之前 abort 就测不到东西了）。
        let mut occupied = false;
        for _ in 0..20_000 {
            if guard.is_in_flight(UPSTREAM_MODELS_KIND) {
                occupied = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(occupied, "夹具前提：那一格真的被占住了");

        task.abort();
        assert!(
            task.await.unwrap_err().is_cancelled(),
            "夹具前提：真被取消了"
        );
        assert!(
            !guard.is_in_flight(UPSTREAM_MODELS_KIND),
            "客户端一断开就永久卡住 ⇒ 那颗按钮从此再也点不动，只能重启进程"
        );
    }

    /// 🔴 **失败那一支也必须放开**：上游连不上恰恰是最想连点重试的那一刻。
    #[tokio::test]
    async fn a_failed_probe_does_not_wedge_the_button_forever() {
        let (state, _) = app_with(vec![]).await; // 连不上
        let guard = ProbeGuard::new();
        let v = call(&state, &guard).await;
        assert_eq!(v["reason"], "network_error");
        // 隔了最小间隔之后必须能再来一次（卡在「在飞」的话这里是 probe_in_flight）。
        assert_eq!(
            guard.try_acquire(UPSTREAM_MODELS_KIND, 1_000 + PROBE_MIN_INTERVAL_MS),
            ProbeAcquire::Ok
        );
    }

    /// 🔴 **零存储写。** 读侧消费 `repo.all()` 那份进程内快照。
    #[tokio::test]
    async fn it_never_writes_to_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let fetcher = ScriptedFetcher::new(vec![
            UpstreamReply::json(json!({ "data": [{ "id": "a" }] })),
            UpstreamReply::json(json!({ "data": [{ "id": "a" }] })),
        ]);
        let TestApp { state, .. } = state_with(counting, fetcher).await;
        state
            .repo
            .add(KEY, crate::adapters::now_ms())
            .await
            .unwrap();

        let before = counts.snapshot();
        let guard = ProbeGuard::new();
        for i in 0..2 {
            let v = body_of(
                upstream_models_with(
                    &state,
                    &guard,
                    &clock_at(1_000 + i * (PROBE_MIN_INTERVAL_MS + 1)),
                )
                .await,
            )
            .await;
            assert_eq!(v["ok"], json!(true), "{v}");
        }
        let after = counts.snapshot();
        assert_eq!(after.1, before.1, "拉上游模型清单不许产生任何存储写");
    }

    /// 🔴 **走真路由：它在闸后面，而且注册在 `/admin/api/upstream/models` 上。**
    ///
    /// ⚠️ 这一格打的是**全进程那把护栏单例**（真路由取的就是它）⇒
    /// 只打一次，且断言的是「不是 404 / 不是 401」这类与护栏无关的东西。
    #[tokio::test]
    async fn it_is_registered_behind_the_gate() {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/api/upstream/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "不带凭据要 401");

        // 带凭据 + 空池 ⇒ 200 `no_key`（一次上游都不打，护栏一格都不消费）。
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/admin/api/upstream/models")
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_of(res).await["reason"], "no_key");
    }
}
