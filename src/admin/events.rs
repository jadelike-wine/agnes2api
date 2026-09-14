//! `GET /admin/api/events` 与 `GET /admin/api/events/download`（02 §9.31、§9.32）。
//!
//! # ⚠️ 面板**只能轮询，不能 SSE**，这条是结构性的
//!
//! `/admin/api/*` 整棵树 🔴 **只认 `X-Admin-Key`**（不收 `Authorization`、不收
//! 查询参数口令），而浏览器的 `EventSource` **设不了任何请求头** ——
//! 于是一条 SSE 的事件流端点在面板里**根本没法带着口令连上**。
//! 剩下的两条路都被否掉过：
//! · 把口令挪进查询串 ⇒ 它会原样进 access log、进浏览器历史、进 Referer；
//! · 给事件流单开一条免鉴权路径 ⇒ 事件里带着未鉴权请求的路径与上游错误体。
//! ⇒ **面板按固定间隔轮询这条端点，用 `after` 游标取增量。**
//! 这不是「还没做 SSE」，是「不做」——别把它当成一条待办捡起来。
//!
//! 连带后果，一并写在这里免得下一个人各自推一遍：
//! · 稳态读吞吐由轮询间隔定死 ⇒ [`cursor`](self) 那一格**永远不许是「字段不存在」**
//!   （游标一冻结，吞吐就从包线 70,560 次/天跳到 276,480 = 3.9 倍，且默认档位下不自愈）；
//! · `shardId` / `buffered` / `dropped` / `budgetExhausted` 是**本实例**的自述状态，
//!   多副本下相邻两次轮询可能落到不同副本 ⇒ 不带 `shardId` 面板就说不清
//!   「这句话说的是哪一个实例」。
//!
//! # ⚠️ `items` 里的字段一律视为**完全不可信**
//!
//! 里面会出现上游返回的内容与**未鉴权请求的路径**（`admin.login_failed` 的
//! `fields.path` 就是攻击者能写的值）。这里**不做**转义、不做截断 ——
//! JSON 响应体是结构化的，字段边界由 JSON 语法本身保证。
//! 🔴 **前端一律 `textContent` 渲染，别以为这里已经清洗过了。**

use crate::adapters::logger_store::ring;
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::errors::GatewayError;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;

// ── 1. 参数（🔴 五语言 API.md 的参数表逐份写着这两个数）────────────────────

/// 一页默认多少条。
///
/// 🔴 **它必须是可导出的真源，不许在文档侧手抄第二份。** TS 版踩过：
/// 源码是 200 / 500 而五份 API.md 逐份写成 50 / 200（照抄了 `keys` 那一族的量级感），
/// **五种语言逐份同错，而当时全仓没有一格判据拿源码核过文档里的值**。
pub const DEFAULT_LIMIT: usize = 200;

/// 一页最多多少条。理由同 [`DEFAULT_LIMIT`]。
pub const MAX_LIMIT: usize = 500;

/// `level` 认得的那几个字面量。
///
/// ⚠️⚠️ **这里有 4 个，而 [`LogLevel`] 只有 3 个（Rust 侧没有 `debug` 档）——
/// 这是刻意的，别「顺手对齐」。** 本参数是**过滤器**，它比的是**存储里那个字符串**，
/// 而存储里的东西一律不可信（运维手改、旧版本写下的、别的实现写下的）。
/// · 收 `"debug"`：过滤结果是空 ⇒ 面板如实显示「这一档没有事件」；
/// · 不收 `"debug"`：它落进「不是合法级别 ⇒ **整个忽略**」那一支 ⇒
///   **用户点了「debug」，拿回来的是全部事件** —— 那比空列表糟得多。
const LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];

/// 从查询串里取一个参数的**首个**出现。
///
/// ⚠️ **不做 URL 解码**：这三个参数的合法取值域（十进制整数、四个 ASCII 级别名）
/// 里没有任何需要转义的字符 ⇒ 解码只会多一条「解码失败怎么办」的分支。
/// 带百分号的值一律落进各自的「非法 ⇒ 回落 / 忽略」那一支，那正是想要的。
fn query_param<'a>(raw: Option<&'a str>, name: &str) -> Option<&'a str> {
    raw?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// 整数参数。**越界一律回落到 `fallback`，不是夹到边界上。**
///
/// ⚠️ `limit=501` 拿到的是 **200（默认值）**，不是 500 —— 与 TS 版逐字同语义。
/// 夹到边界更「友好」，但那会让「发了一个荒唐的 limit」这件事永远不被发现，
/// 而这三个参数都是面板自己发的。
fn int_param(raw: Option<&str>, fallback: usize, min: usize, max: usize) -> usize {
    raw.and_then(|s| s.parse::<usize>().ok())
        .filter(|n| (min..=max).contains(n))
        .unwrap_or(fallback)
}

/// `level`。**不是合法级别时整个忽略**（= 不过滤），见 [`LEVELS`]。
fn level_param(raw: Option<&str>) -> Option<&str> {
    raw.filter(|s| LEVELS.contains(s))
}

