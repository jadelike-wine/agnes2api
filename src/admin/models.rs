//! `/admin/api/models` 与 `POST /admin/api/models/{id}/test`（02 §9.3、§9.5）。
//!
//! # ⚠️ 本文件里住着一件「本该在别处」的东西，如实登记
//!
//! [`PROTOCOLS`] / [`MEDIA_ENDPOINTS`] / [`MODEL_CATALOG`] 是「**怎么调这个网关**」的
//! **单一真源**，按模块布局它该住在 `core/admin/protocol_catalog.rs`。
//! 那个文件本轮还不存在，而 `src/core/` 不归本面改 ⇒ **先住在这里**，
//! 体例与 [`crate::admin::keys`] 文件头登记的三件事逐字相同：
//! 将来 `core/admin/protocol_catalog.rs` 落地时整体搬过去，**判据不必改**
//! （它们钉的是目录内容与端点行为，不是谁持有这张表）。
//!
//! ⚠️ 搬迁时唯一要当心的：本模块是 `pub`，别在 `core` 侧再留第二份 ——
//! 「同一份知识做两遍必漂，而漂了没人会发现」正是这张表存在的全部理由。
//!
//! # 这张表今天有几个消费者
//!
//! 集成示例卡、Playground、模型表（三者都读 `GET /admin/api/models`）、
//! 以及本文件的 [`test`]（它要 `upstream_path` 与 `sample()`）。
//!
//! ⚠️ `path_template`（**客户端怎么调这个网关**）与 `upstream_path`
//! （**网关怎么调上游**）是两个问题，别合并：取错一个不会有编译错误，
//! 而漂了没人会发现。四条协议今天的 `upstream_path` 恰好全是 `/chat/completions`,
//! **「今天一样」不是硬编码的理由**。

use crate::admin::keys::{ProbeAcquire, ProbeGuard, probe_guard};
use crate::core::keypool::select_key;
use crate::core::protocol::openai::Modality;
use crate::ports::fetcher::{FetchError, OutboundRequest};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::errors::GatewayError;
use crate::server::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

// ── 1. 协议目录（单一真源）──────────────────────────────────────────────────

/// 四条协议的样例请求里那句用户输入。**具名常量，不是散在四处的魔法字符串。**
///
/// ⚠️ 面板的 Playground 靠「在 `sampleBody` 里找到这句话并替换掉」注入用户输入。
/// 它与 `sampleBody` 一旦分叉，Playground 会**静默丢弃用户输入、恒发这句样例**，
/// 面板上完全看不出来 —— 由 [`tests::sample_prompt_occurs_exactly_once_per_sample_body`]
/// 在**端点真吐出去的那份 JSON 上**钉着（不是比对本文件自己的两个常量，那是同义反复）。
pub const SAMPLE_PROMPT: &str = "ping";

/// 四条对话协议的标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolId {
    Openai,
    Anthropic,
    Responses,
    Gemini,
}

impl ProtocolId {
    /// 线上的字面量。🔴 **面板按它查文案、按它分组，一个字都不许改。**
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Responses => "responses",
            Self::Gemini => "gemini",
        }
    }
}

/// 一条对话协议。
pub struct ProtocolEntry {
    pub id: ProtocolId,
    /// 展示名。**不进 i18n**：它是协议的专名，翻译它只会制造歧义。
    pub label: &'static str,
    pub method: &'static str,
    /// **网关对外**的路径模板（客户端打这一条）。`{model}` 是唯一允许的占位符。
    pub path_template: &'static str,
    /// **网关对上游**的路径（拼在 `agnes_base_url` 之后）。见文件头。
    pub upstream_path: &'static str,
    /// 这条协议惯用的鉴权头。⚠️ **它回答「客户端用哪个头调网关」**，
    /// 出站一律 `Authorization: Bearer`，别拿这一格去拼出站头。
    pub auth_header: &'static str,
    /// 流式怎么开：`body` = 请求体加一个字段；`path` = 换一条路径（Gemini）。
    pub stream_mode: &'static str,
    /// `stream_mode == "body"` 时是那个字段名；`"path"` 时是流式路径模板。
    pub stream_key: &'static str,
    /// 非流式响应里 usage 的位置。
    ///
    /// ⚠️ `None` **不是「这条协议没有 usage」，是「网关这条路径不解析响应体」**：
    /// openai 那条是四条里唯一原样透传的，usage 确实到得了客户端，只是网关没读它。
    pub usage_path: Option<&'static [&'static str]>,
    /// 流式响应里「这一块新增的回答文本」在一条 SSE 数据行的哪一格。
    ///
    /// ⚠️⚠️ **照本网关真正吐出去的字节填，不是照协议规范抄**：四条里三条的事件
    /// 由本仓自己合成，照规范抄会在那三条上漂。数字段视为数组下标。
    pub stream_text_path: &'static [&'static str],
}

impl ProtocolEntry {
    /// 这条协议的最小可跑请求体。
    ///
    /// ⚠️ **写成 `match` 而不是一格函数指针**：函数指针在常量表里读起来像数据，
    /// 而这四份请求体的形状差异（`max_tokens` 必填、`input` vs `messages` vs `contents`）
    /// 正是这张表要说清楚的东西。
    #[must_use]
    pub fn sample(&self, model: &str) -> Value {
        match self.id {
            ProtocolId::Openai => json!({
                "model": model,
                "messages": [{ "role": "user", "content": SAMPLE_PROMPT }],
            }),
            // `max_tokens` 是 Anthropic 协议的必填项，少了它上游会 400。
            ProtocolId::Anthropic => json!({
                "model": model,
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": SAMPLE_PROMPT }],
            }),
            ProtocolId::Responses => json!({ "model": model, "input": SAMPLE_PROMPT }),
            // ⚠️ Gemini 把模型名拼进路径，请求体里**没有** model 这一格。
            ProtocolId::Gemini => json!({
                "contents": [{ "role": "user", "parts": [{ "text": SAMPLE_PROMPT }] }],
            }),
        }
    }
}

/// 四条对话协议。**顺序是面板的展示顺序。**
pub const PROTOCOLS: [ProtocolEntry; 4] = [
    ProtocolEntry {
        id: ProtocolId::Openai,
        label: "OpenAI Chat Completions",
        method: "POST",
        path_template: "/v1/chat/completions",
        upstream_path: "/chat/completions",
        auth_header: "authorization",
        stream_mode: "body",
        stream_key: "stream",
        usage_path: None,
        // 上游字节原样透传 ⇒ 这里填的是**上游 chat 增量块**的形状。
        stream_text_path: &["choices", "0", "delta", "content"],
    },
    ProtocolEntry {
        id: ProtocolId::Anthropic,
        label: "Anthropic Messages",
        method: "POST",
        path_template: "/v1/messages",
        upstream_path: "/chat/completions",
        auth_header: "x-api-key",
        stream_mode: "body",
        stream_key: "stream",
        usage_path: Some(&["usage"]),
        // ⚠️ 同一条流里的 `message_delta` 也有一格 `delta`（装 `stop_reason`），
        //    它没有 `text` ⇒ 走到底取不到字符串 ⇒ 按「这一行不带正文」处理。
        stream_text_path: &["delta", "text"],
    },
    ProtocolEntry {
        id: ProtocolId::Responses,
        label: "OpenAI Responses",
        method: "POST",
        path_template: "/v1/responses",
        upstream_path: "/chat/completions",
        auth_header: "authorization",
        stream_mode: "body",
        stream_key: "stream",
        usage_path: Some(&["usage"]),
        // ⚠️ **这一条的 `delta` 是字符串本身，不是一个对象** —— 与 anthropic 那条
        //    同名不同形。这正是「照协议规范抄」会翻车的地方。
        stream_text_path: &["delta"],
    },
    ProtocolEntry {
        id: ProtocolId::Gemini,
        label: "Google Gemini generateContent",
        method: "POST",
        // Gemini 把方法名以**最后一个冒号**后缀附在模型名之后。
        path_template: "/v1beta/models/{model}:generateContent",
        upstream_path: "/chat/completions",
        auth_header: "x-goog-api-key",
        stream_mode: "path",
        stream_key: "/v1beta/models/{model}:streamGenerateContent",
        usage_path: Some(&["usageMetadata"]),
        stream_text_path: &["candidates", "0", "content", "parts", "0", "text"],
    },
];

/// 媒体端点的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaOp {
    Generate,
    Poll,
}

impl MediaOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Poll => "poll",
        }
    }
}

/// 媒体那三条端点里的一条。
///
/// ⚠️ **它与 [`ProtocolEntry`] 刻意是两张表，分界不是「对话 vs 媒体」这个分类癖，
/// 而是形状真的不同**：对话四条有 `stream_mode` / `stream_text_path` / `usage_path`
/// 三格媒体一格都用不上，媒体三条有 `op` / `task_slot` 两格对话一格都用不上。
/// 合成一张表的代价是每一条都带着一半恒为 `null` 的字段，
/// 而**恒为 null 的字段与「这一条没填」在消费侧长得一模一样**。
pub struct MediaEndpointEntry {
    pub id: &'static str,
    pub modality: Modality,
    pub op: MediaOp,
    pub method: &'static str,
    /// **网关对外**的路径。带任务标识的那条用 `task_slot` 里那个 token 占位。
    pub path_template: &'static str,
    /// **网关对上游**的路径。占位符与上面同一个。
    pub upstream_path: &'static str,
    pub auth_header: &'static str,
    /// 两条路径里那个任务标识占位符。`None` = 这一条不带任务标识。
    pub task_slot: Option<&'static str>,
}

