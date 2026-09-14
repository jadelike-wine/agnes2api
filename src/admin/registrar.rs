//! 注册机那三条端点（02 §9.15 / §9.16 / §9.17）。
//!
//! # 为什么「没接线」也必须是一个状态码，而不是一条不存在的路由
//!
//! 🔴 02 §1.4：**路由表绝不许随运行时配置变化。**
//! 「注册机没接线」是 handler 里的一个状态码，**不是「这条路由不存在」**。
//! 两者在面板与 curl 上的差别是决定性的：
//! - `503 {reason:"not_wired"}` = 「这个部署没装注册机」——运维知道去装；
//! - `404` = 「这个网关压根没有这个功能 / 我路径拼错了」——运维去翻文档，
//!   而文档逐条写着这三条端点**存在**。
//!
//! 还有一条更硬的理由（02 §1.4 原话）：枚举式鉴权矩阵会因为夹具恰好关着某功能
//! 而让那几条端点从矩阵里**静默消失**，于是一整族鉴权判据在不知不觉中失去对象。
//!
//! # 🔴 三条端点一律**现读配置**，不许读启动快照
//!
//! 面板是这份配置的编辑器。读 [`crate::registrar::runtime::RegistrarRuntime::config`]
//! 那份启动快照意味着「保存成功之后，面板自己显示的还是旧值、
//! 「立即补池」还按旧值判开关、「测试通道」验的还是旧凭据」——
//! 改完必须重启容器才看得见，而**运维没有任何线索知道这一点**。
//! 统一走本文件的 `live()`；定时轮那边走同一个 `reload()`。
//!
//! # 📜 它曾经是 `registrar_stub.rs`
//!
//! P1 时这三条**一律**回 `not_wired`，文件名里的 `_stub` 让「还没实现」
//! 在模块树上一眼可见。P2 接线之后 `_stub` 就成了**一个会误导人的名字**
//! ——它会让人以为这里还是假的，从而不去看真正的逻辑。名字随实现一起改。
//!
//! ⚠️ §9.15 那张**七种拒绝支**的判定顺序表仍是这三条的规格：
//! 拒绝支是**有顺序的判定阶梯**（`not_wired` 排在中间），
//! **一条顺序错了的阶梯比一条诚实的占位更坏**——它会被当成已实现，
//! 而补齐前面几支时没有任何东西会红。今天落地的顺序是
//! `not_wired` → `enabled` → `blocked` →（channel test 再加）`channel_unknown`。

use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

// ── 子树内路径（🔴 `app.rs` 的 `.route()` 从这里取，不许手抄字面量）──────────

/// `POST /admin/api/registrar/tend`（「立即补池」）。
pub const REGISTRAR_TEND_SUBPATH: &str = "/registrar/tend";
/// `GET /admin/api/registrar/status`。
pub const REGISTRAR_STATUS_SUBPATH: &str = "/registrar/status";
/// `POST /admin/api/registrar/channels/{channel}/test`。
///
/// ⚠️ **占位符写成 axum 的 `{channel}`**：同一个字符串既是路由的注册模式、
/// 又是面板要替换的模板，两者因此不可能分叉（体例同
/// [`crate::admin::models::MODEL_TEST_SUBPATH`]）。
pub const REGISTRAR_CHANNEL_TEST_SUBPATH: &str = "/registrar/channels/{channel}/test";

/// 「这个部署没接注册机」的机器可读判别串。
///
/// ⚠️ 与 [`crate::apikey::admin::REASON_NOT_WIRED`] **同一个串、同一个信封形状**
/// （02 §4.4 登记的混合体：不带 `code` 的网关信封 + 顶层 `reason`）：
/// 面板对这一档只有一句固定文案，不需要一条要五语言维护的码。
/// 🔴 这里**直接引它**而不是再写一遍字面量：两份字面量迟早会漂。
pub use crate::apikey::admin::REASON_NOT_WIRED;

/// 手动补池撞上「没接线」时留下的事件。
///
/// 🔴 **事件名是运维的 grep 锚点，一个字都不许改**（02 §9.15 第 5 支）。
/// ⚠️ **只有 tend 打事件，另外两条不打**：那两条是**读**端点，面板会轮询它们
/// （`status` 是状态板块的数据源）—— 给读端点打事件等于让事件环被一个没在
/// 接线的功能刷满，把真正的告警挤出环外。tend 是**人点了一下按钮**，
/// 那一下值得留一条记录。
pub const EVENT_MANUAL_TEND_NOT_WIRED: &str = "registrar.manual_tend_not_wired";
/// 通道连通性测试失败（列域名 / 验凭据这两步里的某一步）。
pub const EVENT_CHANNEL_TEST_FAILED: &str = "registrar.channel_test_failed";

// ── 响应体（🔴 一律结构体，不用 `json!` 宏 —— 键序是对外契约）────────────────

#[derive(Serialize)]
struct GatewayInner {
    r#type: &'static str,
    message: &'static str,
}

/// 02 §4.4 的「handler 自拼」混合体：**网关信封 + 顶层 `reason`**（+ 可选 `channel`）。
#[derive(Serialize)]
struct NotWired {
    error: GatewayInner,
    reason: &'static str,
    /// ⚠️ §9.17 要求通道那一条**把通道名带回来**（面板按它把结果画在对应的卡片上）。
    /// 另外两条没有这一格 ⇒ `skip_serializing_if`，**不发一个恒 null 的键**。
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
}

/// 那句话只有一处。
///
/// ⚠️ **措辞里写清「这个部署」而不是「暂不支持」**：后者会被运维读成
/// 「这个网关没有补池功能」，而真相是「有，但你这套没装起来」——
/// 两者的下一步动作完全不同。
fn not_wired(channel: Option<String>) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(NotWired {
            error: GatewayInner {
                r#type: "internal_error",
                message: "这个部署没有接上注册机（自动铸 key 补池），这几条端点因此没有对象",
            },
            reason: REASON_NOT_WIRED,
            channel,
        }),
    )
        .into_response()
}

// ── 三条端点 ────────────────────────────────────────────────────────────────

/// `POST /admin/api/registrar/tend` —— 「立即补池」。
///
/// 🔴 **端点自己 `await` 这一轮再返回**，不把它交给任何后台载体。
/// TS 版那一版回 202「已开始」之后把整轮交给平台的后台钩子，
/// 而平台在**响应结束后约 30 秒**就把它取消掉（实测 3/3），
/// **取消不抛异常** ⇒ 两层收尾一个都不执行 ⇒ 不写历史、不发事件、锁泄漏到自然过期
/// ⇒ **点一次按钮把注册机整个停摆一刻钟**。
/// 现读一份配置给面板用；读不出来就退回启动那份快照。
///
/// 🔴 **面板的三条端点一条都不许读 [`RegistrarRuntime::config`] 那份启动快照。**
/// 面板是这份配置的编辑器：读快照意味着「保存成功之后，
/// 面板自己显示的还是旧值、「立即补池」还按旧值判开关」——
/// 改完必须重启容器才看得见，而运维没有任何线索知道这一点。
/// 定时轮那边走的是同一个 [`RegistrarRuntime::reload`]。
///
/// ⚠️ **读失败退回快照，而不是退回「空配置」**：后者会让一次存储抖动
/// 把面板上配好的一切显示成「没配过」。
async fn live(
    rt: &crate::registrar::runtime::RegistrarRuntime,
) -> crate::registrar::config::RegistrarLoad {
    rt.reload()
        .await
        .unwrap_or_else(|| crate::registrar::config::RegistrarLoad {
            config: rt.config.clone(),
            blockers: rt.blockers.clone(),
            notices: rt.notices.clone(),
            warnings: rt.warnings.clone(),
        })
}

/// 请求体：**可选**。空体 = `{}`。
///
/// 🔴 **三种不合法一律 400，而且是不带 `code` 的网关信封** ——
/// 它们不是「注册机的某种状态」，是这条请求本身就不对。
/// 唯一允许的键是 `channel`：静默忽略未知字段会让
/// 「我明明传了 `chanel`」变成一次查不出原因的「怎么没换通道」。
fn parse_tend_body(raw: &[u8]) -> Result<Option<String>, &'static str> {
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return Err("请求体不是合法 JSON");
    };
    let Some(o) = v.as_object() else {
        return Err("请求体必须是一个 JSON 对象");
    };
    for k in o.keys() {
        if k != "channel" {
            return Err("请求体只认 channel 这一个键");
        }
    }
    match o.get("channel") {
        None => Ok(None),
        Some(serde_json::Value::String(c)) => Ok(Some(c.clone())),
        Some(_) => Err("channel 必须是字符串"),
    }
}

