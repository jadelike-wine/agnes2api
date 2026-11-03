//! `/admin/api/keys*` —— **key 池管理的九条端点**（02 §9.6 ~ §9.14）。
//!
//! # 三条贯穿全文件的纪律，每一条都对应一次已经发生过的失效
//!
//! ① 🔴 **明文绝不进列表响应。** 投影一律走
//!    [`crate::core::keys::view::KeyView`]（那个结构里根本没有明文那一格），
//!    明文只走 [`reveal`] 这一条专门端点、且**每次必留痕**。
//!    列表是高频、无意识被调用的：塞进去等于每一次轮询、每一条被记下的响应体、
//!    每一个中间层缓存里都带着整池凭据。
//!
//! ② 🔴 **写操作看当前真值**：`DELETE` / `PATCH` / `bulk` / `verify` 一律走
//!    [`read_record`]（**直接读存储、绕过进程内快照**）。读快照的话，一次「停用」
//!    可能建立在一份最多一个 `pool_cache_ttl_ms` 之前的视图上，
//!    把这期间别的副本写下的 `evicted` / `cooldown_until` 原样覆盖回去。
//!
//! ③ 🔴 **落盘一律 `repo.save(next, None, now)`，刻意不传 `prev`。**
//!    传了 `prev` 就要过写消除那道闸（[`crate::pool::repo::only_elidable_changed`]），
//!    而 `note` 在白名单外、`cooldown_until` 清零对一把干净 key 是**零变化** ⇒
//!    一次「只改备注」或「重置一把干净 key」的写会被**整个丢弃**，
//!    面板显示保存成功而存储里什么都没变。
//!    ⚠️ 代价老实写：`prev = None` 那条分支会把这把 key 攒着的 `pending_stats`
//!    增量丢掉（那批计数没落盘）。换来的是「管理员的每一次写都真的落盘」，
//!    而计数本来就是标了 `≈` 的近似值，且这些端点是人点一下才发生的。
//!    另一半代价是绕过了 `still_exists()` 那道闸 —— 这一条**这里不成立**：
//!    上面那次 [`read_record`] 读的就是存储，与 `still_exists` **问的是同一个问题、
//!    读的是同一个键**，只是更早（更早对「现在还在不在」只会更弱，不是更强）。
//!
//! # 🔴 两种错误信封的分界由 [`KeysEndpoint`] 这个枚举编码进类型
//!
//! 02 §4.4 登记了一处**刻意的**不一致：同一棵树上
//! `keys/:id/usage` 的 404、`keys/purge` 的 400、`keys/:id/verify` 的 404 与 400
//! 走的是**不带 `code` 的网关信封**，而 `keys-write` 那一族的同类错误**带 `code`**。
//! TS 版在两处注释里承认这是刻意的（设置页 / 危险区不渲染那几条码，只渲染 message）。
//! ⇒ 本仓照移植，但**不靠人记**：哪条端点用哪种信封写在
//! [`KeysEndpoint::key_not_found_envelope`] 与 [`KeysEndpoint::body_gate`] 里，
//! 由 `envelope_assignment_is_pinned_per_endpoint` 那一格逐条钉住。
//!
//! # ⚠️ 本文件里三件「本该住在别处」的东西，如实登记
//!
//! 1. [`read_record`] / [`add_many`] / [`delete_many`] 是 `KeyPoolRepo` 上
//!    **还不存在**的三个方法（TS 版有 `get` / `addMany` / `deleteMany`）。
//!    它们的配额形态是硬要求（一次导入 = `M + 1` 次 put、一次批量删 = `N` 次 delete
//!    **+ 1 次** 索引 put），循环调 `repo.add()` / `repo.delete()` 会让单价翻三倍，
//!    而 `src/pool/repo.rs` 本轮不归本面改。⇒ 在这里实现，
//!    **写序严格照 `repo.rs` 的两条**（`add` = 记录先、索引后；`delete` = 索引先、记录后），
//!    落盘本身仍然走 `repo.save()`。将来 `KeyPoolRepo` 长出这三个方法时，
//!    这里三处整体搬过去，判据不必改（它们钉的是次数与顺序，不是谁实现的）。
//! 2. [`ProbeGuard`] 归 `admin/probe_guard.rs`（一把 guard 交给**全部**探测端点）。
//!    本轮那个文件还不存在，而验活是一条「按一下就真打一次外网」的端点，
//!    **护栏必须与它同一轮交付**。⇒ 先住在这里，且做成**进程级单例**
//!    （[`probe_guard`]）：那正是「一把 guard」这句话要的东西，
//!    且结构上根本不存在「装配时传错一个实例」的形态。
//! 3. `admin.storage_failed` 这条事件名是本轮新起的（P1 的事件白名单里没有
//!    「管理树读存储失败」这一档）。**它必须有**：500 不许静默 —— 见
//!    [`crate::server::errors::GatewayError::internal`] 那段。

use crate::admin::models::{MODEL_CATALOG, ProtocolId, protocol_by_id};
use crate::core::keypool::is_disabled;
use crate::core::keys::view::{
    BUCKETS, Bucket, BucketCounts, KeyView, bucket_counts, matches_query, to_key_views,
};
use crate::core::pool_index::{KEY_PREFIX, POOL_INDEX_KEY, make_pool_index, same_id_set};
use crate::core::stats::stats_or_empty;
use crate::core::types::{KeyRecord, KeyStats, KeyUpstream};
use crate::ports::fetcher::{FetchError, OutboundRequest};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::ports::storage::Storage;
use crate::server::errors::{AdminError, AdminErrorCode, GatewayError};
use crate::server::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::header};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

// ── 1. 常量（面板契约 / 配额账的真源）────────────────────────────────────────

/// 列表默认每页条数。**五份 `docs/{lang}/API.md` 的参数表逐份写着这个数**，
/// 所以它必须是**可导出的真源**，不许在文档侧手抄第二份
/// （TS 版踩过：`/admin/api/events` 的 `limit` 文档写 50/200、源码是 200/500，
/// 五种语言逐份同错，全仓零判据）。
pub const DEFAULT_SIZE: usize = 20;

/// 列表每页条数上限。理由同 [`DEFAULT_SIZE`]。
pub const MAX_SIZE: usize = 200;

/// 一次导入最多接受多少把 key。
///
/// 取 200（与 [`MAX_SIZE`] 同一个数，面板一页最多就这么多）：一次导入的写侧成本是
/// `M + 1` 次 put，而 `FileStorage` 每次 put 都**重写整个 `store.json`**。
/// 🔴 **超了就 400，不静默截断**：截断会让用户以为 500 把全导进去了，而池子里只有 200。
pub const MAX_IMPORT_KEYS: usize = 200;

/// 备注长度上限。
///
/// 它进每一条 `key:<id>` 记录，而那条记录在**每一次**状态回写里被整体重写；
/// 面板列表一次又要把整池的备注全带出来。没有上限时它就是一个挂在池子热路径上的
/// 任意长文本字段。
pub const MAX_NOTE_LENGTH: usize = 200;

/// 批量事件里最多逐条列出多少个 id。
///
/// 取 20：那是 [`DEFAULT_SIZE`]，而批量条操作的正是「当前页」⇒ 最常见的一次批量恰好装得下。
/// 上限本身的理由：`LogEntry::fields` 的值是标量，id 只能拼成一个字符串，
/// 200 个 16 位 id 就是约 3.4 KB **挂在一条事件上**。
pub const BULK_EVENT_IDS_MAX: usize = 20;

/// 两次探测之间的最小间隔。**验活与（将来的）通道测试共用这一个数** ——
/// 各写一份的话，改了一处而没改另一处时不会有任何信号。
pub const PROBE_MIN_INTERVAL_MS: i64 = 3_000;

/// `PATCH /admin/api/keys/:id` 能改的那几件事。**顺序即文档顺序。**
///
/// 前两个是**状态**（`disabled` / `note`），后四个是**动作**：
/// 传 `false` 等于没传，**不会**反过来「加一段冷却」。
pub const PATCH_FIELDS: [&str; 6] = [
    "disabled",
    "note",
    "clearCooldown",
    "clearStrikes",
    "unevict",
    "clearStats",
];

/// 批量操作的三个动作。**与面板批量条上的三个按钮逐条对应**，没有第四个。
///
/// 🔴 **没有「批量启用」「批量解除剔除」**：它们是「让更多 key 重新上场」的动作，
/// 逐把点比一次点全部安全，而危险的那三个恰恰是需要一次点全部的。
pub const BULK_OPS: [&str; 3] = ["disable", "clearCooldown", "delete"];

/// `POST /admin/api/keys/purge` 的**完整**对外路径。
///
/// 🔴 **它是单一真源**：五语言 DEPLOY.md 的配额账逐份写着它，有判据从这个常量现算。
/// 注册用的子树内路径由 [`KeysEndpoint::Purge`] 给出，两者的关系由
/// `the_purge_path_constant_is_the_single_source` 钉住。
pub const KEYS_PURGE_PATH: &str = "/admin/api/keys/purge";

/// `/admin/api` 这个挂载前缀。路由注册在子树内，对外路径 = 它 + 子树内路径。
pub const ADMIN_API_PREFIX: &str = "/admin/api";

// ── 事件名（运维的 grep 锚点，一个字都不许改）────────────────────────────────

pub const EVENT_KEY_ADDED: &str = "key.added";
pub const EVENT_KEY_RESTORED: &str = "key.restored";
pub const EVENT_KEY_DELETED: &str = "key.deleted";
pub const EVENT_KEY_DISABLED: &str = "key.disabled";
pub const EVENT_KEY_REVEALED: &str = "key.revealed";
/// 管理树读存储失败。见文件头第 3 条登记。
pub const EVENT_ADMIN_STORAGE_FAILED: &str = "admin.storage_failed";

/// 「必须先停用才能删」这条拒绝的机器可读判别串。
///
/// ⚠️⚠️ **同一条约束在线上有两种形状，面板必须分两支处理**：
/// · 单条 `DELETE` ⇒ **HTTP 409** + **顶层** `reason`；
/// · `POST /keys/bulk` ⇒ **HTTP 200** + 逐项结果里那一项的 `reason`
///   （整批 409 会让运维不知道哪几把成功了）。
/// 拿状态码当唯一判据的前端会在批量路径上**完全看不到这条拒绝**。
pub const MUST_DISABLE_FIRST: &str = "must_disable_first";

/// 危险区那条：池子在「你看到的那一刻」与「你按下确认那一刻」之间变了。
pub const POOL_SIZE_CHANGED: &str = "pool_size_changed";

// ── 2. 「哪条端点用哪种信封」—— 编码进类型 ──────────────────────────────────

/// 两种错误信封。🔴 **不许合并**，见 [`crate::server::errors`] 文件头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Envelope {
    /// `{"error":{"type","code","message","params?"}}` —— 面板按 `code` 查五语言字典。
    Admin,
    /// `{"error":{"type","message"}}` —— **没有 `code`**。
    Gateway,
}

/// 请求体那道闸的形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyGate {
    /// 这条端点不读请求体。
    None,
    /// 读 JSON 对象，畸形 / 非对象 / 未知字段一律**带 `code`**。
    AdminJson,
    /// **不接受任何参数**：空体放行、`{}` 放行，其余一律 400，
    /// 而且三种拒绝都走**不带 `code`** 的网关信封（02 §4.4 登记的刻意不一致）。
    NoParamsGateway,
}

/// 本组的九条端点。
///
/// 🔴 存在的理由不是好看：02 §4.4 那处「一半带码、一半不带」的不一致**必须被移植**
/// （改了会破坏面板），而它又是一件**没有任何自然信号**的事 ——
/// 写成一张表，改错时 `envelope_assignment_is_pinned_per_endpoint` 当场红。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysEndpoint {
    List,
    Import,
    Bulk,
    Purge,
    Reveal,
    Delete,
    Patch,
    Usage,
    Verify,
}

impl KeysEndpoint {
    /// 🔴 **唯一真源**：判据遍历它。
    pub const ALL: [KeysEndpoint; 9] = [
        Self::List,
        Self::Import,
        Self::Bulk,
        Self::Purge,
        Self::Reveal,
        Self::Delete,
        Self::Patch,
        Self::Usage,
        Self::Verify,
    ];

    /// 注册用的**子树内**路径（子树挂在 [`ADMIN_API_PREFIX`] 下）。
    ///
    /// 🔴 `app.rs` 的 `.route()` 必须从这里取，不许手抄字面量 ——
    /// 手抄的那一份与 [`KEYS_PURGE_PATH`] 迟早会漂，而漂开的现象是
    /// 「文档里那条危险区路径 404」。
    pub const fn subpath(self) -> &'static str {
        match self {
            Self::List | Self::Import => "/keys",
            Self::Bulk => "/keys/bulk",
            Self::Purge => "/keys/purge",
            Self::Reveal => "/keys/{id}/reveal",
            Self::Delete | Self::Patch => "/keys/{id}",
            Self::Usage => "/keys/{id}/usage",
            Self::Verify => "/keys/{id}/verify",
        }
    }

    /// HTTP 方法。
    pub const fn method(self) -> &'static str {
        match self {
            Self::List | Self::Reveal | Self::Usage => "GET",
            Self::Import | Self::Bulk | Self::Purge | Self::Verify => "POST",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
        }
    }

    /// 这条端点的「没有这把 key」用哪种信封。`None` = 它没有这一档。
    ///
    /// 🔴 **`Usage` 与 `Verify` 是 `Gateway`，这不是疏忽**，见文件头与 02 §4.4。
    pub const fn key_not_found_envelope(self) -> Option<Envelope> {
        match self {
            Self::List | Self::Import | Self::Bulk | Self::Purge => None,
            Self::Reveal | Self::Delete | Self::Patch => Some(Envelope::Admin),
            Self::Usage | Self::Verify => Some(Envelope::Gateway),
        }
    }

    /// 请求体那道闸。
    pub const fn body_gate(self) -> BodyGate {
        match self {
            Self::List | Self::Reveal | Self::Delete | Self::Usage => BodyGate::None,
            Self::Import | Self::Bulk | Self::Purge | Self::Patch => BodyGate::AdminJson,
            Self::Verify => BodyGate::NoParamsGateway,
        }
    }
}

/// 「没有这把 key」——**按端点选信封**，调用方不许自己拼。
fn key_not_found(ep: KeysEndpoint) -> Response {
    match ep.key_not_found_envelope() {
        Some(Envelope::Admin) => AdminError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            AdminErrorCode::KeyNotFound,
            "没有这把 key",
        )
        .into_response(),
        Some(Envelope::Gateway) => {
            GatewayError::new(StatusCode::NOT_FOUND, "not_found", "没有这把 key").into_response()
        }
        // 结构上到不了：没有 `:id` 的那四条端点压根不查 key。
        None => GatewayError::internal().into_response(),
    }
}

// ── 3. 请求体窄化（🔴 一律 `Value` + 逐字段窄化，不做 `Deserialize` 整体绑定）──

/// 读一个 JSON **对象**请求体，带 `code`。
///
/// ⚠️ **空体也是 400 `bad_json`**：TS 的 `c.req.json()` 对空体就是抛，
/// 而这四条端点（导入 / 批量 / 清空 / PATCH）的请求体本来就是必填的。
fn read_admin_object(body: &Bytes) -> Result<Map<String, Value>, AdminError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| AdminError::bad_json())?;
    match v {
        Value::Object(o) => Ok(o),
        // ⚠️ 「请求体」这三个字**刻意不进 `params`**：它是一句中文，
        //    插进 ja/ko 的字典串里等于把中文又漏回屏幕上。
        _ => Err(AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::BodyNotAnObject,
            "请求体必须是一个 JSON 对象",
        )),
    }
}

/// 不认识的字段一律 400。
///
/// **不是洁癖**：`{ disbaled: true }` 这种拼错在宽松实现下是一次
/// 「保存成功、什么都没发生」——而面板会如实地显示保存成功。
///
/// ⚠️ `params.fields` **确实逐字回显调用方送来的字段名** —— 那是这条码的用途，
/// 不说是哪几个字段，`unknown_field` 就退化成一句「有个字段不对」。
fn reject_unknown(o: &Map<String, Value>, allowed: &[&str]) -> Result<(), AdminError> {
    let extra: Vec<&str> = o
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed.contains(k))
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    let joined = extra.join(", ");
    Err(AdminError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        AdminErrorCode::UnknownField,
        format!("不认识的字段：{joined}"),
    )
    .param("fields", joined))
}

/// 可选布尔。🔴 **缺席与 `false` 是两回事**（前者「这次不动它」，后者「设成 false」）
/// ⇒ 返回 `Option`，**绝不下坠到 `false`**。
fn opt_bool(o: &Map<String, Value>, name: &'static str) -> Result<Option<bool>, AdminError> {
    match o.get(name) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::NotABoolean,
            format!("{name} 必须是布尔值"),
        )
        .param("field", name)),
    }
}

/// `note` 的三态：缺席 = 这次不动它；`null` = 清空；字符串 = 设成它。
#[derive(Debug, Clone, PartialEq, Eq)]
enum NotePatch {
    Absent,
    Clear,
    Set(String),
}

fn opt_note(o: &Map<String, Value>) -> Result<NotePatch, AdminError> {
    match o.get("note") {
        None => Ok(NotePatch::Absent),
        Some(Value::Null) => Ok(NotePatch::Clear),
        Some(Value::String(s)) => {
            // ⚠️ 按 `char` 数，不按字节：一条全中文的备注按字节算会在 67 个字上就被拒。
            if s.chars().count() > MAX_NOTE_LENGTH {
                return Err(AdminError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    AdminErrorCode::NoteTooLong,
                    format!("note 最长 {MAX_NOTE_LENGTH} 个字符"),
                )
                .param("max", u64::try_from(MAX_NOTE_LENGTH).unwrap_or(u64::MAX)));
            }
            Ok(NotePatch::Set(s.clone()))
        }
        Some(_) => Err(AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::NoteNotAString,
            "note 必须是字符串或 null",
        )),
    }
}