/// `after` 游标。**负数一律当缺失处理。**
///
/// `after` 不是可信输入 —— 任何人都能拿 curl 打任意值。时间戳没有负数这回事，
/// 放行它除了让语义含糊之外没有任何好处。
///
/// ⚠️ **这里拒负数不是「候选键数有上界」那条安全性质的必要条件**：
/// 那条性质来自 [`ring::candidate_keys`] 里的钳位本身（`from_window >= floor`
/// ⇒ 候选窗口数恒 `<= EVENT_WINDOW_RETAIN`，**与 `after` 是什么值无关**）。
/// 别把这一行读成护栏。
fn after_param(raw: Option<&str>) -> Option<i64> {
    raw.and_then(|s| s.parse::<i64>().ok()).filter(|n| *n >= 0)
}

// ── 2. 响应体（🔴 一律结构体，不用 `json!` 宏 —— 键序是对外契约）───────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsBody {
    items: Vec<Value>,
    /// 🔴 **只有两种合法值：有限数字，或 `null`。永远不许是「字段不存在」。**
    ///
    /// 实测后果：游标就此冻结，稳态读吞吐 276,480 次/天 = 包线的 3.9 倍，
    /// 且默认「全部级别」档位下**不会自愈**。
    ///
    /// ⚠️ Rust 的 `Option<i64>` 在 `serde_json` 下序列化成 `null`（不是省略键），
    /// 所以这条性质在类型上就成立了 —— **但仍然有一格判据钉着它**：
    /// 谁给这个字段加一句 `skip_serializing_if = "Option::is_none"`（一个看起来
    /// 很常规的「省点字节」改动）就会当场把那个缺陷搬回来。
    cursor: Option<i64>,
    /// **本实例**的分片标识。多副本下没有它，面板说不清这几个数是谁的。
    shard_id: String,
    buffered: usize,
    dropped: u64,
    budget_exhausted: bool,
    /// 过滤/截断确实丢掉了本该出现的旧事件。
    truncated: bool,
    /// 这次读里被逐条丢掉的畸形条目数。
    ///
    /// ⚠️ **它恒为 0 —— 「恒为 0」正是它的价值**：`src/` 里没有任何路径能产出畸形
    /// 条目，所以它是那几格窄化判据在生产环境里的**对照组**。运维手改存储、
    /// `store.json` 被写坏时，它是**唯一**能把「面板少了几条事件」与
    /// 「本来就没有事件」区分开的信号。
    /// 🔴 **刻意不进面板的黄条**：挂一个永远为假的分支上去没有意义，走轮询指示灯
    /// 的 tooltip。**代价明写**：它平时不显眼，出事时要运维去看 tooltip 才发现。
    malformed: usize,
    /// 游标所在窗口晚于本次请求的时钟 ⇒ **空结果不代表没有事件，是时钟纠纷**。
    ///
    /// ⚠️ 判据是 `window_index(after) > window_index(now)`，**看的是有没有跨过
    /// 窗口边界，与偏移量的绝对大小无关**。两组实测：偏移仅 **2 毫秒**但恰好跨在
    /// 边界上 ⇒ `true`；偏移足足 **59 分钟**但仍在同一窗口内 ⇒ `false`。
    /// ⚠️ 后者留了一段**真实存在、没有任何代码覆盖**的盲区（`after` 比 `now` 快
    /// 但同窗口时，过滤结果照样是空、而 `cursorAhead` 是 `false`），
    /// 它自限在至多一个窗口宽度内会自愈。**别把这句读成「这个角落已经覆盖」。**
    cursor_ahead: bool,
    generated_at: i64,
    /// 🔴 **已经判好的视图**（06 §5.3 的 A 档）。见
    /// [`crate::core::admin::view::events`]。
    view: EventsViewWire,
}

/// 事件列表那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsViewWire {
    /// 顶部黄条要不要出现。**判据只看后端字段**，渲染层不许自己硬编码一个真假。
    warn: bool,
    /// **与 `items` 同序同长**，逐条给出这一条真实的级别与它的徽章。
    ///
    /// 🔴 **它顶掉的是「`level` 读不出来就当 `info`」那句谎**：`items` 里那个 `level`
    /// 一路来自存储、一个字节都不可信，而绿色的 info 徽章是这一族里最不该给的一个。
    rows: Vec<EventRowViewWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventRowViewWire {
    /// 四个已知级别之一，或者显式的 `"unknown"`。**绝不冒充任何一个已知级别。**
    level: &'static str,
    label_key: &'static str,
    badge_class: &'static str,
}

/// 拼事件列表那份视图。**判定一律转调 [`crate::core::admin::view::events`]。**
fn events_view(
    items: &[Value],
    dropped: u64,
    budget_exhausted: bool,
    truncated: bool,
    cursor_ahead: bool,
) -> EventsViewWire {
    use crate::core::admin::view::events as v;

    EventsViewWire {
        warn: v::should_warn(dropped, budget_exhausted, truncated, cursor_ahead),
        rows: items
            .iter()
            .map(|e| {
                let level = v::effective_level(e.get("level").and_then(Value::as_str));
                EventRowViewWire {
                    level,
                    label_key: v::event_level_label_key(level),
                    badge_class: v::level_badge_class(level),
                }
            })
            .collect(),
    }
}