pub async fn tend(State(st): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(rt) = st.registrar.clone() else {
        // 🔴 **人点了一下按钮，必须留一条记录**：运维在面板上点「立即补池」什么都没发生时，
        //    事件板块是他唯一能查到「为什么没发生」的地方。
        st.logger.log(LogEntry::new(
            LogLevel::Warn,
            EVENT_MANUAL_TEND_NOT_WIRED,
            "有人点了「立即补池」，但这个部署没有接注册机",
        ));
        return not_wired(None);
    };
    let live = live(&rt).await;
    // 🔴 **先判 enabled 再判 blocked** —— 两者含义不同（见 `registrar::config`）。
    if !live.config.enabled {
        return conflict(
            "registrar_disabled",
            "注册机在设置里是关着的 —— 打开它再点这颗按钮。",
        );
    }
    if live.config.blocked {
        let why = live
            .blockers
            .iter()
            .map(|b| b.msg.as_str())
            .collect::<Vec<_>>()
            .join("；");
        return conflict(
            "registrar_blocked",
            &format!("注册机已启用，但本次没跑起来：{why}"),
        );
    }

    // ③ 到这一级才解析请求体。
    //
    // 🔴 **开关两档排在它前面，顺序与被移植的那一版一致。**
    // 反过来的话，一个关着注册机的部署上，一次字段名笔误拿到的是
    // 「请求体只认 channel 这一个键」—— 而运维真正要先做的是**把注册机打开**。
    // 先说那个**必须先解决**的理由。
    let channel_arg = match parse_tend_body(&body) {
        Ok(c) => c,
        Err(msg) => {
            // ⚠️ **不带 `code` 的网关信封**：这三种不是注册机的某种状态，
            // 是这条请求本身就不对。
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": { "type": "invalid_request_error", "message": msg },
                })),
            )
                .into_response();
        }
    };
    // ③ 指定了通道的话，它得是认得出的那两条之一。
    let chosen = match channel_arg.as_deref() {
        None => None,
        Some(raw) => match crate::registrar::config::Channel::parse_public(raw) {
            Some(c) => Some(c),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": { "type": "invalid_request_error",
                                   "message": format!("不认识的通道名「{raw}」，只有 yyds 与 moemail") },
                        "reason": "unknown_channel",
                    })),
                )
                    .into_response();
            }
        },
    };
    // ④ 指定的那条得配了凭据。**一次上游请求都不发。**
    if let Some(c) = chosen {
        let configured = match c {
            crate::registrar::config::Channel::Yyds => live.config.yyds.is_some(),
            crate::registrar::config::Channel::Moemail => live.config.moemail.is_some(),
        };
        if !configured {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": { "type": "conflict",
                               "message": "这条通道还没配凭据 —— 填好再点这颗按钮。" },
                    "reason": "channel_not_configured",
                    "channel": c.as_str(),
                })),
            )
                .into_response();
        }
    }

    // ⑥ 进程内那道闸。**与下面那把存储锁不是冗余**：
    // 一把挡同进程重入（面板连点 / 定时轮撞上按钮），一把挡跨副本重叠。
    let Some(_in_flight) = crate::server::state::TendInFlight::enter(&st.tend_in_flight) else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": { "type": "conflict",
                           "message": "上一轮补池还在跑，等它跑完。" },
                "reason": "tend_in_flight",
            })),
        )
            .into_response();
    };

    // ── 两道闸：冷却 + 日预算 ────────────────────────────────────────────
    //
    // 🔴 它们挡的不是「点得太快」，是**真实的上游代价**：一次点击最多烧掉
    // `MINT_BATCH` 个临时邮箱名额，而那是补池能不能继续工作的前提；
    // 每铸一把 key 还是一次真实的上游建号，同时撞注册风控。
    use crate::registrar::guard as g;
    let now = crate::adapters::now_ms();
    let cur = g::narrow_manual_guard(
        rt.storage
            .get(g::MANUAL_GUARD_KEY)
            .await
            .ok()
            .flatten()
            .as_ref(),
    );
    let verdict = g::check_manual_tend(cur.as_ref(), now);
    let (remaining, reset_at, cooldown_until, retry_after_ms) = match verdict {
        g::ManualTendVerdict::Denied {
            reason,
            remaining,
            reset_at,
            retry_after_ms,
            cooldown_until,
        } => {
            let mut body = serde_json::json!({
                "error": {
                    "type": "rate_limit_error",
                    "code": reason,
                    "message": if reason == g::REASON_WRITE_BUDGET_EXHAUSTED {
                        "今天的「立即补池」次数用完了。".to_string()
                    } else {
                        "刚补过一轮，等冷却结束再点。".to_string()
                    },
                },
                "reason": reason,
                "remaining": remaining,
                "resetAt": reset_at,
                "retryAfterMs": retry_after_ms,
            });
            // ⚠️ `cooldownUntil` **只在真的冷却中那一支给**：给一个已经过去的时刻
            // 会让面板渲染出一个恒为 0 的倒计时。
            if let Some(u) = cooldown_until {
                body["cooldownUntil"] = serde_json::json!(u);
            }
            return (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
        }
        g::ManualTendVerdict::Ok {
            next,
            remaining,
            reset_at,
            cooldown_until,
            retry_after_ms,
        } => {
            // 🔴 **先落盘再跑。** 反过来的话，一轮跑崩（或进程被杀）就等于
            // 白送一格名额与一整个冷却窗口 —— 而连点恰恰最容易在出问题时发生。
            let _ = rt
                .storage
                .put(g::MANUAL_GUARD_KEY, next.to_value(), None)
                .await;
            (remaining, reset_at, cooldown_until, retry_after_ms)
        }
    };

    // 手动这一轮套自己那一族上限，**一律 `min()` 压顶而不是赋值**。
    let (mut capped_cfg, capped) = crate::registrar::runtime::manual_caps(&live.config);
    // 指定了通道就用它跑这一轮 —— 那正是这个参数存在的理由。
    if let Some(c) = chosen {
        capped_cfg.channel = c;
    }
    if let Some(c) = capped {
        rt.logger.log(
            LogEntry::new(
                LogLevel::Info,
                crate::registrar::runtime::EVENT_MANUAL_ROUND_CAPPED,
                "手动补池按手动一族的上限压过了几个参数",
            )
            .with("mintBatchBefore", u64::from(c.before.mint_batch))
            .with("mintBatchAfter", u64::from(c.after.mint_batch))
            .with(
                "domainAttemptsBefore",
                u64::from(c.before.max_domain_attempts),
            )
            .with(
                "domainAttemptsAfter",
                u64::from(c.after.max_domain_attempts),
            )
            .with("codeTimeoutBefore", c.before.code_timeout_ms)
            .with("codeTimeoutAfter", c.after.code_timeout_ms),
        );
    }

    let repo = crate::registrar::runtime::PoolRepoBridge {
        repo: std::sync::Arc::clone(&st.repo),
    };
    let outcome = crate::registrar::runtime::run_round(crate::registrar::runtime::RoundDeps {
        config: &capped_cfg,
        storage: std::sync::Arc::clone(&rt.storage),
        logger: std::sync::Arc::clone(&rt.logger),
        fetcher: std::sync::Arc::clone(&rt.fetcher),
        repo: &repo,
        trigger: crate::registrar::runtime::TendTrigger::Manual,
        round_budget_ms: Some(crate::registrar::runtime::manual_round_budget(&capped_cfg)),
    })
    .await;

    // ⑧ 跨副本那把锁被占着 ⇒ 409，**不是一次失败的补池**。
    // 真话是「另一边正在补，等它 N 秒」，而不是「补池失败了」。
    let result = match outcome {
        crate::registrar::runtime::RoundOutcome::Ran(r) => *r,
        crate::registrar::runtime::RoundOutcome::Locked { until } => {
            let mut b = serde_json::json!({
                "error": { "type": "conflict",
                           "message": "另一个副本正在补池，等它跑完。" },
                "reason": "locked",
            });
            if let Some(u) = until {
                b["until"] = serde_json::json!(u);
            }
            return (StatusCode::CONFLICT, Json(b)).into_response();
        }
    };

    let capped_json = capped.map(|c| {
        serde_json::json!({
            "before": {
                "mintBatch": c.before.mint_batch,
                "maxDomainAttempts": c.before.max_domain_attempts,
                "codeTimeoutMs": c.before.code_timeout_ms,
            },
            "after": {
                "mintBatch": c.after.mint_batch,
                "maxDomainAttempts": c.after.max_domain_attempts,
                "codeTimeoutMs": c.after.code_timeout_ms,
            },
        })
    });

    // 🔴 **`outcome.kind` 三种不许被读成同一种「失败」。**
    //
    // `skipped` **有且只有一个含义**：注册机关着。拿它表示别的就是伪造。
    // 而「跑了一轮、一把都没铸出来」与「压根没跑」在面板上要长得不一样 ——
    // 前者要去看 failures 里的原因，后者要去看开关。
    // ⚠️ **Rust 侧只会出现 `done` / `skipped` 两种，`crashed` 没有产生点** ——
    // `run_round` 是全函数，真 panic 会一路冒到 HTTP 层变成 500。
    // 面板那边三档都接着（它的 `crashed` 分支是防御性的），这里如实登记，
    // 免得下一个人以为少写了一支。
    let kind = if result.skipped { "skipped" } else { "done" };

    // 🔴 **回 200 并带上真实结果**，不是 202「已开始」。
    //
    // 📜 `202 → 200` 是一次已发生的破坏性变更：从前一收到点击就返回、
    // 整轮交给响应之后的后台任务，而平台会在响应结束后约 30 秒把它取消掉（3/3 复现）
    // ⇒ 补池历史、事件与锁的释放**全部落空**，面板上那一轮与「压根没点过」不可区分。
    // `started: true` 保留给旧面板。
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "started": true,
            "done": true,
            "trigger": "manual",
            "channel": chosen.map(crate::registrar::config::Channel::as_str),
            "outcome": {
                "kind": kind,
                "result": {
                    "skipped": result.skipped,
                    "available": result.available,
                    "attempted": result.attempted,
                    "minted": result.minted,
                    "mintedByChannel": result.minted_by_channel,
                    "failures": result.failures,
                    "at": result.at,
                    "primaryChannel": result.primary_channel,
                    "durationMs": result.duration_ms,
                },
                "capped": capped_json,
            },
            // 🔴 **成功那一支也照样给**：只在耗尽那一支给它，
            // 等于让运维毫不知情地一路撞上那堵墙。
            "remaining": remaining,
            "resetAt": reset_at,
            "cooldownUntil": cooldown_until,
            "retryAfterMs": retry_after_ms,
        })),
    )
        .into_response()
}