/// 取一个**字符串数组**。元素类型不对时**整体 400**，不把它算进逐项结果 ——
/// 那是「调用方写错了」与「用户粘错了」的区别。
fn string_array(
    v: Option<&Value>,
    code: AdminErrorCode,
    msg: &str,
) -> Result<Vec<String>, AdminError> {
    let arr = v.and_then(Value::as_array).ok_or_else(|| {
        AdminError::new(StatusCode::BAD_REQUEST, "invalid_request_error", code, msg)
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            AdminError::new(StatusCode::BAD_REQUEST, "invalid_request_error", code, msg)
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

/// 非负整数。JSON 里 `3` 与 `3.0` 都算（`JSON.parse("3.0")` 在 TS 侧就是 `3`）。
fn as_non_negative_int(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    let f = v.as_f64()?;
    if f.is_finite() && f >= 0.0 && f.fract() == 0.0 && f < 9_007_199_254_740_992.0 {
        return Some(f as u64);
    }
    None
}

// ── 4. 存储侧的三个补丁（见文件头第 1 条登记）────────────────────────────────

/// 读一条记录 —— **直接读存储，绕过进程内快照**。
///
/// 🔴 它不是 `repo.all().find(..)` 的同义词：后者最多是一个 `pool_cache_ttl_ms`
/// 之前的视图，拿它去改一把刚被别的副本改过的 key，就是把那次改动原样覆盖回去。
async fn read_record(storage: &Arc<dyn Storage>, id: &str) -> Result<Option<KeyRecord>, Response> {
    let raw = storage
        .get(&format!("{KEY_PREFIX}{id}"))
        .await
        .map_err(|_| GatewayError::internal().into_response())?;
    Ok(raw
        .as_ref()
        .and_then(crate::core::narrow::narrow_key_record))
}

/// 读索引。**读不出来 / 认不出来一律当「缺失」**（与 `repo.rs::read_index` 同一套语义）。
async fn read_index(storage: &Arc<dyn Storage>) -> Option<Vec<String>> {
    let raw = storage.get(POOL_INDEX_KEY).await.ok()?;
    crate::core::narrow::narrow_pool_index(raw.as_ref()).map(|i| i.ids)
}

/// 批量导入的落点。
///
/// **形态：一次读索引 → M 次存在性探测 → M 次记录 put → ≤1 次写索引。**
/// 🔴 **不许退化成循环调 `repo.add()`**：那是「1 次记录 put + 1 次索引读 + 1 次索引 put」，
/// 循环 M 次就是 `3M` 次操作，而 `FileStorage` 每次 put 都重写整个 `store.json`。
///
/// ⚠️ **「这把 key 在不在」的判据是记录本身，不是索引。** 拿索引当判据两个方向都会坏：
/// 索引缺失 ⇒ 已知集合是空的 ⇒ **整池每一把都被当成新 key 覆盖一遍**
/// （「重复导入即解封」那个后门从另一扇门原样回来）；
/// 幽灵索引项 ⇒ 那把 key 被报成「已存在」而其实根本没导进去。
struct ImportOutcome {
    added: Vec<String>,
    duplicated: Vec<String>,
    /// 🔴 装的是**输入里的位置**（1 基），**不是明文、也不是掩码** ——
    /// 这个函数手上根本没有一条通往明文的返回路径。
    invalid: Vec<usize>,
}

async fn add_many(
    st: &AppState,
    keys: &[String],
    reset_existing: bool,
    upstream: Option<KeyUpstream>,
    now: i64,
) -> Result<ImportOutcome, Response> {
    let idx = read_index(&st.storage).await;
    let known: Vec<String> = match &idx {
        Some(ids) => ids.clone(),
        // 索引缺失时回落一次 `list()`：只写 `added` 会把索引里原有的 id 全部抹掉，
        // 那是**正确性问题**，不是配额问题。
        None => {
            let names = st
                .storage
                .list(KEY_PREFIX)
                .await
                .map_err(|_| GatewayError::internal().into_response())?;
            crate::core::pool_index::ids_from_key_names(&names)
        }
    };

    let mut out = ImportOutcome {
        added: Vec::new(),
        duplicated: Vec::new(),
        invalid: Vec::new(),
    };
    // 本批已处理过的 id：同一把 key 粘第二遍时不许再落一次盘、也不许再探一次存在性。
    let mut seen: HashSet<String> = HashSet::new();

    for (at, raw) in keys.iter().enumerate() {
        let key = raw.trim();
        // 🔴 **空行整条跳过**，且**位置仍按原始下标算** ——
        //    于是前端可以原样发（一行一个元素，空行也发），位置就是行号。
        //    把空行报成非法项的话，文本框末尾那个换行会被报成「第 N 行不合法」，
        //    一条**不存在的错误**。
        if key.is_empty() {
            continue;
        }
        if !crate::pool::repo::is_importable_key(key) {
            out.invalid.push(at + 1);
            continue;
        }
        let id = crate::pool::repo::key_id(key);
        if seen.contains(&id) {
            // 同一把在同一批里粘两遍：**对用户就是一条重复**，不该被悄悄吃掉。
            out.duplicated.push(id);
            continue;
        }
        seen.insert(id.clone());

        match read_record(&st.storage, &id).await? {
            None => {
                out.added.push(id.clone());
                let rec = KeyRecord {
                    upstream,
                    ..KeyRecord::fresh(&id, key, now)
                };
                st.repo
                    .save(&rec, None, now)
                    .await
                    .map_err(|_| GatewayError::internal().into_response())?;
            }
            Some(existing) => {
                out.duplicated.push(id);
                if reset_existing {
                    // 🔴 重置的是**系统判定的失败态**，刻意**不动**
                    //    `disabled`（人的决定）/ `added_at`（来历）/ `stats`（历史）/ `note`。
                    let next = KeyRecord {
                        cooldown_until: 0,
                        cooldown_reason: None,
                        strikes: 0,
                        evicted: false,
                        evicted_reason: None,
                        ..existing
                    };
                    st.repo
                        .save(&next, None, now)
                        .await
                        .map_err(|_| GatewayError::internal().into_response())?;
                }
            }
        }
    }

    // ② 再进索引（记录先写、索引后写 —— 中途失败留下的是**孤儿记录**，对账能收养）。
    //    把 `duplicated` 也并进去：记录在、索引里没有（孤儿）时这一步顺手把它捡回来；
    //    正常情形下集合不变 ⇒ **一次 put 都不写**（「重复导入一次盘都不落」靠这个短路）。
    let mut merged = known.clone();
    for id in out.added.iter().chain(out.duplicated.iter()) {
        if !merged.contains(id) {
            merged.push(id.clone());
        }
    }
    if idx.is_none() || !same_id_set(&merged, &known) {
        let v = serde_json::to_value(make_pool_index(&merged))
            .map_err(|_| GatewayError::internal().into_response())?;
        // 🔴 **这一次必须往上抛**：记录已经写进去了，索引没跟上就是一批孤儿 ——
        //    报错让调用方重试（重试是幂等的）比返回「导入成功」而池子看不见它们诚实。
        st.storage
            .put(POOL_INDEX_KEY, v, None)
            .await
            .map_err(|_| GatewayError::internal().into_response())?;
    }
    st.repo.invalidate().await;
    Ok(out)
}

/// 批量删除。**一次读索引 → ≤1 次写索引 → N 次记录 delete。**
///
/// 🔴 **写序照 `repo.rs::delete`：① 出索引 → ② 删记录。**
/// 颠倒的话中途失败留下**孤儿记录**，而对账以 `list()` 为准会把它**加回索引**
/// —— 等于把一次删除悄悄撤销了，**那是正确性问题**。
///
/// 🔴 **不许退化成循环调 `repo.delete()`**：那样每一把都要「读索引 + 写索引」，
/// 一次点击 200 把就是 200 次索引 put，而换来的信息量与一次 put 完全相同。
async fn delete_many(st: &AppState, ids: &[String]) -> Result<(), Response> {
    if ids.is_empty() {
        return Ok(());
    }
    let doomed: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let idx = read_index(&st.storage).await;
    let known = match &idx {
        Some(v) => v.clone(),
        None => {
            let names = st
                .storage
                .list(KEY_PREFIX)
                .await
                .map_err(|_| GatewayError::internal().into_response())?;
            crate::core::pool_index::ids_from_key_names(&names)
        }
    };
    let kept: Vec<String> = known
        .iter()
        .filter(|x| !doomed.contains(x.as_str()))
        .cloned()
        .collect();
    // ① 先出索引。
    if idx.is_none() || kept.len() != known.len() {
        let v = serde_json::to_value(make_pool_index(&kept))
            .map_err(|_| GatewayError::internal().into_response())?;
        st.storage
            .put(POOL_INDEX_KEY, v, None)
            .await
            .map_err(|_| GatewayError::internal().into_response())?;
    }
    // ② 再删记录。
    for id in ids {
        st.storage
            .delete(&format!("{KEY_PREFIX}{id}"))
            .await
            .map_err(|_| GatewayError::internal().into_response())?;
    }
    st.repo.invalidate().await;
    Ok(())
}

// ── 5. 出站探测的护栏（见文件头第 2 条登记）──────────────────────────────────

/// 两道闸，缺一不可：
/// · **在途去重**：同一 kind 同时只许有一次在飞 —— 挡「连点」；
/// · **最小间隔**：两次**发起**之间至少 [`PROBE_MIN_INTERVAL_MS`] —— 挡「按住不放」。
///
/// 只有前者时，在飞期间被挡住的那次点击会在它一结束就放行，「连点」退化成
/// 「跟着上一次的结束节奏连点」；只有后者时，同一毫秒到达的两次会**双双**通过。
///
/// ⚠️ **它是进程内的，不是存储级的。** 代价明写：多副本部署下 N 个副本各自允许一次
/// ⇒ 最小间隔的真实上界是「每 `PROBE_MIN_INTERVAL_MS` 最多 N 次」，不是 1 次。
/// 为它引入一把存储锁反而会给一条**今天零写**的端点新增一次无条件的写。
///
/// ⚠️ **kind 的空间由调用方限界，本结构自己不限**：这张表从不主动清理，
/// 它有界的全部理由是 [`verify`] 把 `try_acquire` 放在「这把 key 真的存在」**之后**。
/// 挪到前面去，任何一个拿到管理口令的人都能用随机 id 往这张表里无限灌键。
pub struct ProbeGuard {
    slots: Mutex<HashMap<String, ProbeSlot>>,
}

#[derive(Debug, Clone, Copy)]
struct ProbeSlot {
    in_flight: bool,
    /// 上一次**开始**探测的时刻，不是结束的时刻。
    ///
    /// 取开始时刻，则「最小间隔」度量的是两次**发起**之间的距离，与上游快慢无关；
    /// 取结束时刻的话，一次 8 秒超时的探测之后还要再等 3 秒才让人重试 ——
    /// 而上游越慢，运维越想赶紧再试一次。
    last_at: i64,
}

/// 占闸的结果。**两种拒绝的处置完全不同，只给状态码的话两者分不开。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAcquire {
    Ok,
    /// 上一次还没回来。
    InFlight,
    /// 刚探过。
    Cooldown,
}

impl ProbeAcquire {
    /// 顶层 `reason`。面板据它选五语言文案，**不解析中文 `message`**。
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Ok => None,
            Self::InFlight => Some("probe_in_flight"),
            Self::Cooldown => Some("probe_cooldown"),
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Ok => "",
            Self::InFlight => {
                "上一次探测还没有返回，等它结束再试（这颗按钮每按一次都会真的打一次外网）"
            }
            Self::Cooldown => "两次探测之间至少要隔一小段时间，请稍后再试",
        }
    }
}

impl ProbeGuard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// 占住这个 kind。
    ///
    /// 🔴 **同步的，里面没有任何 `await`**：写成「先问一句忙不忙、再去占」
    /// 就把检查与占用之间的窗口造回来了，而那种形态下第二个并发请求会被放行、
    /// 真的多打一次外网。
    pub fn try_acquire(&self, kind: &str, now: i64) -> ProbeAcquire {
        let mut slots = self.slots.lock();
        if let Some(s) = slots.get(kind) {
            if s.in_flight {
                return ProbeAcquire::InFlight;
            }
            let since = now - s.last_at;
            // `since < 0` = 时钟回拨（NTP）：立刻放行，与本仓其余几处同一套语义。
            if (0..PROBE_MIN_INTERVAL_MS).contains(&since) {
                return ProbeAcquire::Cooldown;
            }
        }
        slots.insert(
            kind.to_string(),
            ProbeSlot {
                in_flight: true,
                last_at: now,
            },
        );
        ProbeAcquire::Ok
    }

    /// 占住这个 kind，**成功时交出一张 [`ProbeLease`]**（离开作用域即放开）。
    ///
    /// 🔴 **新的调用点一律走这一条，不要手动配对 `try_acquire` / `release`。**
    /// 理由见 [`ProbeLease`]：「把 `release` 放在一定会走到的位置上」这句话在 async 里
    /// **是假的** —— 客户端一断开，axum 直接 drop handler 的 future，
    /// `await` 之后的每一行都不会再执行。
    ///
    /// ✅ **已收口**：全仓三条探测端点（本文件的验活、[`crate::admin::models::test_with`]、
    /// [`crate::admin::upstream_models`]）今天**都走租约**，生产代码里 `release` 只剩
    /// [`ProbeLease::drop`] 这一个调用点。`try_acquire` / `release` 仍然公开，
    /// 但只剩两种正当用途：本函数的内部实现，以及判据里直接观测槽位。
    pub fn acquire(&self, kind: &str, now: i64) -> Result<ProbeLease<'_>, ProbeAcquire> {
        match self.try_acquire(kind, now) {
            ProbeAcquire::Ok => Ok(ProbeLease {
                guard: self,
                kind: kind.to_string(),
            }),
            refused => Err(refused),
        }
    }

    /// 放开在途标记。
    ///
    /// 🔴 **调用方必须放在「一定会走到」的位置上**：只放在成功支的话，
    /// 一次失败的探测会让这个 kind **永久**卡在「在飞」，那颗按钮从此再也点不动
    /// （最小间隔那道闸会自愈，这道不会）—— 而「上游连不上」恰恰是运维最想连点重试的那一刻。
    /// ⚠️ 而「一定会走到」在 async 里没有纯靠位置就能拿到的形态：**请求被取消**那一支
    /// 谁都走不到。⇒ 手写调用点只剩一个正当用法，就是 [`ProbeLease`] 的 drop。
    ///
    /// 没占过就放是调用方的接线错误，但**这里不抛**：一条护栏不该把一个本来成功的
    /// 请求变成 500。
    pub fn release(&self, kind: &str) {
        if let Some(s) = self.slots.lock().get_mut(kind) {
            s.in_flight = false;
        }
    }

    /// **判据专用的观测口**：这张表里有没有 `kind` 这一格。
    ///
    /// 存在的理由：`slots` 是私有的 `HashMap` 且**从不主动清理**，
    /// 它有界的全部理由是调用方把 [`Self::try_acquire`] 放在「这把 key 真的存在」之后
    /// （见本结构文件头那段）。那条理由在它之前**没有任何可观测量** ——
    /// 唯一在看的判据只观测状态码（「连着两次都是 404 不是 429」），
    /// 而那对「先占一格、再回 404」结构性地看不见：变异复检里
    /// 「占一格不放」与「占了再放」两种**都全绿逃逸**。⇒ 把「落没落脚」做成可观测量。
    ///
    /// ⚠️ `#[cfg(test)]`：它只回答判据的问题，**不是一项对外能力** ——
    /// 生产代码里没有任何一处该关心「这张表里有谁」，能关心就迟早会有人按它分支。
    #[cfg(test)]
    #[must_use]
    pub fn tracks(&self, kind: &str) -> bool {
        self.slots.lock().contains_key(kind)
    }

    /// **判据专用的观测口**：`kind` 这一格是不是还卡在「在飞」。
    ///
    /// 「槽位有没有被放开」在它之前只能间接观测（「隔过最小间隔还能不能再来一次」），
    /// 而那条间接观测量要求判据能控制时钟 —— 走真 handler 的那一族控制不了。
    /// 没这一格的话，「客户端断开 ⇒ 永久卡在在飞」这条性质在端点这一层测不出来。
    #[cfg(test)]
    #[must_use]
    pub fn is_in_flight(&self, kind: &str) -> bool {
        self.slots.lock().get(kind).is_some_and(|s| s.in_flight)
    }
}

impl Default for ProbeGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 占闸成功时交出来的凭据。🔴 **它是「放开」这件事唯一有保证的那个点。**
///
/// 回归对象是一条已经发生过的形态：`try_acquire` → `send().await` → `release`
/// 这个形状只覆盖了两条支（上游回了 / 上游报错），**漏掉第三条：请求被取消**。
/// axum 在客户端断开时直接 drop handler 的 future ⇒ `release` 那一行根本不执行 ⇒
/// `in_flight` 永远停在 `true`，而 [`ProbeGuard::try_acquire`] 第一句就是
/// 「在飞就拒」⇒ **那个 kind 的按钮从此永久点不动**（最小间隔那道闸会自愈，这道不会）。
///
/// ⇒ 放开**不许依赖执行流走到某一行**，只许挂在 drop 上 —— 而 drop 在
/// 「正常返回 / 提前 return / panic 展开 / future 被取消」四种收场里都会执行。
pub struct ProbeLease<'a> {
    guard: &'a ProbeGuard,
    /// 占的是哪一格。**自己存一份**：调用方那个 `kind` 的作用域不归它管。
    kind: String,
}

impl Drop for ProbeLease<'_> {
    fn drop(&mut self) {
        self.guard.release(&self.kind);
    }
}

/// 🔴 **全进程一把。** 「一把 guard 交给全部探测端点」这句话要的就是它，
/// 而做成单例之后**结构上不存在「装配时传错一个实例」的形态**。
///
/// ⚠️ 代价老实写：它跨判据存活。本组的验活判据靠**每格用不同的 key 明文**
/// 来隔离（kind = `verify:<sha256 前 8 字节>`，明文不同 ⇒ 槽位不同），
/// **别在两格判据里用同一把 key 明文**。
pub fn probe_guard() -> &'static ProbeGuard {
    static GUARD: LazyLock<ProbeGuard> = LazyLock::new(ProbeGuard::new);
    &GUARD
}

// ── 6. 响应体（🔴 一律结构体，不用 `json!` 宏 —— 键序是对外契约）──────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeysListBody {
    items: Vec<KeyView>,
    total: usize,
    page: usize,
    size: usize,
    pages: usize,
    counts: BucketCounts,
    /// Tier-1 是**近似值**：并发下少计，且最多晚一个 `pool_touch_interval_ms` 落盘。
    approximate: bool,
    generated_at: i64,
    /// 🔴 **已经判好的视图**（06 §5.3 的 A 档）。
    ///
    /// ⚠️ 它不是 `items` 的复述 —— 里面有**两件 `items` 里根本没有的知识**：
    /// ① 逐颗行内动作能不能点（判据是 `disabled` / `evicted` / `cooldownUntil` /
    ///    `strikes` 四个事实的组合，而 `evicted` 这个布尔**不在 `KeyView` 里**，
    ///    面板拿不到它 —— TS 版那边是靠一个额外字段绕过去的）；
    /// ② 「列表为空」该说哪一句（「池子里还没有 key」vs「筛不出来」，
    ///    判据是 `counts.all`，**指向完全相反的动作**）。
    view: KeysViewWire,
}

/// 列表那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeysViewWire {
    /// **与 `items` 同序同长**，逐条带 `id` 以便面板自查对齐。
    rows: Vec<KeyRowViewWire>,
    pager: PagerWire,
    /// `null` = 显示表格；否则显示这句话。
    list_message_key: Option<&'static str>,
}

/// 一行 key 的视图。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyRowViewWire {
    /// 与 `items[i].id` 相同。**带上它是为了让面板能断言两边对齐**，
    /// 而不是靠下标信任。
    id: String,
    badge_class: &'static str,
    bucket_label_key: &'static str,
    /// 冷却剩余毫秒。**只有 `cooling` 档有**，其余是 `null`（渲染成 `—`）。
    /// ⚠️ 它与 `canClearCooldown` **刻意用不同的判据**，见
    /// [`crate::core::admin::view::keys::cooldown_remaining`]。
    cooldown_remaining_ms: Option<i64>,
    toggle_label_key: &'static str,
    /// 🔴 与 [`deletable`] 是**同一份实现**。
    can_delete: bool,
    can_clear_cooldown: bool,
    can_unevict: bool,
    can_clear_strikes: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PagerWire {
    prev_disabled: bool,
    next_disabled: bool,
    /// `null` = **把「第 1/2 页 · 共 3 条」整条抹掉**（空列表时必须一并复位，
    /// 否则屏幕上挂着一份已经不存在的数据的页码）。
    info: Option<PagerInfoWire>,
}

#[derive(Serialize)]
struct PagerInfoWire {
    page: usize,
    pages: usize,
    total: usize,
}

#[derive(Serialize)]
struct ImportBody {
    added: Vec<String>,
    duplicated: Vec<String>,
    invalid: Vec<usize>,
    /// 🔴 **`reset` 与 `duplicated.len()` 不是一个数**：本批新建的那把被粘第二遍
    /// 也算重复，但它谈不上「被重置」。面板显示 `duplicated.length` 就是在撒谎。
    reset: usize,
}

#[derive(Serialize)]
struct BulkItem {
    id: String,
    ok: bool,
    reason: Option<&'static str>,
}

#[derive(Serialize)]
struct BulkBody {
    results: Vec<BulkItem>,
}

#[derive(Serialize)]
struct PurgeBody {
    deleted: usize,
    remaining: usize,
    expected: usize,
}

#[derive(Serialize)]
struct RevealBody {
    /// 06 §5.3 要的那一格：**端点直接给档位**，面板不必自己从字段组合里反推。
    ///
    /// ⚠️ **这一族恒是 `"ok"`，而这不是一条永远为假的分支**：上游 Agnes key
    /// **本来就以明文存**（要拿去打上游），所以「服务端从来就没有过它的明文」
    /// 那一档在这一族里不存在 —— 那一档只属于我们签发的 `sk-`
    /// （见 [`crate::apikey::admin`]）。
    /// 🔴 **它照样要发**：两族共用同一段渲染代码
    /// （[`crate::core::admin::view::reveal`] 的状态机逐字相同），
    /// 少一格就逼着面板为这一族写第二条分支。
    kind: &'static str,
    key: String,
}