// ── 3. handler ─────────────────────────────────────────────────────────────

/// 存储读不出来 ⇒ 500 + 一条事件（🔴 **500 不许静默**）。
///
/// ⚠️ **「读不出来」与「本来就没有事件」在面板上必须分得开** ——
/// 吞掉错误回一个空 `items` 就是把前者渲染成后者。
fn read_failed(st: &AppState, op: &'static str, err: &dyn std::fmt::Display) -> Response {
    st.logger.log(
        LogEntry::new(LogLevel::Error, "admin.storage_failed", "管理树读存储失败")
            .with("op", op)
            .with("error", err.to_string()),
    );
    GatewayError::internal().into_response()
}

/// 事件列表端点在 `/admin/api` 之下的子路径。**路由表从这里取，不许手抄字面量。**
pub const EVENTS_SUBPATH: &str = "/events";

/// 整份下载端点的子路径。
pub const EVENTS_DOWNLOAD_SUBPATH: &str = "/events/download";

/// `GET /admin/api/events`。
///
/// **零 `list()`、零索引读**：候选键由 [`ring::candidate_keys`] 从时钟直接算出来，
/// 且这个次数**对任何 `after` 值都有硬上界**（`EVENT_WINDOW_RETAIN × EVENT_SLOTS`）
/// —— 不随「游标陈旧了多久」「部署跑了多久」增长。
/// ⚠️ 那条钳位是一次真修复：没有它时 `after=0` 单次请求实测打出约 **99 万次 get**。
///
/// 🔴 **顺序是「归并 → 过滤（after / level）→ 截到 limit」，不能倒过来**：
/// 先截断再按 level 过滤会把本该出现的旧事件漏掉。
pub async fn list(State(st): State<AppState>, RawQuery(q): RawQuery) -> Response {
    let raw = q.as_deref();
    let after = after_param(query_param(raw, "after"));
    let level = level_param(query_param(raw, "level"));
    let limit = int_param(query_param(raw, "limit"), DEFAULT_LIMIT, 1, MAX_LIMIT);
    // ⚠️ **这里的 `now` 与 sink 内部那个时钟是两个读点**（sink 的时钟可注入，
    //    生产上两者都是墙钟）。它只服务 `cursorAhead` 与 `generatedAt`，
    //    候选键那一半由 sink 自己的时钟算 —— 别把它当成同一个值去推理。
    let now = crate::adapters::now_ms();

    let read = match st.event_sink.read_events(after).await {
        Ok(v) => v,
        Err(e) => return read_failed(&st, "events.list", &e),
    };

    let filtered: Vec<Value> = read
        .items
        .into_iter()
        .filter(|e| after.is_none_or(|a| ring::entry_ts(e).is_some_and(|ts| ts > a as f64)))
        .filter(|e| level.is_none_or(|l| e.get("level").and_then(Value::as_str) == Some(l)))
        .collect();
    let truncated = filtered.len() > limit;
    let items: Vec<Value> = filtered.into_iter().take(limit).collect();

    // 归并结果按 ts 降序 ⇒ `items[0]` 是本页最新的一条。空结果给 `null`，
    // 前端据此判断「保留上一次的 after」还是「推进到新值」。
    //
    // ⚠️ 经过窄化之后 `items[0].ts` 必然已经是有限数字，**但契约要被钉住而不是
    // 被推理保证**：这一步与那次窄化是两道独立的闸，删掉任何一道另一道都还在。
    let cursor = items
        .first()
        .and_then(ring::entry_ts)
        .filter(|f| f.is_finite())
        .map(|f| f as i64);

    let status = st.event_sink.status();
    let cursor_ahead = after.is_some_and(|a| ring::window_index(a) > ring::window_index(now));
    // ⚠️ 必须在 `items` 被 move 进响应体**之前**算完。
    let view = events_view(
        &items,
        status.dropped,
        status.budget_exhausted,
        truncated,
        cursor_ahead,
    );
    Json(EventsBody {
        items,
        cursor,
        shard_id: status.shard_id,
        buffered: status.buffered,
        dropped: status.dropped,
        budget_exhausted: status.budget_exhausted,
        truncated,
        malformed: read.malformed,
        cursor_ahead,
        generated_at: now,
        view,
    })
    .into_response()
}

/// 下载用的文件名。**具名常量**：它进 `content-disposition`，
/// 而运维的脚本按这个名字找文件。
pub const EVENTS_DOWNLOAD_FILENAME: &str = "agnes2api-events.txt";