/// `GET /admin/api/registrar/status`。
///
/// 🔴 **这条端点一次上游请求都不发。**
/// 面板会轮询它 —— 为了凑一个「域名总数」之类的数去打一次上游，
/// **等于给面板加了一颗每几秒就打一次上游的按钮**。取不到就如实说没有。
pub async fn status(State(st): State<AppState>) -> Response {
    let Some(rt) = st.registrar.clone() else {
        return not_wired(None);
    };
    let live = live(&rt).await;
    // 面板要能说「还剩 19 次（共 24 次）」，而不是等运维撞上那堵墙才知道有墙。
    // ⚠️ **一格名额都不消费**，与 `tend` 那侧共用 `used_today` 免得跨天那一瞬对不上。
    let quota = {
        use crate::registrar::guard as g;
        let cur = g::narrow_manual_guard(
            rt.storage
                .get(g::MANUAL_GUARD_KEY)
                .await
                .ok()
                .flatten()
                .as_ref(),
        );
        g::manual_tend_quota(cur.as_ref(), crate::adapters::now_ms())
    };
    let ledger = rt.ledger().await;
    let backoff = rt.backoff().await;
    let history = rt.history().await;
    let now = crate::adapters::now_ms();
    let summary = crate::registrar::domain_ledger::summarize_ledger(&ledger, now);

    // 池子那一格。**`counted` 刻意不叫 `available`**，见下。
    let records = st.repo.all(now).await.unwrap_or_default();
    let counted = records
        .iter()
        .filter(|r| crate::core::keypool::counts_toward_target(r))
        .count();
    let health = crate::core::keypool::pool_health(&records, now);
    let target = live.config.target_keys as usize;

    // 跨副本那把锁还占着吗。**过期就给 null**：一个已经过去的时刻
    // 会让面板渲染出一个恒为负的倒计时，而那与「没锁着」是同一件事。
    let locked_until = rt
        .storage
        .get(crate::registrar::runtime::TEND_LOCK_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.get("until").and_then(serde_json::Value::as_i64))
        .filter(|u| *u > now);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "wired": true,
            "serverTime": now,
            "enabled": live.config.enabled,
            // ⚠️ **`blocked` 为真时 `enabled` 一个字都不改** ——
            // 真话是「已启用 · 本次没跑起来」，不是「未启用」。
            "blocked": live.config.blocked,
            "channel": live.config.channel.as_str(),
            "targetKeys": live.config.target_keys,
            // 两条通道**都**报配置状态 —— 不然「想在切换前先测一下另一条」就成了鸡生蛋。
            //
            // ⚠️ `selected` 与 `configured` **是两件事**：选中的那条可能没配凭据，
            // 没选中的那条可能配好了。两格都给，面板两张卡才画得出四种组合。
            // 🔴 **渲染顺序不从这个对象来** —— 对象键序是后端写出来的，
            // 改后端一行就能悄悄改变面板上谁排在前面，而任何顺序都会被读成排名。
            "channels": {
                "yyds": {
                    "configured": live.config.yyds.is_some(),
                    "selected": live.config.channel == crate::registrar::config::Channel::Yyds,
                },
                "moemail": {
                    "configured": live.config.moemail.is_some(),
                    "selected": live.config.channel == crate::registrar::config::Channel::Moemail,
                },
            },
            // 🔴 **`counted` 刻意不叫 `available`**：它的判据是「占名额数」，
            // **被停用的与正在冷却的都算在里面，而这两种恰恰都不能打上游**。
            // 真正能打的是并列的 `fresh`。两个数都给，且都住在 `pool` 里不在顶层
            // —— 放顶层就迟早有人把 `counted` 当成「还能用几把」。
            "pool": {
                "target": target,
                "counted": counted,
                "gap": target.saturating_sub(counted),
                "fresh": health.fresh,
                // 报的是**压顶之后**那个数：面板上这一格要与「立即补池」真的会铸几把一致。
                "mintBatch": live.config.mint_batch.min(crate::registrar::types::MANUAL_MINT_BATCH),
            },
            "lockedUntil": locked_until,
            "blockers": live.blockers,
            "notices": live.notices,
            "warnings": live.warnings,
            // ⚠️ 只在**还没过期**时才给值，理由同 `lockedUntil`。
            "backoff": backoff.filter(|b| b.until > now).map(|b| serde_json::json!({
                "until": b.until,
                "kind": b.kind,
                "since": b.since,
                "hits": b.hits,
                "retryAfterMs": crate::registrar::backoff::retry_after_ms(Some(&b), now),
            })),
            "domains": {
                // ⚠️ **上一轮记下来的观测值，不是现打一次问出来的。取不到就如实 null。**
                "total": summary.total,
                "ok": summary.ok,
                "blocked": summary.blocked,
                "suspect": summary.suspect,
                "unknown": summary.unknown,
                "updatedAt": summary.updated_at,
                // 「值有没有快撑满」只有两个数一起才说得出来。
                "size": summary.size,
                "cap": summary.cap,
            },
            "manual": {
                "remaining": quota.remaining,
                "used": quota.used,
                "perDay": quota.per_day,
                "resetAt": quota.reset_at,
                // ⚠️ 不在冷却中时**两个都是 null**，不是 0 ——
                // 一个「已经过去的时刻」会渲染成恒为 0 的倒计时。
                "cooldownUntil": quota.cooldown_until,
                "retryAfterMs": quota.retry_after_ms,
            },
            "history": { "entries": history, "malformed": 0 },
        })),
    )
        .into_response()
}

/// `POST /admin/api/registrar/channels/{channel}/test`（**真打一次外网**）。
///
/// 🔴 **真的验一次凭据** —— 不是只读一次域名列表就报绿。
/// TS 版有过这个真缺陷：**凭据粘错时它照样报绿**，因为列域名那一步在有的站点上不查凭据。
/// 一次真机观测（2026-08-25 北京时间）：同一把**故意写错**的 key，
/// 打 YYDS 的列域名端点回 **200**、打 MoeMail 的回 **401**。
/// 🔴 **射程**：那是那一天那两个部署的反应，**不是这两家服务的性质，更不是常数**
/// ⇒ 处置是「别把它写成按通道分支」，而不是照着它给某条通道写一档。
///
/// # ⚠️ 它不是零副作用的，代价明写
///
/// 验凭据怎么验**由适配器决定，本层不知道也不该知道**：YYDS 那条是
/// 「建一个临时邮箱再删掉」⇒ **每点一次消耗一个活跃邮箱名额**；MoeMail 那条零名额。
/// **这个差别不许被搬到 handler 里做分支** —— 那等于把一句关于别人家服务
/// 今天行为的断言钉进面板。
///
/// # ⚠️ 它挡不住的那一档，明写
///
/// 上游把鉴权失败回成 **200 + 错误体**时，两步都不报错，这里照样报绿。
/// 这一档**没有判据、也做不出判据**（把「什么样的 200 算失败」写成规则，
/// 又是一张关于别人家服务今天行为的手写表）—— 如实登记为已知射程边界。
///
/// **成功那一支一次存储写都不产生。**
pub async fn channel_test(State(st): State<AppState>, Path(channel): Path<String>) -> Response {
    channel_test_with(&st, channel, crate::admin::keys::probe_guard()).await
}

/// [`channel_test`] 的本体，**护栏可注入**。
///
/// 理由与别的探测端点逐字相同：护栏是**全进程单例**，
/// 而「连点被挡」「挡完之后能恢复」这两条性质在固定单例上不可观测。
pub async fn channel_test_with(
    st: &AppState,
    channel: String,
    guard: &crate::admin::keys::ProbeGuard,
) -> Response {
    let Some(rt) = st.registrar.clone() else {
        return not_wired(Some(channel));
    };
    // ① 通道名先判 —— 认不出来的名字连「注册机开没开」都不必问。
    let Some(ch) = crate::registrar::config::Channel::parse_public(&channel) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": { "type": "invalid_request_error",
                           "message": format!("不认识的通道名「{channel}」，只有 yyds 与 moemail") },
                "reason": "unknown_channel",
            })),
        )
            .into_response();
    };

    // ② 装配这一截单独接住：它失败时**一次上游请求都没发出去**。
    //
    // 🔴 **`not_attempted` 是独立的一档，不许并进任何一个上游档。**
    // 并进去就是「把本网关自己的故障说成上游故障」，而处置方向正好相反：
    // 这一档要去看的是**存储**，不是地址、DNS、TLS 与上游。
    let Some(live) = rt.reload().await else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": false,
                "channel": channel,
                "reason": "not_attempted",
                "latencyMs": 0,
            })),
        )
            .into_response();
    };

    // ③④ 开关两档**分开报**：面板对 `registrar_disabled` 的文案是
    // 「注册机没有打开……请先在设置里打开它」，而 blocked 那一档开关明明是开的
    // —— 照旧混报就是撒谎。
    if !live.config.enabled {
        return channel_conflict(
            "registrar_disabled",
            &channel,
            "注册机在设置里是关着的 —— 打开它再点这颗按钮。",
        );
    }
    if live.config.blocked {
        let why = live
            .blockers
            .iter()
            .map(|b| b.msg.as_str())
            .collect::<Vec<_>>()
            .join("；");
        return channel_conflict(
            "registrar_blocked",
            &channel,
            &format!("注册机已启用，但本次没跑起来：{why}"),
        );
    }
    // ⑤ 这条通道没配凭据 ⇒ 409，**一次上游请求都不发**。
    let configured = match ch {
        crate::registrar::config::Channel::Yyds => live.config.yyds.is_some(),
        crate::registrar::config::Channel::Moemail => live.config.moemail.is_some(),
    };
    if !configured {
        return channel_conflict(
            "channel_not_configured",
            &channel,
            "这条通道还没配凭据 —— 填好再点这颗按钮。",
        );
    }

    // ⑥ 连点护栏。**每条通道各占一格**：测 yyds 不该把 moemail 那颗按钮也锁上。
    //
    // 🔴 走 `acquire()` 拿租约，不手写 `release` —— 手写配对漏掉「请求被取消」那一支，
    // 而「上游不可达」恰恰是这条端点的常态支路。
    let kind = format!("channel:{}", ch.as_str());
    let _lease = match guard.acquire(&kind, crate::adapters::now_ms()) {
        Ok(l) => l,
        Err(refused) => {
            let reason = refused.reason().expect("acquire 的 Err 支只可能是两种拒绝");
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": { "type": "rate_limit_error",
                               "message": "刚测过这条通道，等一下再点。" },
                    "reason": reason,
                    "channel": channel,
                })),
            )
                .into_response();
        }
    };

    let started = crate::adapters::now_ms();
    match rt.probe_channel(&live.config, ch).await {
        Ok(ok) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "channel": channel,
                "domains": ok.domains,
                "latencyMs": crate::adapters::now_ms() - started,
                "credentials": ok.credentials,
                // ⚠️ `false` = 建出来了却没删掉，它**占着一条活跃邮箱名额**。
                // 面板要让运维去事件板块找 `registrar.delete_mailbox_failed`。
                "cleaned": ok.cleaned,
            })),
        )
            .into_response(),
        Err(e) => {
            // 🔴 **分档依据是这一次的状态码，不是通道名。**
            // 按通道写死的静态表说的是「别人家服务今天怎么反应」，
            // 上游改一次版它就变成一句门禁看不见的假话。
            // ⇒ 同一个 401 在两条通道上必须拿到逐字相同的结论。
            //
            // ⚠️ `401` 与 `403` **刻意合成一档**：403 也可能是「凭据有效但没这个权限」
            // 「活跃邮箱名额用光」「出口地址被拒」。要分开就得去解析上游的错误码字符串，
            // 而那正是本仓明令禁止的手写关键词表。
            // ⇒ 保留这处粗粒度，靠措辞承住，**不写死「你的 key 错了」**。
            let reason = match e.status {
                Some(401 | 403) => "credentials_rejected",
                Some(429) => "rate_limited",
                _ => "upstream_error",
            };
            let latency = crate::adapters::now_ms() - started;
            st.logger.log(
                LogEntry::new(
                    LogLevel::Warn,
                    EVENT_CHANNEL_TEST_FAILED,
                    "通道连通性测试失败（列域名 / 验凭据这两步里的某一步）",
                )
                .with("channel", channel.clone())
                .with("reason", reason)
                .with("latencyMs", latency)
                .with("err", e.detail.clone()),
            );
            let mut body = serde_json::json!({
                "ok": false,
                "channel": channel,
                "reason": reason,
                "latencyMs": latency,
                // 🔴 三档 `reason` 是给面板选文案的，
                // 而「到底哪一步、上游说了什么」只有这一句说得出来。
                "detail": e.detail,
            });
            // ⚠️ **拿不到状态码时这一格不出现，不伪造一个兜底值。**
            if let Some(code) = e.status {
                body["status"] = serde_json::json!(code);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
    }
}