#[derive(Serialize)]
struct OkBody {
    ok: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyUsageBody {
    id: String,
    stats: KeyStats,
    approximate: bool,
    generated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyBody {
    ok: bool,
    /// 上游状态码。**只回状态码，一个字节的上游正文都不回。**
    status: Option<u16>,
    latency_ms: i64,
    reason: Option<&'static str>,
}

/// 网关信封那一层的内壳（键序 `type` 在前）。
#[derive(Serialize)]
struct GatewayInner {
    r#type: &'static str,
    message: String,
}

/// 管理信封那一层的内壳（键序 `type` → `code` → `message`）。
#[derive(Serialize)]
struct AdminInner {
    r#type: &'static str,
    code: &'static str,
    message: String,
}

/// 02 §4.4 的「handler 自拼」混合体：**信封 + 顶层附加字段**。
#[derive(Serialize)]
struct ConflictWithCode {
    error: AdminInner,
    reason: &'static str,
}

#[derive(Serialize)]
struct PurgeConflict {
    error: GatewayInner,
    reason: &'static str,
    expected: usize,
    actual: usize,
}

#[derive(Serialize)]
struct ProbeBusy {
    error: GatewayInner,
    reason: &'static str,
}

// ── 7. 九条 handler ────────────────────────────────────────────────────────

/// 读整池；存储读不出来 ⇒ 500 + 一条事件（🔴 500 不许静默）。
async fn all_or_500(st: &AppState, now: i64) -> Result<Vec<KeyRecord>, Response> {
    match st.repo.all(now).await {
        Ok(v) => Ok(v),
        Err(e) => {
            st.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    EVENT_ADMIN_STORAGE_FAILED,
                    "管理面读 key 池失败",
                )
                .with("err", e.to_string()),
            );
            Err(GatewayError::internal().into_response())
        }
    }
}

/// 查询参数里的一个整数。**非法值一律回落到 `fallback`，不报错** ——
/// 面板拼错一个分页参数不该把整张列表打成 400。
fn int_param(raw: Option<&str>, fallback: usize, min: usize, max: usize) -> usize {
    let Some(raw) = raw else { return fallback };
    match raw.parse::<usize>() {
        Ok(n) if n >= min && n <= max => n,
        _ => fallback,
    }
}

/// `GET /admin/api/keys` —— **零 `list()`、零额外读**。
///
/// 走 `repo.all()`，与转发路径共用同一个进程内快照 ⇒ 面板每刷新一次**不产生**
/// 独立的存储开销。
///
/// 🔴 **`counts` 永远按整池算，不受筛选影响**：拿筛完的集合去算就恒等于当前
/// 这一档的条数，另外三档全是 0 —— 而筛选器旁边那几个数的含义正是
/// 「切换过去能看到几条」。
pub async fn list(State(st): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let at = crate::adapters::now_ms();
    let records = match all_or_500(&st, at).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let all = to_key_views(&records, at);

    let q = query_map(raw.as_deref());
    let needle = q.get("q").map_or("", String::as_str);
    // `bucket` 不是合法档位时**整个忽略**（而不是「筛出 0 条」）。
    let bucket: Option<Bucket> = q
        .get("bucket")
        .and_then(|b| BUCKETS.iter().copied().find(|x| x.as_str() == b));

    let filtered: Vec<KeyView> = all
        .iter()
        .filter(|v| bucket.is_none_or(|b| v.bucket == b) && matches_query(v, needle))
        .cloned()
        .collect();

    let size = int_param(q.get("size").map(String::as_str), DEFAULT_SIZE, 1, MAX_SIZE);
    let pages = filtered.len().div_ceil(size).max(1);
    let page = int_param(q.get("page").map(String::as_str), 1, 1, pages);
    let items: Vec<KeyView> = filtered
        .iter()
        .skip((page - 1) * size)
        .take(size)
        .cloned()
        .collect();

    let counts = bucket_counts(&all);
    // ⚠️ 必须在 `items` 被 move 进响应体**之前**算完 —— 它读的正是这一页的条目。
    let view = keys_view(
        &items,
        &records,
        page,
        pages,
        filtered.len(),
        counts.all,
        at,
    );
    Json(KeysListBody {
        total: filtered.len(),
        items,
        page,
        size,
        pages,
        counts,
        approximate: true,
        generated_at: at,
        view,
    })
    .into_response()
}

/// 拼列表那份视图。
///
/// 🔴 **判定一律转调 [`crate::core::admin::view::keys`]，这里只做搬运**：
/// 在这个函数里写哪怕一个 `if`，那条判定就有了第二份实现。
///
/// ⚠️ **`evicted` 要从 `records` 里取，不能从 [`KeyView`] 里取。**
/// 投影里只有 `bucket`，而 `bucket` 是 `disabled > evicted > cooling > fresh` 的
/// **优先级投影** —— 一把「既被剔除、又被手动停用」的 key 的 `bucket` 是 `disabled`，
/// 拿它反推 `evicted` 会得到 `false`，于是那把 key 的「解除剔除」按钮永远禁用。
/// 同一条坑在 TS 侧的 `canClearCooldown` 上真实发生过一次。
fn keys_view(
    items: &[KeyView],
    records: &[KeyRecord],
    page: usize,
    pages: usize,
    total: usize,
    total_in_pool: usize,
    now: i64,
) -> KeysViewWire {
    use crate::core::admin::view::keys as v;

    let evicted: HashMap<&str, bool> = records.iter().map(|r| (r.id.as_str(), r.evicted)).collect();

    let rows = items
        .iter()
        .map(|it| {
            // 记录在两次读之间被别的副本删掉时取不到 —— 那种情形下**按「没被剔除」
            // 处置**：`canUnevict` 因此是 `false`，而那颗按钮点下去本来也是 404。
            let ev = evicted.get(it.id.as_str()).copied().unwrap_or(false);
            KeyRowViewWire {
                id: it.id.clone(),
                badge_class: v::badge_class(it.bucket.as_str()),
                bucket_label_key: v::bucket_label_key(it.bucket.as_str()),
                cooldown_remaining_ms: v::cooldown_remaining(
                    it.bucket.as_str(),
                    it.cooldown_until,
                    now,
                ),
                toggle_label_key: v::toggle_disable_label_key(it.disabled),
                can_delete: v::is_deletable(it.disabled, ev),
                can_clear_cooldown: v::can_clear_cooldown(it.cooldown_until, now),
                can_unevict: v::can_unevict(ev),
                can_clear_strikes: v::can_clear_strikes(it.strikes),
            }
        })
        .collect();

    let pager = v::pager_state(items.len(), page, pages, total);
    KeysViewWire {
        rows,
        pager: PagerWire {
            prev_disabled: pager.prev_disabled,
            next_disabled: pager.next_disabled,
            info: pager.info.map(|i| PagerInfoWire {
                page: i.page,
                pages: i.pages,
                total: i.total,
            }),
        },
        list_message_key: v::list_message_key(items.len(), total_in_pool),
    }
}

/// 解析查询串。**重复键取第一个**（与 `URLSearchParams.get` 同语义）。
fn query_map(raw: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(raw) = raw else { return out };
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        out.entry(k.into_owned()).or_insert_with(|| v.into_owned());
    }
    out
}

/// `POST /admin/api/keys` —— 批量导入。
pub async fn import(State(st): State<AppState>, body: Bytes) -> Response {
    let now = crate::adapters::now_ms();
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &["keys", "resetExisting", "upstream"]) {
        return e.into_response();
    }
    let keys = match string_array(
        o.get("keys"),
        AdminErrorCode::KeysNotAStringArray,
        "keys 必须是字符串数组",
    ) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    if keys.len() > MAX_IMPORT_KEYS {
        return AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::TooManyImportKeys,
            format!("一次最多导入 {MAX_IMPORT_KEYS} 把 key"),
        )
        .param("max", u64::try_from(MAX_IMPORT_KEYS).unwrap_or(u64::MAX))
        .into_response();
    }
    let reset_existing = match opt_bool(&o, "resetExisting") {
        Ok(v) => v.unwrap_or(false),
        Err(e) => return e.into_response(),
    };

    let upstream = match o.get("upstream") {
        None | Some(Value::Null) => Some(KeyUpstream::International),
        Some(value) => match value.as_str().and_then(KeyUpstream::parse) {
            Some(route) => Some(route),
            None => {
                return AdminError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    AdminErrorCode::InvalidUpstream,
                    "请选择国际站、国际站备用或中国站上游",
                )
                .into_response();
            }
        },
    };
    let out = match add_many(&st, &keys, reset_existing, upstream, now).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    if !out.added.is_empty() {
        st.logger.log(
            LogEntry::new(LogLevel::Info, EVENT_KEY_ADDED, "面板导入了新的 key")
                .with("added", out.added.len() as u64)
                .with("duplicated", out.duplicated.len() as u64)
                .with("invalid", out.invalid.len() as u64),
        );
    }
    // **被重置的是「本批之前就已经在池子里」的那些**：本批刚新建的那把即使被粘了
    // 两遍也谈不上重置。`duplicated` 逐条对应输入、可能含重复项，所以要去重。
    let reset = if reset_existing {
        let added: HashSet<&str> = out.added.iter().map(String::as_str).collect();
        out.duplicated
            .iter()
            .filter(|id| !added.contains(id.as_str()))
            .collect::<HashSet<_>>()
            .len()
    } else {
        0
    };
    if reset > 0 {
        // 🔴 **知情重置必须留痕**：`resetExisting` 是一条正当的「重新导入即解封」路径
        //    （它把后门变成一个要显式勾选的动作），但它同时意味着
        //    **一个能调这个端点的人可以绕过 `evicted`**。
        st.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_KEY_RESTORED,
                "面板导入时勾选了「重置已存在 key 的状态」，这些 key 的冷却 / strikes / 剔除标记已被清空",
            )
            .with("reset", reset as u64),
        );
    }

    Json(ImportBody {
        added: out.added,
        duplicated: out.duplicated,
        invalid: out.invalid,
        reset,
    })
    .into_response()
}

/// `POST /admin/api/keys/bulk` —— `{ op, ids }` → **逐项结果**。
///
/// 🔴 **逐项结果不是装饰**：批量删除里混着一把没停用的 key 时，整批 409 会让运维
/// 不知道哪几把成功了、要不要重试；而只回一个「成功」则会把那一把的失败吞掉。
pub async fn bulk(State(st): State<AppState>, body: Bytes) -> Response {
    let now = crate::adapters::now_ms();
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &["op", "ids"]) {
        return e.into_response();
    }
    let op = match o.get("op").and_then(Value::as_str) {
        Some(s) if BULK_OPS.contains(&s) => s.to_string(),
        _ => {
            let ops = BULK_OPS.join(" / ");
            return AdminError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                AdminErrorCode::NotABulkOp,
                format!("op 必须是 {ops} 之一"),
            )
            .param("ops", ops)
            .into_response();
        }
    };
    let ids = match string_array(
        o.get("ids"),
        AdminErrorCode::IdsNotAStringArray,
        "ids 必须是字符串数组",
    ) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    if ids.len() > MAX_IMPORT_KEYS {
        return AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::TooManyBulkIds,
            format!("一次最多操作 {MAX_IMPORT_KEYS} 把 key"),
        )
        .param("max", u64::try_from(MAX_IMPORT_KEYS).unwrap_or(u64::MAX))
        .into_response();
    }

    let mut results: Vec<BulkItem> = Vec::with_capacity(ids.len());
    let mut to_delete: Vec<String> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for id in &ids {
        let rec = match read_record(&st.storage, id).await {
            Ok(v) => v,
            Err(r) => return r,
        };
        let Some(rec) = rec else {
            results.push(BulkItem {
                id: id.clone(),
                ok: false,
                reason: Some("not_found"),
            });
            continue;
        };
        if op == "delete" && !deletable(&rec) {
            // 🔴 **逐项用的是同一个判据函数**：批量路径上另写一份的话，
            //    「必须先停用才能删」就有了两份实现，而批量删除恰恰是后果最不可挽回的那条路。
            results.push(BulkItem {
                id: id.clone(),
                ok: false,
                reason: Some(MUST_DISABLE_FIRST),
            });
            continue;
        }
        if op == "delete" {
            to_delete.push(id.clone());
        } else {
            let next = if op == "disable" {
                KeyRecord {
                    disabled: Some(true),
                    ..rec
                }
            } else {
                KeyRecord {
                    cooldown_until: 0,
                    cooldown_reason: None,
                    ..rec
                }
            };
            // 不传 `prev`，理由见文件头第 ③ 条。
            if st.repo.save(&next, None, now).await.is_err() {
                return GatewayError::internal().into_response();
            }
        }
        results.push(BulkItem {
            id: id.clone(),
            ok: true,
            reason: None,
        });
        changed.push(id.clone());
    }
    if let Err(r) = delete_many(&st, &to_delete).await {
        return r;
    }

    if !changed.is_empty() {
        st.logger.log(bulk_event(&op, &changed));
    }
    Json(BulkBody { results }).into_response()
}

/// 409「必须先停用才能删」的判据：🔴 **两条都不成立才拦。**
///
/// 只看 `evicted` 的话，一把健康的 key **永远删不掉**（运维只能去手工改存储，
/// 那条路绕过整个面板）。`disabled` 走 [`is_disabled`] 而不是 `r.disabled` ——
/// 那是全仓唯一一份判据，各写各的后果是「面板显示已停用、删除按钮说没停用」。
///
/// 🔴 **判据本身转调 [`crate::core::admin::view::keys::is_deletable`]，这里只负责
/// 把两个事实取出来。** 那一份同时喂给列表响应里的 `view.rows[].canDelete`
/// —— 服务端的前置校验与面板那颗按钮的可用性**必须是同一条判据**，
/// 否则就会出现「按钮说不能，后端说能」（或者反过来，更糟：按钮亮着、点下去 409）。
fn deletable(r: &KeyRecord) -> bool {
    crate::core::admin::view::keys::is_deletable(is_disabled(r), r.evicted)
}

/// 批量操作的事件。
///
/// ⚠️ **不超过 [`BULK_EVENT_IDS_MAX`] 就逐个列出，超了就明说自己没列**
/// （`idsOmitted: true`，而不是让 `ids: null` 去暗示这件事）——
/// 「面板说不出自己缺了什么」在本仓是被单独裁过的一类缺陷。
/// 逐条打 200 条事件不是选项（会直接撞穿写预算）。
fn bulk_event(op: &str, changed: &[String]) -> LogEntry {
    let listed = changed.len() <= BULK_EVENT_IDS_MAX;
    let (level, event, msg) = match op {
        "delete" => (LogLevel::Warn, EVENT_KEY_DELETED, "面板批量删除了 key"),
        "disable" => (
            LogLevel::Warn,
            EVENT_KEY_DISABLED,
            "面板批量停用了 key（它们仍然占着 targetKeys 的名额，不会触发补池）",
        ),
        _ => (LogLevel::Info, EVENT_KEY_RESTORED, "面板批量清掉了冷却"),
    };
    LogEntry::new(level, event, msg)
        .with("count", changed.len() as u64)
        .with(
            "ids",
            if listed {
                Value::String(changed.join(","))
            } else {
                Value::Null
            },
        )
        .with("idsOmitted", !listed)
}

/// `POST /admin/api/keys/purge` —— 危险区。
///
/// 🔴 **`expect` 不许改成可选**：枚举式鉴权矩阵会拿正确口令把每条路由真打一遍，
/// `purge` 打通了就是清掉一池 key —— 它安全的**唯一**理由就是必填 `expect`
/// 让不带体的 POST 在解析那一步就 400。
///
/// ⚠️ `expect` 不合法那一档走的是**不带 `code` 的网关信封**（02 §4.4 登记的
/// 刻意不一致：设置页的危险区卡从不渲染 `code`，只按顶层 `reason` 选文案）。
pub async fn purge(State(st): State<AppState>, body: Bytes) -> Response {
    let now = crate::adapters::now_ms();
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &["expect"]) {
        return e.into_response();
    }
    let Some(expected) = as_non_negative_int(o.get("expect")) else {
        return GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "expect 必须是一个非负整数：它是你在屏幕上看到的池大小",
        )
        .into_response();
    };
    let expected = usize::try_from(expected).unwrap_or(usize::MAX);

    let before = match all_or_500(&st, now).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if before.len() != expected {
        // 🔴 **一把都不删。** 面板上那个数字是运维**读过一眼**的；中间被别人导入 /
        //    补池铸出新 key 时，一次「我确认清掉这 3 把」会连带把第 4 把一起删掉，
        //    而那把 key 他从来没见过。
        let actual = before.len();
        return (
            StatusCode::CONFLICT,
            Json(PurgeConflict {
                error: GatewayInner {
                    r#type: "conflict",
                    message: format!(
                        "池子在你确认之前变了：你看到的是 {expected} 把，现在是 {actual} 把。一把都没有删，请刷新后重来"
                    ),
                },
                reason: POOL_SIZE_CHANGED,
                expected,
                actual,
            }),
        )
            .into_response();
    }

    let ids: Vec<String> = before.iter().map(|r| r.id.clone()).collect();
    if let Err(r) = delete_many(&st, &ids).await {
        return r;
    }
    // ④ **回读**：从存储读回来，不是拿 `before.len()` 反推。
    //    它顺带把「索引说空了、而存储里还躺着记录」那一档如实报出来 ——
    //    **那正是「清空之后真的空了吗」这个问题的唯一诚实答案**，
    //    写死一个 `0` 只是把 handler 的心愿印在屏幕上。
    let after = match all_or_500(&st, now).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    if !before.is_empty() {
        st.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_KEY_DELETED,
                "面板清空了整个 Key 池（每把 key 的用量历史随记录一起没了，没有第二份）",
            )
            // **只记数**：一次清空的 id 可能有几百个，而 `fields` 的值是标量。
            .with("count", before.len() as u64)
            .with("remaining", after.len() as u64),
        );
    }
    Json(PurgeBody {
        deleted: before.len(),
        remaining: after.len(),
        expected,
    })
    .into_response()
}

/// `GET /admin/api/keys/:id/reveal` —— 取回一把上游 key 的明文。
///
/// 🔴 **明文只走这一条端点，不进列表响应**，且**每次调用必留痕**。
///
/// 这一族是**别人的凭据**，我们本来就不得不可逆地存（要拿去打上游），
/// 五份 DEPLOY.md 从第一天就写着它「以明文落在 `store.json` 里，请按凭据处置」——
/// 所以这条端点**没有引入新的存储风险**，只是把已经存在的东西显式露出来。
pub async fn reveal(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let now = crate::adapters::now_ms();
    let all = match all_or_500(&st, now).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(rec) = all.into_iter().find(|r| r.id == id) else {
        return key_not_found(KeysEndpoint::Reveal);
    };
    st.logger.log(
        LogEntry::new(
            LogLevel::Warn,
            EVENT_KEY_REVEALED,
            "面板取回了一把上游 key 的明文",
        )
        // 🔴 **只记 id，绝不记明文。**
        .with("id", id),
    );
    // 🔴 走同一条状态机，**别在这里写死 `"ok"`**：写死的话，
    //    「空串不是明文」那条判定在这一族上就不成立了
    //    （一把被手工改坏成空串的记录会被当成明文交出去）。
    let kind = crate::core::admin::view::reveal::reveal_kind(Some(&rec.key), false);
    Json(RevealBody {
        kind: kind.as_str(),
        key: rec.key,
    })
    .into_response()
}