impl MediaEndpointEntry {
    /// 最小可跑请求体；`None` = 这一条没有请求体（GET）。
    ///
    /// ⚠️ `None` **不是「忘了填」**：面板据它决定这一条不走提示词注入，
    /// 而一个空对象会让它去找那句占位文本、找不到、判成整份读不出来。
    #[must_use]
    pub fn sample(&self, model: &str) -> Option<Value> {
        match self.op {
            MediaOp::Generate => Some(json!({ "model": model, "prompt": SAMPLE_PROMPT })),
            MediaOp::Poll => None,
        }
    }
}

/// 三条媒体端点（图片一条同步、视频两段式）。
pub const MEDIA_ENDPOINTS: [MediaEndpointEntry; 3] = [
    MediaEndpointEntry {
        id: "image.generate",
        modality: Modality::Image,
        op: MediaOp::Generate,
        method: "POST",
        path_template: "/v1/images/generations",
        upstream_path: "/images/generations",
        auth_header: "authorization",
        task_slot: None,
    },
    MediaEndpointEntry {
        id: "video.create",
        modality: Modality::Video,
        op: MediaOp::Generate,
        method: "POST",
        path_template: "/v1/videos",
        upstream_path: "/videos",
        auth_header: "authorization",
        task_slot: None,
    },
    MediaEndpointEntry {
        id: "video.poll",
        modality: Modality::Video,
        op: MediaOp::Poll,
        method: "GET",
        path_template: "/v1/videos/{id}",
        upstream_path: "/videos/{id}",
        auth_header: "authorization",
        task_slot: Some("{id}"),
    },
];

/// 一条端点（方法 + 对外路径）。
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
}

/// 模型目录里的一条。
pub struct CatalogModel {
    pub id: &'static str,
    pub modality: Modality,
    /// 这个模型在哪几条对话协议上可用。图片/视频模型是**空**。
    pub protocols: &'static [ProtocolId],
    /// 这个模型真正要打的端点。
    pub endpoints: &'static [Endpoint],
}

const CHAT_PROTOCOLS: &[ProtocolId] = &[
    ProtocolId::Openai,
    ProtocolId::Anthropic,
    ProtocolId::Responses,
    ProtocolId::Gemini,
];

const IMAGE_ENDPOINTS: &[Endpoint] = &[Endpoint {
    method: "POST",
    path: "/v1/images/generations",
}];

const VIDEO_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "POST",
        path: "/v1/videos",
    },
    Endpoint {
        method: "GET",
        path: "/v1/videos/{id}",
    },
];

/// 一条对话模型的四条端点。`{model}` 已经展开成字面量 —— 见 [`MODEL_CATALOG`]。
macro_rules! chat_endpoints {
    ($model:literal) => {
        &[
            Endpoint {
                method: "POST",
                path: "/v1/chat/completions",
            },
            Endpoint {
                method: "POST",
                path: "/v1/messages",
            },
            Endpoint {
                method: "POST",
                path: "/v1/responses",
            },
            Endpoint {
                method: "POST",
                path: concat!("/v1beta/models/", $model, ":generateContent"),
            },
        ]
    };
}

/// 模型目录。
///
/// ⚠️ **id 必须与 [`crate::core::protocol::openai::MODELS`] 逐条一致**（连顺序），
/// 由 [`tests::catalog_ids_match_the_models_endpoint_source_exactly`] 钉着 ——
/// **不是「包含」**：少一个、多一个、顺序不同都要红。那份 `MODELS` 是
/// `/v1/models` 与 `/v1beta/models` 的来源，本目录多写一个就是在面板上
/// 承诺一个网关不认的 id。
///
/// ⚠️ **形态逐条手写，不从模型名里的关键字推**：从名字推形态等于把一条上游命名
/// 约定当成本仓的控制流 —— 上游哪天发一个不带 `image` 的图片模型，推导会静默把它
/// 归成对话模型并教出一条注定 4xx 的调法，而没有任何东西会红。
///
/// ⚠️ **`endpoints` 逐条手写，不从 [`PROTOCOLS`] 生成**：生成会把
/// [`tests::chat_models_endpoints_match_the_protocol_table`] 变成同义反复
/// （它比的正是手写的这一端对 `PROTOCOLS` 的推导值）。
pub const MODEL_CATALOG: [CatalogModel; 12] = [
    CatalogModel {
        id: "agnes-2.0-flash",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-2.0-flash"),
    },
    CatalogModel {
        id: "agnes-2.5-flash",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-2.5-flash"),
    },
    CatalogModel {
        id: "agnes-2.5-pro",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-2.5-pro"),
    },
    CatalogModel {
        id: "agnes-2.5-pro-alpha",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-2.5-pro-alpha"),
    },
    CatalogModel {
        id: "agnes-2.5-pro-beta",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-2.5-pro-beta"),
    },
    CatalogModel {
        id: "agnes-3.0-flash",
        modality: Modality::Chat,
        protocols: CHAT_PROTOCOLS,
        endpoints: chat_endpoints!("agnes-3.0-flash"),
    },
    CatalogModel {
        id: "agnes-image-2.1-flash",
        modality: Modality::Image,
        protocols: &[],
        endpoints: IMAGE_ENDPOINTS,
    },
    CatalogModel {
        id: "agnes-image-2.0-flash",
        modality: Modality::Image,
        protocols: &[],
        endpoints: IMAGE_ENDPOINTS,
    },
    CatalogModel {
        id: "agnes-image-2.5-flash",
        modality: Modality::Image,
        protocols: &[],
        endpoints: IMAGE_ENDPOINTS,
    },
    CatalogModel {
        id: "agnes-video-v2.0",
        modality: Modality::Video,
        protocols: &[],
        endpoints: VIDEO_ENDPOINTS,
    },
    CatalogModel {
        id: "agnes-video-2.5",
        modality: Modality::Video,
        protocols: &[],
        endpoints: VIDEO_ENDPOINTS,
    },
    CatalogModel {
        id: "agnes-video-2.5-flash",
        modality: Modality::Video,
        protocols: &[],
        endpoints: VIDEO_ENDPOINTS,
    },
];

/// 形态的线上字面量。🔴 **面板按它分组，一个字都不许改。**
const fn modality_str(m: Modality) -> &'static str {
    match m {
        Modality::Chat => "chat",
        Modality::Image => "image",
        Modality::Video => "video",
    }
}

/// 目录里的一条对话模型。**只查目录，零存储读、零网络。**
#[must_use]
pub fn catalog_model(id: &str) -> Option<&'static CatalogModel> {
    MODEL_CATALOG.iter().find(|m| m.id == id)
}

/// 按 id 取一条协议。
#[must_use]
pub fn protocol_by_id(id: ProtocolId) -> &'static ProtocolEntry {
    // `expect` 不可达：`ProtocolId` 是闭集，四格在 `PROTOCOLS` 里各一条，
    // 由 `protocol_table_covers_every_id` 钉着。
    PROTOCOLS
        .iter()
        .find(|p| p.id == id)
        .expect("PROTOCOLS 必须覆盖 ProtocolId 的每一格")
}

// ── 2. `GET /admin/api/models` 的响应体 ────────────────────────────────────
//
// 🔴 **一律结构体，不用 `json!` 宏 —— 键序是对外契约。**

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolWire {
    id: ProtocolId,
    label: &'static str,
    method: &'static str,
    path_template: &'static str,
    upstream_path: &'static str,
    auth_header: &'static str,
    stream_mode: &'static str,
    stream_key: &'static str,
    usage_path: Option<&'static [&'static str]>,
    stream_text_path: &'static [&'static str],
    sample_body: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaWire {
    id: &'static str,
    modality: &'static str,
    op: &'static str,
    method: &'static str,
    path_template: &'static str,
    upstream_path: &'static str,
    auth_header: &'static str,
    task_slot: Option<&'static str>,
    sample_body: Option<Value>,
}

#[derive(Serialize)]
struct EndpointWire {
    method: &'static str,
    path: &'static str,
}