/// §9.17 那几支 409：**都带上 `channel`**，面板两张卡各自渲染各自那张。
fn channel_conflict(code: &'static str, channel: &str, msg: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": { "type": "conflict", "message": msg },
            "reason": code,
            "channel": channel,
        })),
    )
        .into_response()
}

/// 注册机关着 / 被拦下时那一档。
///
/// ⚠️ **走 409 而不是 503**：`503 not_wired` 的含义是「这个部署没装注册机」，
/// 而这里是「装了、但这一刻不能跑」—— 两者的下一步动作完全不同。
/// 🔴 **两支拒绝各有各的 `code`，不许合并成一个。**
///
/// `registrar_disabled` = 「你自己把它关了」，处置是去面板打开；
/// `registrar_blocked` = 「你打开了，但这份配置跑不起来」，处置是去补凭据 / 改参数。
/// 合并成一个 `registrar_not_runnable` 之后，**面板对这两种情况只能说同一句话**，
/// 而运维要做的事完全相反。
fn conflict(code: &'static str, msg: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            // ⚠️ **`type` 跟着 HTTP 那一档走**（02 §4.4 逐字：409 是 `conflict`），
            // 而**顶层 `reason` 才是面板据以选文案的那一格**。
            // 🔴 这一族**不带 `code`** —— 带 `code` 的是管理信封（§4.2），
            // 两种信封不许合并。
            "error": { "type": "conflict", "message": msg },
            "reason": code,
        })),
    )
        .into_response()
}

// ── 判据 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::app::build_router;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    /// 三条端点的**真实**方法与对外路径。判据遍历它。
    const ALL: [(Method, &str); 3] = [
        (Method::POST, "/admin/api/registrar/tend"),
        (Method::GET, "/admin/api/registrar/status"),
        (Method::POST, "/admin/api/registrar/channels/moemail/test"),
    ];

    async fn call(app: &Router, method: Method, uri: &str) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&b)
                .unwrap_or_else(|_| panic!("响应必须是合法 JSON：{}", String::from_utf8_lossy(&b))),
        )
    }

    async fn app() -> Router {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        assert!(!state.registrar_wired(), "夹具前提：P1 恒没接线");
        build_router(state)
    }

    /// 🔴 **三条逐条 `503 not_wired`，而且是 02 §4.4 那个混合信封。**
    ///
    /// ⚠️ **不许是 404**：那会把「这个部署没装注册机」说成「这个网关没有这个功能」，
    /// 而五语言 API.md 逐条写着这三条端点存在。
    /// ⚠️ **也不许是 200 + 一份空状态**：一份 `enabled:false` 的假状态在面板上与
    /// 「真的关着」长得一模一样。
    #[tokio::test]
    async fn all_three_report_not_wired_with_the_mixed_envelope() {
        let app = app().await;
        for (method, uri) in ALL {
            let (status, v) = call(&app, method.clone(), uri).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} {uri} 必须是 503"
            );
            assert_eq!(v["reason"], "not_wired", "{method} {uri} 的顶层 reason");
            assert_eq!(
                v["error"]["type"], "internal_error",
                "{method} {uri} 的信封类型"
            );
            // 🔴 **不带 `code`**：`AdminErrorCode` 是闭集，这一档不在里面（面板按 `reason` 选文案）。
            assert!(
                v["error"].get("code").is_none(),
                "{method} {uri} 不许带 code：{v}"
            );
        }
    }

    /// ⚠️ **只有通道那一条带 `channel`，另外两条一格都不许多。**
    ///
    /// 一个恒 `null` 的 `channel` 在面板上说的是「这次没取到通道名」——
    /// 而那两条端点本来就不针对某一个通道。
    #[tokio::test]
    async fn only_the_channel_endpoint_echoes_the_channel() {
        let app = app().await;
        let (_, tend) = call(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert!(tend.get("channel").is_none(), "tend 不该有 channel：{tend}");
        let (_, status) = call(&app, Method::GET, "/admin/api/registrar/status").await;
        assert!(
            status.get("channel").is_none(),
            "status 不该有 channel：{status}"
        );

        for ch in ["moemail", "yyds"] {
            let (_, v) = call(
                &app,
                Method::POST,
                &format!("/admin/api/registrar/channels/{ch}/test"),
            )
            .await;
            assert_eq!(v["channel"], ch, "通道名要原样带回来");
        }
    }

    /// 🔴 **三条照常在闸后面**：不带凭据一律 401，而且与「不存在的管理路径」分不出来。
    ///
    /// 占位端点最容易犯的错是「反正没实现，闸不闸无所谓」——
    /// 而那正好把这三条变成三个免鉴权的、可以用来确认「这个网关是 agnes2api」的探针。
    #[tokio::test]
    async fn all_three_still_sit_behind_the_gate() {
        let app = app().await;
        for (method, uri) in ALL {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} 不带凭据必须 401"
            );
        }
    }

    /// 🔴 **点一下「立即补池」要留下一条事件；两条读端点一条都不许打。**
    ///
    /// 前半：运维点了没反应时，事件板块是他唯一查得到「为什么」的地方。
    /// 后半：面板会**轮询** `status`，给它打事件等于让这个没接线的功能把事件环刷满，
    /// 把真正的告警挤出环外。
    #[tokio::test]
    async fn only_the_manual_button_leaves_an_event() {
        let TestApp {
            mut state,
            recorded,
            ..
        } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);

        for _ in 0..3 {
            let _ = call(&app, Method::GET, "/admin/api/registrar/status").await;
            let _ = call(
                &app,
                Method::POST,
                "/admin/api/registrar/channels/moemail/test",
            )
            .await;
        }
        assert!(
            !recorded.events().contains(&EVENT_MANUAL_TEND_NOT_WIRED),
            "读端点不许打这条事件：{:?}",
            recorded.events()
        );

        let _ = call(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(
            recorded
                .events()
                .iter()
                .filter(|e| **e == EVENT_MANUAL_TEND_NOT_WIRED)
                .count(),
            1,
            "点一下 = 恰好一条：{:?}",
            recorded.events()
        );
    }

    /// 🔴 **请求路径上零存储 I/O。** 没接线的功能不许给存储加一次读写。
    ///
    /// ⚠️ 范围说清楚：tend 那条打的事件会进事件环，而环的落盘由 `log_flush`
    /// 按它自己的节流统一负责 —— **那不算这三条端点的账**，也正因如此
    /// 这一格在冻结的 sink 时钟下数得出精确次数。
    #[tokio::test]
    async fn a_not_wired_registrar_touches_storage_zero_times() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { mut state, .. } = state_with_storage(counting).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);

        // 先跑一轮，把「与本组无关的那些一次性读」（配置刷新）挡在计数之外。
        for (method, uri) in ALL {
            let _ = call(&app, method, uri).await;
        }
        let before = counts.snapshot();
        for (method, uri) in ALL {
            let _ = call(&app, method, uri).await;
        }
        assert_eq!(counts.snapshot(), before, "没接线的三条必须零存储 I/O");
    }
}