/// `DELETE /admin/api/keys/:id` —— `204` / `404` / `409 must_disable_first`。
///
/// 删除**不可撤销**（记录里那把 key 材料就此消失，没有任何地方还留着它），
/// 所以它是本组唯一一条带前置条件的写操作。
pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let now = crate::adapters::now_ms();
    let rec = match read_record(&st.storage, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(rec) = rec else {
        return key_not_found(KeysEndpoint::Delete);
    };
    if !deletable(&rec) {
        return (
            StatusCode::CONFLICT,
            Json(ConflictWithCode {
                error: AdminInner {
                    r#type: "conflict",
                    code: AdminErrorCode::MustDisableFirst.as_str(),
                    message: "请先停用这把 key 再删除（删除不可撤销，而停用随时可以撤销）"
                        .to_string(),
                },
                reason: MUST_DISABLE_FIRST,
            }),
        )
            .into_response();
    }
    if st.repo.delete(&id, now).await.is_err() {
        return GatewayError::internal().into_response();
    }
    st.logger.log(
        LogEntry::new(LogLevel::Warn, EVENT_KEY_DELETED, "面板删除了一把 key")
            // **只记 id（内容哈希）**，日志常被转发到第三方。
            .with("id", id)
            .with("wasEvicted", rec.evicted)
            .with("wasDisabled", is_disabled(&rec)),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// `PATCH /admin/api/keys/:id` —— 停用/启用、备注、清冷却、清 strikes、解除剔除、
/// 重置用量计数。
///
/// 🔴 **空对象 → `400 empty_patch`**：返回 200 就是一次「保存成功」而什么都没做。
pub async fn patch(State(st): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    let now = crate::adapters::now_ms();
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &PATCH_FIELDS) {
        return e.into_response();
    }
    if o.is_empty() {
        return AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::EmptyPatch,
            "至少要改一个字段",
        )
        .into_response();
    }
    let disabled = match opt_bool(&o, "disabled") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let note = match opt_note(&o) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    // 🔴 **后四个是动作不是状态：缺席与 `false` 都等于「这次不做」**，
    //    所以这里 `unwrap_or(false)` 是对的；而 `disabled` / `note` 上面那两格
    //    **绝不许**这么下坠（缺席 = 不动它，`false` = 设成 false）。
    let mut flags = [false; 4];
    for (slot, name) in
        flags
            .iter_mut()
            .zip(["clearCooldown", "clearStrikes", "unevict", "clearStats"])
    {
        match opt_bool(&o, name) {
            Ok(v) => *slot = v.unwrap_or(false),
            Err(e) => return e.into_response(),
        }
    }
    let [clear_cooldown, clear_strikes, unevict, clear_stats] = flags;

    let prev = match read_record(&st.storage, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(prev) = prev else {
        return key_not_found(KeysEndpoint::Patch);
    };

    let mut next = prev;
    if let Some(d) = disabled {
        next.disabled = Some(d);
    }
    match note {
        NotePatch::Absent => {}
        NotePatch::Clear => next.note = None,
        NotePatch::Set(s) => next.note = Some(s),
    }
    if clear_cooldown {
        next.cooldown_until = 0;
        next.cooldown_reason = None;
    }
    if clear_strikes {
        next.strikes = 0;
    }
    if unevict {
        next.evicted = false;
        next.evicted_reason = None;
    }
    if clear_stats {
        next.stats = Some(KeyStats::empty());
    }

    // 🔴 不传 `prev` —— 见文件头第 ③ 条。**这是本组最容易写错的一行**，
    //    而写错时面板照样显示「保存成功」。
    if st.repo.save(&next, None, now).await.is_err() {
        return GatewayError::internal().into_response();
    }

    // 一次 PATCH 最多打一条事件，**停用优先**：它是唯一一个让 key 停止服役的动作。
    // 只改备注不打事件 —— 备注不改变池子的任何行为，而事件板块说的是池子为什么变了。
    if disabled == Some(true) {
        st.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_KEY_DISABLED,
                "面板停用了一把 key（它仍然占着 targetKeys 的名额，不会触发补池）",
            )
            .with("id", next.id.clone()),
        );
    } else if disabled == Some(false) || clear_cooldown || clear_strikes || unevict {
        st.logger.log(
            LogEntry::new(
                LogLevel::Info,
                EVENT_KEY_RESTORED,
                "面板解除了一把 key 上的限制",
            )
            .with("id", next.id.clone())
            .with("enabled", disabled == Some(false))
            .with("clearCooldown", clear_cooldown)
            .with("clearStrikes", clear_strikes)
            .with("unevict", unevict),
        );
    }
    Json(OkBody { ok: true }).into_response()
}

/// `GET /admin/api/keys/:id/usage` —— **Tier-1，与 Tier-2 完全无关**。
///
/// 走 `repo.all()`（**零额外读**，与转发路径共用进程内快照），
/// 🔴 **Tier-2 关着时它照样可用**。
///
/// ⚠️ 404 走的是**不带 `code`** 的网关信封（02 §4.4 登记的刻意不一致）。
pub async fn usage(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let now = crate::adapters::now_ms();
    let all = match all_or_500(&st, now).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(rec) = all.iter().find(|r| r.id == id) else {
        return key_not_found(KeysEndpoint::Usage);
    };
    Json(KeyUsageBody {
        id,
        stats: stats_or_empty(rec.stats.as_ref()),
        approximate: true,
        generated_at: now,
    })
    .into_response()
}

/// 「这条端点不收任何选项」那道闸：空体放行、`{}` 放行，其余一律 400。
///
/// ⚠️ **空体必须放行**：面板与枚举式鉴权矩阵都是**不带体**调这条 POST 的。
/// ⚠️ **不静默忽略请求体**：`{"model":"agnes-2.0-flash"}` 这种「我以为能指定模型」
/// 的写法在宽松实现下是一次「点了、但完全没按你想的那样发生」的静默误操作，
/// 而面板会如实显示成功。
fn reject_any_body(body: &Bytes, what: &str) -> Result<(), GatewayError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| {
        GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "请求体不是合法的 JSON",
        )
    })?;
    let Value::Object(o) = v else {
        return Err(GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "请求体必须是一个 JSON 对象",
        ));
    };
    if o.is_empty() {
        return Ok(());
    }
    let extra = o.keys().cloned().collect::<Vec<_>>().join(", ");
    Err(GatewayError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        format!("{what}不接受任何参数，不认识的字段：{extra}"),
    ))
}