/// `GET /admin/api/events/download`。
///
/// 内容是归并结果**逐行 `JSON.stringify`**（🔴 **不是一个 JSON 数组**）：
/// 这是给人在终端里 `grep` / 逐行处理用的格式，不是给程序反序列化的 API。
///
/// ⚠️ **同样 clamp 到 [`MAX_LIMIT`]**：候选键空间虽然结构性有界，
/// 单次归并结果的理论上限仍是「候选键数 × 每键 100 条」。与列表端点同一个上限，
/// 行为可预期。
///
/// ⚠️⚠️ **它刻意返回一个自己拼的 `Response`（不是 `Json` / `String` 那一族）**：
/// 它是「全局 nosniff 必须写在 `next()` 之后」那条性质在生产路由上的**可观测锚点**
/// —— 本端点自己**不**设 `x-content-type-options`，全靠那条全局中间件。
/// **别顺手改成让框架代拼**：那会把这条护栏变回摆设。
/// 由 [`tests::the_download_endpoint_is_hand_rolled_and_still_gets_global_nosniff`] 钉着。
pub async fn download(State(st): State<AppState>) -> Response {
    let read = match st.event_sink.read_events(None).await {
        Ok(v) => v,
        Err(e) => return read_failed(&st, "events.download", &e),
    };
    let text = read
        .items
        .iter()
        .take(MAX_LIMIT)
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                // 🔴 **文件名从 [`EVENTS_DOWNLOAD_FILENAME`] 现拼，不在这里抄第二份**
                //    —— 判据那一侧手写的是字面量，两边因此不是同义反复。
                // ⚠️ 它是 ASCII 常量 ⇒ 不必做 RFC 5987 编码。
                //    哪天它变成可配置的，这里要先过一道编码，别直接拼。
                format!("attachment; filename=\"{EVENTS_DOWNLOAD_FILENAME}\""),
            ),
        ],
        text,
    )
        .into_response()
}