// ── 面板读的是「现在那份」，不是启动那份 ──────────────────────────────────
//
// 这三条钉的是同一个缺陷的三个出口。它曾经同时存在于三条端点上：
// 面板是这份配置的编辑器，而三条端点读的都是启动那一刻的快照 ⇒
// **保存成功，界面照旧、按钮照旧、测试按钮验的还是旧凭据**，
// 而运维没有任何线索知道「其实要重启容器」。

#[cfg(test)]
mod live_config_tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::ports::storage::Storage;
    use crate::server::app::build_router;
    use crate::server::test_support::{TestApp, UnreachableFetcher, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    /// 装一套**接了注册机**的面板，env 只给凭据 ——
    /// `REGISTRAR_ENABLED` 一律不给，否则 env 压过存储，面板永远改不动。
    async fn wired(storage: Arc<dyn Storage>) -> (Router, TestApp) {
        let mut app = state_with_storage(Arc::clone(&storage)).await;
        let env: crate::core::config_provenance::EnvMap =
            [("YYDS_API_KEY".to_string(), "yk".to_string())]
                .into_iter()
                .collect();
        let l = crate::registrar::runtime::load(&env, &serde_json::Value::Null);
        app.state.registrar = Some(Arc::new(crate::registrar::runtime::RegistrarRuntime {
            config: l.config,
            blockers: l.blockers,
            notices: l.notices,
            warnings: l.warnings,
            storage,
            logger: Arc::clone(&app.state.logger),
            fetcher: Arc::new(UnreachableFetcher),
            env,
        }));
        app.state.admin_token = Some(Arc::new(ADMIN.to_string()));
        (build_router(app.state.clone()), app)
    }

    /// 同上，但上游是**脚本化**的 —— §9.17 的失败分档要按真实状态码观测。
    async fn wired_with_upstream(
        storage: Arc<dyn Storage>,
        script: Vec<crate::server::test_support::UpstreamReply>,
    ) -> (AppState, Arc<crate::server::test_support::ScriptedFetcher>) {
        let f = crate::server::test_support::ScriptedFetcher::new(script);
        let mut app = state_with_storage(Arc::clone(&storage)).await;
        let env: crate::core::config_provenance::EnvMap =
            [("YYDS_API_KEY".to_string(), "yk".to_string())]
                .into_iter()
                .collect();
        let l = crate::registrar::runtime::load(&env, &serde_json::Value::Null);
        app.state.registrar = Some(Arc::new(crate::registrar::runtime::RegistrarRuntime {
            config: l.config,
            blockers: l.blockers,
            notices: l.notices,
            warnings: l.warnings,
            storage,
            logger: Arc::clone(&app.state.logger),
            fetcher: f.clone(),
            env,
        }));
        app.state.admin_token = Some(Arc::new(ADMIN.to_string()));
        (app.state.clone(), f)
    }

    /// 🔴 **走本体、各判据自带一把护栏，不碰全进程那把单例。**
    ///
    /// 护栏是进程级的，而「连点被挡」这条性质恰恰要把它占住 ——
    /// 用单例就会让同进程里跑在旁边的判据随机地拿到 429。
    async fn probe(
        st: &AppState,
        ch: &str,
        guard: &crate::admin::keys::ProbeGuard,
    ) -> (StatusCode, Value) {
        let res = channel_test_with(st, ch.to_string(), guard).await;
        let code = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    fn reply(status: u16, body: Value) -> crate::server::test_support::UpstreamReply {
        crate::server::test_support::UpstreamReply {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![bytes::Bytes::from(body.to_string().into_bytes())],
        }
    }

    async fn get(app: &Router, method: Method, uri: &str) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let st = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (st, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn panel_saves(s: &dyn Storage, patch: Value) {
        s.put(
            crate::core::config_provenance::CONFIG_KEY,
            serde_json::json!({ "registrar": patch }),
            None,
        )
        .await
        .unwrap();
    }

    /// 🔴 **保存之后，面板自己显示的必须是新值。**
    #[tokio::test]
    async fn the_status_endpoint_shows_what_was_just_saved_not_the_startup_snapshot() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;

        let (_, before) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert_eq!(before["enabled"], false, "起点必须是关着的");

        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "targetKeys": "42" }),
        )
        .await;

        let (_, after) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert_eq!(
            after["enabled"], true,
            "🔴 保存成功，而面板显示的还是旧值 —— 运维只会以为自己没点到",
        );
        assert_eq!(after["targetKeys"], 42, "数值字段同理");
    }

    /// 🔴 **面板打开注册机之后，「立即补池」必须真的能按。**
    ///
    /// 读启动快照的形态是：刚在面板上打开，一点按钮却回
    /// 409「注册机在设置里是关着的 —— 打开它再点这颗按钮」。
    /// **那句话此刻是假的**，而它恰恰是在指挥运维去做他已经做过的事。
    #[tokio::test]
    async fn the_tend_button_works_right_after_the_panel_enables_the_registrar() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;

        let (st, body) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st, StatusCode::CONFLICT, "还关着的时候本来就该 409：{body}");

        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let (st2, body2) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_ne!(
            st2,
            StatusCode::CONFLICT,
            "🔴 刚在面板上打开，按钮却说「去打开它」—— 指挥运维去做他已经做过的事：{body2}",
        );
    }

    /// 🔴 **`counted` 与 `fresh` 是两个数，而且都必须给。**
    ///
    /// `counted` 的判据是「占名额数」：**被停用的、正在冷却的都算在里面，
    /// 而这两种恰恰都不能打上游**。只给它，面板就会说「池子里有 3 把」
    /// 而转发那边正在回 503。
    #[tokio::test]
    async fn the_pool_block_separates_counted_from_actually_usable() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, t) = wired(Arc::clone(&mem)).await;

        // 两把 key：一把能用，一把被停用。
        t.state.repo.add("sk-good", 0).await.unwrap();
        let bad = t.state.repo.add("sk-bad", 0).await.unwrap();
        let mut rec = bad.clone();
        rec.disabled = Some(true);
        mem.put(
            &format!("key:{}", rec.id),
            serde_json::to_value(&rec).unwrap(),
            None,
        )
        .await
        .unwrap();
        t.state.repo.invalidate().await;

        let (_, b) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert_eq!(b["pool"]["counted"], 2, "停用的照样占着名额：{b}");
        assert_eq!(
            b["pool"]["fresh"], 1,
            "🔴 只给一个数的话，面板会说「有 2 把」而转发那边正在 503：{b}",
        );
        assert!(
            b["pool"]["gap"].is_i64(),
            "缺口要直接给，别让面板自己减：{b}"
        );
        assert!(
            b["counted"].is_null() && b["fresh"].is_null(),
            "🔴 这两个数不许放顶层 —— 放上去迟早有人把 counted 当成「还能用几把」：{b}",
        );
    }

    /// 🔴 **两条通道的 `selected` 与 `configured` 是两件事，四种组合都要画得出。**
    #[tokio::test]
    async fn each_channel_reports_both_whether_it_is_configured_and_whether_it_is_selected() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        // ⚠️ 先打开它：**关着的注册机一条凭据都不解析**（那是刻意的，
        // 有判据钉着），所以 `configured` 在关着时两条都是 false。
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;
        let (_, b) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        // env 只给了 YYDS 的 key，而选中的也是 yyds（默认）。
        assert_eq!(b["channels"]["yyds"]["configured"], true, "{b}");
        assert_eq!(b["channels"]["yyds"]["selected"], true);
        assert_eq!(b["channels"]["moemail"]["configured"], false);
        assert_eq!(
            b["channels"]["moemail"]["selected"], false,
            "🔴 少了 selected 这一格，面板分不出「没配」和「没选」：{b}",
        );
    }

    /// 🔴 **锁过期了就给 `null`，不给一个已经过去的时刻。**
    ///
    /// 给过去的时刻会让面板渲染出一个恒为负的倒计时，
    /// 而那与「根本没锁着」是同一件事、长得却不一样。
    #[tokio::test]
    async fn an_expired_tend_lock_is_reported_as_absent_not_as_a_past_timestamp() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        // 写一把**早就过期**的锁。
        mem.put(
            crate::registrar::runtime::TEND_LOCK_KEY,
            serde_json::json!({ "until": 1 }),
            None,
        )
        .await
        .unwrap();

        let (_, b) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert!(
            b["lockedUntil"].is_null(),
            "🔴 过期的锁被报成还锁着 —— 面板上一个恒为负的倒计时：{b}",
        );
    }

    /// 域名那一格要按选择器的口径分档，而不是只给一个总数。
    #[tokio::test]
    async fn the_domains_block_reports_the_same_buckets_the_selector_uses() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        let (_, b) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        for k in [
            "total",
            "ok",
            "blocked",
            "suspect",
            "unknown",
            "updatedAt",
            "size",
            "cap",
        ] {
            assert!(
                b["domains"].get(k).is_some(),
                "🔴 少了 domains.{k} —— 面板说不出「哪些还能用」：{b}",
            );
        }
        assert!(
            b["domains"]["total"].is_null(),
            "⚠️ 一轮都没跑过就如实说不知道，**不为了凑一个数去打上游**：{b}",
        );
    }

    /// `serverTime` 要给 —— 面板所有倒计时都靠它对齐本地时钟偏差。
    #[tokio::test]
    async fn the_status_reports_server_time_so_countdowns_can_be_aligned() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        let (_, b) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert!(
            b["serverTime"].is_i64(),
            "🔴 没有它，面板只能拿浏览器本地时钟去减服务端的时刻：{b}",
        );
    }

    /// 🔴 **`/status` 一次上游请求都不发。**
    ///
    /// 面板会轮询它。为了凑一个「域名总数」之类的数去打一次上游，
    /// **等于给面板加了一颗每几秒就打一次上游的按钮**。
    #[tokio::test]
    async fn the_status_endpoint_never_touches_the_upstream() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(Arc::clone(&mem), vec![]).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;
        let app = build_router(st_app);
        for _ in 0..3 {
            let (code, _) = get(&app, Method::GET, "/admin/api/registrar/status").await;
            assert_eq!(code, StatusCode::OK);
        }
        assert!(
            f.seen().is_empty(),
            "🔴 面板每轮询一次就打一次上游：{:?}",
            f.seen(),
        );
    }

    // ── §9.15 补池：请求体与那道八支拒绝阶梯 ──────────────────────────────

    async fn post_body(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header("x-admin-key", ADMIN)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let code = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// 🔴 **未知字段要当场 400，不许静默忽略。**
    ///
    /// 忽略的话，「我明明传了 `chanel`」会变成一次查不出原因的
    /// 「怎么没换通道」—— 而运维手上唯一的线索是一个 200。
    #[tokio::test]
    async fn an_unknown_field_in_the_body_is_rejected_instead_of_silently_ignored() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let (st, b) = post_body(&app, "/admin/api/registrar/tend", r#"{"chanel":"yyds"}"#).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "🔴 拼错的字段被当成没传 —— 运维只会看见「怎么没换通道」：{b}",
        );
        assert!(
            b["error"]["code"].is_null(),
            "⚠️ 这三支是**不带 code 的网关信封**：它们不是注册机的某种状态，\
             是这条请求本身就不对：{b}",
        );
    }

    /// 非法 JSON / 非对象 ⇒ 400；空体 ⇒ 当 `{}` 放行。
    #[tokio::test]
    async fn a_malformed_body_is_a_400_but_an_empty_body_is_fine() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        for bad in ["{不是 JSON", "[1,2]", r#""字符串""#, r#"{"channel":1}"#] {
            let (st, b) = post_body(&app, "/admin/api/registrar/tend", bad).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "`{bad}` 该 400：{b}");
        }
        let (st, b) = post_body(&app, "/admin/api/registrar/tend", "").await;
        assert_eq!(st, StatusCode::OK, "空体 = {{}}，不是错误：{b}");
    }

    /// 🔴 **信封的 `type` 跟着 HTTP 那一档走，而且这一族不带 `code`。**
    ///
    /// 02 §4.4 把这两条端点的**全部拒绝支**都归进「网关信封 + 顶层附加字段」
    /// 那一类：409 的 `type` 是 `conflict`、400 是 `invalid_request_error`、
    /// 429 是 `rate_limit_error`，而**面板据以选文案的是顶层 `reason`**。
    /// 带 `code` 的是另一种信封（管理信封，§4.2）—— **两种不许合并**。
    #[tokio::test]
    async fn the_rejection_envelopes_match_the_http_class_and_carry_no_code() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;

        // 409：关着的注册机。
        let (s1, b1) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(s1, StatusCode::CONFLICT);
        assert_eq!(b1["error"]["type"], "conflict", "{b1}");
        assert!(
            b1["error"]["code"].is_null(),
            "🔴 混进了管理信封那一格：{b1}"
        );
        assert_eq!(b1["reason"], "registrar_disabled");

        // 400：认不出来的通道名。
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;
        let (s2, b2) = post_body(&app, "/admin/api/registrar/tend", r#"{"channel":"x"}"#).await;
        assert_eq!(s2, StatusCode::BAD_REQUEST);
        assert_eq!(b2["error"]["type"], "invalid_request_error", "{b2}");
        assert!(b2["error"]["code"].is_null(), "{b2}");

        // 409（通道测试那一侧）。
        let g = crate::admin::keys::ProbeGuard::new();
        let (s3, b3) = probe(&_t.state, "moemail", &g).await;
        assert_eq!(s3, StatusCode::CONFLICT);
        assert_eq!(b3["error"]["type"], "conflict", "{b3}");
        assert!(b3["error"]["code"].is_null(), "{b3}");
        assert_eq!(b3["channel"], "moemail", "409 那几支要带 channel");
    }

    /// 🔴 **开关两档排在解析请求体之前。**
    ///
    /// 反过来的话，一个关着注册机的部署上，一次字段名笔误拿到的是
    /// 「请求体只认 channel 这一个键」—— 而运维真正要先做的是**把注册机打开**。
    /// 先说那个**必须先解决**的理由，他才不会去修一个改完照样不工作的东西。
    #[tokio::test]
    async fn the_disabled_check_comes_before_the_body_is_parsed() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        // 注册机关着，同时请求体也是坏的 —— 两条都成立，看它先说哪一条。
        let (st, b) = post_body(&app, "/admin/api/registrar/tend", "{不是 JSON").await;
        assert_eq!(
            st,
            StatusCode::CONFLICT,
            "🔴 先说了「请求体不对」—— 而那位运维改完请求体，照样什么都不会发生：{b}",
        );
        assert_eq!(b["reason"], "registrar_disabled");
    }

    /// 指定的通道认不出来 ⇒ 400 `unknown_channel`。
    #[tokio::test]
    async fn an_unknown_channel_in_the_body_is_rejected() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let (st, b) = post_body(&app, "/admin/api/registrar/tend", r#"{"channel":"yydss"}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{b}");
        assert_eq!(b["reason"], "unknown_channel");
    }

    /// 🔴 **指定一条没配凭据的通道 ⇒ 409，而且一次上游都不打。**
    #[tokio::test]
    async fn asking_for_an_unconfigured_channel_is_a_409_before_any_upstream_call() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(Arc::clone(&mem), vec![]).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;
        let app = build_router(st_app);

        let (st, b) = post_body(
            &app,
            "/admin/api/registrar/tend",
            r#"{"channel":"moemail"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{b}");
        assert_eq!(b["reason"], "channel_not_configured");
        assert_eq!(b["channel"], "moemail");
        assert!(
            f.seen().is_empty(),
            "🔴 没配凭据还去打了上游：{:?}",
            f.seen()
        );
    }

    /// 🔴 **跨副本那把锁占着 ⇒ 409 `locked`，不是「补池失败了」。**
    ///
    /// 真话是「另一边正在补，等它 N 秒」。混成一次失败的补池之后，
    /// 面板那行会说「补池失败」，而运维会去查一个根本不存在的铸造故障。
    #[tokio::test]
    async fn a_held_cross_replica_lock_is_a_409_not_a_failed_round() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;
        // 另一个副本正占着锁。
        mem.put(
            crate::registrar::runtime::TEND_LOCK_KEY,
            serde_json::json!({ "until": crate::adapters::now_ms() + 60_000 }),
            None,
        )
        .await
        .unwrap();

        let (st, b) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st, StatusCode::CONFLICT, "{b}");
        assert_eq!(b["reason"], "locked");
        assert!(
            b["until"].is_i64(),
            "🔴 只说「锁着」说不出「还要等多久」：{b}",
        );
        // 而且这一轮什么都没发生 ⇒ 不许往那张定长表里挤一行。
        assert!(
            mem.get(crate::registrar::runtime::TEND_HISTORY_KEY)
                .await
                .unwrap()
                .is_none(),
            "🔴 什么都没发生却记了一行 —— 它会把一条真实记录挤出去",
        );
    }

    /// 🔴 **进程内那道闸与跨副本那把锁不是冗余。**
    ///
    /// 一把挡同进程重入（面板连点、定时轮撞上按钮），一把挡跨副本重叠。
    /// 少了进程内这道，同一个进程里两条路会各自去抢那把**没有 CAS** 的存储锁，
    /// 而那正是存储锁自己挡不住的那一档。
    #[tokio::test]
    async fn a_round_already_running_in_this_process_is_refused_before_the_storage_lock() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        // 本进程里已经有一轮在跑。
        let held = crate::server::state::TendInFlight::enter(&t.state.tend_in_flight);
        assert!(held.is_some(), "前提：这一把占得住");

        let (st, b) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st, StatusCode::CONFLICT, "{b}");
        assert_eq!(
            b["reason"], "tend_in_flight",
            "🔴 与跨副本那把锁的 `locked` 混成了一档 —— 两者的处置不同：\
             一个等本进程，一个等别的副本：{b}",
        );

        // 放开之后要能接着点 —— 这道闸**不会自愈**，卡住就是永久的。
        drop(held);
        let (st2, b2) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_ne!(st2, StatusCode::CONFLICT, "🔴 放开了还挡着：{b2}");
    }

    /// 🔴 **租约放开挂在 `Drop` 上，请求被取消那一支也覆盖得到。**
    ///
    /// 手写配对只覆盖「跑完了」「跑错了」两条支；客户端一断开，
    /// axum 直接 drop 这个 future，那一行放开根本不执行，
    /// 而这道闸不会自愈 —— 那颗按钮从此再也点不动，面板上看不出原因。
    #[test]
    fn the_in_flight_lease_releases_even_when_the_request_is_dropped_mid_flight() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _l = crate::server::state::TendInFlight::enter(&flag).expect("占得住");
            assert!(flag.load(Ordering::Acquire));
            assert!(
                crate::server::state::TendInFlight::enter(&flag).is_none(),
                "占着的时候第二把必须占不住",
            );
            // 这里直接离开作用域，模拟「future 被 drop」——没有任何显式放开。
        }
        assert!(
            !flag.load(Ordering::Acquire),
            "🔴 没放开 —— 那颗按钮从此再也点不动，而且这道闸不会自愈",
        );
        assert!(crate::server::state::TendInFlight::enter(&flag).is_some());
    }

    /// `outcome.kind` 要分得出「跑了一轮」与「压根没跑」。
    #[tokio::test]
    async fn the_outcome_reports_its_kind_and_carries_the_whole_result() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let (_, b) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(b["outcome"]["kind"], "done", "{b}");
        assert_eq!(b["started"], true, "`started` 留给旧面板");
        for k in [
            "skipped",
            "available",
            "attempted",
            "minted",
            "mintedByChannel",
            "failures",
            "at",
            "primaryChannel",
            "durationMs",
        ] {
            assert!(
                b["outcome"]["result"].get(k).is_some(),
                "🔴 少了 result.{k} —— 补池历史那一行与这里是同一个形状：{b}",
            );
        }
    }

    // ── §9.17 通道测试 ──────────────────────────────────────────────────
    //
    // 这条端点存在的全部理由是回答**一个**问题：「这条通道现在能用吗」。
    // 它的每一档拒绝、每一格失败分类，都是为了让那个答案不会变成一句假话。

    /// 🔴 **失败分档按这一次的状态码，不是按通道名。**
    ///
    /// 按通道写死的静态表说的是「别人家服务今天怎么反应」，
    /// 上游改一次版它就变成一句门禁看不见的假话。
    /// ⇒ 同一个 401 在两条通道上必须拿到**逐字相同**的结论（通道名除外）。
    #[tokio::test]
    async fn the_same_401_yields_the_same_verdict_on_both_channels() {
        let mut verdicts = Vec::new();
        for ch in ["yyds", "moemail"] {
            let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
            let (st_app, _f) = wired_with_upstream(
                Arc::clone(&mem),
                vec![reply(401, serde_json::json!({ "error": "nope" }))],
            )
            .await;
            panel_saves(
                mem.as_ref(),
                serde_json::json!({
                    "enabled": "true",
                    "channel": "yyds",
                    "moemail": { "baseUrl": "https://m.invalid", "apiKey": "mk" },
                }),
            )
            .await;
            let g = crate::admin::keys::ProbeGuard::new();
            let (st, b) = probe(&st_app, ch, &g).await;
            assert_eq!(
                st,
                StatusCode::OK,
                "失败是 200 + ok:false，不是 HTTP 错误：{b}"
            );
            verdicts.push((b["ok"].clone(), b["reason"].clone(), b["status"].clone()));
        }
        assert_eq!(
            verdicts[0], verdicts[1],
            "🔴 同一个 401 在两条通道上给了不同结论 —— 那是一张按通道写死的表",
        );
        assert_eq!(verdicts[0].1, "credentials_rejected");
        assert_eq!(verdicts[0].2, 401);
    }

    /// 429 与其余状态各归各档 —— **三档的处置完全相反**。
    #[tokio::test]
    async fn a_429_is_rate_limited_and_a_500_is_an_upstream_error() {
        for (code, want) in [(429u16, "rate_limited"), (500, "upstream_error")] {
            let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
            let (st_app, _f) =
                wired_with_upstream(Arc::clone(&mem), vec![reply(code, serde_json::json!({}))])
                    .await;
            panel_saves(
                mem.as_ref(),
                serde_json::json!({ "enabled": "true", "channel": "yyds" }),
            )
            .await;
            let g = crate::admin::keys::ProbeGuard::new();
            let (_, b) = probe(&st_app, "yyds", &g).await;
            assert_eq!(b["reason"], want, "{code} 该落 {want}：{b}");
            assert_eq!(b["status"], code);
        }
    }

    /// 🔴 **一个域名都读不到 ⇒ 不去验凭据，而且如实说没验。**
    ///
    /// 硬编一个域名去验就是伪造；而列域名那一步本身是成功的，所以不是失败档。
    /// 面板靠 `domains: 0` 与 `credentials: "not_checked"` 说清这件事。
    #[tokio::test]
    async fn an_empty_domain_list_reports_not_checked_instead_of_faking_a_domain() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(
            Arc::clone(&mem),
            vec![reply(200, serde_json::json!({ "domains": [] }))],
        )
        .await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let g = crate::admin::keys::ProbeGuard::new();
        let (_, b) = probe(&st_app, "yyds", &g).await;
        assert_eq!(b["ok"], true, "列域名那一步是成功的：{b}");
        assert_eq!(b["domains"], 0);
        assert_eq!(
            b["credentials"], "not_checked",
            "🔴 没域名却报「凭据已验」—— 那只可能是硬编了一个域名：{b}",
        );
        assert_eq!(
            f.seen().len(),
            1,
            "🔴 没域名还往下打了 —— 建邮箱那一步要一个域名，它哪来的：{:?}",
            f.seen(),
        );
    }

    /// 🔴 **传输层失败不许挂着上一次那个状态码。**
    ///
    /// 「列域名 200 → 建邮箱连不上」时报 `status: 200`，
    /// 说的是「上游答了 200 却失败了」—— 一句会把运维支去查上游返回体的假话，
    /// 而真相是这一次**根本没连上**。
    #[tokio::test]
    async fn a_transport_failure_reports_no_status_instead_of_the_previous_one() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        // 只给列域名那一条脚本；建邮箱那一步脚本用完 ⇒ 传输层失败。
        let (st_app, _f) = wired_with_upstream(
            Arc::clone(&mem),
            vec![reply(200, serde_json::json!({ "domains": ["a.test"] }))],
        )
        .await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let g = crate::admin::keys::ProbeGuard::new();
        let (_, b) = probe(&st_app, "yyds", &g).await;
        assert_eq!(b["ok"], false, "建邮箱那一步确实失败了：{b}");
        assert!(
            b["status"].is_null(),
            "🔴 挂着列域名那一次的 200 —— 「上游答了 200 却失败了」是假话：{b}",
        );
        assert_eq!(b["reason"], "upstream_error", "拿不到状态码就落这一档");
    }

    /// 成功那一支：域名数、耗时、凭据结论、残留标记都要给。
    #[tokio::test]
    async fn a_successful_probe_reports_the_domain_count_and_that_it_cleaned_up() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(
            Arc::clone(&mem),
            vec![
                reply(200, serde_json::json!({ "domains": ["a.test", "b.test"] })),
                reply(
                    200,
                    serde_json::json!({ "id": "mb1", "address": "x@a.test" }),
                ),
                reply(200, serde_json::json!({ "ok": true })),
            ],
        )
        .await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let g = crate::admin::keys::ProbeGuard::new();
        let (_, b) = probe(&st_app, "yyds", &g).await;
        assert_eq!(b["ok"], true, "{b}");
        assert_eq!(b["domains"], 2);
        assert_eq!(b["credentials"], "accepted", "🔴 只列了域名就报绿：{b}");
        assert_eq!(
            b["cleaned"], true,
            "⚠️ 含义是「确认删掉了」—— 没删掉就占着一条活跃邮箱名额：{b}",
        );
        assert!(b["latencyMs"].is_i64(), "面板要显示这一次花了多久：{b}");
        assert_eq!(f.seen().len(), 3, "列域名 + 建 + 删：{:?}", f.seen());
    }

    /// 🔴 **成功那一支一次存储写都不产生。**
    #[tokio::test]
    async fn a_successful_probe_writes_nothing_to_storage() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let (st_app, _f) = wired_with_upstream(
            counting.clone(),
            vec![
                reply(200, serde_json::json!({ "domains": ["a.test"] })),
                reply(
                    200,
                    serde_json::json!({ "id": "mb1", "address": "x@a.test" }),
                ),
                reply(200, serde_json::json!({ "ok": true })),
            ],
        )
        .await;
        panel_saves(
            counting.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let before = counting.counts.snapshot().1;
        let g = crate::admin::keys::ProbeGuard::new();
        let (_, b) = probe(&st_app, "yyds", &g).await;
        assert_eq!(b["ok"], true, "{b}");
        assert_eq!(
            counting.counts.snapshot().1,
            before,
            "🔴 一次连通性测试写了存储 —— 那是一笔没人记在账上的写",
        );
    }

    /// 🔴 **这条通道没配凭据 ⇒ 409，而且一次上游请求都不发。**
    ///
    /// 回成 `{ok:false, reason:"upstream_error"}` 是把本网关自己的配置缺失
    /// 说成上游故障 —— 运维会去查地址、DNS、TLS，而该填的是那一格凭据。
    #[tokio::test]
    async fn an_unconfigured_channel_is_a_409_and_touches_no_upstream() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(Arc::clone(&mem), vec![]).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        // env 只给了 YYDS 的 key，moemail 那条没配。
        let g = crate::admin::keys::ProbeGuard::new();
        let (st, b) = probe(&st_app, "moemail", &g).await;
        assert_eq!(st, StatusCode::CONFLICT, "{b}");
        assert_eq!(b["reason"], "channel_not_configured");
        assert_eq!(
            b["channel"], "moemail",
            "面板两张卡各渲染各的，得知道是哪条"
        );
        assert!(
            f.seen().is_empty(),
            "🔴 没配凭据还去打了上游：{:?}",
            f.seen()
        );
    }

    /// 认不出来的通道名 ⇒ 400，**排在「注册机开没开」之前**。
    #[tokio::test]
    async fn an_unknown_channel_name_is_rejected_before_anything_else() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, f) = wired_with_upstream(Arc::clone(&mem), vec![]).await;
        // 注册机是关着的 —— 而通道名这一档要排在它前面。
        let g = crate::admin::keys::ProbeGuard::new();
        let (st, b) = probe(&st_app, "yydss", &g).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{b}");
        assert_eq!(b["reason"], "unknown_channel");
        assert!(f.seen().is_empty());
    }

    /// 🔴 **连点被挡，而且两条通道各占一格。**
    ///
    /// 测 yyds 不该把 moemail 那颗按钮也锁上 —— 两条通道完全平级。
    #[tokio::test]
    async fn the_probe_guard_is_per_channel_not_global() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (st_app, _f) = wired_with_upstream(
            Arc::clone(&mem),
            vec![
                reply(200, serde_json::json!({ "domains": ["a.test"] })),
                reply(200, serde_json::json!({ "id": "m", "address": "x@a.test" })),
                reply(200, serde_json::json!({})),
                reply(200, serde_json::json!({ "domains": ["a.test"] })),
                reply(200, serde_json::json!({ "id": "m", "address": "x@a.test" })),
                reply(200, serde_json::json!({})),
            ],
        )
        .await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({
                "enabled": "true",
                "channel": "yyds",
                "moemail": { "baseUrl": "https://m.invalid", "apiKey": "mk" },
            }),
        )
        .await;

        let g = crate::admin::keys::ProbeGuard::new();
        let (s1, _) = probe(&st_app, "yyds", &g).await;
        assert_eq!(s1, StatusCode::OK);
        let (s2, b2) = probe(&st_app, "yyds", &g).await;
        assert_eq!(s2, StatusCode::TOO_MANY_REQUESTS, "同一条连点该被挡：{b2}");
        assert_eq!(b2["reason"], "probe_cooldown");

        let (s3, b3) = probe(&st_app, "moemail", &g).await;
        assert_ne!(
            s3,
            StatusCode::TOO_MANY_REQUESTS,
            "🔴 测了 yyds 把 moemail 那颗按钮也锁上了 —— 两条通道不是平级的了：{b3}",
        );
    }

    /// 🔴 **「关着」与「配坏了」必须是两个 code —— 运维要做的事完全相反。**
    ///
    /// 合并成一个之后，面板对这两种情况只能说同一句话：
    /// 一种要去面板打开开关，另一种要去补凭据 / 改参数。
    #[tokio::test]
    async fn disabled_and_blocked_are_two_distinct_reason_codes() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;

        // ① 关着的。
        let (st, b) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(b["reason"], "registrar_disabled", "{b}");

        // ② 打开了，但把通道切到一条没配凭据的 ⇒ 装载器产 blocker。
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "moemail" }),
        )
        .await;
        let (st2, b2) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st2, StatusCode::CONFLICT, "{b2}");
        assert_eq!(
            b2["reason"], "registrar_blocked",
            "🔴 与「关着」用了同一个 code —— 面板对两种相反的处置只能说同一句话：{b2}",
        );
    }

    /// 🔴 **连点必须被挡住 —— 挡的是上游代价，不是手速。**
    ///
    /// 一次点击最多烧掉 `MINT_BATCH` 个临时邮箱名额，而那是补池能不能继续工作的前提。
    #[tokio::test]
    async fn a_second_click_inside_the_cooldown_is_refused_with_a_live_countdown() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        let (st1, b1) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st1, StatusCode::OK, "头一次点该放行：{b1}");
        assert_eq!(
            b1["remaining"],
            (crate::registrar::guard::MANUAL_TENDS_PER_DAY - 1) as i64,
            "🔴 成功那一支也要给 remaining —— 只在耗尽时才说，等于让运维一路撞上墙",
        );

        let (st2, b2) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(
            st2,
            StatusCode::TOO_MANY_REQUESTS,
            "🔴 连点没被挡住 —— 每点一次都在真实地烧活跃邮箱名额：{b2}",
        );
        assert_eq!(b2["reason"], "manual_cooldown");
        assert!(
            b2["retryAfterMs"].as_i64().unwrap_or(0) > 0,
            "倒计时要能渲染：{b2}",
        );
        assert!(
            b2["cooldownUntil"].is_i64(),
            "冷却那一支两个字段成对给：{b2}"
        );
    }

    /// 🔴 **名额在真跑之前就落盘。**
    ///
    /// 反过来的话，一轮跑崩（或进程被杀）等于白送一格名额与一整个冷却窗口
    /// —— 而连点恰恰最容易在出问题的时候发生。
    #[tokio::test]
    async fn the_slot_is_committed_before_the_round_runs_not_after() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds" }),
        )
        .await;

        // 这一轮必定跑不成（上游是 UnreachableFetcher）。
        let (st, _) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(st, StatusCode::OK);

        let g = crate::registrar::guard::narrow_manual_guard(
            mem.get(crate::registrar::guard::MANUAL_GUARD_KEY)
                .await
                .unwrap()
                .as_ref(),
        );
        assert_eq!(
            g.map(|x| x.used),
            Some(1),
            "🔴 一轮没跑成就不记名额 —— 那把「反复点到成功为止」变成了免费的",
        );
    }

    /// 🔴 **状态里要能读出配额，而不是等撞墙才知道有墙。**
    #[tokio::test]
    async fn the_status_endpoint_reports_the_manual_tend_quota() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;

        let (_, before) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert_eq!(
            before["manual"]["remaining"],
            crate::registrar::guard::MANUAL_TENDS_PER_DAY as i64,
        );
        assert_eq!(
            before["manual"]["perDay"],
            crate::registrar::guard::MANUAL_TENDS_PER_DAY as i64,
            "面板要说「还剩 19 次（共 24 次）」，只给一个数说不出那句话",
        );
        assert!(
            before["manual"]["cooldownUntil"].is_null(),
            "不在冷却中时是 null，不是 0 —— 0 会渲染成一个恒为 0 的倒计时",
        );

        // 读配额本身不许消费名额。
        let (_, again) = get(&app, Method::GET, "/admin/api/registrar/status").await;
        assert_eq!(again["manual"]["used"], 0, "🔴 读一眼状态就掉了一格名额");
    }

    /// 🔴 **手动一轮的参数要被压顶，而且压了要说。**
    ///
    /// `MANUAL_MAX_DOMAIN_ATTEMPTS = 1` 不是「保守一点」：它让最坏尝试退化成常量。
    /// 不压的话运维把 `MAX_DOMAIN_ATTEMPTS` 调成 2，最坏尝试就超过预算 ⇒
    /// **按钮变成永远铸不出 key 的诚实空转，而且空转得毫无征兆。**
    #[tokio::test]
    async fn the_manual_round_caps_its_own_parameters_and_says_so() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (app, _t) = wired(Arc::clone(&mem)).await;
        panel_saves(
            mem.as_ref(),
            serde_json::json!({ "enabled": "true", "channel": "yyds", "maxDomainAttempts": "3", "mintBatch": "5" }),
        )
        .await;

        let (_, b) = get(&app, Method::POST, "/admin/api/registrar/tend").await;
        assert_eq!(
            b["outcome"]["capped"]["before"]["maxDomainAttempts"], 3,
            "压前那组数要给 —— 只报压后等于让运维看不出自己配的数被动过：{b}",
        );
        assert_eq!(b["outcome"]["capped"]["after"]["maxDomainAttempts"], 1);
        assert_eq!(b["outcome"]["capped"]["after"]["mintBatch"], 1);

        // 🔴 **这一格才是真正钉住「压顶交到了这一轮手上」的那一格。**
        // 上面几格只看报告 —— 报告说压了、而 `run_round` 拿的是原配置时它们全绿。
        //
        // 观测点正是那条压顶要防的失败本身：`MAX_DOMAIN_ATTEMPTS = 3` 之下
        // 最坏尝试 150 秒 > 手动预算 70 秒 ⇒ 预算判据判「这一次尝试开不起来」
        // ⇒ **`attempted` 恒为 0，按钮诚实空转，而且毫无征兆。**
        assert_eq!(
            b["outcome"]["result"]["attempted"], 1,
            "🔴 这一轮一次都没试 —— 按钮成了永远铸不出 key 的空转：{b}",
        );
    }

    /// 🔴 **压顶要落在「交给这一轮的那份配置」上，不是只落在报告里。**
    ///
    /// 回归对象：`capped` 里数字都对、而 `run_round` 拿到的还是原配置 ——
    /// 报告说「已压到 1 个域名」，实际照旧按运维配的 3 个跑，
    /// **而那正是这条压顶要防的空转。** 只看报告的判据对这件事完全瞎。
    #[test]
    fn the_capped_values_land_on_the_config_the_round_actually_gets() {
        let e: crate::core::config_provenance::EnvMap = [
            ("YYDS_API_KEY".to_string(), "yk".to_string()),
            ("REGISTRAR_ENABLED".to_string(), "true".to_string()),
            ("REGISTRAR_MAX_DOMAIN_ATTEMPTS".to_string(), "3".to_string()),
            ("REGISTRAR_MINT_BATCH".to_string(), "5".to_string()),
        ]
        .into_iter()
        .collect();
        let cfg = crate::registrar::runtime::load(&e, &serde_json::Value::Null).config;
        assert_eq!((cfg.max_domain_attempts, cfg.mint_batch), (3, 5), "前提");

        let (capped, report) = crate::registrar::runtime::manual_caps(&cfg);
        assert!(report.is_some(), "确实压了，才谈得上压到哪去了");
        assert_eq!(
            capped.max_domain_attempts,
            crate::registrar::types::MANUAL_MAX_DOMAIN_ATTEMPTS,
            "🔴 报告说压了，而交给这一轮的配置一个字没变",
        );
        assert_eq!(
            capped.mint_batch,
            crate::registrar::types::MANUAL_MINT_BATCH,
            "🔴 同上 —— 一次点击照旧烧 5 个活跃邮箱名额",
        );
        assert_eq!(
            capped.code_timeout_ms,
            crate::registrar::types::MANUAL_CODE_TIMEOUT_MS,
        );
    }

    /// ⚠️ **压顶是 `min()`，不是赋值。**
    ///
    /// 运维把等码超时调到 30 秒，是因为他知道自己的上游快；
    /// 赋值会把它**抬**回 60 秒 —— 那是拿运维的判断去换一个默认值。
    #[test]
    fn capping_never_raises_a_value_the_operator_deliberately_lowered() {
        let e: crate::core::config_provenance::EnvMap = [
            ("YYDS_API_KEY".to_string(), "yk".to_string()),
            ("REGISTRAR_CODE_TIMEOUT_MS".to_string(), "30000".to_string()),
            ("REGISTRAR_ENABLED".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect();
        let cfg = crate::registrar::runtime::load(&e, &serde_json::Value::Null).config;
        assert_eq!(cfg.code_timeout_ms, 30_000, "前提：运维自己调小了");

        let (capped, _) = crate::registrar::runtime::manual_caps(&cfg);
        assert_eq!(
            capped.code_timeout_ms, 30_000,
            "🔴 被抬回了默认值 —— 那是拿运维的判断去换一个默认值",
        );
    }
}