/// `POST /admin/api/keys/:id/verify` —— **真打一次上游**。
///
/// ── 六条约束，一条都不许省 ──────────────────────────────────────────────────
///
/// 1. 🔴 **一次存储写都不产生。** 设计上「显然」的两个答案各自是一颗自毁按钮：
///    失败记 strike ⇒ 面板上连点几次就能把一把好 key 打进长冷却甚至剔除；
///    成功清 strike / 解除 evicted ⇒ **「重新导入即解封」后门的同型复发**。
///    连 Tier-1 的 `stats` 也不写 —— `stats` 是「真实流量的证据」，
///    掺进人造探测就不再是证据了。
/// 2. 🔴 **一个字节的上游正文都不许回给面板。** 上游 401/403 的错误体恰恰是各家 API
///    最爱回显 key 片段的地方。只回结构化结果 `{ok,status,latencyMs,reason}`。
///    ⚠️ **响应体不是唯一的出口，第二条是事件环**（它能整份下载）——
///    所以这条 handler **一条事件都不打**，是结构性的，不是靠「记得别写」。
/// 3. 🔴 **走注入的 `Fetcher` 端口，不许裸 `reqwest`**：否则测试桩不掉、
///    每跑一次判据就真打一次外网。
/// 4. **自带超时**：它不经 `dispatch()`，那套超时一样都没有。档位取
///    `upstream_timeout_ms`（首字节档，默认 8 秒）—— 验活只要一个响应头就够了，
///    给它同步档（默认 2 分钟）等于让面板挂两分钟。
/// 5. 🔴 **直读存储，不读进程内快照**：「刚改过就想验一下」正是运维点这颗按钮的主要场景。
/// 6. **护栏的 kind 是 `verify:<id>`，逐把 key 一个槽位**：全局粒度下
///    「验 A」会把「验 B」一起挡掉一个最小间隔，20 把 key 逐个验要串行等 60 秒。
///    ⚠️ 它**必须排在 404 之后**，否则随机 id 就能往护栏那张表里无限灌键。
///    ⚠️ **放开走 [`ProbeLease`] 的 drop，不是 `send()` 之后那一行**：客户端一断开，
///    axum 直接 drop 掉这个 future，`await` 之后的每一行都不会再执行 ⇒
///    写在那里的 `release` 会让这把 key 的验活按钮**永久**点不动。
pub async fn verify(State(st): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    // 校验放在最前面：**一次存储读都不产生、一格护栏都不消费**。
    if let Err(e) = reject_any_body(&body, "验活") {
        return e.into_response();
    }
    let rec = match read_record(&st.storage, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(rec) = rec else {
        return key_not_found(KeysEndpoint::Verify);
    };

    let guard = probe_guard();
    let kind = format!("verify:{id}");
    // 🔴 占到的那一格握在一张 [`ProbeLease`] 里 ⇒ **放开挂在 drop 上**，
    //    「请求被取消」那一支也覆盖得到（`await` 之后的行在那一支里根本不执行）。
    let lease = match guard.acquire(&kind, crate::adapters::now_ms()) {
        Ok(lease) => lease,
        Err(refused) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ProbeBusy {
                    error: GatewayInner {
                        r#type: "rate_limit_error",
                        message: refused.message().to_string(),
                    },
                    // `expect` 不可达：占到了的那一档走的是上面 `Ok` 那条支。
                    reason: refused.reason().expect("被拒的那两档各自都有 reason"),
                }),
            )
                .into_response();
        }
    };

    let cfg = st.config.current();
    // 计时起点：**只读这一次**，`latency_ms` 因此恰好是「这一次读」到「收尾那一次读」之间。
    let started_at = crate::adapters::now_ms();
    // 🔴 探测固定用 openai 那一条，模型取 `MODEL_CATALOG[0]`（§9.14 逐字）。
    //    **整份探针从协议目录现取，一个字面量都不手搓**：路径与样例体的真源是
    //    `PROTOCOLS` / `MODEL_CATALOG`，而隔壁 `models::test_with` 走的就是这张表 ——
    //    各手写一份的话两边今天恰好一致，改了目录而没改这里时**不会有任何信号**，
    //    面板上「测模型」与「验活」会一个绿一个红。
    //    ⚠️ 目录里那一格是**对上游**的路径（不是对外的 `/v1/chat/completions`）——
    //    两者混用的后果是对所有 key 一律报失败，而面板会显示「整池 key 都死了」。
    let proto = protocol_by_id(ProtocolId::Openai);
    let sample = proto.sample(MODEL_CATALOG[0].id);
    let req = OutboundRequest {
        method: proto.method.to_string(),
        url: format!(
            "{}{}",
            rec.upstream_base_url(&cfg.agnes_base_url),
            proto.upstream_path
        ),
        headers: vec![
            (
                header::CONTENT_TYPE.as_str().to_string(),
                "application/json".to_string(),
            ),
            (
                header::AUTHORIZATION.as_str().to_string(),
                format!("Bearer {}", rec.key),
            ),
        ],
        body: Some(Bytes::from(sample.to_string().into_bytes())),
        timeout_ms: cfg.upstream_timeout_ms,
    };
    let sent = st.fetcher.send(req).await;
    // **这一句只是把放开的时机钉在「上游一回来就放」**，不是放开的保证 ——
    // 保证在 `lease` 的 drop 上（🔴 删掉这一行只会把放开推迟到函数收尾，不会漏放）。
    drop(lease);
    let latency_ms = crate::adapters::now_ms() - started_at;

    match sent {
        Ok(res) => {
            let status = res.status;
            // ⚠️ **响应体在这里被丢掉**（`res` 出作用域，流被 drop）——
            //    它同时是约束 2 的**物理**保证：正文从来没有被读进来过。
            drop(res);
            Json(VerifyBody {
                ok: (200..300).contains(&status),
                status: Some(status),
                latency_ms,
                reason: None,
            })
            .into_response()
        }
        Err(e) => Json(VerifyBody {
            ok: false,
            status: None,
            latency_ms,
            // **`reason` 是机器可读的 code，不是异常消息**：异常消息里可能带上游 URL。
            reason: Some(match e {
                FetchError::TimeoutBeforeHeaders => "timeout",
                FetchError::Connect(_) | FetchError::BodyInterrupted(_) => "network_error",
            }),
        })
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::app::build_router;
    use crate::server::test_support::{
        RecordingLogger, ScriptedFetcher, TestApp, UpstreamReply, state_with, state_with_storage,
    };
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::json;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    struct Harness {
        app: Router,
        state: AppState,
        recorded: Arc<RecordingLogger>,
    }

    async fn harness_with(
        storage: Arc<dyn Storage>,
        fetcher: Option<Arc<dyn crate::ports::fetcher::Fetcher>>,
    ) -> Harness {
        let TestApp {
            mut state,
            recorded,
            ..
        } = match fetcher {
            Some(f) => state_with(storage, f).await,
            None => state_with_storage(storage).await,
        };
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state.clone());
        Harness {
            app,
            state,
            recorded,
        }
    }

    async fn harness() -> Harness {
        harness_with(Arc::new(MemoryStorage::new()), None).await
    }

    async fn call(
        h: &Harness,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-admin-key", ADMIN);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        let req = b
            .body(body.map_or_else(Body::empty, |s| Body::from(s.to_string())))
            .unwrap();
        let res = h.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn get(h: &Harness, uri: &str) -> (StatusCode, Value) {
        let (s, b) = call(h, Method::GET, uri, None).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    async fn post(h: &Harness, uri: &str, body: &str) -> (StatusCode, Value) {
        let (s, b) = call(h, Method::POST, uri, Some(body)).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    async fn patch_req(h: &Harness, uri: &str, body: &str) -> (StatusCode, Value) {
        let (s, b) = call(h, Method::PATCH, uri, Some(body)).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    /// 往池子里放一把 key，返回它的 id。
    async fn seed(h: &Harness, key: &str) -> String {
        h.state.repo.add(key, 1_000).await.unwrap().id
    }

    // ── 信封归属 ───────────────────────────────────────────────────────────

    /// 🔴🔴 **哪条端点用哪种信封，逐条钉死**（02 §4.4 那处刻意的不一致）。
    ///
    /// 期望值**手写**，不从被测的 `match` 现算 —— 后者是同义反复。
    /// 回归对象：顺手把 `usage` / `verify` 的 404「统一」成带码的那一份 ⇒
    /// 设置页那几支的文案选择当场失配。
    #[test]
    fn envelope_assignment_is_pinned_per_endpoint() {
        use Envelope::{Admin, Gateway};
        use KeysEndpoint as E;
        let expected: [(E, Option<Envelope>, BodyGate); 9] = [
            (E::List, None, BodyGate::None),
            (E::Import, None, BodyGate::AdminJson),
            (E::Bulk, None, BodyGate::AdminJson),
            (E::Purge, None, BodyGate::AdminJson),
            (E::Reveal, Some(Admin), BodyGate::None),
            (E::Delete, Some(Admin), BodyGate::None),
            (E::Patch, Some(Admin), BodyGate::AdminJson),
            (E::Usage, Some(Gateway), BodyGate::None),
            (E::Verify, Some(Gateway), BodyGate::NoParamsGateway),
        ];
        assert_eq!(expected.len(), E::ALL.len(), "表长与真源对不上");
        for (ep, env, gate) in expected {
            assert_eq!(ep.key_not_found_envelope(), env, "{ep:?} 的 404 信封");
            assert_eq!(ep.body_gate(), gate, "{ep:?} 的请求体闸");
        }
    }

    /// 🔴 **四条路径 + 方法逐字钉死**，且 `/keys/bulk`、`/keys/purge`
    /// 必须是**字面段**（不是 `{id}` 的一个取值）。
    #[test]
    fn endpoint_paths_and_methods_are_pinned_literals() {
        use KeysEndpoint as E;
        assert_eq!(E::List.subpath(), "/keys");
        assert_eq!(E::List.method(), "GET");
        assert_eq!(E::Import.subpath(), "/keys");
        assert_eq!(E::Import.method(), "POST");
        assert_eq!(E::Bulk.subpath(), "/keys/bulk");
        assert_eq!(E::Purge.subpath(), "/keys/purge");
        assert_eq!(E::Reveal.subpath(), "/keys/{id}/reveal");
        assert_eq!(E::Delete.subpath(), "/keys/{id}");
        assert_eq!(E::Delete.method(), "DELETE");
        assert_eq!(E::Patch.method(), "PATCH");
        assert_eq!(E::Usage.subpath(), "/keys/{id}/usage");
        assert_eq!(E::Verify.subpath(), "/keys/{id}/verify");
    }

    /// 🔴 **危险区那条路径是单一真源**：五语言 DEPLOY.md 的配额账逐份写着
    /// `KEYS_PURGE_PATH`，而路由注册用的是 `subpath()` —— 两者必须拼得回去。
    #[test]
    fn the_purge_path_constant_is_the_single_source() {
        assert_eq!(KEYS_PURGE_PATH, "/admin/api/keys/purge");
        assert_eq!(
            format!("{ADMIN_API_PREFIX}{}", KeysEndpoint::Purge.subpath()),
            KEYS_PURGE_PATH
        );
    }

    /// 分页档位与几个上限是对外契约（五语言 API.md 逐份写着这些数）。
    #[test]
    fn the_published_limits_are_pinned_literals() {
        assert_eq!(DEFAULT_SIZE, 20);
        assert_eq!(MAX_SIZE, 200);
        assert_eq!(MAX_IMPORT_KEYS, 200);
        assert_eq!(MAX_NOTE_LENGTH, 200);
        assert_eq!(BULK_EVENT_IDS_MAX, 20);
        assert_eq!(PROBE_MIN_INTERVAL_MS, 3_000);
        assert_eq!(
            PATCH_FIELDS,
            [
                "disabled",
                "note",
                "clearCooldown",
                "clearStrikes",
                "unevict",
                "clearStats"
            ]
        );
        assert_eq!(BULK_OPS, ["disable", "clearCooldown", "delete"]);
    }

    // ── GET /admin/api/keys ────────────────────────────────────────────────

    /// 🔴🔴 **列表响应体整段文本里都找不到明文 key。**
    ///
    /// 断言的是**整段响应体**而不是逐字段查：handler 将来多回一个字段时，
    /// 逐字段查的那种写法会静默漏掉。
    #[tokio::test]
    async fn the_list_response_never_carries_any_plaintext_key() {
        let h = harness().await;
        seed(&h, "sk-super-secret-value-0001").await;
        seed(&h, "sk-super-secret-value-0002").await;
        let (status, body) = call(&h, Method::GET, "/admin/api/keys", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body.contains("super-secret"),
            "明文泄漏进了列表响应：{body}"
        );
        assert!(body.contains("sk-su…0001"), "掩码该在：{body}");
    }

    /// 列表的形状逐字段钉住（面板契约）。
    #[tokio::test]
    async fn the_list_shape_matches_the_published_contract() {
        let h = harness().await;
        seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        let (status, v) = get(&h, "/admin/api/keys").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["total"], json!(1));
        assert_eq!(v["page"], json!(1));
        assert_eq!(v["size"], json!(20));
        assert_eq!(v["pages"], json!(1));
        assert_eq!(v["approximate"], json!(true));
        assert!(v["generatedAt"].as_i64().unwrap() > 0);
        assert_eq!(
            v["counts"],
            json!({ "all": 1, "fresh": 1, "cooling": 0, "evicted": 0, "disabled": 0 })
        );
        let item = &v["items"][0];
        assert_eq!(item["seq"], json!(1));
        assert_eq!(item["bucket"], json!("fresh"));
        assert_eq!(item["disabled"], json!(false));
        assert_eq!(item["note"], json!(null));
        assert_eq!(item["stats"]["requests"], json!(0));
    }

    /// 🔴 **一把「既被剔除、又被手动停用」的 key，它的行内动作不许被 `bucket` 判死。**
    ///
    /// `bucket` 是 `disabled > evicted > cooling > fresh` 的**优先级投影** ⇒ 这把 key
    /// 的 `bucket` 是 `disabled`，而它**同时**还在冷却、还被剔除着。
    /// 拿 `bucket` 反推三个事实的后果在 TS 侧真实发生过：
    /// 「清冷却」与「解除剔除」两颗按钮**永远禁用**，而后端的 PATCH 照样接受
    /// —— **按钮说「不能」，后端说「能」。**
    ///
    /// ⚠️ 这一格同时钉住 `cooldownRemainingMs` **刻意用另一条判据**：那一格问的是
    /// 「这一行该不该显示倒计时」（`cooling` 档才显示），所以这把 key 上它是 `null`，
    /// 而 `canClearCooldown` 是 `true`。**两者不同不是 bug。**
    #[tokio::test]
    async fn row_actions_read_the_facts_not_the_bucket_projection() {
        let h = harness().await;
        let id = seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        // 直接写记录：同时停用 + 剔除 + 还在冷却 + 有 strikes。
        let mut rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        rec.disabled = Some(true);
        rec.evicted = true;
        rec.cooldown_until = crate::adapters::now_ms() + 600_000;
        rec.strikes = 3;
        h.state
            .repo
            .save(&rec, None, crate::adapters::now_ms())
            .await
            .unwrap();

        let (_, v) = get(&h, "/admin/api/keys").await;
        let item = &v["items"][0];
        assert_eq!(item["bucket"], json!("disabled"), "投影确实压过了另外两档");
        let row = &v["view"]["rows"][0];
        assert_eq!(row["id"], item["id"], "逐行带 id，面板不必靠下标信任");
        assert_eq!(
            row["canClearCooldown"],
            json!(true),
            "后端 PATCH 照样清得掉"
        );
        assert_eq!(row["canUnevict"], json!(true), "它确实被剔除着");
        assert_eq!(row["canClearStrikes"], json!(true));
        assert_eq!(row["canDelete"], json!(true));
        assert_eq!(
            row["cooldownRemainingMs"],
            json!(null),
            "倒计时只画在 cooling 档上 —— 与 canClearCooldown 刻意不同源"
        );
        assert_eq!(row["badgeClass"], json!("badge"), "「已停用」是中性灰");
        assert_eq!(row["toggleLabelKey"], json!("keys.action.enable"));
    }

    /// 一把干净的 key：四颗动作**全灭**，删除也不许放行。
    ///
    /// 回归对象：`canDelete` 写成「或」以外的东西 ⇒ 面板上一颗亮着的删除按钮，
    /// 点下去是 409。
    #[tokio::test]
    async fn a_healthy_key_offers_no_row_actions_at_all() {
        let h = harness().await;
        seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        let (_, v) = get(&h, "/admin/api/keys").await;
        let row = &v["view"]["rows"][0];
        assert_eq!(
            row["canDelete"],
            json!(false),
            "🔴 与 409 那条判据同一份实现"
        );
        assert_eq!(row["canClearCooldown"], json!(false));
        assert_eq!(row["canUnevict"], json!(false));
        assert_eq!(row["canClearStrikes"], json!(false));
        assert_eq!(row["cooldownRemainingMs"], json!(null));
        assert_eq!(row["badgeClass"], json!("badge badge-ok"));
        assert_eq!(row["bucketLabelKey"], json!("keys.bucket.fresh"));
        assert_eq!(row["toggleLabelKey"], json!("keys.action.disable"));
    }

    /// 🔴 **`canDelete` 与 409 那条判据是同一份实现**，而这一格是它们的对表。
    ///
    /// 回归对象：两处各写一份 ⇒ 面板的按钮与后端的前置校验在某一档上分叉，
    /// 而分叉那一档正好是运维最常碰的（手动停用过、但从没被系统剔除）。
    #[tokio::test]
    async fn the_delete_button_and_the_409_agree_on_every_combination() {
        let h = harness().await;
        // (disabled, evicted) 四种组合，逐个建一把 key。
        for (i, (disabled, evicted)) in [(false, false), (true, false), (false, true), (true, true)]
            .into_iter()
            .enumerate()
        {
            let id = seed(&h, &format!("sk-combo-number-{i:04}")).await;
            let mut rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
            rec.disabled = Some(disabled);
            rec.evicted = evicted;
            h.state
                .repo
                .save(&rec, None, crate::adapters::now_ms())
                .await
                .unwrap();
        }
        let (_, v) = get(&h, "/admin/api/keys").await;
        for row in v["view"]["rows"].as_array().expect("有行") {
            let id = row["id"].as_str().expect("id");
            let can = row["canDelete"].as_bool().expect("canDelete");
            let (status, _) =
                call(&h, Method::DELETE, &format!("/admin/api/keys/{id}"), None).await;
            let accepted = status != StatusCode::CONFLICT;
            assert_eq!(
                can, accepted,
                "{id}: 按钮说 {can}，而 DELETE 回的是 {status}"
            );
        }
    }

    /// 🔴 **「池子里还没有 key」与「筛不出来」是两句话，指向相反的动作。**
    ///
    /// 回归对象：两者合成一句「暂无数据」，运维不知道该去导入还是去放宽筛选。
    /// ⚠️ 顺带钉住空列表时**分页信息一并复位** —— 否则屏幕上挂着一份
    /// 已经不存在的数据的页码。
    #[tokio::test]
    async fn the_empty_list_message_distinguishes_an_empty_pool_from_an_empty_filter() {
        let h = harness().await;
        let (_, empty) = get(&h, "/admin/api/keys").await;
        assert_eq!(empty["view"]["listMessageKey"], json!("keys.empty"));
        assert_eq!(empty["view"]["pager"]["info"], json!(null));
        assert_eq!(empty["view"]["pager"]["prevDisabled"], json!(true));
        assert_eq!(empty["view"]["pager"]["nextDisabled"], json!(true));

        seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        let (_, filtered) = get(&h, "/admin/api/keys?bucket=evicted").await;
        assert_eq!(
            filtered["view"]["listMessageKey"],
            json!("keys.noMatch"),
            "池子里有 key，只是这一档没有"
        );
        assert_eq!(filtered["view"]["pager"]["info"], json!(null));

        let (_, shown) = get(&h, "/admin/api/keys").await;
        assert_eq!(
            shown["view"]["listMessageKey"],
            json!(null),
            "有条目就显示表格"
        );
        assert_eq!(
            shown["view"]["pager"]["info"],
            json!({ "page": 1, "pages": 1, "total": 1 })
        );
    }

    /// 分页两端的按钮状态跟着页码走，**且 `view.rows` 与 `items` 同序同长**。
    #[tokio::test]
    async fn the_pager_view_tracks_the_page_and_stays_aligned_with_items() {
        let h = harness().await;
        for i in 0..5 {
            seed(&h, &format!("sk-key-number-{i:04}")).await;
        }
        for (page, prev, next) in [(1, true, false), (2, false, false), (3, false, true)] {
            let (_, v) = get(&h, &format!("/admin/api/keys?size=2&page={page}")).await;
            assert_eq!(
                v["view"]["pager"]["prevDisabled"],
                json!(prev),
                "page={page}"
            );
            assert_eq!(
                v["view"]["pager"]["nextDisabled"],
                json!(next),
                "page={page}"
            );
            let items = v["items"].as_array().expect("items");
            let rows = v["view"]["rows"].as_array().expect("rows");
            assert_eq!(items.len(), rows.len(), "同长");
            for (it, row) in items.iter().zip(rows) {
                assert_eq!(it["id"], row["id"], "同序");
            }
        }
    }

    /// 冷却中的那一行：`cooldownRemainingMs` 用**服务端**的时钟算，
    /// 而 `generatedAt` 是同一次 `now_ms()` ⇒ 两者恒自洽。
    ///
    /// ⚠️ TS 侧这个数是面板拿浏览器时钟减出来的，时钟有偏差时会算出负数或虚高的
    /// 剩余时间 —— **上移之后那条偏差从结构上消失了**。
    #[tokio::test]
    async fn the_cooldown_countdown_is_computed_against_the_server_clock() {
        let h = harness().await;
        let id = seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        let mut rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        rec.cooldown_until = crate::adapters::now_ms() + 600_000;
        h.state
            .repo
            .save(&rec, None, crate::adapters::now_ms())
            .await
            .unwrap();

        let (_, v) = get(&h, "/admin/api/keys").await;
        assert_eq!(v["items"][0]["bucket"], json!("cooling"));
        let row = &v["view"]["rows"][0];
        let remaining = row["cooldownRemainingMs"].as_i64().expect("有倒计时");
        let generated = v["generatedAt"].as_i64().expect("generatedAt");
        let until = v["items"][0]["cooldownUntil"]
            .as_i64()
            .expect("cooldownUntil");
        assert_eq!(
            remaining,
            until - generated,
            "🔴 参照时刻必须是响应里那个 generatedAt，不是别的什么钟"
        );
        assert_eq!(row["badgeClass"], json!("badge badge-warn"));
        assert_eq!(row["canClearCooldown"], json!(true));
    }

    /// 🔴 **`counts` 永远按整池算，不受筛选影响。**
    ///
    /// 回归对象：拿筛完的集合去算 ⇒ 恒等于当前这一档的条数，另外三档全是 0，
    /// 于是筛选器旁边那几个数永远说「别的档一条都没有」。
    #[tokio::test]
    async fn counts_are_always_computed_over_the_whole_pool() {
        let h = harness().await;
        let a = seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        seed(&h, "sk-bbbbbbbbbbbbbbbb").await;
        // 把 a 停用掉。
        let (s, _) = patch_req(&h, &format!("/admin/api/keys/{a}"), r#"{"disabled":true}"#).await;
        assert_eq!(s, StatusCode::OK);

        let (_, v) = get(&h, "/admin/api/keys?bucket=disabled").await;
        assert_eq!(v["total"], json!(1), "筛选后只剩 1 条");
        assert_eq!(
            v["counts"],
            json!({ "all": 2, "fresh": 1, "cooling": 0, "evicted": 0, "disabled": 1 }),
            "counts 必须按整池算"
        );
    }

    /// `bucket` 不是合法档位时**整个忽略**（而不是筛出 0 条）。
    #[tokio::test]
    async fn an_unknown_bucket_filter_is_ignored_entirely() {
        let h = harness().await;
        seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        let (_, v) = get(&h, "/admin/api/keys?bucket=nonsense").await;
        assert_eq!(v["total"], json!(1), "不认识的档位不许把结果筛空");
        let (_, v) = get(&h, "/admin/api/keys?bucket=evicted").await;
        assert_eq!(v["total"], json!(0), "认识的档位要真的筛");
    }

    /// 分页：`size` / `page` 非法值回落，`pages = max(1, ceil(total/size))`。
    ///
    /// 🔴 **`page` 越界是「回落 1」，不是「夹到最后一页」**（02 §9.6 逐字：「越界回落 1」）。
    ///
    /// ⚠️ 夹具必须是 **`pages > 1`** 的形状，这一条才有观测量：`pages == 1` 时
    /// 「回落 1」与「夹到最后一页」**得到的是同一个数**，两种语义在响应里分不开。
    /// 变异复检做过：`clamp(1, pages)` 在只有 `size=0&page=99`（⇒ 回落 20 ⇒ 1 页）
    /// 那一句时**全绿逃逸**。
    #[tokio::test]
    async fn paging_falls_back_on_bad_values_and_pages_is_ceil() {
        let h = harness().await;
        for i in 0..5 {
            seed(&h, &format!("sk-key-number-{i:04}")).await;
        }
        let (_, v) = get(&h, "/admin/api/keys?size=2").await;
        assert_eq!(v["pages"], json!(3), "5 条、每页 2 条 ⇒ 3 页");
        assert_eq!(v["items"].as_array().unwrap().len(), 2);

        // 🔴 越界 ⇒ 回落 1。**`pages` 是 3，所以「夹到最后一页」会得到 3** ——
        //    两种语义在这里才真的分得开，连带 `items` 也必须是第一页那一段。
        let (_, v) = get(&h, "/admin/api/keys?size=2&page=99").await;
        assert_eq!(v["pages"], json!(3), "夹具前提：这一次必须有不止一页");
        assert_eq!(v["page"], json!(1), "🔴 越界回落 1，不是夹到最后一页");
        let seqs: Vec<i64> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["seq"].as_i64().unwrap())
            .collect();
        assert_eq!(seqs, vec![1, 2], "回落之后给的必须真的是第一页那一段");

        // size 非法 ⇒ 回落默认；page 越界 ⇒ 回落 1。
        let (_, v) = get(&h, "/admin/api/keys?size=0&page=99").await;
        assert_eq!(v["size"], json!(20));
        assert_eq!(v["page"], json!(1));
        let (_, v) = get(&h, "/admin/api/keys?size=99999").await;
        assert_eq!(v["size"], json!(20), "超上限也回落默认");
        // 空池仍然是 1 页。
        let h2 = harness().await;
        let (_, v) = get(&h2, "/admin/api/keys").await;
        assert_eq!(v["pages"], json!(1));
        assert_eq!(v["total"], json!(0));
    }

    /// 第二页取的是**后面那一段**（分页真的在切，不是每页都回头几条）。
    #[tokio::test]
    async fn the_second_page_returns_the_next_slice_in_seq_order() {
        let h = harness().await;
        for i in 0..5 {
            seed(&h, &format!("sk-key-number-{i:04}")).await;
        }
        let (_, p1) = get(&h, "/admin/api/keys?size=2&page=1").await;
        let (_, p2) = get(&h, "/admin/api/keys?size=2&page=2").await;
        let seqs = |v: &Value| -> Vec<i64> {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["seq"].as_i64().unwrap())
                .collect()
        };
        assert_eq!(seqs(&p1), vec![1, 2]);
        assert_eq!(seqs(&p2), vec![3, 4]);
    }

    /// 🔴 **搜索绝不匹配明文**（否则列表就是一台慢速预言机）。
    #[tokio::test]
    async fn search_never_matches_the_plaintext() {
        let h = harness().await;
        seed(&h, "sk-aaaa-MIDDLE-SECRET-zzzz").await;
        let (_, v) = get(&h, "/admin/api/keys?q=MIDDLE").await;
        assert_eq!(v["total"], json!(0), "🔴 明文中段绝不许可搜");
        let (_, v) = get(&h, "/admin/api/keys?q=sk-aa").await;
        assert_eq!(v["total"], json!(1), "掩码露出来的那一段可搜");
    }

    /// 🔴 **面板轮询零额外存储读**：连打 20 次列表，存储调用次数一次都不涨。
    ///
    /// 回归对象：handler 里顺手加一次 `list()` 或逐条 `get()`，
    /// 而 `FileStorage` 的 `list` 是一次整份文件读 + 解析。
    #[tokio::test]
    async fn polling_the_list_does_not_hit_storage_again() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        seed(&h, "sk-aaaaaaaaaaaaaaaa").await;
        // 先热一次快照。
        let (s, _) = get(&h, "/admin/api/keys").await;
        assert_eq!(s, StatusCode::OK);
        let before = counts.snapshot();
        for _ in 0..20 {
            let (s, _) = get(&h, "/admin/api/keys").await;
            assert_eq!(s, StatusCode::OK);
        }
        assert_eq!(counts.snapshot(), before, "列表轮询不许各自去读一遍存储");
    }

    // ── POST /admin/api/keys（导入）────────────────────────────────────────

    /// 导入的三个数组 + `reset`：形状与口径逐条钉住。
    ///
    /// 🔴 `invalid` 装的是**输入里的位置**（1 基），**不是明文、也不是掩码**。
    #[tokio::test]
    async fn upstream_selection_invalid_imports_never_write() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        for route in [
            json!("unknown"),
            json!(42),
            json!("https://other.example/v1"),
        ] {
            let before = counts.snapshot();
            let body = json!({"keys":["sk-invalid-upstream-aaaa"], "upstream":route}).to_string();
            let (status, response) = post(&h, "/admin/api/keys", &body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(response["error"]["code"], "invalid_upstream");
            assert_eq!(counts.snapshot().1, before.1);
        }
    }

    #[tokio::test]
    async fn upstream_selection_defaults_to_international_and_preserves_duplicates() {
        let h = harness().await;
        for (i, selector) in [None, Some(Value::Null)].into_iter().enumerate() {
            let key = format!("sk-upstream-default-{i}-aaaa");
            let mut body = json!({"keys":[key]});
            if let Some(selector) = selector {
                body["upstream"] = selector;
            }
            let (status, response) = post(&h, "/admin/api/keys", &body.to_string()).await;
            assert_eq!(status, StatusCode::OK);
            let id = response["added"][0].as_str().unwrap();
            for reset in [false, true] {
                let body =
                    json!({"keys":[key],"upstream":"china","resetExisting":reset}).to_string();
                let (status, _) = post(&h, "/admin/api/keys", &body).await;
                assert_eq!(status, StatusCode::OK);
                let raw = h
                    .state
                    .storage
                    .get(&format!("key:{id}"))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(raw["upstream"], "international");
            }
        }
    }

    #[tokio::test]
    async fn upstream_selection_is_saved_and_listed_for_each_import() {
        let h = harness().await;
        for (i, route) in ["international", "internationalBackup", "china"]
            .iter()
            .enumerate()
        {
            let key = format!("sk-test-upstream-route-{i}-aaaa");
            let body = serde_json::json!({"keys":[key], "upstream":route}).to_string();
            let (status, imported) = post(&h, "/admin/api/keys", &body).await;
            assert_eq!(status, StatusCode::OK, "{imported}");
            let id = imported["added"][0].as_str().unwrap();
            let raw = h
                .state
                .storage
                .get(&format!("key:{id}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(raw["upstream"], *route);
            let (_, listed) = get(&h, "/admin/api/keys").await;
            let item = listed["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == id)
                .unwrap();
            assert_eq!(item["upstream"], *route);
            assert!(!listed.to_string().contains(&key));
        }
    }

    #[tokio::test]
    async fn import_reports_added_duplicated_and_positions_of_invalid_lines() {
        let h = harness().await;
        let (status, v) = post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-good-one-aaaa","有中文","sk-good-one-aaaa","","sk-good-two-bbbb"]}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["added"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["duplicated"].as_array().unwrap().len(),
            1,
            "同一批里粘第二遍也算一条重复"
        );
        assert_eq!(v["invalid"], json!([2]), "位置是 1 基，且空行不占位置");
        assert_eq!(v["reset"], json!(0));
        // 明文一个字节都不许回。
        let (_, raw) = call(
            &h,
            Method::POST,
            "/admin/api/keys",
            Some(r#"{"keys":["sk-another-secret-cccc"]}"#),
        )
        .await;
        assert!(
            !raw.contains("another-secret"),
            "导入回执里出现了明文：{raw}"
        );
    }

    /// 🔴 **`reset` 与 `duplicated.len()` 不是一个数。**
    ///
    /// 回归对象：面板显示 `duplicated.length` 就是在撒谎 —— 本批**新建**的那把
    /// 被粘第二遍也算重复，但它谈不上「被重置」。
    #[tokio::test]
    async fn reset_is_not_the_same_number_as_duplicated_length() {
        let h = harness().await;
        // 同一批里粘两遍一把**全新**的 key：duplicated = 1，而 reset 必须是 0。
        let (_, v) = post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-brand-new-aaaa","sk-brand-new-aaaa"],"resetExisting":true}"#,
        )
        .await;
        assert_eq!(v["duplicated"].as_array().unwrap().len(), 1);
        assert_eq!(v["reset"], json!(0), "本批新建的那把不算「被重置」");

        // 再导一次同一把（这次它是「本批之前就在池子里」的）⇒ reset = 1。
        let (_, v) = post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-brand-new-aaaa"],"resetExisting":true}"#,
        )
        .await;
        assert_eq!(v["duplicated"].as_array().unwrap().len(), 1);
        assert_eq!(v["reset"], json!(1));
    }

    /// 🔴 **重复导入默认不覆盖**（「重复导入即解封」那个后门被堵死）。
    #[tokio::test]
    async fn a_repeated_import_never_silently_resets_an_evicted_key() {
        let h = harness().await;
        let id = seed(&h, "sk-evicted-one-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    evicted: true,
                    evicted_reason: Some("upstream 401".into()),
                    strikes: 3,
                    ..rec
                },
                None,
                2_000,
            )
            .await
            .unwrap();

        let (_, v) = post(&h, "/admin/api/keys", r#"{"keys":["sk-evicted-one-aaaa"]}"#).await;
        assert_eq!(v["duplicated"].as_array().unwrap().len(), 1);
        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert!(after.evicted, "🔴 不勾选时重复导入绝不许解封");
        assert_eq!(after.strikes, 3);

        // 勾了才重置，而且是**知情动作**。
        let (_, _) = post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-evicted-one-aaaa"],"resetExisting":true}"#,
        )
        .await;
        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert!(!after.evicted);
        assert_eq!(after.strikes, 0);
    }

    /// 🔴 `resetExisting` **只重置失败态**：`disabled` / `added_at` / `note` / `stats`
    /// 一个都不许动。
    ///
    /// 回归对象：顺手把 `disabled` 也翻回去 = 把「停用一把可疑 key」这个安全动作静默撤销。
    #[tokio::test]
    async fn reset_existing_never_touches_disabled_added_at_note_or_stats() {
        let h = harness().await;
        let id = seed(&h, "sk-reset-scope-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        let before = KeyRecord {
            disabled: Some(true),
            note: Some("换了下游客户，先停着".into()),
            stats: Some(KeyStats {
                requests: 7,
                success: 5,
                failed: 2,
                ..KeyStats::empty()
            }),
            cooldown_until: 9_999_999,
            strikes: 4,
            evicted: true,
            ..rec
        };
        h.state.repo.save(&before, None, 2_000).await.unwrap();

        post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-reset-scope-aaaa"],"resetExisting":true}"#,
        )
        .await;

        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert_eq!(after.disabled, Some(true), "🔴 人的决定不许被一次粘贴撤销");
        assert_eq!(after.added_at, before.added_at, "来历不许被刷新");
        assert_eq!(after.note, before.note);
        assert_eq!(
            after.stats.as_ref().unwrap().requests,
            7,
            "用量是历史不是失败态"
        );
        // 失败态确实被清了。
        assert_eq!(after.cooldown_until, 0);
        assert_eq!(after.strikes, 0);
        assert!(!after.evicted);
    }

    /// 🔴 **一次导入 M 把 key 只产生 `M + 1` 次 put，且零 `list()`。**
    ///
    /// 回归对象：退化成循环调 `repo.add()` ⇒ 每把各写一次索引 ⇒ 单价翻三倍，
    /// 而 `FileStorage` 每次 put 都重写整个 `store.json`。
    /// **结果是对的、只是贵三倍 ⇒ 判据只能是计数。**
    #[tokio::test]
    async fn importing_twenty_keys_costs_exactly_twenty_one_puts_and_zero_lists() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        // 先把索引建起来（否则第一次导入会为「索引缺失」多一次 list，那是另一条路径）。
        seed(&h, "sk-seed-for-index-0").await;
        let before = counts.snapshot();

        let keys: Vec<String> = (0..20).map(|i| format!("sk-bulk-import-{i:04}")).collect();
        let body = serde_json::to_string(&json!({ "keys": keys })).unwrap();
        let (status, v) = post(&h, "/admin/api/keys", &body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["added"].as_array().unwrap().len(), 20);

        let after = counts.snapshot();
        assert_eq!(after.1 - before.1, 21, "20 条记录 + 1 次索引，一次都不许多");
        assert_eq!(after.3 - before.3, 0, "导入路径上不许有 list()");
    }

    /// **重复导入一次盘都不落**（索引集合不变 ⇒ 短路）。
    #[tokio::test]
    async fn importing_the_same_keys_again_writes_no_index() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        post(&h, "/admin/api/keys", r#"{"keys":["sk-idempotent-aaaa"]}"#).await;
        let before = counts.snapshot();
        let (_, v) = post(&h, "/admin/api/keys", r#"{"keys":["sk-idempotent-aaaa"]}"#).await;
        assert_eq!(v["added"], json!([]));
        assert_eq!(
            counts.snapshot().1,
            before.1,
            "不勾重置的重复导入必须零 put"
        );
    }

    /// 导入的四档拒绝：未知字段 / 非字符串数组 / 超上限 / 畸形 JSON。
    #[tokio::test]
    async fn import_rejects_with_the_documented_codes() {
        let h = harness().await;
        let cases: [(&str, &str); 5] = [
            (r#"{"keys":[],"nope":1}"#, "unknown_field"),
            (r#"{"keys":"not-an-array"}"#, "keys_not_a_string_array"),
            (r#"{"keys":[1,2]}"#, "keys_not_a_string_array"),
            (r#"{oops"#, "bad_json"),
            (r#"[1,2]"#, "body_not_an_object"),
        ];
        for (body, code) in cases {
            let (s, v) = post(&h, "/admin/api/keys", body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], json!(code), "{body}");
        }
        // 超上限：带 `params.max`。
        let keys: Vec<String> = (0..=MAX_IMPORT_KEYS)
            .map(|i| format!("sk-too-many-{i:05}"))
            .collect();
        let body = serde_json::to_string(&json!({ "keys": keys })).unwrap();
        let (s, v) = post(&h, "/admin/api/keys", &body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], json!("too_many_import_keys"));
        assert_eq!(v["error"]["params"]["max"], json!(200));
    }

    /// ⚠️ **`unknown_field` 逐字回显字段名**（那是这条码的用途），
    /// 而 `params` 里**一个汉字都不许有**（它会被插进 ja/ko 的句子）。
    #[tokio::test]
    async fn unknown_field_echoes_the_field_names_and_params_carry_no_chinese() {
        let h = harness().await;
        let (_, v) = post(&h, "/admin/api/keys", r#"{"keys":[],"disbaled":1,"zz":2}"#).await;
        let fields = v["error"]["params"]["fields"].as_str().unwrap();
        assert!(
            fields.contains("disbaled") && fields.contains("zz"),
            "{fields}"
        );
        assert!(
            !fields
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "params 里出现了汉字：{fields}"
        );
    }

    /// 导入打 `key.added`；勾了重置**必须**另打一条 `key.restored`（知情重置留痕）。
    #[tokio::test]
    async fn import_emits_key_added_and_a_separate_event_for_knowing_resets() {
        let h = harness().await;
        post(&h, "/admin/api/keys", r#"{"keys":["sk-evented-aaaa"]}"#).await;
        assert!(h.recorded.events().contains(&EVENT_KEY_ADDED));

        let before = h.recorded.events().len();
        post(
            &h,
            "/admin/api/keys",
            r#"{"keys":["sk-evented-aaaa"],"resetExisting":true}"#,
        )
        .await;
        let after = h.recorded.entries();
        let restored = after
            .iter()
            .skip(before)
            .find(|e| e.event == EVENT_KEY_RESTORED)
            .expect("🔴 知情重置必须留痕");
        assert_eq!(restored.fields["reset"], json!(1));
    }

    // ── POST /admin/api/keys/bulk ─────────────────────────────────────────

    /// 🔴 **批量删除里混着一把没停用的 key：整体 200，那一项 `ok:false`。**
    ///
    /// 回归对象：整批 409 会让运维不知道哪几把成功了；只回一个「成功」会把那一把
    /// 的失败吞掉。拿状态码当唯一判据的前端在批量路径上**完全看不到这条拒绝**。
    #[tokio::test]
    async fn bulk_delete_reports_per_item_results_instead_of_failing_the_whole_batch() {
        let h = harness().await;
        let ok = seed(&h, "sk-bulk-deletable-aa").await;
        let blocked = seed(&h, "sk-bulk-healthy-bbbb").await;
        // 把第一把停用（可删），第二把保持健康（不可删）。
        patch_req(&h, &format!("/admin/api/keys/{ok}"), r#"{"disabled":true}"#).await;

        let body = serde_json::to_string(&json!({
            "op": "delete",
            "ids": [ok.clone(), blocked.clone(), "no-such-id"],
        }))
        .unwrap();
        let (status, v) = post(&h, "/admin/api/keys/bulk", &body).await;
        assert_eq!(status, StatusCode::OK, "整批不许 409");
        let r = v["results"].as_array().unwrap();
        assert_eq!(r[0], json!({ "id": ok, "ok": true, "reason": null }));
        assert_eq!(
            r[1],
            json!({ "id": blocked, "ok": false, "reason": "must_disable_first" })
        );
        assert_eq!(
            r[2],
            json!({ "id": "no-such-id", "ok": false, "reason": "not_found" })
        );
        // 真的只删掉了该删的那一把。
        assert!(read_record(&h.state.storage, &ok).await.unwrap().is_none());
        assert!(
            read_record(&h.state.storage, &blocked)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// 🔴 **批量删 N 把只写 1 次索引**（循环调 `repo.delete()` 会写 N 次）。
    #[tokio::test]
    async fn bulk_delete_writes_the_index_exactly_once() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        let mut ids = Vec::new();
        for i in 0..5 {
            let id = seed(&h, &format!("sk-bulk-del-{i:04}")).await;
            ids.push(id);
        }
        // 先全部停用（这几次 put 不算在下面那段里）。
        let body = serde_json::to_string(&json!({ "op": "disable", "ids": ids })).unwrap();
        post(&h, "/admin/api/keys/bulk", &body).await;

        let before = counts.snapshot();
        let body = serde_json::to_string(&json!({ "op": "delete", "ids": ids })).unwrap();
        let (s, _) = post(&h, "/admin/api/keys/bulk", &body).await;
        assert_eq!(s, StatusCode::OK);
        let after = counts.snapshot();
        assert_eq!(after.1 - before.1, 1, "5 把只许写一次索引");
        assert_eq!(after.2 - before.2, 5, "记录逐条删");
    }

    /// `disable` 与 `clearCooldown` 真的落盘。
    #[tokio::test]
    async fn bulk_disable_and_clear_cooldown_are_persisted() {
        let h = harness().await;
        let id = seed(&h, "sk-bulk-apply-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    cooldown_until: 9_999_999,
                    cooldown_reason: Some("rate limited".into()),
                    ..rec
                },
                None,
                2_000,
            )
            .await
            .unwrap();

        let body =
            serde_json::to_string(&json!({ "op": "clearCooldown", "ids": [id.clone()] })).unwrap();
        post(&h, "/admin/api/keys/bulk", &body).await;
        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert_eq!(after.cooldown_until, 0);
        assert_eq!(after.cooldown_reason, None);

        let body = serde_json::to_string(&json!({ "op": "disable", "ids": [id.clone()] })).unwrap();
        post(&h, "/admin/api/keys/bulk", &body).await;
        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert_eq!(after.disabled, Some(true));
    }

    /// 🔴 **没有第四个动作**：`enable` / `unevict` 这类「让更多 key 上场」的批量操作
    /// 一律 `400 not_a_bulk_op`，且 `params.ops` 把三个合法值报出来。
    #[tokio::test]
    async fn there_is_no_fourth_bulk_op() {
        let h = harness().await;
        for op in ["enable", "unevict", "purge", ""] {
            let body = serde_json::to_string(&json!({ "op": op, "ids": [] })).unwrap();
            let (s, v) = post(&h, "/admin/api/keys/bulk", &body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{op}");
            assert_eq!(v["error"]["code"], json!("not_a_bulk_op"), "{op}");
            assert_eq!(
                v["error"]["params"]["ops"],
                json!("disable / clearCooldown / delete")
            );
        }
    }

    /// 批量的其余三档拒绝：未知字段 / ids 非字符串数组 / 超上限。
    #[tokio::test]
    async fn bulk_rejects_with_the_documented_codes() {
        let h = harness().await;
        let (s, v) = post(
            &h,
            "/admin/api/keys/bulk",
            r#"{"op":"delete","ids":[],"x":1}"#,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], json!("unknown_field"));

        let (_, v) = post(&h, "/admin/api/keys/bulk", r#"{"op":"delete","ids":[1]}"#).await;
        assert_eq!(v["error"]["code"], json!("ids_not_a_string_array"));

        let ids: Vec<String> = (0..=MAX_IMPORT_KEYS).map(|i| format!("id{i}")).collect();
        let body = serde_json::to_string(&json!({ "op": "delete", "ids": ids })).unwrap();
        let (_, v) = post(&h, "/admin/api/keys/bulk", &body).await;
        assert_eq!(v["error"]["code"], json!("too_many_bulk_ids"));
        assert_eq!(v["error"]["params"]["max"], json!(200));
    }

    /// ⚠️ **批量事件：不超过上限就逐个列 id，超了就明说自己没列。**
    ///
    /// 回归对象：只记 `count` ⇒ 删了 200 把却说不出是哪 200 把，而删除不可撤销。
    #[tokio::test]
    async fn the_bulk_event_lists_ids_up_to_the_cap_and_says_so_when_it_does_not() {
        let h = harness().await;
        let mut ids = Vec::new();
        for i in 0..(BULK_EVENT_IDS_MAX + 1) {
            ids.push(seed(&h, &format!("sk-evt-{i:04}")).await);
        }
        // 小批：逐个列。
        let small: Vec<&String> = ids.iter().take(2).collect();
        let body = serde_json::to_string(&json!({ "op": "disable", "ids": small })).unwrap();
        post(&h, "/admin/api/keys/bulk", &body).await;
        let e = h
            .recorded
            .entries()
            .into_iter()
            .rev()
            .find(|e| e.event == EVENT_KEY_DISABLED)
            .unwrap();
        assert_eq!(e.fields["idsOmitted"], json!(false));
        assert_eq!(e.fields["ids"], json!(format!("{},{}", ids[0], ids[1])));

        // 大批：`ids` 是 null，且 `idsOmitted` 明说。
        let body = serde_json::to_string(&json!({ "op": "disable", "ids": ids })).unwrap();
        post(&h, "/admin/api/keys/bulk", &body).await;
        let e = h
            .recorded
            .entries()
            .into_iter()
            .rev()
            .find(|e| e.event == EVENT_KEY_DISABLED)
            .unwrap();
        assert_eq!(e.fields["count"], json!(BULK_EVENT_IDS_MAX as u64 + 1));
        assert_eq!(e.fields["ids"], json!(null));
        assert_eq!(
            e.fields["idsOmitted"],
            json!(true),
            "面板要说得出自己缺了什么"
        );
    }

    /// 一条都没改动时**不打事件**（别在事件板块里造一条什么都没发生的记录）。
    #[tokio::test]
    async fn a_bulk_that_changed_nothing_emits_no_event() {
        let h = harness().await;
        let before = h.recorded.events().len();
        let (s, _) = post(
            &h,
            "/admin/api/keys/bulk",
            r#"{"op":"disable","ids":["no-such-id"]}"#,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h.recorded.events().len(), before);
    }

    // ── DELETE / PATCH ────────────────────────────────────────────────────

    /// 🔴 **健康的 key 删不掉（409），停用之后能删（204）。**
    ///
    /// 回归对象：判据写成「只看 `evicted`」⇒ 一把健康的 key **永远删不掉**，
    /// 运维只能去手工改存储，而那条路绕过整个面板。
    #[tokio::test]
    async fn deleting_needs_a_disable_first_and_then_returns_204() {
        let h = harness().await;
        let id = seed(&h, "sk-delete-flow-aaaa").await;
        let (s, v) = call(&h, Method::DELETE, &format!("/admin/api/keys/{id}"), None).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let v: Value = serde_json::from_str(&v).unwrap();
        assert_eq!(v["error"]["code"], json!("must_disable_first"));
        assert_eq!(
            v["reason"],
            json!("must_disable_first"),
            "顶层 reason 必须在"
        );

        patch_req(&h, &format!("/admin/api/keys/{id}"), r#"{"disabled":true}"#).await;
        let (s, body) = call(&h, Method::DELETE, &format!("/admin/api/keys/{id}"), None).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        assert!(body.is_empty(), "204 必须是空体");
        assert!(read_record(&h.state.storage, &id).await.unwrap().is_none());
    }

    /// **被剔除的那把不用先停用也能删**（两条都不成立才拦）。
    #[tokio::test]
    async fn an_evicted_key_can_be_deleted_without_disabling_it_first() {
        let h = harness().await;
        let id = seed(&h, "sk-evicted-del-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    evicted: true,
                    ..rec
                },
                None,
                2_000,
            )
            .await
            .unwrap();
        let (s, _) = call(&h, Method::DELETE, &format!("/admin/api/keys/{id}"), None).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    /// 删除的事件只记 id 与两个布尔，**绝不记明文**。
    #[tokio::test]
    async fn the_delete_event_records_only_the_id() {
        let h = harness().await;
        let id = seed(&h, "sk-delete-evt-secret").await;
        patch_req(&h, &format!("/admin/api/keys/{id}"), r#"{"disabled":true}"#).await;
        call(&h, Method::DELETE, &format!("/admin/api/keys/{id}"), None).await;
        let e = h
            .recorded
            .entries()
            .into_iter()
            .find(|e| e.event == EVENT_KEY_DELETED)
            .expect("删除必须留痕");
        assert_eq!(e.fields["id"], json!(id));
        assert_eq!(e.fields["wasDisabled"], json!(true));
        assert_eq!(e.fields["wasEvicted"], json!(false));
        assert!(
            !format!("{:?}", e.fields).contains("delete-evt-secret"),
            "事件里出现了明文"
        );
    }

    /// 🔴🔴 **只改备注的 PATCH：改完之后 GET 真的读得回来。**
    ///
    /// 回归对象（本组最容易写错的一行）：落盘传了 `prev` ⇒ `note` 在写消除白名单外
    /// 但一次「只改备注」的写……不，正相反 —— 传 `prev` 时
    /// `only_elidable_changed` 对 `note` 返回 false，所以那条路今天**碰巧**能落盘；
    /// 真正会被吃掉的是下一格那种「重置一把干净 key」。
    /// 这一格仍然要在：它是端到端读回来的那道门，而口径是「面板说保存成功就真的保存了」。
    #[tokio::test]
    async fn a_note_only_patch_is_readable_afterwards() {
        let h = harness().await;
        let id = seed(&h, "sk-note-patch-aaaa").await;
        let (s, v) = patch_req(
            &h,
            &format!("/admin/api/keys/{id}"),
            r#"{"note":"换了下游客户，先停着"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v, json!({ "ok": true }));
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["items"][0]["note"], json!("换了下游客户，先停着"));
        // `null` 清空。
        patch_req(&h, &format!("/admin/api/keys/{id}"), r#"{"note":null}"#).await;
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["items"][0]["note"], json!(null));
    }

    /// 🔴🔴 **「重置一把干净 key」这次写绝不许被写消除吃掉。**
    ///
    /// 回归对象（实测形态）：落盘传 `prev` ⇒ 这次写对
    /// `only_elidable_changed` 而言**一个字段都没变**，且 `last_used_at` 近期有值
    /// ⇒ 整个被丢弃，而面板显示保存成功。
    /// 观测量是**存储里的字节**（`put` 次数），不是响应码。
    #[tokio::test]
    async fn patching_a_clean_key_still_really_writes_to_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        let id = seed(&h, "sk-clean-patch-aaaa").await;
        // 造出写消除最爱咬的形态：干净 + `last_used_at` 刚刚有值。
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    last_used_at: Some(crate::adapters::now_ms()),
                    ..rec
                },
                None,
                crate::adapters::now_ms(),
            )
            .await
            .unwrap();

        let before = counts.snapshot().1;
        // 这一次 PATCH 对一把干净 key 而言「什么都没变」。
        let (s, _) = patch_req(
            &h,
            &format!("/admin/api/keys/{id}"),
            r#"{"clearCooldown":true,"clearStrikes":true,"unevict":true}"#,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            counts.snapshot().1,
            before + 1,
            "🔴 管理员的每一次写都必须真的落盘（传 prev 会被写消除整个吃掉）"
        );
    }

    /// PATCH 的四个动作 + 两个状态都真的生效。
    #[tokio::test]
    async fn patch_applies_every_documented_field() {
        let h = harness().await;
        let id = seed(&h, "sk-patch-all-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    cooldown_until: 9_999_999,
                    cooldown_reason: Some("rate limited".into()),
                    strikes: 5,
                    evicted: true,
                    evicted_reason: Some("upstream 401".into()),
                    stats: Some(KeyStats {
                        requests: 9,
                        ..KeyStats::empty()
                    }),
                    disabled: Some(true),
                    ..rec
                },
                None,
                2_000,
            )
            .await
            .unwrap();

        let (s, _) = patch_req(
            &h,
            &format!("/admin/api/keys/{id}"),
            r#"{"disabled":false,"note":"ok","clearCooldown":true,"clearStrikes":true,"unevict":true,"clearStats":true}"#,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let a = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert_eq!(a.disabled, Some(false));
        assert_eq!(a.note.as_deref(), Some("ok"));
        assert_eq!(a.cooldown_until, 0);
        assert_eq!(a.cooldown_reason, None);
        assert_eq!(a.strikes, 0);
        assert!(!a.evicted);
        assert_eq!(a.evicted_reason, None);
        assert_eq!(a.stats, Some(KeyStats::empty()));
    }

    /// 🔴 **后四个是动作不是状态：传 `false` 等于没传。**
    ///
    /// 回归对象：把它们当状态实现 ⇒ `clearCooldown:false` 反过来「加一段冷却」，
    /// 或者把已有的冷却当成「要设成 false」而清掉。
    #[tokio::test]
    async fn the_four_action_flags_do_nothing_when_false() {
        let h = harness().await;
        let id = seed(&h, "sk-action-false-aaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        let before = KeyRecord {
            cooldown_until: 9_999_999,
            strikes: 5,
            evicted: true,
            stats: Some(KeyStats {
                requests: 9,
                ..KeyStats::empty()
            }),
            ..rec
        };
        h.state.repo.save(&before, None, 2_000).await.unwrap();

        let (s, _) = patch_req(
            &h,
            &format!("/admin/api/keys/{id}"),
            r#"{"clearCooldown":false,"clearStrikes":false,"unevict":false,"clearStats":false}"#,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let a = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert_eq!(a.cooldown_until, 9_999_999);
        assert_eq!(a.strikes, 5);
        assert!(a.evicted);
        assert_eq!(a.stats.as_ref().unwrap().requests, 9);
    }

    /// PATCH 的五档拒绝，逐条带码。**空对象 → `empty_patch`**（200 就是一次
    /// 「保存成功」而什么都没做）。
    #[tokio::test]
    async fn patch_rejects_with_the_documented_codes() {
        let h = harness().await;
        let id = seed(&h, "sk-patch-reject-aaaa").await;
        let uri = format!("/admin/api/keys/{id}");
        let long = "字".repeat(MAX_NOTE_LENGTH + 1);
        let cases: [(String, &str); 5] = [
            (r#"{}"#.to_string(), "empty_patch"),
            (r#"{"disbaled":true}"#.to_string(), "unknown_field"),
            (r#"{"disabled":"yes"}"#.to_string(), "not_a_boolean"),
            (r#"{"note":123}"#.to_string(), "note_not_a_string"),
            (
                serde_json::to_string(&json!({ "note": long })).unwrap(),
                "note_too_long",
            ),
        ];
        for (body, code) in cases {
            let (s, v) = patch_req(&h, &uri, &body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], json!(code), "{body}");
        }
        // `not_a_boolean` 带 `params.field`；`note_too_long` 带 `params.max`。
        let (_, v) = patch_req(&h, &uri, r#"{"disabled":"yes"}"#).await;
        assert_eq!(v["error"]["params"]["field"], json!("disabled"));
        let (_, v) = patch_req(
            &h,
            &uri,
            &serde_json::to_string(&json!({ "note": "字".repeat(MAX_NOTE_LENGTH + 1) })).unwrap(),
        )
        .await;
        assert_eq!(v["error"]["params"]["max"], json!(200));
    }

    /// ⚠️ **`note` 的上限按字符数而不是字节数**：按字节算的话一条全中文的备注
    /// 在 67 个字上就被拒了。
    #[tokio::test]
    async fn the_note_limit_counts_characters_not_bytes() {
        let h = harness().await;
        let id = seed(&h, "sk-note-len-aaaa").await;
        let uri = format!("/admin/api/keys/{id}");
        let body = serde_json::to_string(&json!({ "note": "字".repeat(MAX_NOTE_LENGTH) })).unwrap();
        let (s, _) = patch_req(&h, &uri, &body).await;
        assert_eq!(s, StatusCode::OK, "恰好到上限的全中文备注必须收下");
    }

    /// PATCH 的事件：**停用优先**，只改备注不打事件。
    #[tokio::test]
    async fn patch_emits_at_most_one_event_and_prefers_the_disable_one() {
        let h = harness().await;
        let id = seed(&h, "sk-patch-evt-aaaa").await;
        let uri = format!("/admin/api/keys/{id}");

        let before = h.recorded.events().len();
        patch_req(&h, &uri, r#"{"note":"只改备注"}"#).await;
        assert_eq!(
            h.recorded.events().len(),
            before,
            "只改备注不打事件（备注不改变池子的任何行为）"
        );

        patch_req(&h, &uri, r#"{"disabled":true,"clearStrikes":true}"#).await;
        let tail: Vec<&'static str> = h.recorded.events().split_off(before);
        assert_eq!(
            tail,
            vec![EVENT_KEY_DISABLED],
            "一次 PATCH 最多一条，且停用优先"
        );

        let before = h.recorded.events().len();
        patch_req(&h, &uri, r#"{"disabled":false}"#).await;
        let tail: Vec<&'static str> = h.recorded.events().split_off(before);
        assert_eq!(tail, vec![EVENT_KEY_RESTORED]);
    }

    /// 🔴 **PATCH 读的是存储里的当前真值，不是最多一个 TTL 前的快照。**
    ///
    /// 回归对象：读快照的话，这次「改备注」会把快照建立之后别的副本写下的
    /// `evicted` 原样覆盖回去。
    #[tokio::test]
    async fn patch_reads_the_current_truth_not_the_cached_snapshot() {
        let h = harness().await;
        let id = seed(&h, "sk-truth-patch-aaaa").await;
        // 把快照热起来。
        get(&h, "/admin/api/keys").await;
        // 模拟「别的副本」直接写存储：把它标成 evicted。
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        let v = serde_json::to_value(KeyRecord {
            evicted: true,
            evicted_reason: Some("upstream 401".into()),
            ..rec
        })
        .unwrap();
        h.state
            .storage
            .put(&format!("key:{id}"), v, None)
            .await
            .unwrap();

        patch_req(&h, &format!("/admin/api/keys/{id}"), r#"{"note":"x"}"#).await;
        let after = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        assert!(
            after.evicted,
            "🔴 一次改备注不许把别的副本写下的 evicted 覆盖掉"
        );
        assert_eq!(after.note.as_deref(), Some("x"));
    }

    // ── reveal / usage ────────────────────────────────────────────────────

    /// `reveal` 回明文，且**每次必留痕**（事件里只有 id）。
    #[tokio::test]
    async fn reveal_returns_the_plaintext_and_always_leaves_a_trace() {
        let h = harness().await;
        let id = seed(&h, "sk-reveal-me-0123456").await;
        let (s, v) = get(&h, &format!("/admin/api/keys/{id}/reveal")).await;
        assert_eq!(s, StatusCode::OK);
        // ⚠️ 整体比对**保留**（响应体的键集是对外契约，多一格少一格都当场红）；
        //    `kind` 是 06 §5.3 的 A 档带进来的那一格，两族共用同一段渲染代码。
        assert_eq!(v, json!({ "kind": "ok", "key": "sk-reveal-me-0123456" }));
        let e = h
            .recorded
            .entries()
            .into_iter()
            .find(|e| e.event == EVENT_KEY_REVEALED)
            .expect("🔴 取明文必须留痕");
        assert_eq!(e.fields["id"], json!(id));
        assert!(
            !format!("{:?}", e.fields).contains("reveal-me"),
            "事件里绝不许出现明文"
        );
    }

    /// `usage` 是 Tier-1，**Tier-2 关着照样可用**，且形状逐字段钉住。
    #[tokio::test]
    async fn key_usage_works_even_with_tier2_off() {
        let h = harness().await;
        assert!(h.state.usage_sink.is_none(), "夹具必须是 Tier-2 关着的");
        let id = seed(&h, "sk-usage-aaaaaaaa").await;
        let rec = read_record(&h.state.storage, &id).await.unwrap().unwrap();
        h.state
            .repo
            .save(
                &KeyRecord {
                    stats: Some(KeyStats {
                        requests: 3,
                        success: 2,
                        failed: 1,
                        ..KeyStats::empty()
                    }),
                    ..rec
                },
                None,
                2_000,
            )
            .await
            .unwrap();

        let (s, v) = get(&h, &format!("/admin/api/keys/{id}/usage")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["id"], json!(id));
        assert_eq!(v["stats"]["requests"], json!(3));
        assert_eq!(v["stats"]["clientErrors"], json!(0));
        assert_eq!(v["approximate"], json!(true));
        assert!(v["generatedAt"].as_i64().unwrap() > 0);
    }

    /// 存量记录（没有 `stats` 那一格）在 `usage` 里投影成**全零，不是缺席**。
    #[tokio::test]
    async fn a_record_without_stats_reports_zeroes_not_missing_fields() {
        let h = harness().await;
        let id = seed(&h, "sk-nostats-aaaaaaa").await;
        let (_, v) = get(&h, &format!("/admin/api/keys/{id}/usage")).await;
        assert_eq!(
            v["stats"],
            json!({
                "requests": 0, "success": 0, "failed": 0, "clientErrors": 0,
                "lastErrorAt": null, "lastErrorKind": null
            })
        );
    }

    /// 🔴🔴 **五条带 `:id` 的端点各自的 404 信封逐条对**：
    /// `reveal` / `DELETE` / `PATCH` **带 `code`**，`usage` / `verify` **不带**。
    ///
    /// 回归对象：把它们「统一」成一种 ⇒ 设置页那两支的文案选择当场失配，
    /// 而两侧都是 404，没有任何自然信号。
    ///
    /// ⚠️ **期望值是手写的字面量 `Some("key_not_found")` / `None`，
    /// 刻意不从 [`KeysEndpoint::key_not_found_envelope`] 现算** ——
    /// 从那张表现算的话，把表改坏这一格会跟着一起变绿（本仓登记的第 ⑥ 种假阳性）。
    /// 那张表自己的正确性由 `envelope_assignment_is_pinned_per_endpoint` 单独守。
    #[tokio::test]
    async fn the_five_id_endpoints_use_exactly_the_documented_not_found_envelope() {
        let h = harness().await;
        let cases: [(Method, &str, Option<&str>, Option<&str>); 5] = [
            (
                Method::GET,
                "/admin/api/keys/nope/reveal",
                None,
                Some("key_not_found"),
            ),
            (
                Method::DELETE,
                "/admin/api/keys/nope",
                None,
                Some("key_not_found"),
            ),
            (
                Method::PATCH,
                "/admin/api/keys/nope",
                Some(r#"{"note":"x"}"#),
                Some("key_not_found"),
            ),
            (Method::GET, "/admin/api/keys/nope/usage", None, None),
            (Method::POST, "/admin/api/keys/nope/verify", None, None),
        ];
        for (method, uri, body, expected_code) in cases {
            let (s, raw) = call(&h, method, uri, body).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
            let v: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["error"]["message"], json!("没有这把 key"), "{uri}");
            match expected_code {
                Some(code) => assert_eq!(v["error"]["code"], json!(code), "{uri} 该带 code"),
                None => assert!(
                    v["error"].get("code").is_none(),
                    "{uri} 绝不许带 code（02 §4.4 登记的刻意不一致）：{raw}"
                ),
            }
        }
    }

    // ── purge（危险区）─────────────────────────────────────────────────────

    /// 🔴 **`expect` 对不上 ⇒ 409 + 两边的数，一把都不删。**
    #[tokio::test]
    async fn purge_refuses_when_the_pool_changed_and_deletes_nothing() {
        let h = harness().await;
        seed(&h, "sk-purge-one-aaaa").await;
        seed(&h, "sk-purge-two-bbbb").await;
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":5}"#).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["reason"], json!("pool_size_changed"));
        assert_eq!(v["expected"], json!(5));
        assert_eq!(v["actual"], json!(2));
        assert_eq!(v["error"]["type"], json!("conflict"));
        assert!(
            v["error"].get("code").is_none(),
            "危险区那条走的是不带 code 的网关信封"
        );
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["total"], json!(2), "一把都不许删");
    }

    /// 对上了就真清空。
    #[tokio::test]
    async fn purge_clears_the_pool() {
        let h = harness().await;
        seed(&h, "sk-purge-one-aaaa").await;
        seed(&h, "sk-purge-two-bbbb").await;
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":2}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v, json!({ "deleted": 2, "remaining": 0, "expected": 2 }));
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["total"], json!(0));
    }

    /// 🔴 **`remaining` 是从存储回读出来的，不是 `0` 这个常数。**
    ///
    /// 夹具刻意造出「索引说空了、而存储里还躺着一条记录」那一档：
    /// 清空之后空池兜底会 `list()` 一次并如实把那条孤儿报出来 ——
    /// **那正是「清空之后真的空了吗」这个问题的唯一诚实答案**。
    /// 写死一个 `0` 只是把 handler 的心愿印在屏幕上，而这一格就是为了让那种写法变红。
    #[tokio::test]
    async fn purge_reports_remaining_read_back_from_storage_not_a_hardcoded_zero() {
        let h = harness().await;
        seed(&h, "sk-purge-real-aaaa").await;
        // 一条**不在索引里**的孤儿记录（运维手工往 store.json 里塞过东西）。
        let orphan = KeyRecord::fresh("orphanidorphanid", "sk-orphan-record-x", 500);
        h.state
            .storage
            .put(
                "key:orphanidorphanid",
                serde_json::to_value(&orphan).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 索引说池子里只有 1 把 ⇒ 运维屏幕上看到的也是 1。
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":1}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["deleted"], json!(1));
        assert_eq!(
            v["remaining"],
            json!(1),
            "🔴 回读必须如实报出那条还躺在存储里的记录（写死 0 会在这里红）"
        );
    }

    /// 🔴 **不带体的 POST 在解析那一步就 400** —— 这是这颗按钮安全的唯一理由
    /// （枚举式鉴权矩阵会拿正确口令把每条路由真打一遍）。
    #[tokio::test]
    async fn purge_without_a_body_never_wipes_the_pool() {
        let h = harness().await;
        seed(&h, "sk-purge-guard-aaaa").await;
        let (s, _) = call(&h, Method::POST, KEYS_PURGE_PATH, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = post(&h, KEYS_PURGE_PATH, "").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["total"], json!(1), "夹具自己的池子必须还在");
    }

    /// `expect` 非法 ⇒ **不带 `code`** 的 400；未知字段 ⇒ **带 `code`** 的 400。
    /// ⚠️ 同一条端点上的两种信封，02 §4.4 登记为刻意。
    #[tokio::test]
    async fn purge_mixes_two_envelopes_on_purpose() {
        let h = harness().await;
        for body in [
            r#"{"expect":-1}"#,
            r#"{"expect":"2"}"#,
            r#"{"expect":1.5}"#,
            r#"{}"#,
        ] {
            let (s, v) = post(&h, KEYS_PURGE_PATH, body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert!(
                v["error"].get("code").is_none(),
                "{body} 的拒绝不许带 code：{v}"
            );
        }
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":0,"nope":1}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(
            v["error"]["code"],
            json!("unknown_field"),
            "未知字段这一档带 code"
        );
    }

    /// 🔴🔴 **空请求体是 `400 bad_json`，不是「当成 `{}`」。**
    ///
    /// ⚠️ 这一格是**变异复检逼出来的**：原先四条 `AdminJson` 端点的空体只被断言了
    /// 「状态码是 400」，而把空体折叠成 `{}` 之后每一条**照样是 400**
    /// （导入 → `keys_not_a_string_array`、批量 → `not_a_bulk_op`、
    /// PATCH → `empty_patch`、清空 → `expect` 非法）—— 那次变异**完整逃逸**。
    /// ⇒ 判据必须钉到 **`code`** 上，而 `code` 才是对外契约。
    ///
    /// 🔴 危险区那条尤其要紧：`expect` 必填是「不带体的 POST 清不掉一池 key」的
    /// **唯一**理由，而折叠成 `{}` 会把那道闸从「解析就失败」降级成「靠 `expect` 缺席」。
    #[tokio::test]
    async fn an_empty_body_is_bad_json_on_every_admin_json_endpoint() {
        let h = harness().await;
        let id = seed(&h, "sk-empty-body-aaaa").await;
        let cases: [(&str, String); 4] = [
            ("/admin/api/keys", "/admin/api/keys".into()),
            ("/admin/api/keys/bulk", "/admin/api/keys/bulk".into()),
            (KEYS_PURGE_PATH, KEYS_PURGE_PATH.into()),
            ("PATCH", format!("/admin/api/keys/{id}")),
        ];
        for (label, uri) in cases {
            let method = if label == "PATCH" {
                Method::PATCH
            } else {
                Method::POST
            };
            for body in [None, Some("")] {
                let (s, raw) = call(&h, method.clone(), &uri, body).await;
                assert_eq!(s, StatusCode::BAD_REQUEST, "{uri} {body:?}");
                let v: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(
                    v["error"]["code"],
                    json!("bad_json"),
                    "{uri} {body:?} 的空体必须是 bad_json，实际：{raw}"
                );
            }
        }
        // 而池子一把都没少（尤其是危险区那条）。
        let (_, list) = get(&h, "/admin/api/keys").await;
        assert_eq!(list["total"], json!(1));
    }

    /// 🔴 **清空只动 `key:*` 与 `pool:index` 两族键**：别的键一个字节都不动。
    ///
    /// 回归对象：任何一颗重置按钮顺手去删诊断历史，等于把「诊断用的历史」和
    /// 「运维的一次误操作」绑在一起 —— 而运维往往正是**因为**出事才来按它的。
    #[tokio::test]
    async fn purge_touches_only_the_two_key_families() {
        let h = harness().await;
        seed(&h, "sk-purge-scope-aaaa").await;
        for k in ["config", "tend:history", "usage:2026-09-11", "event:abc"] {
            h.state
                .storage
                .put(k, json!({ "keep": "me" }), None)
                .await
                .unwrap();
        }
        post(&h, KEYS_PURGE_PATH, r#"{"expect":1}"#).await;
        for k in ["config", "tend:history", "usage:2026-09-11", "event:abc"] {
            assert_eq!(
                h.state.storage.get(k).await.unwrap(),
                Some(json!({ "keep": "me" })),
                "{k} 被清空按钮顺手删了"
            );
        }
    }

    /// 空池上清空是合法的，且**不打 `key.deleted`**（什么都没发生就别在板块里造一条
    /// 「面板清空了整个 Key 池」）。
    ///
    /// ⚠️ 判据只看 `key.deleted` 这一条，**不数事件总数**：空池那条读路径本身会打
    /// 一条 `pool.index_backfilled`（索引缺失时的重建），数总数会把它算进来 ——
    /// 那是「夹具的副作用被当成被测对象」的一种。
    #[tokio::test]
    async fn purging_an_empty_pool_is_a_no_op_without_a_delete_event() {
        let h = harness().await;
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":0}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["deleted"], json!(0));
        assert!(
            !h.recorded.events().contains(&EVENT_KEY_DELETED),
            "空池清空不该留下一条「删过东西」的痕迹"
        );
    }

    // ── verify（真打一次上游）──────────────────────────────────────────────

    async fn verify_harness(
        script: Vec<UpstreamReply>,
    ) -> (Harness, Arc<ScriptedFetcher>, Arc<CountingStorage>) {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let fetcher = ScriptedFetcher::new(script);
        let h = harness_with(counting.clone(), Some(fetcher.clone())).await;
        (h, fetcher, counting)
    }

    /// 🔴 **真的打一次上游，并且拿指定那一把 key 的明文去打。**
    ///
    /// 回归对象（TS 版的真缺陷）：只读一遍域名列表就报绿 ⇒
    /// **凭据粘错时照样报绿**。观测量是「出站请求里那个 `Authorization` 头」。
    #[tokio::test]
    async fn upstream_probe_uses_each_keys_selected_service() {
        for (route, base) in [
            ("international", "https://apihub.agnes-ai.com/v1"),
            ("internationalBackup", "https://apihub.agnes-ai.cn/v1"),
            ("china", "https://api.agnes-ai.cn/v1"),
        ] {
            let (h, fetcher, _) = verify_harness(vec![UpstreamReply::json(json!({"ok":1}))]).await;
            let key = format!("sk-verify-service-{route}-aaaa");
            let body = json!({"keys":[key],"upstream":route}).to_string();
            let (status, response) = post(&h, "/admin/api/keys", &body).await;
            assert_eq!(status, StatusCode::OK);
            let id = response["added"][0].as_str().unwrap();
            let (status, response) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response["ok"], true);
            let sent = fetcher.only();
            assert_eq!(sent.url, format!("{base}/chat/completions"));
            assert!(
                sent.headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == &format!("Bearer {key}"))
            );
        }
    }

    #[tokio::test]
    async fn verify_really_calls_upstream_with_that_key() {
        let (h, fetcher, _) = verify_harness(vec![UpstreamReply::json(json!({ "ok": 1 }))]).await;
        let id = seed(&h, "sk-verify-real-0001").await;
        let (s, v) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["status"], json!(200));
        assert_eq!(v["reason"], json!(null));

        let sent = fetcher.only();
        assert_eq!(sent.method, "POST");
        assert!(
            sent.url.ends_with("/chat/completions"),
            "🔴 上游路径是 /chat/completions（对上游那条），实际 {}",
            sent.url
        );
        let auth = sent
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.clone())
            .expect("必须带 Authorization");
        assert_eq!(
            auth, "Bearer sk-verify-real-0001",
            "🔴 必须拿**指定那一把**的明文去打"
        );
        assert_eq!(sent.body_json()["model"], json!("agnes-2.0-flash"));
    }

    /// 🔴 **探针的上游路径与请求体来自协议目录，不是这条 handler 里手搓的字面量。**
    ///
    /// 02 §9.14 逐字：请求体 = **openai 协议的 `sample(MODEL_CATALOG[0].id)`**，
    /// 打的是那条协议的 `upstream_path`。隔壁 [`crate::admin::models::test_with`]
    /// 走的就是这张表 —— 两边今天**恰好**一致，而「恰好」不是约束：
    /// 目录改一格而这里没跟，面板上「测模型」与「验活」会一个绿一个红，
    /// 而两者本该打同一个上游形状，没有任何东西会响。
    ///
    /// ⚠️ 期望值**从目录现算**，不手写字面量 —— 手写的话这一格自己就成了第二份真源。
    /// ⚠️ 变异复检的形态是**动目录**（改 `upstream_path` / 改 `sample` 的形状）：
    /// 手搓字面量的实现下它会红，走目录的实现下它跟着一起变，这正是「同源」的定义。
    #[tokio::test]
    async fn verify_probes_straight_from_the_protocol_catalog() {
        let (h, fetcher, _) = verify_harness(vec![UpstreamReply::json(json!({ "ok": 1 }))]).await;
        let id = seed(&h, "sk-verify-catalog-10").await;
        let (s, _) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(s, StatusCode::OK);

        let proto = protocol_by_id(ProtocolId::Openai);
        let sent = fetcher.only();
        assert_eq!(sent.method, proto.method);
        assert_eq!(
            sent.url,
            format!(
                "{}{}",
                h.state.config.current().agnes_base_url,
                proto.upstream_path
            ),
            "上游路径必须是目录里那一格（**对上游**那条，不是对外的 /v1/…）"
        );
        assert_eq!(
            sent.body_json(),
            proto.sample(MODEL_CATALOG[0].id),
            "🔴 探针体必须逐字是 openai 协议的 sample(MODEL_CATALOG[0].id)"
        );
    }

    /// 🔴 **上游非 2xx：`ok:false` 但照样回 200 与真实状态码，且一个字节的上游正文都不回。**
    ///
    /// 断言的是**整段响应体**（逐字段查的写法在 handler 多回一个字段时会静默漏掉）。
    #[tokio::test]
    async fn verify_never_relays_a_single_byte_of_the_upstream_body() {
        let mut reply = UpstreamReply::json(json!({
            "error": { "message": "invalid api key: sk-verify-leak-0002xyz" }
        }));
        reply.status = 401;
        let (h, _, _) = verify_harness(vec![reply]).await;
        let id = seed(&h, "sk-verify-leak-0002").await;
        let (s, raw) = call(
            &h,
            Method::POST,
            &format!("/admin/api/keys/{id}/verify"),
            Some(""),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "探测本身成功了，失败的是凭据");
        assert!(
            !raw.contains("invalid api key") && !raw.contains("sk-verify-leak"),
            "🔴 上游正文泄漏进了面板响应：{raw}"
        );
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["status"], json!(401));
    }

    /// 🔴 **验活一次存储写都不产生**（失败记 strike 与成功清 strike 各自是一颗自毁按钮）。
    #[tokio::test]
    async fn verify_writes_nothing_to_storage() {
        let (h, _, counting) = verify_harness(vec![UpstreamReply::json(json!({ "ok": 1 }))]).await;
        let id = seed(&h, "sk-verify-nowrite-03").await;
        let before = counting.counts.snapshot();
        let (s, _) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(s, StatusCode::OK);
        let after = counting.counts.snapshot();
        assert_eq!(after.1, before.1, "put 必须是 0");
        assert_eq!(after.2, before.2, "delete 必须是 0");
    }

    /// 🔴 **验活一条事件都不打**：事件环能整份下载，那是第二条出口。
    #[tokio::test]
    async fn verify_emits_no_event_at_all() {
        let (h, _, _) = verify_harness(vec![UpstreamReply::json(json!({ "ok": 1 }))]).await;
        let id = seed(&h, "sk-verify-noevent-04").await;
        let before = h.recorded.events().len();
        post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(
            h.recorded.events().len(),
            before,
            "🔴 这条端点在结构上就不该有日志出口"
        );
    }

    /// 🔴 **出站带的是首字节档（8 秒），不是同步档（2 分钟）** ——
    /// 给它同步档等于让面板挂两分钟。
    ///
    /// ⚠️ 观测量必须是**真正发给端口的那个 `timeout_ms`**：
    /// 断言「配置里那个数是 8000」是同义反复，而断言「打到了 `/chat/completions`」
    /// 对档位**结构性地看不见**（两档产出的出站 URL 一模一样）。
    /// ⚠️ 期望值**手写字面量**，不从 `cfg.upstream_timeout_ms` 现算 ——
    /// 现算的话「把两档互换一下」会让这一格跟着一起变绿。
    #[tokio::test]
    async fn verify_uses_the_first_byte_timeout_budget() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let spy = Arc::new(BudgetSpy {
            inner: ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "ok": 1 }))]),
            budgets: Mutex::new(Vec::new()),
        });
        let h = harness_with(counting, Some(spy.clone())).await;
        // 两档必须真的不一样，否则这一格区分不了档位。
        let cfg = h.state.config.current();
        assert_ne!(cfg.upstream_timeout_ms, cfg.upstream_sync_timeout_ms);

        let id = seed(&h, "sk-verify-budget-05").await;
        let (s, _) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(spy.only_budget(), 8_000, "验活必须走首字节档");
    }

    /// 把每次出站带的 `timeout_ms` 记下来。**套在 `ScriptedFetcher` 外面**，
    /// 于是「脚本用完 = 判据写错了」那条纪律照样生效。
    struct BudgetSpy {
        inner: Arc<ScriptedFetcher>,
        budgets: Mutex<Vec<i64>>,
    }

    impl BudgetSpy {
        /// 恰好发了一次上游调用，取那一次的预算。**次数本身是判据的一部分。**
        fn only_budget(&self) -> i64 {
            let b = self.budgets.lock();
            assert_eq!(b.len(), 1, "应当恰好发一次上游调用");
            b[0]
        }
    }

    #[async_trait::async_trait]
    impl crate::ports::fetcher::Fetcher for BudgetSpy {
        async fn send(
            &self,
            req: OutboundRequest,
        ) -> Result<crate::ports::fetcher::OutboundResponse, FetchError> {
            self.budgets.lock().push(req.timeout_ms);
            self.inner.send(req).await
        }
    }

    /// 上游连不上 / 超时：`reason` 是机器可读的 code，`status` 是 `null`。
    #[tokio::test]
    async fn verify_maps_transport_failures_to_machine_readable_reasons() {
        // 脚本为空 ⇒ ScriptedFetcher 以 Connect 失败收场。
        let (h, _, _) = verify_harness(vec![]).await;
        let id = seed(&h, "sk-verify-netfail-06").await;
        let (s, v) = post(&h, &format!("/admin/api/keys/{id}/verify"), "").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["status"], json!(null));
        assert_eq!(v["reason"], json!("network_error"));
        assert!(v["latencyMs"].as_i64().unwrap() >= 0);
    }

    /// 🔴 **这条端点不收任何选项**：空体放行、`{}` 放行，其余一律 400（不带 code）。
    ///
    /// 回归对象：`{"model":"agnes-2.5-pro"}` 这种「我以为能指定模型」的写法在宽松
    /// 实现下是一次「点了、但完全没按你想的那样发生」的静默误操作。
    #[tokio::test]
    async fn verify_accepts_no_parameters_at_all() {
        let (h, _, _) = verify_harness(vec![
            UpstreamReply::json(json!({ "ok": 1 })),
            UpstreamReply::json(json!({ "ok": 1 })),
        ])
        .await;
        // ⚠️ 两次**成功**的验活必须落在两把不同的 key 上：护栏是逐把一个槽位的，
        //    同一把连打两次第二次必然是 429（而那是另一格判据的对象）。
        let bare = seed(&h, "sk-verify-noargs-07a").await;
        let empty_obj = seed(&h, "sk-verify-noargs-07b").await;
        assert_eq!(
            call(
                &h,
                Method::POST,
                &format!("/admin/api/keys/{bare}/verify"),
                None
            )
            .await
            .0,
            StatusCode::OK,
            "不带体必须放行（面板与鉴权矩阵都是这么调的）"
        );
        assert_eq!(
            post(&h, &format!("/admin/api/keys/{empty_obj}/verify"), "{}")
                .await
                .0,
            StatusCode::OK,
            "`{{}}` 也必须放行"
        );

        // 有字段 / 非对象 / 畸形 JSON 一律 400，且**不带 code**。
        // 这几次在护栏之前就被拒了，所以复用同一把 key 不会撞上冷却。
        let uri = format!("/admin/api/keys/{bare}/verify");
        for body in [r#"{"model":"agnes-2.5-pro"}"#, r#"[1]"#, r#"{oops"#] {
            let (s, v) = post(&h, &uri, body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert!(v["error"].get("code").is_none(), "{body} 不许带 code");
        }
        let (_, v) = post(&h, &uri, r#"{"model":"agnes-2.5-pro"}"#).await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("model"),
            "报文要点名多余的字段：{v}"
        );
    }

    /// 🔴 **护栏排在 404 之后**：验一把不存在的 key，连着两次都是 404 而不是第二次 429，
    /// **而且那个随机 id 一格都没在护栏那张表里落下**。
    ///
    /// 危害是后半句：`ProbeGuard.slots` 从不主动清理，它有界的全部理由就是
    /// 「`try_acquire` 排在『这把 key 真的存在』之后」—— 落了脚就意味着任何拿到
    /// 管理口令的人都能用随机 id 往里**无限灌键**。
    ///
    /// ⚠️ **只观测状态码是挡不住这一半的**：变异复检做过三种 ——
    /// ① 在 404 分支之前 `try_acquire` 一次、结果丢弃 ⇒ 状态码照旧两次 404（**逃逸**）；
    /// ② 之前 `try_acquire` 并 `release` ⇒ 同样**逃逸**；③ 整块挪到 404 之前 ⇒ 才红。
    /// ⇒ 观测量必须直接落在「表里有没有这一格」上（[`ProbeGuard::tracks`]）。
    #[tokio::test]
    async fn the_probe_guard_runs_after_the_404_so_random_ids_cannot_flood_it() {
        let (h, _, _) = verify_harness(vec![]).await;
        let bogus = "definitely-not-a-real-id";
        let uri = format!("/admin/api/keys/{bogus}/verify");
        assert_eq!(post(&h, &uri, "").await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            post(&h, &uri, "").await.0,
            StatusCode::NOT_FOUND,
            "🔴 第二次也必须是 404，不是 429"
        );
        assert!(
            !probe_guard().tracks(&format!("verify:{bogus}")),
            "🔴 404 掉的 id 不许在护栏那张表里落脚 —— 那张表从不清理，落一个就是灌一格"
        );
    }

    /// 🔴 **连点被挡：第二次拿到 429 + `probe_cooldown`，而上游只被打了一次。**
    #[tokio::test]
    async fn a_second_click_within_the_minimum_interval_is_refused() {
        let (h, fetcher, _) = verify_harness(vec![UpstreamReply::json(json!({ "ok": 1 }))]).await;
        let id = seed(&h, "sk-verify-cooldown-08").await;
        let uri = format!("/admin/api/keys/{id}/verify");
        assert_eq!(post(&h, &uri, "").await.0, StatusCode::OK);

        let (s, v) = post(&h, &uri, "").await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(v["reason"], json!("probe_cooldown"));
        assert_eq!(v["error"]["type"], json!("rate_limit_error"));
        assert!(
            v["error"].get("code").is_none(),
            "429 走的是网关信封 + 顶层 reason"
        );
        // `only()` 自带「恰好一次上游调用」⇒ 被挡下的那一次真的没打出去。
        assert!(fetcher.only().url.contains("/chat/completions"));
    }

    /// ⚠️ **护栏的粒度是 `verify:<id>`，逐把 key 一个槽位。**
    ///
    /// 回归对象：全局粒度下「验 A」会把「验 B」一起挡掉一个最小间隔，
    /// 20 把 key 逐个验要串行等 60 秒。
    #[tokio::test]
    async fn the_guard_is_per_key_not_global() {
        let (h, _, _) = verify_harness(vec![
            UpstreamReply::json(json!({ "ok": 1 })),
            UpstreamReply::json(json!({ "ok": 1 })),
        ])
        .await;
        let a = seed(&h, "sk-verify-slot-a-09").await;
        let b = seed(&h, "sk-verify-slot-b-09").await;
        assert_eq!(
            post(&h, &format!("/admin/api/keys/{a}/verify"), "").await.0,
            StatusCode::OK
        );
        assert_eq!(
            post(&h, &format!("/admin/api/keys/{b}/verify"), "").await.0,
            StatusCode::OK,
            "🔴 验 A 不许把验 B 一起挡掉"
        );
    }

    /// 一个**永不返回**的上游：`send` 一进来先打个招呼，然后挂住。
    ///
    /// 它造的是那个真实形态：**客户端在探测还没回来的时候断开连接**。
    struct HangingFetcher {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[async_trait::async_trait]
    impl crate::ports::fetcher::Fetcher for HangingFetcher {
        async fn send(
            &self,
            _req: OutboundRequest,
        ) -> Result<crate::ports::fetcher::OutboundResponse, FetchError> {
            if let Some(tx) = self.entered.lock().take() {
                let _ = tx.send(());
            }
            std::future::pending().await
        }
    }

    /// 🔴 **客户端中途断开连接，槽位照样必须被放开。**
    ///
    /// axum 在客户端断开时**直接 drop handler 的 future** ⇒ `send().await` 之后的
    /// 每一行都不再执行。把 `release` 写在那之后的话，`in_flight` 永远停在 `true`，
    /// 而 `try_acquire` 第一句就是「在飞就拒」⇒ **那把 key 的验活按钮从此永久点不动**。
    ///
    /// ⚠️ 它与 [`a_failed_probe_never_leaves_the_slot_stuck_in_flight`] 不是同一件事：
    /// 那一格钉的是「上游返回错误」那条支（`release` 排在它后面，本来就走得到），
    /// 这一格钉的是**第三条支「请求被取消」** —— 护栏文件头那句
    /// 「最小间隔那道闸会自愈，**这道不会**」说的正是它。
    /// ⇒ 只有 drop guard（RAII）这类**不依赖执行流走到那一行**的办法能担保。
    #[tokio::test]
    async fn a_cancelled_request_still_releases_the_slot() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fetcher: Arc<dyn crate::ports::fetcher::Fetcher> = Arc::new(HangingFetcher {
            entered: Mutex::new(Some(tx)),
        });
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher)).await;
        let id = seed(&h, "sk-verify-cancelled-11").await;
        let kind = format!("verify:{id}");

        let st = h.state.clone();
        let spawned_id = id.clone();
        let task =
            tokio::spawn(async move { verify(State(st), Path(spawned_id), Bytes::new()).await });
        // 等到探针**真的发出去了**再动手 —— 否则取消的可能是护栏之前那一段。
        rx.await.expect("上游探针必须真的发出去");
        assert!(
            probe_guard().is_in_flight(&kind),
            "夹具前提：这一刻它确实占着闸"
        );

        // 客户端断开 = handler 的 future 被 drop 在 `send().await` 上。
        task.abort();
        assert!(
            task.await.unwrap_err().is_cancelled(),
            "夹具前提：真被取消了"
        );

        assert!(
            !probe_guard().is_in_flight(&kind),
            "🔴 被取消的那一次没放开槽位 —— 这把 key 的验活按钮从此永久点不动"
        );
    }

    /// 🔴 **上游失败之后，这把 key 的槽位必须被放开**（`release` 不在成功支里）。
    ///
    /// 判据是「失败之后**隔过最小间隔**还能不能再来一次」——
    /// 只测一次失败是看不出来的：卡在「在飞」的那种形态下，
    /// 隔多久都会是 `probe_in_flight` 而不是放行。
    #[test]
    fn a_failed_probe_never_leaves_the_slot_stuck_in_flight() {
        let g = ProbeGuard::new();
        assert_eq!(g.try_acquire("verify:x", 0), ProbeAcquire::Ok);
        // 探测失败了，但 release 照样走到。
        g.release("verify:x");
        // 最小间隔之内仍然被挡（挡的是「按住不放」）。
        assert_eq!(
            g.try_acquire("verify:x", PROBE_MIN_INTERVAL_MS - 1),
            ProbeAcquire::Cooldown
        );
        // 隔过最小间隔就该放行 —— 卡在「在飞」的话这里会是 InFlight。
        assert_eq!(
            g.try_acquire("verify:x", PROBE_MIN_INTERVAL_MS),
            ProbeAcquire::Ok
        );
    }

    /// 在途去重与最小间隔是**两道**闸，少一道就漏一半。
    #[test]
    fn the_guard_has_two_independent_gates() {
        let g = ProbeGuard::new();
        assert_eq!(g.try_acquire("k", 0), ProbeAcquire::Ok);
        // ① 在途：同一毫秒的第二次（「连点」）。
        assert_eq!(g.try_acquire("k", 0), ProbeAcquire::InFlight);
        // 上一次回来了。
        g.release("k");
        // ② 最小间隔：「跟着上一次的结束节奏连点」。
        assert_eq!(g.try_acquire("k", 1), ProbeAcquire::Cooldown);
        // 两种拒绝的顶层 reason 分得开。
        assert_eq!(ProbeAcquire::InFlight.reason(), Some("probe_in_flight"));
        assert_eq!(ProbeAcquire::Cooldown.reason(), Some("probe_cooldown"));
        assert_eq!(ProbeAcquire::Ok.reason(), None);
    }

    /// `last_at` 记的是**发起**时刻而不是结束时刻 —— 上游越慢，运维越想赶紧再试一次。
    #[test]
    fn the_minimum_interval_is_measured_from_the_start_of_the_previous_probe() {
        let g = ProbeGuard::new();
        g.try_acquire("k", 1_000);
        // 一次「耗了 8 秒」的探测。
        g.release("k");
        // 从**发起**算：1_000 + 3_000 = 4_000，而现在已经 9_000 ⇒ 立刻放行。
        assert_eq!(g.try_acquire("k", 9_000), ProbeAcquire::Ok);
    }

    /// 时钟回拨（NTP）不许把这扇闸冻住 —— 与本仓其余几处同一套语义。
    #[test]
    fn a_backwards_clock_does_not_freeze_the_guard() {
        let g = ProbeGuard::new();
        g.try_acquire("k", 10_000);
        g.release("k");
        assert_eq!(g.try_acquire("k", 1), ProbeAcquire::Ok);
    }

    // ── 路由形态 ───────────────────────────────────────────────────────────

    /// ⚠️ **`/keys/bulk` 与 `/keys/purge` 不许被 `/keys/{id}` 吃掉。**
    ///
    /// axum 0.8 用 matchit，静态段天然优先 —— **这条风险在 Rust 侧本身会消失，
    /// 但必须写判据钉住**，否则将来换匹配器会复发（Hono 那边靠注册顺序保证）。
    /// 被吃掉时的现象是一个**看起来合理的 400**。
    #[tokio::test]
    async fn the_literal_key_subpaths_are_not_swallowed_by_the_id_route() {
        let h = harness().await;
        // bulk：走到 bulk 的 handler（`not_a_bulk_op` 是它独有的码）。
        let (s, v) = post(&h, "/admin/api/keys/bulk", r#"{"op":"nope","ids":[]}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], json!("not_a_bulk_op"));
        // purge：走到 purge 的 handler（不带 code 的 `expect` 那一档是它独有的）。
        let (s, v) = post(&h, KEYS_PURGE_PATH, r#"{"expect":"x"}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(v["error"].get("code").is_none());
        assert!(
            v["error"]["message"].as_str().unwrap().contains("expect"),
            "{v}"
        );
    }

    /// 🔴 **九条端点全部注册在鉴权闸之后**：不带口令一律 401（不是 404、不是 200）。
    ///
    /// 挪到闸前 = 一条免鉴权的管理端点；挪到静态兜底之后 = 拿着正确口令也恒 404。
    #[tokio::test]
    async fn every_keys_endpoint_sits_behind_the_admin_gate() {
        let h = harness().await;
        let probes: [(Method, &str); 9] = [
            (Method::GET, "/admin/api/keys"),
            (Method::POST, "/admin/api/keys"),
            (Method::POST, "/admin/api/keys/bulk"),
            (Method::POST, KEYS_PURGE_PATH),
            (Method::GET, "/admin/api/keys/xx/reveal"),
            (Method::DELETE, "/admin/api/keys/xx"),
            (Method::PATCH, "/admin/api/keys/xx"),
            (Method::GET, "/admin/api/keys/xx/usage"),
            (Method::POST, "/admin/api/keys/xx/verify"),
        ];
        assert_eq!(probes.len(), KeysEndpoint::ALL.len(), "端点表长对不上");
        for (method, uri) in probes {
            let req = Request::builder()
                .method(method.clone())
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let res = h.app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} 没在闸后面"
            );
        }
    }

    /// 🔴 **没配 `ADMIN_TOKEN` 时这九条一条都不存在（404）** ——
    /// 不泄漏「这里有个后台」。
    #[tokio::test]
    async fn without_an_admin_token_none_of_the_keys_endpoints_exist() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        assert!(state.admin_token.is_none());
        let app = build_router(state);
        for uri in [
            "/admin/api/keys",
            "/admin/api/keys/xx/usage",
            KEYS_PURGE_PATH,
        ] {
            let req = Request::builder()
                .uri(uri)
                .header("x-admin-key", ADMIN)
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }
}