#[derive(Serialize)]
struct ModelWire {
    id: &'static str,
    modality: &'static str,
    protocols: Vec<&'static str>,
    endpoints: Vec<EndpointWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogBody {
    protocols: Vec<ProtocolWire>,
    media: Vec<MediaWire>,
    models: Vec<ModelWire>,
    sample_prompt: &'static str,
    /// 🔴 **已经判好的视图**（06 §5.3 的 A 档）。见
    /// [`crate::core::admin::view::models`]。
    view: ModelsViewWire,
}

/// 目录那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelsViewWire {
    /// **与 `models` 同序同长**，逐条给出那一行的「协议 × 可用性」矩阵。
    rows: Vec<ModelRowViewWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelRowViewWire {
    /// 与 `models[i].id` 相同（面板不必靠下标信任）。
    id: &'static str,
    /// 形态那一列的文案键。`null` = **目录里出现了一个这张表还不认识的形态**
    /// ⇒ 渲染层把原值照实画出来，**绝不冒充任何一档已知形态**。
    modality_label_key: Option<&'static str>,
    /// 🔴 **恒与 `protocols` 等长**，不可用的那几格 `available: false`。
    /// 把不可用的过滤掉之后，一个只在一条协议上可用的模型那一行会是空的
    /// —— **与「读不出来」长得一模一样**。
    protocols: Vec<ProtocolBadgeWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolBadgeWire {
    id: &'static str,
    label: &'static str,
    available: bool,
    badge_class: &'static str,
}

/// 序列化给面板的那一份。**`sample()` 是函数，过不了网络，这里替换成算好的请求体。**
///
/// ⚠️ **`samplePrompt` 必须与 `sampleBody` 里那句话同源**（都从 [`SAMPLE_PROMPT`] 来）：
/// 面板靠「在 `sampleBody` 里找到这句话并把它换成用户输入」注入提示词，两边一旦分叉，
/// Playground 会**静默丢弃用户输入、恒发样例那句话**。
///
/// ⚠️ 媒体那三条的样例模型**不沿用对话默认模型**：样例值是**画给人看的**，
/// 写一个图片端点配对话模型的组合就是在教一条注定 4xx 的调法。
/// 找不到对应形态的模型时给空串 —— **不是抛**：模型清单少一个形态是一条运营事实，
/// 让整个面板的目录请求 500 是把它放大成一次故障。
fn catalog_payload() -> CatalogBody {
    let default_model = MODEL_CATALOG[0].id;
    let model_for = |m: Modality| -> &'static str {
        MODEL_CATALOG
            .iter()
            .find(|x| x.modality == m)
            .map_or("", |x| x.id)
    };
    CatalogBody {
        protocols: PROTOCOLS
            .iter()
            .map(|p| ProtocolWire {
                id: p.id,
                label: p.label,
                method: p.method,
                path_template: p.path_template,
                upstream_path: p.upstream_path,
                auth_header: p.auth_header,
                stream_mode: p.stream_mode,
                stream_key: p.stream_key,
                usage_path: p.usage_path,
                stream_text_path: p.stream_text_path,
                sample_body: p.sample(default_model),
            })
            .collect(),
        media: MEDIA_ENDPOINTS
            .iter()
            .map(|e| MediaWire {
                id: e.id,
                modality: modality_str(e.modality),
                op: e.op.as_str(),
                method: e.method,
                path_template: e.path_template,
                upstream_path: e.upstream_path,
                auth_header: e.auth_header,
                task_slot: e.task_slot,
                sample_body: e.sample(model_for(e.modality)),
            })
            .collect(),
        models: MODEL_CATALOG
            .iter()
            .map(|m| ModelWire {
                id: m.id,
                modality: modality_str(m.modality),
                protocols: m.protocols.iter().map(|p| p.as_str()).collect(),
                endpoints: m
                    .endpoints
                    .iter()
                    .map(|e| EndpointWire {
                        method: e.method,
                        path: e.path,
                    })
                    .collect(),
            })
            .collect(),
        sample_prompt: SAMPLE_PROMPT,
        view: models_view(),
    }
}

/// 拼目录那份视图。**判定一律转调 [`crate::core::admin::view::models`]。**
///
/// ⚠️ **协议目录的顺序就是 [`PROTOCOLS`] 的顺序**，而那也是 `protocols` 那一格
/// 的序列化顺序 ⇒ 矩阵的列序与表头的列序天然同源，面板不必自己对齐。
fn models_view() -> ModelsViewWire {
    use crate::core::admin::view::models as v;

    let catalog: Vec<(&'static str, &'static str)> =
        PROTOCOLS.iter().map(|p| (p.id.as_str(), p.label)).collect();
    ModelsViewWire {
        rows: MODEL_CATALOG
            .iter()
            .map(|m| {
                let owned: Vec<&'static str> = m.protocols.iter().map(|p| p.as_str()).collect();
                ModelRowViewWire {
                    id: m.id,
                    modality_label_key: v::modality_label_key(modality_str(m.modality)),
                    protocols: v::protocol_badges(&owned, &catalog)
                        .into_iter()
                        .map(|b| ProtocolBadgeWire {
                            id: b.id,
                            label: b.label,
                            available: b.available,
                            badge_class: b.badge_class(),
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

/// 目录端点在 `/admin/api` 之下的子路径。**路由表从这里取，不许手抄字面量。**
pub const MODELS_SUBPATH: &str = "/models";

/// 连通性测试端点的子路径。
///
/// ⚠️ **占位符写成 axum 的 `{id}`**：同一个字符串既是路由的注册模式、
/// 又是面板要替换的模板，两者因此不可能分叉。
pub const MODEL_TEST_SUBPATH: &str = "/models/{id}/test";

/// `GET /admin/api/models` —— **零存储读、零网络。**
///
/// 它和 `capabilities` 一样是面板启动必调的静态数据，让它读一次存储就等于给
/// 每一次刷新加一次整份 `store.json` 的读+解析。
/// 由 [`tests::the_models_endpoint_never_touches_storage`] 数着四个桶钉住
/// （**四个桶都数，只数 `get` 的版本漏掉的恰恰是写那一侧**）。
pub async fn list() -> Response {
    Json(catalog_payload()).into_response()
}

// ── 3. `POST /admin/api/models/{id}/test` ──────────────────────────────────

/// 护栏的 kind。
///
/// 🔴 **它是常量，不是 `model-test:<id>`，这一条与验活刻意相反。**
///
/// 验活用带 id 的 kind（`verify:<id>`）是因为那 20 把 key 是 20 个**互不相干**的对象。
/// 而这里恰恰**要**它们互相挡：**逐模型测试串起来的每一次，打的都是同一个上游账号**。
/// 共用一个 kind ⇒ 整轮测试受同一个最小间隔约束，那就是**扛上游边缘限流的那道闸**
/// （上游约 2 次快请求就回 1015，一片红回来之后运维分不清是模型不通还是被限流了）。
/// 换成 `model-test:<id>` 的后果不是「粒度更细」，是**整道闸消失**。
///
/// ⚠️⚠️ **TS 版曾在这里写下一句假话，而 v0.3.0 的面板正是照着那句话写的**：
/// 「共用一个 kind ⇒ 整轮测试自动被摊到最小间隔上」。真相是**共用一个 kind 只会
/// 截断一轮，摊开一轮必须由发起方限速** —— 零间隔的串行循环撞上这道闸的结果是
/// 整轮 824ms 跑完、6 行里 5 行 `probe_cooldown`，也就是把**我们自己的节流**
/// 当成一张连通性矩阵交给运维，而那 5 行盖住的可能是真的不通的模型。
///
/// ⇒ **两半缺一不可，而且两半各有一格判据**：
/// · 缺发起方那一半 ⇒ [`tests::a_zero_interval_sweep_only_ever_reaches_the_first_model`]
///   （整轮只测得到第一个模型 —— 那次实测长什么样）；
/// · 有发起方那一半 ⇒ [`tests::a_paced_sweep_really_tests_every_single_model`]
///   （N 个模型**逐个真的打到了上游**，且 model 那一格逐条不同）。
pub const MODEL_TEST_KIND: &str = "model-test";

/// 连通性测试的结果。**只回状态码，一个字节的上游正文都不回。**
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestBody {
    ok: bool,
    status: Option<u16>,
    latency_ms: i64,
    reason: Option<&'static str>,
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

/// 429 那两种拒绝的文案。
///
/// ⚠️ **[`crate::admin::keys::ProbeAcquire`] 上还有一份同语义的 `message()`，
/// 但它今天是私有的，而 `keys.rs` 不归本面改。** `message` 不是对外契约
/// （`reason` 才是），所以两处措辞可以不同；哪天要统一，把那个方法改成 `pub`
/// 并在这里调它 —— **别反过来在那边再抄一份本文件的串**。
const fn busy_message(a: ProbeAcquire) -> &'static str {
    match a {
        ProbeAcquire::Ok => "",
        ProbeAcquire::InFlight => {
            "上一次模型测试还没有返回，等它结束再试（这颗按钮每按一次都会真的打一次外网）"
        }
        ProbeAcquire::Cooldown => "两次模型测试之间至少要隔一小段时间，请稍后再试",
    }
}

/// 「这条端点不收任何选项」那道闸：空体放行、`{}` 放行，其余一律 400。
///
/// ⚠️ **空体必须放行**：面板与枚举式鉴权矩阵都是**不带体**调这条 POST 的。
/// ⚠️ **不静默忽略请求体**：`{"model":"agnes-2.5-pro"}` 这种「我以为能指定模型」
/// 的写法在宽松实现下是一次「点了、但完全没按你想的那样发生」的静默误操作。
fn reject_any_body(body: &Bytes) -> Result<(), GatewayError> {
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
        format!("模型测试不接受任何参数，不认识的字段：{extra}"),
    ))
}

/// `POST /admin/api/models/{id}/test` —— **真打一次对话**。
///
/// 骨架取自 [`crate::admin::keys::verify`]，那六条约束在这里原样成立
/// （零存储写、上游正文一个字节都不回、走注入的 `Fetcher`、自带首字节超时、
/// 上游路径经协议目录取）。下面只写**与那条端点不同的三处**。
///
/// ── 不同点 ①：**选谁的 key** —— 随便一把能用的，不是指定的那一把 ────────────
///
/// 验活回答「**这一把** key 好不好用」所以直读存储；这一条回答「**这个模型**通不通」，
/// key 只是手段 ⇒ 走 `repo.all()` 那份进程内快照 + `select_key`。
/// ⚠️ 代价明写：快照最多是一个 `pool_cache_ttl_ms` 之前的，刚加完 key 就点这颗按钮
/// 仍可能落到 `no_key`。**这里可以** —— 我们只需要「随便一把能用的」。
/// **判据是 `select_key()` 而不是「池子长度为 0」**：一池全在冷却 / 全被停用时
/// 长度不为 0 而一把都用不了，那两种状态在这条端点上是同一件事。
///
/// ── 不同点 ②：**模型参数化，而且只接受对话模型** ───────────────────────────
///
/// 🔴 **只接受 `Modality::Chat`，image / video 一律 400 `modality_not_testable`。
/// 这是硬边界，不是保守**：测一次图片模型 = **真生成一张图**；测一次视频模型 =
/// **建一个任务 + 反复轮询**。把这两样塞进一颗「全部测一遍」的按钮，就是把一颗按钮
/// 做成自毁按钮 —— 运维以为自己在做连通性检查，实际在替这个账号烧生成额度，
/// 而**那笔消耗按下去之后收不回来**。
/// ⚠️ 面板那一侧也筛过一遍，但**两处不是一条判据的两半**：curl / 脚本同样是它的
/// 调用方，面板筛掉了不等于这里可以放宽。
///
/// ── 不同点 ③：护栏的 kind 是常量 ──────────────────────────────────────────
/// 见 [`MODEL_TEST_KIND`]。
///
/// ⚠️ **护栏排在 404 / 400 / 选 key 之后**：一次注定失败的调用不该消费一次最小间隔
/// （那等于「点了一下、什么都没打，但接下来几秒不许再点」）。
pub async fn test(State(st): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    test_with(&st, &id, &body, probe_guard(), &crate::adapters::now_ms).await
}

/// [`test`] 的本体，**护栏与时钟可注入**。
///
/// 🔴 **它不是「为测试而拆」的样板**：`MODEL_TEST_KIND` 是常量，一轮 N 个模型的
/// 扫描在真墙钟下要 `N × PROBE_MIN_INTERVAL_MS` 才跑得完
/// ⇒ 「N 个模型真的都测到了」这条性质在固定全局单例 + 真墙钟上**不可观测**，
/// 而那正是 TS 版那个缺陷藏了一整版的地方。可注入的那一格是那条判据的前提。
pub async fn test_with(
    st: &AppState,
    id: &str,
    body: &Bytes,
    guard: &ProbeGuard,
    now: &(dyn Fn() -> i64 + Sync),
) -> Response {
    // 校验放在最前：**一次存储读都不产生、一格护栏都不消费**。
    if let Err(e) = reject_any_body(body) {
        return e.into_response();
    }

    let Some(entry) = catalog_model(id) else {
        // ⚠️ **不带 `code` 的网关信封**（02 §4.4 那处刻意的不一致）：
        //    `AdminErrorCode` 是闭集，里面没有「模型目录里没有这个」这一格。
        return GatewayError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "这个网关的模型目录里没有这个模型",
        )
        .into_response();
    };

    if entry.modality != Modality::Chat {
        return (
            StatusCode::BAD_REQUEST,
            Json(WithReason {
                error: GatewayInner {
                    r#type: "invalid_request_error",
                    message: "只有对话模型能做连通性测试：测一次图片模型会真的生成一张图，\
                              测一次视频模型会建一个任务并反复轮询，\
                              两者都会真的花掉这个账号的生成额度"
                        .to_string(),
                },
                reason: "modality_not_testable",
            }),
        )
            .into_response();
    }

    let at = now();
    let records = match st.repo.all(at).await {
        Ok(v) => v,
        Err(e) => {
            // 🔴 **500 不许静默** —— 运维要能在事件流里看见它。
            st.logger.log(
                LogEntry::new(LogLevel::Error, "admin.storage_failed", "管理树读存储失败")
                    .with("op", "models.test")
                    .with("error", e.to_string()),
            );
            return GatewayError::internal().into_response();
        }
    };
    let Some((idx, _)) = select_key(&records, 0, at) else {
        // 空池不是错误，是一句要说清的话。**200，不是 500、也不是转圈。**
        return Json(TestBody {
            ok: false,
            status: None,
            latency_ms: 0,
            reason: Some("no_key"),
        })
        .into_response();
    };
    let picked = &records[idx];

    // 🔴 **走 `acquire()` 拿一张租约，不手写 `release`。**
    // 手写配对只覆盖得到「上游回了」「上游报错」两条支，**漏掉第三条：请求被取消**
    // —— 客户端一断开，axum 直接 drop 这个 future，那一行 `release` 根本不执行。
    // ⚠️ 本端点的 kind 是**常量** ⇒ 卡住的不是某一把 key，是整颗「全部测一遍」按钮，
    // 而这道闸不像最小间隔那道会自愈（只能重启进程）。
    // 租约的 drop 在「正常返回 / 提前 return / panic 展开 / 被取消」四种收场里都走到，
    // 由 [`tests::a_cancelled_request_does_not_wedge_the_button_forever`] 钉住。
    let lease = match guard.acquire(MODEL_TEST_KIND, now()) {
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
    // 探测固定用 openai 那一条。**四条协议的 `upstream_path` 今天完全相同 ⇒
    // 选哪条对出站毫无差别**，选它只因为它是四条的参照系。**仍然经目录取** ——
    // 「今天一样」不是硬编码的理由。
    let proto = protocol_by_id(ProtocolId::Openai);
    // 计时起点：**只读这一次**。
    let started_at = now();
    let req = OutboundRequest {
        method: proto.method.to_string(),
        url: format!(
            "{}{}",
            picked.upstream_base_url(&cfg.agnes_base_url),
            proto.upstream_path
        ),
        headers: vec![
            (
                header::CONTENT_TYPE.as_str().to_string(),
                "application/json".to_string(),
            ),
            // ⚠️ **刻意不取 `proto.auth_header`**：那一格回答「客户端用哪个头调这个
            //    网关」，而上游只认 `Authorization: Bearer`。
            (
                header::AUTHORIZATION.as_str().to_string(),
                format!("Bearer {}", picked.key),
            ),
        ],
        // **这就是「逐模型」那一半**：同一份最小请求体，只有 `model` 这一格在变。
        body: Some(Bytes::from(proto.sample(entry.id).to_string().into_bytes())),
        // 首字节档（默认 8 秒）：这条端点只要一个响应头就够了，
        // 给它同步档（默认 2 分钟）等于让面板挂两分钟。
        timeout_ms: cfg.upstream_timeout_ms,
    };
    let sent = st.fetcher.send(req).await;
    // 上游那一段到此为止 ⇒ **显式还闸**，别把它拖到序列化之后
    // （「最小间隔」量的是两次**发起**之间的距离，多占那一会儿是白占）。
    // ⚠️ 下面两条支里都**没有** `release`：那不是漏了，是租约在管
    // —— 包括手写配对到不了的第三条支「请求被取消」。
    drop(lease);
    let latency_ms = now() - started_at;

    match sent {
        Ok(res) => {
            let status = res.status;
            // ⚠️ 响应体在这里被丢掉（流被 drop）—— 它同时是「一个字节都不回」的
            //    **物理**保证：正文从来没有被读进来过。
            drop(res);
            let ok = (200..300).contains(&status);
            Json(TestBody {
                ok,
                status: Some(status),
                latency_ms,
                // ⚠️ 非 2xx 时带真实状态码 + `upstream_error`：这张矩阵的每一行只有
                //    一格结果，「上游没正常回」本身就是那一格要说的话。
                reason: if ok { None } else { Some("upstream_error") },
            })
            .into_response()
        }
        Err(e) => Json(TestBody {
            ok: false,
            status: None,
            latency_ms,
            // **机器可读的 code，不是异常消息**：异常消息里可能带上游 URL。
            reason: Some(match e {
                FetchError::TimeoutBeforeHeaders => "timeout",
                FetchError::Connect(_) | FetchError::BodyInterrupted(_) => "network_error",
            }),
        })
        .into_response(),
    }
}

// ── 判据 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::admin::keys::PROBE_MIN_INTERVAL_MS;
    use crate::core::protocol::openai::MODELS;
    use crate::server::app::build_router;
    use crate::server::test_support::{
        RecordingLogger, ScriptedFetcher, TestApp, UpstreamReply, state_with, state_with_storage,
    };
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    struct Harness {
        app: Router,
        state: AppState,
        recorded: Arc<RecordingLogger>,
    }

    async fn harness_with(
        storage: Arc<dyn crate::ports::storage::Storage>,
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

    async fn get_json(h: &Harness, uri: &str) -> (StatusCode, Value) {
        let (s, b) = call(h, Method::GET, uri, None).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    async fn post_json(h: &Harness, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
        let (s, b) = call(h, Method::POST, uri, body).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    /// 把一份响应体解析出来的 `Value` 当 JSON 文本再吐回去（找子串用）。
    fn text_of(v: &Value) -> String {
        v.to_string()
    }

    // ── 目录本身 ───────────────────────────────────────────────────────────

    /// 🔴 **模型 id 与 `/v1/models` 的来源逐条一致 —— 相等，不是包含。**
    ///
    /// 期望值取自另一个真源（`openai::MODELS`），不是从被测对象现算。
    /// 回归对象：往目录里补一个上游新模型而忘了补 `MODELS`（或反过来）⇒
    /// 面板承诺一个网关不认的 id，或 `/v1/models` 列出一个面板画不出来的 id。
    #[test]
    fn catalog_ids_match_the_models_endpoint_source_exactly() {
        let catalog: Vec<&str> = MODEL_CATALOG.iter().map(|m| m.id).collect();
        let source: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
        assert_eq!(catalog, source);
        // 形态也必须逐条一致 —— 只比 id 的话，把一个视频模型在目录里写成 chat
        // 仍然全绿，而那正好会让「全部测一遍」去烧生成额度。
        let cat_modality: Vec<&str> = MODEL_CATALOG
            .iter()
            .map(|m| modality_str(m.modality))
            .collect();
        let src_modality: Vec<&str> = MODELS.iter().map(|m| modality_str(m.modality)).collect();
        assert_eq!(cat_modality, src_modality);
    }

    /// 目录的三种形态各有几条 —— **期望值手写**（6 + 3 + 3 = 12）。
    #[test]
    fn the_catalog_has_six_chat_three_image_and_three_video_models() {
        let count = |m: Modality| MODEL_CATALOG.iter().filter(|x| x.modality == m).count();
        assert_eq!(count(Modality::Chat), 6);
        assert_eq!(count(Modality::Image), 3);
        assert_eq!(count(Modality::Video), 3);
        assert_eq!(MODEL_CATALOG.len(), 12);
    }

    /// `ProtocolId` 的每一格在 `PROTOCOLS` 里恰好一条 —— [`protocol_by_id`] 的
    /// 那句 `expect` 不可达，靠的就是这一格。
    #[test]
    fn protocol_table_covers_every_id() {
        for id in [
            ProtocolId::Openai,
            ProtocolId::Anthropic,
            ProtocolId::Responses,
            ProtocolId::Gemini,
        ] {
            assert_eq!(
                PROTOCOLS.iter().filter(|p| p.id == id).count(),
                1,
                "{} 必须恰好一条",
                id.as_str()
            );
        }
    }

    /// 四条协议的 `usagePath` 逐条钉死。
    ///
    /// 🔴 `openai` 的 `None` **不是「这条协议没有 usage」，是「网关不解析响应体」**。
    /// 期望值手写，回归对象：顺手把 openai 那格「补成」`["usage"]`，
    /// 面板于是对一条网关从没解析过的路径宣称拿得到 token 数。
    #[test]
    fn usage_paths_are_pinned_per_protocol() {
        let got: Vec<(&str, Option<Vec<&str>>)> = PROTOCOLS
            .iter()
            .map(|p| (p.id.as_str(), p.usage_path.map(<[&str]>::to_vec)))
            .collect();
        assert_eq!(
            got,
            vec![
                ("openai", None),
                ("anthropic", Some(vec!["usage"])),
                ("responses", Some(vec!["usage"])),
                ("gemini", Some(vec!["usageMetadata"])),
            ]
        );
    }

    /// 四条 `streamTextPath` 逐条钉死。
    ///
    /// ⚠️ responses 那条的 `delta` 是**字符串本身**，anthropic 那条的 `delta` 是
    /// **一个对象**（还要再往下走一层 `text`）—— 同名不同形，
    /// 这正是「照协议规范抄」会翻车的地方。
    #[test]
    fn stream_text_paths_are_pinned_per_protocol() {
        let got: Vec<(&str, Vec<&str>)> = PROTOCOLS
            .iter()
            .map(|p| (p.id.as_str(), p.stream_text_path.to_vec()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("openai", vec!["choices", "0", "delta", "content"]),
                ("anthropic", vec!["delta", "text"]),
                ("responses", vec!["delta"]),
                (
                    "gemini",
                    vec!["candidates", "0", "content", "parts", "0", "text"]
                ),
            ]
        );
    }

    /// 对话模型的 `endpoints` 与 `PROTOCOLS` 是同一批对外路径。
    ///
    /// **观测点是手写的那一端对 `PROTOCOLS` 的推导值**（`{model}` 展开），
    /// 所以它不是同义反复：目录里那四行是逐条手写的字面量。
    #[test]
    fn chat_models_endpoints_match_the_protocol_table() {
        for m in MODEL_CATALOG
            .iter()
            .filter(|m| m.modality == Modality::Chat)
        {
            let expected: Vec<String> = PROTOCOLS
                .iter()
                .map(|p| p.path_template.replace("{model}", m.id))
                .collect();
            let got: Vec<String> = m.endpoints.iter().map(|e| e.path.to_string()).collect();
            assert_eq!(got, expected, "{}", m.id);
            assert!(m.endpoints.iter().all(|e| e.method == "POST"));
        }
    }

    /// 媒体端点表与媒体模型的 `endpoints` 是同一批路径。
    ///
    /// 两张表是同一批路径的两个视图（一个按端点分组给机器、一个按模型分组给人看），
    /// **会漂** —— 这一格是唯一绑住它们的东西。
    #[test]
    fn media_endpoint_table_and_media_models_list_the_same_paths() {
        for modality in [Modality::Image, Modality::Video] {
            let from_table: Vec<(&str, &str)> = MEDIA_ENDPOINTS
                .iter()
                .filter(|e| e.modality == modality)
                .map(|e| (e.method, e.path_template))
                .collect();
            for m in MODEL_CATALOG.iter().filter(|m| m.modality == modality) {
                let from_models: Vec<(&str, &str)> =
                    m.endpoints.iter().map(|e| (e.method, e.path)).collect();
                assert_eq!(from_models, from_table, "{}", m.id);
            }
        }
    }

    /// 媒体模型一条对话协议都不许挂；对话模型四条全挂。
    ///
    /// 回归对象：给视频模型挂上 `protocols: CHAT_PROTOCOLS`（一次「顺手补全」）⇒
    /// 面板把视频模型列进 Playground 的对话下拉框，运维选中发一次对话，
    /// **白烧一把 key + 一次全网关共享的限流额度**。
    #[test]
    fn only_chat_models_carry_chat_protocols() {
        for m in &MODEL_CATALOG {
            let n = m.protocols.len();
            match m.modality {
                Modality::Chat => assert_eq!(n, 4, "{}", m.id),
                Modality::Image | Modality::Video => assert_eq!(n, 0, "{}", m.id),
            }
        }
    }

    /// `video.poll` 的占位符在两条路径里都在，且 `task_slot` 指的就是它。
    ///
    /// 回归对象：`task_slot` 改成一个两条路径里都没有的 token ⇒ 面板拼出来的轮询 URL
    /// 里**占位符原样留着**，网关只会 404，而面板上看不出为什么。
    #[test]
    fn the_video_poll_slot_really_appears_in_both_paths() {
        let e = MEDIA_ENDPOINTS
            .iter()
            .find(|e| e.id == "video.poll")
            .expect("目录里必须有 video.poll");
        let slot = e.task_slot.expect("轮询那条必须带任务标识占位符");
        assert!(e.path_template.contains(slot));
        assert!(e.upstream_path.contains(slot));
        // 另外两条不带占位符 —— 带了就是给面板一个它替换不掉的 token。
        for other in MEDIA_ENDPOINTS.iter().filter(|x| x.id != "video.poll") {
            assert_eq!(other.task_slot, None, "{}", other.id);
        }
    }

    // ── `GET /admin/api/models` ────────────────────────────────────────────

    /// 🔴 **连打 20 次：get / put / delete / list 四个桶一次都不增加。**
    ///
    /// 四个桶都数 —— 只数 `get` 的版本漏掉的恰恰是写那一侧。
    #[tokio::test]
    async fn the_models_endpoint_never_touches_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, None).await;
        // 装配本身会读几次（配置、池子预热），从这里开始数。
        let before = counts.snapshot();
        for _ in 0..20 {
            let (s, _) = get_json(&h, "/admin/api/models").await;
            assert_eq!(s, StatusCode::OK);
        }
        assert_eq!(counts.snapshot(), before, "目录端点必须零存储 I/O");
    }

    /// 响应体的三段格数与 `samplePrompt`。**期望值手写。**
    #[tokio::test]
    async fn the_catalog_payload_carries_four_protocols_three_media_and_twelve_models() {
        let h = harness().await;
        let (s, v) = get_json(&h, "/admin/api/models").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["protocols"].as_array().unwrap().len(), 4);
        assert_eq!(v["media"].as_array().unwrap().len(), 3);
        assert_eq!(v["models"].as_array().unwrap().len(), 12);
        assert_eq!(v["samplePrompt"], json!("ping"));
    }

    /// 🔴 **协议矩阵每一行恒有四个格子，不可用的画灰不隐藏。**
    ///
    /// 回归对象：把不可用的格子过滤掉 ⇒ 一个只在一条协议上可用的模型那一行
    /// 看起来与「这一行读不出来」一模一样，而那是两件事。
    ///
    /// ⚠️ 末尾那条自检刻意要求夹具里**有一个协议数少于目录的模型**（今天是图片 /
    /// 视频那六条，它们的 `protocols` 是空的）：对话模型四条协议全有，
    /// 「过滤」与「不过滤」在它身上**数学上等价** ⇒ 只看对话模型的话，
    /// 这一格测不到任何东西。
    #[tokio::test]
    async fn every_model_row_keeps_one_cell_per_protocol_even_when_unavailable() {
        let h = harness().await;
        let (_, v) = get_json(&h, "/admin/api/models").await;
        let protocol_ids: Vec<&str> = v["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        let models = v["models"].as_array().unwrap();
        let rows = v["view"]["rows"].as_array().expect("view.rows");
        assert_eq!(models.len(), rows.len(), "与 models 同长");

        let mut saw_a_partially_available_model = false;
        for (m, row) in models.iter().zip(rows) {
            assert_eq!(m["id"], row["id"], "同序");
            let cells = row["protocols"].as_array().expect("protocols");
            let ids: Vec<&str> = cells.iter().map(|c| c["id"].as_str().unwrap()).collect();
            assert_eq!(ids, protocol_ids, "列序与表头同源：{}", m["id"]);

            let owned: Vec<&str> = m["protocols"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap())
                .collect();
            for c in cells {
                let id = c["id"].as_str().unwrap();
                let want = owned.contains(&id);
                assert_eq!(c["available"], json!(want), "{} × {id}", m["id"]);
                assert_eq!(
                    c["badgeClass"],
                    json!(if want {
                        "badge badge-success"
                    } else {
                        "badge badge-off"
                    }),
                    "{} × {id}",
                    m["id"]
                );
            }
            if owned.len() < protocol_ids.len() {
                saw_a_partially_available_model = true;
                assert_eq!(
                    cells.len(),
                    protocol_ids.len(),
                    "🔴 不是每条协议都可用的模型，格子照样是满的：{}",
                    m["id"]
                );
            }
        }
        assert!(
            saw_a_partially_available_model,
            "🔴 夹具里必须有一个「不是每条协议都可用」的模型，否则「过滤」与「不过滤」\
             数学上等价，这一格测不到任何东西"
        );
    }

    /// 形态那一列：三档各有一条文案键；**目录里有形态不在表内时给 `null`**
    /// —— 渲染层照实画原值，绝不冒充任何一档已知形态。
    #[tokio::test]
    async fn the_modality_column_never_disguises_an_unknown_shape() {
        let h = harness().await;
        let (_, v) = get_json(&h, "/admin/api/models").await;
        let models = v["models"].as_array().unwrap();
        let rows = v["view"]["rows"].as_array().unwrap();
        for (m, row) in models.iter().zip(rows) {
            let want = match m["modality"].as_str().unwrap() {
                "chat" => json!("models.modality.chat"),
                "image" => json!("models.modality.image"),
                "video" => json!("models.modality.video"),
                // 目录今天只有这三种；多出来的一种必须落 `null`。
                _ => json!(null),
            };
            assert_eq!(row["modalityLabelKey"], want, "{}", m["id"]);
        }
    }

    /// 🔴 **端点真吐出去的那份 JSON 里，`samplePrompt` 在每一条 `sampleBody`
    /// 中恰好出现一次。**
    ///
    /// 观测点在响应体上，不是比对本文件的两个常量（那是同义反复）。
    /// 回归对象：把某条协议的样例文本改成别的 ⇒ Playground 对那条协议
    /// **静默丢弃用户输入、恒发样例那句话**。
    #[tokio::test]
    async fn sample_prompt_occurs_exactly_once_per_sample_body() {
        let h = harness().await;
        let (_, v) = get_json(&h, "/admin/api/models").await;
        let prompt = v["samplePrompt"].as_str().unwrap();
        for p in v["protocols"].as_array().unwrap() {
            let body = text_of(&p["sampleBody"]);
            assert_eq!(
                body.matches(prompt).count(),
                1,
                "{} 的 sampleBody 里那句提示词必须恰好一次：{body}",
                p["id"]
            );
        }
        for m in v["media"].as_array().unwrap() {
            let sb = &m["sampleBody"];
            if sb.is_null() {
                // GET 那条没有请求体。**`null` 不是「忘了填」。**
                assert_eq!(m["method"], json!("GET"));
                continue;
            }
            assert_eq!(text_of(sb).matches(prompt).count(), 1, "{}", m["id"]);
        }
    }

    /// 媒体样例体里那个 `model` 必须是**同形态**的模型，不是对话默认模型。
    ///
    /// 回归对象：沿用 `MODEL_CATALOG[0]`（对话模型）⇒ 示例卡教出一条
    /// 「拿对话模型打 `/v1/images/generations`」的注定 4xx 的调法。
    #[tokio::test]
    async fn media_samples_use_a_model_of_their_own_modality() {
        let h = harness().await;
        let (_, v) = get_json(&h, "/admin/api/models").await;
        for m in v["media"].as_array().unwrap() {
            let Some(model) = m["sampleBody"]["model"].as_str() else {
                continue;
            };
            let entry = catalog_model(model).expect("样例模型必须在目录里");
            assert_eq!(
                modality_str(entry.modality),
                m["modality"].as_str().unwrap(),
                "{}",
                m["id"]
            );
        }
    }

    /// gemini 那条的 `pathTemplate` 带占位符、`streamKey` 是另一条**路径**。
    ///
    /// 回归对象：把 `streamMode` 顺手统一成 `"body"` ⇒ 面板教出
    /// 「往 gemini 请求体里塞 `stream: true`」这条上游不认的调法。
    #[tokio::test]
    async fn gemini_switches_streaming_by_path_not_by_body() {
        let h = harness().await;
        let (_, v) = get_json(&h, "/admin/api/models").await;
        let g = v["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == json!("gemini"))
            .unwrap();
        assert_eq!(g["streamMode"], json!("path"));
        assert!(g["pathTemplate"].as_str().unwrap().contains("{model}"));
        assert!(
            g["streamKey"]
                .as_str()
                .unwrap()
                .contains("streamGenerateContent")
        );
        // 另外三条走请求体，`streamKey` 是字段名。
        for p in v["protocols"].as_array().unwrap() {
            if p["id"] == json!("gemini") {
                continue;
            }
            assert_eq!(p["streamMode"], json!("body"));
            assert_eq!(p["streamKey"], json!("stream"));
        }
    }

    // ── `POST /admin/api/models/{id}/test` ─────────────────────────────────

    /// 往池子里放一把 key。
    async fn seed(h: &Harness, key: &str) {
        h.state.repo.add(key, 1_000).await.unwrap();
    }

    /// 目录里没有的 id ⇒ **404 网关信封（不带 `code`）**，且**一次上游都不打**。
    #[tokio::test]
    async fn an_unknown_model_id_is_a_plain_404() {
        let fetcher = ScriptedFetcher::new(vec![]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-404-0123456789").await;
        let (s, v) = post_json(&h, "/admin/api/models/no-such-model/test", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["type"], json!("not_found"));
        assert!(v["error"]["code"].is_null(), "这一条刻意不带 code");
        assert!(fetcher.seen().is_empty(), "404 不许打上游");
    }

    /// 🔴 **图片 / 视频模型一律 400 `modality_not_testable`，而且零上游调用。**
    ///
    /// 回归对象：把这道闸放开（「反正都是打一次上游」）⇒ 一颗「全部测一遍」按钮
    /// 会替这个账号真生成 3 张图 + 建 3 个视频任务，**那笔消耗收不回来**。
    #[tokio::test]
    async fn image_and_video_models_are_refused_before_any_upstream_call() {
        let fetcher = ScriptedFetcher::new(vec![]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-modality-0123456789").await;
        let non_chat: Vec<&str> = MODEL_CATALOG
            .iter()
            .filter(|m| m.modality != Modality::Chat)
            .map(|m| m.id)
            .collect();
        assert_eq!(non_chat.len(), 6, "目录里应有 6 个非对话模型");
        for id in non_chat {
            let (s, v) = post_json(&h, &format!("/admin/api/models/{id}/test"), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{id}");
            assert_eq!(v["reason"], json!("modality_not_testable"), "{id}");
            assert_eq!(v["error"]["type"], json!("invalid_request_error"), "{id}");
        }
        assert!(fetcher.seen().is_empty(), "一次上游都不许打");
    }

    /// 这条端点不收任何选项：空体放行、`{}` 放行、其余 400（报文点名那些字段）。
    ///
    /// ⚠️ 三条 400 走真路由（它们排在护栏之前，不会消费全局那把 guard）；
    /// 放行那两档走 [`test_with`] + 本地 guard，理由见
    /// [`tests::only_one_criterion_may_take_the_process_wide_guard`]。
    #[tokio::test]
    async fn the_test_endpoint_accepts_no_parameters_at_all() {
        let fetcher = ScriptedFetcher::new(vec![
            UpstreamReply::json(json!({ "ok": 1 })),
            UpstreamReply::json(json!({ "ok": 1 })),
        ]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-body-0123456789").await;
        let id = MODEL_CATALOG[0].id;
        let uri = format!("/admin/api/models/{id}/test");

        // 非法 JSON。
        let (s, v) = post_json(&h, &uri, Some("{")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["message"], json!("请求体不是合法的 JSON"));
        // 不是对象。
        let (s, v) = post_json(&h, &uri, Some("[1,2]")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["message"], json!("请求体必须是一个 JSON 对象"));
        // 有键 —— **字段名逐字回显**，那正是这条报文的用途。
        let (s, v) = post_json(&h, &uri, Some(r#"{"model":"x","stream":true}"#)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("模型测试不接受任何参数"), "{msg}");
        assert!(msg.contains("model"), "{msg}");
        assert!(msg.contains("stream"), "{msg}");
        assert!(fetcher.seen().is_empty(), "参数错就不该打上游");

        // 空体与 `{}` 都放行 —— 面板与鉴权矩阵都是不带体调这条 POST 的。
        let guard = ProbeGuard::new();
        for (n, body) in [Bytes::new(), Bytes::from_static(b"  ")]
            .into_iter()
            .enumerate()
        {
            let at = (n as i64 + 1) * (PROBE_MIN_INTERVAL_MS + 1);
            let res = test_with(&h.state, id, &body, &guard, &|| at).await;
            assert_eq!(res.status(), StatusCode::OK);
        }
        assert_eq!(fetcher.seen().len(), 2, "两次都该真的打到上游");
    }

    /// ⚠️ **这不是一格业务判据，是一条给写判据的人的约束**，写成用例是为了让它
    /// 被人读到：[`MODEL_TEST_KIND`] 是常量，而 [`probe_guard`] 是**进程级单例**
    /// ⇒ 任何两格「走真路由且真占到闸」的用例并行跑时会互相挡成 429，**必然间歇红**。
    ///
    /// ⇒ 本组里**只有** [`tests::a_successful_test_really_calls_upstream_with_that_model`]
    /// 走真路由占那把闸，其余一律用 [`test_with`] + 本地 `ProbeGuard`。
    /// 这一格把那个数钉住：源码里 `post_json(.., "/test", ..)` 之后**期望 200** 的地方
    /// 只许有一处。
    #[test]
    fn only_one_criterion_may_take_the_process_wide_guard() {
        let src = include_str!("models.rs");
        // 走真路由 + 期望 OK 的那一格自报家门用这个记号。
        assert_eq!(
            src.matches(concat!("TAKES", "-PROCESS-GUARD")).count(),
            1,
            "只许有一格用例去占进程级那把 model-test 闸"
        );
    }

    /// 池里一把能用的都没有 ⇒ **200 `no_key`**，`latencyMs` 是 0，零上游调用。
    ///
    /// **判据是 `select_key()` 而不是「池子长度为 0」**：这里池子是空的，
    /// 而「全在冷却」「全被停用」在这条端点上是同一件事。
    #[tokio::test]
    async fn an_empty_pool_answers_no_key_with_a_200() {
        let fetcher = ScriptedFetcher::new(vec![]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        let id = MODEL_CATALOG[0].id;
        let (s, v) = post_json(&h, &format!("/admin/api/models/{id}/test"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ok"], json!(false));
        assert!(v["status"].is_null());
        assert_eq!(v["latencyMs"], json!(0));
        assert_eq!(v["reason"], json!("no_key"));
        assert!(fetcher.seen().is_empty());
    }

    /// 成功那一支：**真的拿池里那把 key 打了上游**，请求体里的 `model` 就是被测的那个，
    /// 路径是 `agnesBaseUrl + upstreamPath`，出站头是 `Authorization: Bearer`。
    ///
    /// TAKES-PROCESS-GUARD  —— 见 [`tests::only_one_criterion_may_take_the_process_wide_guard`]。
    #[tokio::test]
    async fn a_successful_test_really_calls_upstream_with_that_model() {
        let fetcher = ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "id": "c1" }))]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-ok-0123456789").await;
        let id = "agnes-2.5-pro";
        let (s, v) = post_json(&h, &format!("/admin/api/models/{id}/test"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["status"], json!(200));
        assert!(v["reason"].is_null());

        let seen = fetcher.only();
        assert_eq!(seen.method, "POST");
        let cfg = h.state.config.current();
        assert_eq!(
            seen.url,
            format!("{}/chat/completions", cfg.agnes_base_url),
            "上游路径必须来自协议目录的 upstreamPath"
        );
        assert_eq!(seen.body_json()["model"], json!(id));
        let auth = seen
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .expect("出站必须带 Authorization");
        assert_eq!(auth.1, "Bearer sk-models-ok-0123456789");
    }

    #[tokio::test]
    async fn model_test_routes_the_selected_key_to_its_upstream() {
        for (selector, base) in [
            (
                Some("international"),
                Some("https://apihub.agnes-ai.com/v1"),
            ),
            (
                Some("internationalBackup"),
                Some("https://apihub.agnes-ai.cn/v1"),
            ),
            (Some("china"), Some("https://api.agnes-ai.cn/v1")),
            (None, None),
        ] {
            let fetcher = ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "id": "c1" }))]);
            let mut h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
            let mut config = h.state.config.current().as_ref().clone();
            config.agnes_base_url = "https://legacy.example/v1".into();
            h.state.config = Arc::new(crate::server::holders::config_holder::ConfigHolder::fixed(
                config,
            ));
            let key = format!("sk-model-route-{}-0123456789", selector.unwrap_or("legacy"));
            let prev = h.state.repo.add(&key, 1_000).await.unwrap();
            let mut raw = serde_json::to_value(&prev).unwrap();
            if let Some(selector) = selector {
                raw["upstream"] = json!(selector);
            }
            let record = crate::core::narrow::narrow_key_record(&raw).unwrap();
            h.state
                .repo
                .save(&record, Some(&prev), 1_000)
                .await
                .unwrap();
            let response = test_with(
                &h.state,
                "agnes-2.5-pro",
                &Bytes::new(),
                &ProbeGuard::new(),
                &|| 5_000,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let seen = fetcher.only();
            let cfg = h.state.config.current();
            assert_eq!(
                seen.url,
                format!("{}/chat/completions", base.unwrap_or(&cfg.agnes_base_url)),
                "selector: {selector:?}"
            );
            assert!(
                seen.headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                        && value == &format!("Bearer {key}"))
            );
        }
    }

    /// 🔴 **一个字节的上游正文都不回，也不写任何存储、不打任何事件。**
    ///
    /// 事件环能整份下载，而这条 handler 手里握着一把明文 key 与一份上游错误体。
    #[tokio::test]
    async fn a_model_test_leaks_no_upstream_body_writes_nothing_and_logs_nothing() {
        let secret = "AgnesAI_error: key sk-models-leak-0123456789 已被封禁";
        let reply = UpstreamReply {
            status: 403,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![Bytes::from(
                json!({ "error": secret }).to_string().into_bytes(),
            )],
        };
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let fetcher = ScriptedFetcher::new(vec![reply]);
        let h = harness_with(counting, Some(fetcher.clone())).await;
        seed(&h, "sk-models-leak-0123456789").await;
        let before_put = counts.snapshot().1;
        let before_del = counts.snapshot().2;
        let before_events = h.recorded.entries().len();

        let id = MODEL_CATALOG[0].id;
        // 本地 guard：进程级那把只许一格用例去占（见那一格的说明）。
        let guard = ProbeGuard::new();
        let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 0).await;
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        assert!(!raw.contains(secret), "上游正文不许出现在响应里：{raw}");
        assert!(!raw.contains("sk-models-leak"), "明文 key 不许出现：{raw}");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["status"], json!(403));
        assert_eq!(v["reason"], json!("upstream_error"));

        assert_eq!(counts.snapshot().1, before_put, "一次存储写都不许有");
        assert_eq!(counts.snapshot().2, before_del, "一次删除都不许有");
        assert_eq!(
            h.recorded.entries().len(),
            before_events,
            "这条端点一条事件都不许打"
        );
    }

    /// 传输层失败映射成机器可读的 code，**不是异常消息**（那里面可能带上游 URL）。
    #[tokio::test]
    async fn transport_failures_map_to_machine_readable_reasons() {
        struct Failing(FetchError);
        #[async_trait::async_trait]
        impl crate::ports::fetcher::Fetcher for Failing {
            async fn send(
                &self,
                _r: OutboundRequest,
            ) -> Result<crate::ports::fetcher::OutboundResponse, FetchError> {
                Err(match &self.0 {
                    FetchError::TimeoutBeforeHeaders => FetchError::TimeoutBeforeHeaders,
                    FetchError::Connect(m) => FetchError::Connect(m.clone()),
                    FetchError::BodyInterrupted(m) => FetchError::BodyInterrupted(m.clone()),
                })
            }
        }
        for (err, want) in [
            (FetchError::TimeoutBeforeHeaders, "timeout"),
            (
                FetchError::Connect("https://upstream.example/chat".into()),
                "network_error",
            ),
        ] {
            let h = harness_with(
                Arc::new(MemoryStorage::new()),
                Some(Arc::new(Failing(err)) as Arc<dyn crate::ports::fetcher::Fetcher>),
            )
            .await;
            seed(&h, "sk-models-fail-0123456789").await;
            let id = MODEL_CATALOG[0].id;
            let g = ProbeGuard::new();
            let res = test_with(&h.state, id, &Bytes::new(), &g, &|| 0).await;
            let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["reason"], json!(want));
            assert!(v["status"].is_null());
            assert!(!v.to_string().contains("upstream.example"), "{v}");
        }
    }

    /// 🔴🔴 **N 个模型真的都测到了。**
    ///
    /// 这是 TS 版那个真缺陷的正向判据：`MODEL_TEST_KIND` 是**常量**，
    /// 一轮扫描的每一次都撞同一个槽位 ⇒ 发起方**必须**隔满
    /// [`PROBE_MIN_INTERVAL_MS`] 才测得到第二个模型。这一格模拟的正是
    /// 「面板那一半在做限速」的那种发起方：6 个对话模型逐个测，
    /// **6 次上游调用、6 个互不相同的 model**。
    ///
    /// 变异对象：把 `guard.acquire()` 排到 404 之前、或者把请求体里的 `model`
    /// 写死成第一个 —— 两种都让这一格红。
    /// ⚠️ 曾经还有第三种（「把放开挪进成功支」⇒ 一次失败就永久卡死），
    /// 自打护栏换成 [`crate::admin::keys::ProbeLease`] 之后它**写不出来了**：
    /// 放开挂在 drop 上，哪条支都躲不开。
    #[tokio::test]
    async fn a_paced_sweep_really_tests_every_single_model() {
        let chat: Vec<&str> = MODEL_CATALOG
            .iter()
            .filter(|m| m.modality == Modality::Chat)
            .map(|m| m.id)
            .collect();
        assert_eq!(chat.len(), 6, "这一格要的就是「不止一个」");
        let script: Vec<UpstreamReply> = chat
            .iter()
            .map(|_| UpstreamReply::json(json!({ "ok": 1 })))
            .collect();
        let fetcher = ScriptedFetcher::new(script);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-sweep-0123456789").await;

        let guard = ProbeGuard::new();
        let clock = std::sync::atomic::AtomicI64::new(1_000_000);
        for id in &chat {
            let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| {
                clock.load(std::sync::atomic::Ordering::Relaxed)
            })
            .await;
            assert_eq!(res.status(), StatusCode::OK, "{id}");
            let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                v["ok"],
                json!(true),
                "{id} 应当真的测通，而不是被自己的闸挡下"
            );
            // 发起方隔满最小间隔 —— 这一半在生产里由面板做。
            clock.fetch_add(PROBE_MIN_INTERVAL_MS, std::sync::atomic::Ordering::Relaxed);
        }

        let seen = fetcher.seen();
        assert_eq!(seen.len(), chat.len(), "每一个模型都必须真的打到上游");
        let models: Vec<String> = seen
            .iter()
            .map(|r| r.body_json()["model"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(models, chat, "打出去的 model 必须逐条就是被测的那个");
    }

    /// 🔴 **零间隔的扫描只测得到第一个模型 —— 这是后端那道闸的真实形状。**
    ///
    /// TS 版曾写「共用一个 kind ⇒ 整轮自动被摊到最小间隔上」，而那句是假的：
    /// 共用一个 kind 只会**截断**一轮。这一格把那次线上实测钉成判据，
    /// 免得下一个人照那句假话去写面板。
    ///
    /// ⚠️ 它与上面那一格是**同一件事的两半**，缺哪一半都不成立。
    #[tokio::test]
    async fn a_zero_interval_sweep_only_ever_reaches_the_first_model() {
        let chat: Vec<&str> = MODEL_CATALOG
            .iter()
            .filter(|m| m.modality == Modality::Chat)
            .map(|m| m.id)
            .collect();
        // 脚本只放一条：第二次真打上游的话，`ScriptedFetcher` 会以可观察的失败收场。
        let fetcher = ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "ok": 1 }))]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-burst-0123456789").await;

        let guard = ProbeGuard::new();
        let mut reasons = Vec::new();
        for id in &chat {
            let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 5_000_000).await;
            let status = res.status();
            let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            reasons.push((status, v["reason"].as_str().unwrap_or("").to_string()));
        }
        assert_eq!(fetcher.seen().len(), 1, "整轮只打得出第一次");
        assert_eq!(reasons[0].0, StatusCode::OK);
        for (status, reason) in &reasons[1..] {
            assert_eq!(*status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(reason, "probe_cooldown");
        }
    }

    /// 护栏排在校验之后：一次注定 404 的调用**不消费一格最小间隔**。
    ///
    /// 回归对象：把 `try_acquire` 挪到查目录之前 ⇒ 拿一个不存在的 id 打一下，
    /// 接下来 3 秒整颗「全部测一遍」按钮点不动，而面板上完全看不出为什么。
    #[tokio::test]
    async fn a_doomed_404_does_not_consume_the_probe_interval() {
        let fetcher = ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "ok": 1 }))]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-order-0123456789").await;
        let guard = ProbeGuard::new();

        let res = test_with(&h.state, "no-such-model", &Bytes::new(), &guard, &|| 7_000).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        // 同一毫秒内紧接着测一个真模型：闸没被那次 404 占掉 ⇒ 照样放行。
        let id = MODEL_CATALOG[0].id;
        let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 7_000).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(fetcher.seen().len(), 1);
    }

    /// 空池那一档同样不消费护栏 —— 「点了一下、什么都没打，但接下来几秒不许再点」
    /// 是最难排查的一种手感。
    #[tokio::test]
    async fn an_empty_pool_does_not_consume_the_probe_interval_either() {
        let fetcher = ScriptedFetcher::new(vec![UpstreamReply::json(json!({ "ok": 1 }))]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        let guard = ProbeGuard::new();
        let id = MODEL_CATALOG[0].id;

        let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 9_000).await;
        assert_eq!(res.status(), StatusCode::OK);
        // 现在把 key 放进去，同一毫秒再点一次 —— 闸没被上一次空池占掉。
        seed(&h, "sk-models-emptyguard-0123456789").await;
        let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 9_000).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(fetcher.seen().len(), 1);
    }

    /// 上游失败之后护栏照样放开 —— 放开不挂在成功支上，挂在租约的 drop 上。
    ///
    /// 而「上游连不上」恰恰是运维最想连点重试的那一刻。
    #[tokio::test]
    async fn the_guard_is_released_even_when_the_upstream_call_fails() {
        // 脚本是空的 ⇒ 第一次调用就以 `Connect` 失败收场。
        let fetcher = ScriptedFetcher::new(vec![]);
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher.clone())).await;
        seed(&h, "sk-models-release-0123456789").await;
        let guard = ProbeGuard::new();
        let id = MODEL_CATALOG[0].id;

        let res = test_with(&h.state, id, &Bytes::new(), &guard, &|| 0).await;
        assert_eq!(res.status(), StatusCode::OK);
        // 卡在「在飞」的话下面这次会是 `probe_in_flight`；放开了才是 `probe_cooldown`。
        assert_eq!(
            guard.try_acquire(MODEL_TEST_KIND, 1),
            ProbeAcquire::Cooldown
        );
        // 隔满最小间隔就完全恢复。
        assert_eq!(
            guard.try_acquire(MODEL_TEST_KIND, PROBE_MIN_INTERVAL_MS + 1),
            ProbeAcquire::Ok
        );
    }

    /// 🔴🔴 **客户端断开（future 被取消）也不许把那颗按钮永久卡死。**
    ///
    /// 它与上面那一格不是同一件事：那一格钉的是「上游返回错误」那条支
    /// （放开排在它后面，本来就走得到），这一格钉的是**第三条支「请求被取消」**——
    /// axum 在客户端断开时**直接 drop handler 的 future** ⇒ `send().await` 之后的
    /// 每一行都不再执行。手写 `try_acquire` / `release` 的形状只覆盖得到前两条支，
    /// 漏掉这一条 ⇒ `in_flight` 永远停在 `true`，而 `try_acquire` 第一句就是
    /// 「在飞就拒」。
    ///
    /// ⚠️ **这条端点的 kind 是常量**（[`MODEL_TEST_KIND`]）⇒ 卡住的不是某一把 key，
    /// 是**整颗「全部测一遍」按钮**，对所有人、且只能重启进程才好
    /// （最小间隔那道闸会自愈，这道不会）。
    ///
    /// ⇒ 放开**不许依赖执行流走到某一行**，只许挂在
    /// [`crate::admin::keys::ProbeLease`] 的 drop 上 —— drop 在
    /// 「正常返回 / 提前 return / panic 展开 / 被取消」四种收场里都执行。
    #[tokio::test]
    async fn a_cancelled_request_does_not_wedge_the_button_forever() {
        /// 永远不返回的上游。**进了 `send` 才发信号**：不等这一下的话，
        /// 取消掉的可能是护栏之前那一段，而那种形态本来就不会卡住。
        struct Hangs {
            entered: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        }
        #[async_trait::async_trait]
        impl crate::ports::fetcher::Fetcher for Hangs {
            async fn send(
                &self,
                _r: OutboundRequest,
            ) -> Result<crate::ports::fetcher::OutboundResponse, FetchError> {
                if let Some(tx) = self.entered.lock().take() {
                    let _ = tx.send(());
                }
                std::future::pending().await
            }
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let fetcher: Arc<dyn crate::ports::fetcher::Fetcher> = Arc::new(Hangs {
            entered: parking_lot::Mutex::new(Some(tx)),
        });
        let h = harness_with(Arc::new(MemoryStorage::new()), Some(fetcher)).await;
        seed(&h, "sk-models-cancelled-0123456789").await;
        // ⚠️ **不碰进程级那把单例**：这一格要观察的正是「卡没卡住」，
        //    用单例的话一旦真卡住就会把同进程里其余判据一起拖红。
        let guard: &'static ProbeGuard = Box::leak(Box::new(ProbeGuard::new()));

        let st = h.state.clone();
        let id = MODEL_CATALOG[0].id;
        let task =
            tokio::spawn(async move { test_with(&st, id, &Bytes::new(), guard, &|| 1_000).await });
        // 等到探针**真的发出去了**再动手。
        rx.await.expect("夹具前提：上游探针必须真的发出去");
        assert!(
            guard.is_in_flight(MODEL_TEST_KIND),
            "夹具前提：这一刻它确实占着闸"
        );

        // 客户端断开 = handler 的 future 被 drop 在 `send().await` 上。
        task.abort();
        assert!(
            task.await.unwrap_err().is_cancelled(),
            "夹具前提：真被取消了"
        );

        assert!(
            !guard.is_in_flight(MODEL_TEST_KIND),
            "🔴 被取消的那一次没放开槽位 ⇒ 整颗「全部测一遍」按钮从此永久点不动"
        );
    }
}