// ── 判据 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::ports::storage::Storage;
    use crate::server::app::build_router;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    struct Harness {
        app: Router,
        state: AppState,
        storage: Arc<dyn Storage>,
    }

    async fn harness() -> Harness {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let TestApp { mut state, .. } = state_with_storage(storage.clone()).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state.clone());
        Harness {
            app,
            state,
            storage,
        }
    }

    async fn call(h: &Harness, uri: &str) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("x-admin-key", ADMIN)
            .body(Body::empty())
            .unwrap();
        h.app.clone().oneshot(req).await.unwrap()
    }

    async fn get_json(h: &Harness, uri: &str) -> (StatusCode, Value) {
        let res = call(h, uri).await;
        let s = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// 直接往存储里写一个分片（绕过 sink，这样 `ts` 与 `level` 完全由判据说了算）。
    async fn seed_shard(h: &Harness, at: i64, entries: Vec<Value>) {
        let key = ring::shard_key(ring::window_index(at), ring::slot_of("testshrd"));
        h.storage
            .put(&key, Value::Array(entries), None)
            .await
            .unwrap();
    }

    fn entry(ts: i64, level: &str, event: &str) -> Value {
        json!({ "ts": ts, "level": level, "event": event, "msg": "m", "fields": {} })
    }

    /// 当前这一刻（sink 的时钟在 `state_with` 里初始化成真墙钟）。
    fn now() -> i64 {
        crate::adapters::now_ms()
    }

    // ── 视图（06 §5.3 的 A 档）─────────────────────────────────────────────

    /// 🔴 **一条 `level` 读不出来的事件不许被画成绿色的 info 徽章。**
    ///
    /// `items` 里那个 `level` 一路来自存储、一个字节都不可信
    /// （02 §9.31 逐字：「`items` 里的字段一律视为完全不可信」）。
    /// TS 侧原来写的是 `typeof item.level === "string" ? item.level : "info"`
    /// —— 那正是把「我们读不懂这一条」伪装成「这一条一切正常」。
    ///
    /// ⚠️ 夹具里四种畸形形态各一条：缺字段 / 空串 / 大小写不对 / 根本不是字符串。
    #[tokio::test]
    async fn a_malformed_level_gets_an_explicit_unknown_badge_not_a_green_one() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                json!({ "ts": t, "event": "a", "msg": "m" }),
                json!({ "ts": t - 1, "level": "", "event": "b", "msg": "m" }),
                json!({ "ts": t - 2, "level": "INFO", "event": "c", "msg": "m" }),
                json!({ "ts": t - 3, "level": 7, "event": "d", "msg": "m" }),
                entry(t - 4, "info", "e"),
            ],
        )
        .await;
        let (s, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(s, StatusCode::OK);
        let items = v["items"].as_array().expect("items");
        let rows = v["view"]["rows"].as_array().expect("rows");
        assert_eq!(items.len(), rows.len(), "与 items 同序同长");
        assert_eq!(rows.len(), 5);

        let good = rows.last().expect("最后一条是那条正常的 info");
        assert_eq!(good["level"], json!("info"));
        assert_eq!(good["badgeClass"], json!("badge badge-ok"));
        for row in &rows[..4] {
            assert_eq!(row["level"], json!("unknown"), "{row}");
            assert_eq!(row["labelKey"], json!("ev.level.unknown"));
            assert_eq!(
                row["badgeClass"],
                json!("badge"),
                "中性灰 —— 不给任何一种「正常 / 异常」的颜色暗示"
            );
            assert_ne!(row["badgeClass"], good["badgeClass"]);
        }
    }

    /// 四个已知级别原样透传，且三种徽章（ok / warn / danger）分得开。
    #[tokio::test]
    async fn the_four_known_levels_keep_their_own_badges() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                entry(t, "error", "a"),
                entry(t - 1, "warn", "b"),
                entry(t - 2, "info", "c"),
                entry(t - 3, "debug", "d"),
            ],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events").await;
        let rows = v["view"]["rows"].as_array().expect("rows");
        let got: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| {
                (
                    r["level"].as_str().unwrap(),
                    r["badgeClass"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("error", "badge badge-danger"),
                ("warn", "badge badge-warn"),
                ("info", "badge badge-ok"),
                ("debug", "badge badge-ok"),
            ]
        );
    }

    /// 常态下**不出黄条** —— 「没丢事件」不是问题。
    ///
    /// 回归对象：把 `warn` 写成恒真（或者挂上 `buffered` / `malformed` 这两个
    /// 平时就非零 / 恒为零的字段），于是那条黄条要么永远在、要么永远不在。
    #[tokio::test]
    async fn a_healthy_read_does_not_raise_the_banner() {
        let h = harness().await;
        seed_shard(&h, now(), vec![entry(now(), "info", "a")]).await;
        let (_, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(v["dropped"], json!(0));
        assert_eq!(v["truncated"], json!(false));
        assert_eq!(v["cursorAhead"], json!(false));
        assert_eq!(v["view"]["warn"], json!(false));
    }

    /// 🔴 **`truncated` 单独一条就足以点亮黄条。**
    ///
    /// 它意味着「过滤 / 截断确实丢掉了本该出现的旧事件」—— 屏幕上少了东西，
    /// 而运维必须知道少了。
    #[tokio::test]
    async fn a_truncated_page_raises_the_banner_on_its_own() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![entry(t, "info", "a"), entry(t - 1, "info", "b")],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events?limit=1").await;
        assert_eq!(v["truncated"], json!(true));
        assert_eq!(v["dropped"], json!(0), "别的三条判据都没成立");
        assert_eq!(v["budgetExhausted"], json!(false));
        assert_eq!(v["cursorAhead"], json!(false));
        assert_eq!(v["view"]["warn"], json!(true));
    }

    /// 🔴 **`cursorAhead` 同样单独一条就足够** —— 空结果**不代表没有事件**，
    /// 是时钟纠纷。面板不说这一句的话，运维会以为「一切安静」。
    #[tokio::test]
    async fn a_cursor_from_the_future_raises_the_banner_on_its_own() {
        let h = harness().await;
        let ahead = now() + 4 * 60 * 60 * 1_000;
        let (_, v) = get_json(&h, &format!("/admin/api/events?after={ahead}")).await;
        assert_eq!(v["cursorAhead"], json!(true));
        assert!(
            v["items"].as_array().expect("items").is_empty(),
            "这一档下 items 恒为空 —— 而那正是它容易被读成「一切安静」的原因"
        );
        assert_eq!(v["view"]["warn"], json!(true));
    }

    // ── 参数解析 ───────────────────────────────────────────────────────────

    /// `limit` 越界**回落到默认值，不夹到边界**。期望值手写。
    ///
    /// 回归对象：把 `int_param` 改成 `clamp` ——「更友好」，但 `limit=99999`
    /// 这种一眼是 bug 的调用从此永远不会被发现。
    #[test]
    fn an_out_of_range_limit_falls_back_to_the_default_rather_than_clamping() {
        assert_eq!(int_param(Some("501"), DEFAULT_LIMIT, 1, MAX_LIMIT), 200);
        assert_eq!(int_param(Some("0"), DEFAULT_LIMIT, 1, MAX_LIMIT), 200);
        assert_eq!(int_param(Some("-3"), DEFAULT_LIMIT, 1, MAX_LIMIT), 200);
        assert_eq!(int_param(Some("abc"), DEFAULT_LIMIT, 1, MAX_LIMIT), 200);
        assert_eq!(int_param(None, DEFAULT_LIMIT, 1, MAX_LIMIT), 200);
        // 合法值原样通过。
        assert_eq!(int_param(Some("1"), DEFAULT_LIMIT, 1, MAX_LIMIT), 1);
        assert_eq!(int_param(Some("500"), DEFAULT_LIMIT, 1, MAX_LIMIT), 500);
    }

    /// `after` 负数当缺失；合法非负数原样收。
    #[test]
    fn a_negative_after_is_treated_as_absent() {
        assert_eq!(after_param(Some("-1")), None);
        assert_eq!(after_param(Some("-100000000000")), None);
        assert_eq!(after_param(Some("nope")), None);
        assert_eq!(after_param(None), None);
        assert_eq!(after_param(Some("0")), Some(0));
        assert_eq!(after_param(Some("1735689600000")), Some(1_735_689_600_000));
    }

    /// 🔴 **`level=debug` 必须被收下（过滤成空），不许落进「整个忽略」那一支。**
    ///
    /// Rust 侧的 [`LogLevel`] 只有三档，「顺手对齐成三个」的后果是：
    /// 用户点了 debug，拿回来的是**全部事件** —— 比空列表糟得多。
    #[test]
    fn debug_is_a_recognised_filter_even_though_nothing_writes_it() {
        assert_eq!(level_param(Some("debug")), Some("debug"));
        assert_eq!(level_param(Some("info")), Some("info"));
        assert_eq!(level_param(Some("warn")), Some("warn"));
        assert_eq!(level_param(Some("error")), Some("error"));
        // 不认识的一律忽略（= 不过滤）。
        assert_eq!(level_param(Some("loud")), None);
        assert_eq!(level_param(Some("INFO")), None);
        assert_eq!(level_param(Some("")), None);
    }

    /// 查询串取值只认**首个**同名参数，且不做 URL 解码。
    #[test]
    fn query_lookup_takes_the_first_occurrence_only() {
        assert_eq!(query_param(Some("a=1&b=2&a=3"), "a"), Some("1"));
        assert_eq!(query_param(Some("a=1&b=2"), "b"), Some("2"));
        assert_eq!(query_param(Some("a=1"), "c"), None);
        assert_eq!(query_param(None, "a"), None);
        // 没有 `=` 的裸键不算数（`?after` 与 `?after=` 是两回事）。
        assert_eq!(query_param(Some("after"), "after"), None);
        assert_eq!(query_param(Some("after="), "after"), Some(""));
    }

    // ── 端点行为 ───────────────────────────────────────────────────────────

    /// 空事件环：`items` 是空数组、**`cursor` 是 `null` 而不是缺席**。
    ///
    /// 🔴 变异对象：给 `cursor` 加一句 `skip_serializing_if` ⇒ 前端读到
    /// 「字段不存在」，与「没有新事件」完全无法区分，游标就此冻结。
    #[tokio::test]
    async fn an_empty_ring_still_carries_an_explicit_null_cursor() {
        let h = harness().await;
        let (s, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["items"], json!([]));
        assert!(
            v.as_object().unwrap().contains_key("cursor"),
            "cursor 这一格必须**在**，哪怕值是 null"
        );
        assert!(v["cursor"].is_null());
        assert_eq!(v["truncated"], json!(false));
        assert_eq!(v["malformed"], json!(0));
        assert_eq!(v["cursorAhead"], json!(false));
    }

    /// 归并结果按 `ts` **降序**，`cursor` 就是最新那一条的 `ts`。
    #[tokio::test]
    async fn items_come_back_newest_first_and_the_cursor_is_the_newest_ts() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                entry(t - 3, "info", "a"),
                entry(t - 1, "warn", "b"),
                entry(t - 2, "error", "c"),
            ],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events").await;
        let tss: Vec<i64> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["ts"].as_i64().unwrap())
            .collect();
        assert_eq!(tss, vec![t - 1, t - 2, t - 3]);
        assert_eq!(v["cursor"], json!(t - 1));
    }

    /// `after` 严格大于：**等于游标的那一条不再回**（否则每次轮询都重发最后一条）。
    #[tokio::test]
    async fn after_is_strictly_greater_than() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![entry(t - 2, "info", "a"), entry(t - 1, "info", "b")],
        )
        .await;
        let (_, v) = get_json(&h, &format!("/admin/api/events?after={}", t - 2)).await;
        let tss: Vec<i64> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["ts"].as_i64().unwrap())
            .collect();
        assert_eq!(tss, vec![t - 1], "`after` 那一条本身不许再回一遍");
    }

    /// `level` 过滤真的在过滤；**不认识的 level 一律忽略（回全部）**。
    #[tokio::test]
    async fn the_level_filter_filters_and_an_unknown_level_is_ignored_entirely() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                entry(t - 3, "info", "a"),
                entry(t - 2, "warn", "b"),
                entry(t - 1, "error", "c"),
            ],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events?level=warn").await;
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["event"], json!("b"));

        let (_, v) = get_json(&h, "/admin/api/events?level=loud").await;
        assert_eq!(v["items"].as_array().unwrap().len(), 3, "不合法 ⇒ 整个忽略");

        // `debug` 是合法档位，只是没人写 ⇒ 空列表，**不是全部**。
        let (_, v) = get_json(&h, "/admin/api/events?level=debug").await;
        assert_eq!(v["items"].as_array().unwrap().len(), 0);
        assert!(v["cursor"].is_null());
    }

    /// 🔴 **顺序必须是「归并 → 过滤 → 截断」。**
    ///
    /// 装置：5 条事件里只有 1 条是 `error`，而它是**最旧**的那一条。
    /// `limit=1` 下正确的答案是那条 error；先截断再过滤的实现会把它漏掉、回空列表。
    #[tokio::test]
    async fn filtering_happens_before_truncation_not_after() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                entry(t - 5, "error", "the-one"),
                entry(t - 4, "info", "a"),
                entry(t - 3, "info", "b"),
                entry(t - 2, "info", "c"),
                entry(t - 1, "info", "d"),
            ],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events?level=error&limit=1").await;
        assert_eq!(
            v["items"].as_array().unwrap().len(),
            1,
            "先截断再过滤会把它整个漏掉"
        );
        assert_eq!(v["items"][0]["event"], json!("the-one"));
        assert_eq!(v["truncated"], json!(false), "过滤完只有 1 条，没有被截");
    }

    /// `truncated` = 过滤/截断确实丢掉了本该出现的旧事件。
    ///
    /// **两个方向都断言**：只写正向的话「`truncated` 恒为 true」的实现也全绿。
    #[tokio::test]
    async fn truncated_says_when_this_page_is_not_the_whole_story() {
        let h = harness().await;
        let t = now();
        let three = vec![
            entry(t - 3, "info", "a"),
            entry(t - 2, "info", "b"),
            entry(t - 1, "info", "c"),
        ];
        seed_shard(&h, t, three).await;
        let (_, v) = get_json(&h, "/admin/api/events?limit=2").await;
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        assert_eq!(v["truncated"], json!(true));

        let (_, v) = get_json(&h, "/admin/api/events?limit=3").await;
        assert_eq!(v["truncated"], json!(false), "恰好装得下就不算截断");
    }

    /// `cursorAhead`：判据是**有没有跨过窗口边界**，与偏移量大小无关。
    ///
    /// 两组实测方向相反 —— 只写一边的话「恒 false」或「恒 true」都能全绿。
    #[tokio::test]
    async fn cursor_ahead_is_about_window_boundaries_not_about_how_far_ahead() {
        let h = harness().await;
        let t = now();
        // ① 偏移一整个窗口 ⇒ 一定跨过边界 ⇒ true。
        let far = t + ring::EVENT_WINDOW_MS;
        let (_, v) = get_json(&h, &format!("/admin/api/events?after={far}")).await;
        assert_eq!(v["cursorAhead"], json!(true));

        // ② 落在**同一个窗口**里的 `after`（取当前窗口起点，它 <= now）⇒ false。
        let same_window = ring::window_index(t) * ring::EVENT_WINDOW_MS;
        let (_, v) = get_json(&h, &format!("/admin/api/events?after={same_window}")).await;
        assert_eq!(v["cursorAhead"], json!(false));

        // ③ 不带 `after` ⇒ 恒 false（没有游标就没有时钟纠纷）。
        let (_, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(v["cursorAhead"], json!(false));
    }

    /// `malformed` 数的是**这一次读**里被逐条丢掉的畸形条目。
    ///
    /// 🔴 它是那几格窄化判据在生产里的对照组 —— 少了它，「面板少了几条事件」
    /// 与「本来就没有事件」在响应体上无法区分。
    ///
    /// ⚠️ **同时钉住反向**：分片键根本不存在（`null`）**不算**畸形 ——
    /// 否则一个全新部署一上来就报「有畸形数据」。
    #[tokio::test]
    async fn malformed_counts_broken_entries_but_not_absent_shards() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![
                entry(t - 1, "info", "ok"),
                json!({ "level": "info" }),      // 没有 ts
                json!({ "ts": "not-a-number" }), // ts 不是数字
                json!("a string"),               // 根本不是对象
            ],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(v["malformed"], json!(3));
        assert_eq!(v["items"].as_array().unwrap().len(), 1);

        // 对照组：什么都没写过的部署，`malformed` 是 0 而不是「候选键数」。
        let h2 = harness().await;
        let (_, v2) = get_json(&h2, "/admin/api/events").await;
        assert_eq!(v2["malformed"], json!(0));
    }

    /// 自述状态四格来自 sink 本体，不是 handler 现编的。
    #[tokio::test]
    async fn the_self_reported_status_comes_from_the_sink() {
        let h = harness().await;
        // 往 sink 里塞一条（只缓冲、不落盘）。
        h.state
            .logger
            .log(LogEntry::new(LogLevel::Warn, "pool.load_failed", "judge"));
        let (_, v) = get_json(&h, "/admin/api/events").await;
        let st = h.state.event_sink.status();
        assert_eq!(v["shardId"], json!(st.shard_id));
        assert_eq!(v["buffered"], json!(st.buffered));
        assert!(st.buffered >= 1, "刚记了一条，缓冲里应当看得见");
        assert_eq!(v["dropped"], json!(st.dropped));
        assert_eq!(v["budgetExhausted"], json!(st.budget_exhausted));
    }

    /// 🔴 **`items` 里的字段原样交出去：不转义、不截断。**
    ///
    /// 那些值里有未鉴权请求的路径（`admin.login_failed` 的 `fields.path`）。
    /// 后端做半套转义会让前端以为「这里清洗过了」，而那正是最危险的形态。
    #[tokio::test]
    async fn untrusted_fields_are_relayed_verbatim() {
        let h = harness().await;
        let t = now();
        let nasty = "</script><img src=x onerror=alert(1)>";
        seed_shard(
            &h,
            t,
            vec![json!({
                "ts": t - 1, "level": "warn", "event": "admin.login_failed",
                "msg": "x", "fields": { "path": nasty },
            })],
        )
        .await;
        let (_, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(v["items"][0]["fields"]["path"], json!(nasty));
    }

    /// 存储读不出来 ⇒ **500 + 一条 `admin.storage_failed`**，不是一个空 `items`。
    #[tokio::test]
    async fn a_storage_failure_is_a_500_with_an_event_not_an_empty_list() {
        let storage = Arc::new(MemoryStorage::new());
        let TestApp {
            mut state,
            recorded,
            ..
        } = state_with_storage(storage.clone()).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state.clone());
        let h = Harness {
            app,
            state,
            storage: storage.clone(),
        };
        storage.fail_next("磁盘坏了");
        let (s, v) = get_json(&h, "/admin/api/events").await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        // 🔴 内部错误文本不许回显。
        assert!(!v.to_string().contains("磁盘坏了"), "{v}");
        assert!(
            recorded.events().contains(&"admin.storage_failed"),
            "500 不许静默：{:?}",
            recorded.events()
        );
    }

    // ── 下载端点 ───────────────────────────────────────────────────────────

    /// 下载端点：`text/plain`、带 `content-disposition`、**逐行 JSON 不是数组**。
    #[tokio::test]
    async fn the_download_endpoint_emits_line_delimited_json_not_an_array() {
        let h = harness().await;
        let t = now();
        seed_shard(
            &h,
            t,
            vec![entry(t - 2, "info", "a"), entry(t - 1, "warn", "b")],
        )
        .await;
        let res = call(&h, "/admin/api/events/download").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            res.headers()
                .get(header::CONTENT_DISPOSITION)
                .unwrap()
                .to_str()
                .unwrap(),
            // 🔴 **期望值手写字面量，不从 [`EVENTS_DOWNLOAD_FILENAME`] 现算**：
            //    从常量现算就是同义反复（改了常量这一格照样绿），
            //    而运维的脚本按这个名字找文件。
            "attachment; filename=\"agnes2api-events.txt\""
        );
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.starts_with('['), "不许是一个 JSON 数组：{text}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for l in lines {
            // 每一行自己就是一份合法 JSON —— 这正是 `grep` / `jq -c` 要的格式。
            let one: Value = serde_json::from_str(l).unwrap();
            assert!(one["ts"].is_number());
        }
    }

    /// 🔴 **它是自己拼的 `Response`，而全局 nosniff 仍然盖得到它。**
    ///
    /// 这一条是「nosniff 必须写在 `next()` 之后」那条性质在生产路由上的可观测锚点：
    /// 顺序写反时，只有这一族自拼响应的端点会静默少一条头。
    #[tokio::test]
    async fn the_download_endpoint_is_hand_rolled_and_still_gets_global_nosniff() {
        let h = harness().await;
        let res = call(&h, "/admin/api/events/download").await;
        assert_eq!(
            res.headers()
                .get("x-content-type-options")
                .map(|v| v.to_str().unwrap().to_string()),
            Some("nosniff".to_string()),
            "全局 nosniff 必须盖到自拼响应上"
        );
    }

    /// 下载同样 clamp 到 [`MAX_LIMIT`]，且**忽略查询参数**（它不吃 `limit`）。
    #[tokio::test]
    async fn the_download_endpoint_caps_at_max_limit() {
        let h = harness().await;
        let t = now();
        // 一个分片最多 100 条 ⇒ 用 6 个窗口凑够 600 条，才够压到 500 这条线上。
        for w in 0..6i64 {
            let at = t - w * ring::EVENT_WINDOW_MS;
            let entries: Vec<Value> = (0..100)
                .map(|i| entry(at - i64::from(i) - 1, "info", "x"))
                .collect();
            seed_shard(&h, at, entries).await;
        }
        let res = call(&h, "/admin/api/events/download?limit=9999").await;
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(text.lines().count(), MAX_LIMIT);
    }

    /// 存储读不出来时下载端点同样是 500 —— **不是一个空文件**。
    ///
    /// 一个 0 字节的「事件全量导出」是这条端点最坏的失败形态：运维会拿它当证据。
    #[tokio::test]
    async fn a_failed_download_is_a_500_not_an_empty_file() {
        let storage = Arc::new(MemoryStorage::new());
        let TestApp { mut state, .. } = state_with_storage(storage.clone()).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state.clone());
        let h = Harness {
            app,
            state,
            storage: storage.clone(),
        };
        storage.fail_next("磁盘坏了");
        let res = call(&h, "/admin/api/events/download").await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// 🔴 **读路径零 `list()`，且 get 次数对任何 `after` 都有硬上界。**
    ///
    /// 那条钳位是一次真修复：没有它时 `after=0` 单次请求实测约 **99 万次 get**。
    /// 这一格数的正是那个次数。
    #[tokio::test]
    async fn the_read_path_never_lists_and_its_fan_out_is_hard_bounded() {
        use crate::adapters::storage_test_doubles::CountingStorage;
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let storage: Arc<dyn Storage> = counting;
        let TestApp { mut state, .. } = state_with_storage(storage.clone()).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state.clone());
        let h = Harness {
            app,
            state,
            storage,
        };
        let bound = (ring::EVENT_WINDOW_RETAIN as u64) * (ring::EVENT_SLOTS as u64);

        for after in ["", "?after=0", "?after=1", "?after=9999999999999"] {
            let before = counts.snapshot();
            let (s, _) = get_json(&h, &format!("/admin/api/events{after}")).await;
            assert_eq!(s, StatusCode::OK);
            let (g, _, _, l) = counts.snapshot();
            assert_eq!(l, before.3, "读路径不许调 list（`{after}`）");
            assert!(
                g - before.0 <= bound,
                "`{after}` 打了 {} 次 get，上界是 {bound}",
                g - before.0
            );
        }
    }
}
