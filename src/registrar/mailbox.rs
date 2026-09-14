//! 两条临时邮箱通道的真实实现（YYDS 与 MoeMail）。
//!
//! 移植自 TS 版 `src/adapters/mailbox-{yyds,moemail}.ts`（300 + 260 行）
//! 与端口契约 `src/ports/mailbox.ts`。
//!
//! # 两家的差异全部封在这里，[`super::mint`] 对它们一视同仁
//!
//! 差异有三处，每一处都是**真机实测**出来的，不是读文档读来的：
//!
//! | | YYDS | MoeMail |
//! |---|---|---|
//! | 列域名 | `GET /v1/domains` | `GET /api/config`（域名在站点配置里） |
//! | 建邮箱 | `POST /v1/accounts` | `POST /api/emails/generate` |
//! | 收信定位 | **按 address** | 按 id |
//! | 删除定位 | **按 id** | 按 id |
//!
//! 🔴 **YYDS 那一行「收信按 address、删除按 id」是真机实测的契约**：
//! ```text
//! GET    /v1/messages?address={address} → 200，换成 id → 404 inbox_not_found
//! DELETE /v1/accounts/{id}              → 204，换成 address → 404 account_not_found
//! ```
//! **此前存的是 address，删邮箱因此每次都 404，「用完即删」从未真正生效。**

use super::code::{extract_code, normalize_body};
use super::fetch::{ChannelReply, fetch_channel};
use super::mint::{Mailbox, MailboxProvider};
use super::types::REGISTRAR_REQUEST_TIMEOUT_MS;
use crate::ports::fetcher::{Fetcher, OutboundRequest};
use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashSet;

// ── 活跃邮箱上限：全仓只此一处复述数字 ───────────────────────────────────
//
// · **YYDS：免费档同时 15 个。**
//   出处是注册机设计阶段的一份内部文档（**未随公开仓发布**）里的通道对照表，
//   那一行逐字写着「取决于账号档位（免费档同时 15 个邮箱）」。
//   复核日期 **2026-08-21（北京时间）**，本次复核只到「设计文档这么记着」为止，
//   **没有**去官方文档二次核对。
//
// ⚠️ **它不是常数，是一个与账号档位绑定的当前取值。** 换个档位就不是 15。
// 因此：
// · 源码注释里要它，是因为「用完即删」这条功能性前提**需要一个量级才说得通**；
// · 🔴 **面板文案里一律不许出现这个数字** —— 运维会把面板上的数字当成
//   自己这套部署的事实，而这个数字对他很可能是错的。
//
// **刻意不做成一个导出的常量**：没有任何代码需要在运行期读它
//（它不是本网关的配置，是别人家服务的默认档位），
// 而**一个零消费者的导出迟早会漂**。它就该是一段注释。

/// 轮询验证码时两次之间歇多久。
const POLL_INTERVAL_MS: i64 = 3_000;

/// 一条日志。与 [`super::tender::TendEvent`] 同形，这里只用得着 warn。
pub struct MailEvent {
    pub level: &'static str,
    pub event: &'static str,
    pub msg: String,
    pub fields: Vec<(&'static str, String)>,
}

/// 两条通道共用的那几样依赖。
pub struct MailDeps<'a> {
    pub fetcher: &'a dyn Fetcher,
    pub base_url: String,
    pub api_key: String,
    pub now: &'a (dyn Fn() -> i64 + Sync),
    pub sleep: &'a (dyn Fn(i64) -> futures_core::future::BoxFuture<'static, ()> + Sync),
    pub emit: &'a (dyn Fn(MailEvent) + Sync),
}

impl MailDeps<'_> {
    fn req(&self, method: &str, url: String, body: Option<Bytes>) -> OutboundRequest {
        OutboundRequest {
            method: method.into(),
            url,
            headers: vec![
                ("authorization".into(), format!("Bearer {}", self.api_key)),
                ("accept".into(), "application/json".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body,
            timeout_ms: REGISTRAR_REQUEST_TIMEOUT_MS,
        }
    }
    fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 非 2xx ⇒ 一条带脱敏地址的错误。
fn require_ok(
    r: &ChannelReply,
    channel: &str,
    action: &str,
    method: &str,
    url: &str,
) -> Result<(), String> {
    if (200..300).contains(&r.status) {
        return Ok(());
    }
    Err(super::url::http_fail_message(
        channel, action, r.status, method, url,
    ))
}

/// 从一份正文里挖出一个非空字符串（先看顶层，再看 `data`）。
fn pick<'a>(v: &'a Value, name: &str) -> Option<&'a str> {
    for scope in [Some(v), v.get("data")].into_iter().flatten() {
        if let Some(s) = scope.get(name).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some(s);
        }
    }
    None
}

/// 从一份正文里挖出一个字符串数组（顶层或 `data` 下）。
fn pick_list(v: &Value, names: &[&str]) -> Vec<String> {
    for scope in [Some(v), v.get("data")].into_iter().flatten() {
        for n in names {
            if let Some(a) = scope.get(*n).and_then(Value::as_array) {
                return a
                    .iter()
                    .filter_map(|x| {
                        x.as_str()
                            .map(str::to_string)
                            // 有的站点把域名装在 `{ "domain": "x" }` 里。
                            .or_else(|| x.get("domain").and_then(Value::as_str).map(str::to_string))
                    })
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

// ── YYDS ──────────────────────────────────────────────────────────────────

pub struct Yyds<'a> {
    pub deps: MailDeps<'a>,
}

#[async_trait]
impl MailboxProvider for Yyds<'_> {
    async fn create_mailbox(&self, domain: &str) -> Result<Mailbox, String> {
        let d = &self.deps;
        let url = format!("{}/v1/accounts", d.base());
        let body = serde_json::json!({ "domain": domain });
        let r = fetch_channel(
            d.fetcher,
            d.req("POST", url.clone(), Some(Bytes::from(body.to_string()))),
            "yyds",
            "建邮箱",
        )
        .await?;
        require_ok(&r, "yyds", "建邮箱", "POST", &url)?;

        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let address = pick(&v, "address").map(str::to_string);
        let id = pick(&v, "id").map(str::to_string);

        // 🔴 **id 与 address 都要校验**：两个接口用的键不一样（见本文件头那张表），
        // 少任何一个都会让后续调用 **100% 打空**。
        match (address, id) {
            (Some(address), Some(id)) => Ok(Mailbox { id, address }),
            _ => {
                // 🔴 **不做兜底删除。**
                //
                // 此前这里用 `本地部分@域名` 兜底调了一次删除，但**删除需要 id，
                // 而 id 恰恰就是这条路径丢掉的那个东西** —— 真机实测用 address 删一定 404，
                // 而且没有按 address 反查 id 的列表端点。
                // 发一次注定失败的删除只会往「删邮箱失败」那条 warn 里**灌稳定的假 404**，
                // 稀释「邮箱正在堆积、配额即将耗尽」这个**真信号** ——
                // 运维看多了就会开始忽略这类日志。
                //
                // ⇒ 改成**诚实告警**：说清这个邮箱可能已经建出来了、我们删不掉、
                // 以及它什么时候自己消失（真机实测约 24 小时后随过期时间自动清掉）。
                (d.emit)(MailEvent {
                    level: "warn",
                    event: "registrar.mailbox_create_unparseable",
                    msg: "YYDS 建邮箱的响应解析不开、或者缺 address / id：\
                          邮箱**可能已经在上游建出来了**但我们拿不到 id，无法主动删除，\
                          约 24 小时后随过期时间自动消失；这期间它占着一个活跃邮箱名额。"
                        .into(),
                    fields: vec![
                        ("provider", "yyds".into()),
                        ("domain", domain.to_string()),
                        ("expiresAfterHours", "24".into()),
                        ("url", super::url::redact_url(&url)),
                    ],
                });
                Err(format!(
                    "yyds 建邮箱失败: 响应缺 address / id (POST {})",
                    super::url::redact_url(&url),
                ))
            }
        }
    }

    /// ⚠️ **收信按 address 定位** —— 换成 id 会 404。
    async fn poll_code(
        &self,
        mailbox: &Mailbox,
        timeout_ms: i64,
    ) -> Result<Option<String>, String> {
        let d = &self.deps;
        let list = format!(
            "{}/v1/messages?address={}",
            d.base(),
            urlencode(&mailbox.address)
        );
        poll_loop(d, timeout_ms, &list, |id| {
            format!(
                "{}/v1/messages/{}?address={}",
                d.base(),
                urlencode(id),
                urlencode(&mailbox.address)
            )
        })
        .await
    }

    /// ⚠️ **删除按 id 定位** —— 换成 address 会 404。
    async fn delete_mailbox(&self, mailbox: &Mailbox) -> Result<(), String> {
        let d = &self.deps;
        let url = format!("{}/v1/accounts/{}", d.base(), urlencode(&mailbox.id));
        let r = fetch_channel(
            d.fetcher,
            d.req("DELETE", url.clone(), None),
            "yyds",
            "删邮箱",
        )
        .await?;
        // ⚠️ **返回值的含义是「确认删掉了没有」，不是「抛没抛错」**（端口契约逐字写着）：
        // 非 2xx 会正常返回，只看抛不抛错的调用方一律会读成「删掉了」。
        require_ok(&r, "yyds", "删邮箱", "DELETE", &url)
    }
}

// ── MoeMail ───────────────────────────────────────────────────────────────

pub struct Moemail<'a> {
    pub deps: MailDeps<'a>,
}

#[async_trait]
impl MailboxProvider for Moemail<'_> {
    async fn create_mailbox(&self, domain: &str) -> Result<Mailbox, String> {
        let d = &self.deps;
        let url = format!("{}/api/emails/generate", d.base());
        let body = serde_json::json!({ "domain": domain });
        let r = fetch_channel(
            d.fetcher,
            d.req("POST", url.clone(), Some(Bytes::from(body.to_string()))),
            "moemail",
            "建邮箱",
        )
        .await?;
        require_ok(&r, "moemail", "建邮箱", "POST", &url)?;

        let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let address = pick(&v, "email")
            .or_else(|| pick(&v, "address"))
            .map(str::to_string);
        let id = pick(&v, "id").map(str::to_string);
        match (address, id) {
            (Some(address), Some(id)) => Ok(Mailbox { id, address }),
            _ => Err(format!(
                "moemail 建邮箱失败: 响应缺 email / id (POST {})",
                super::url::redact_url(&url),
            )),
        }
    }

    /// MoeMail 两处都按 id 定位。
    async fn poll_code(
        &self,
        mailbox: &Mailbox,
        timeout_ms: i64,
    ) -> Result<Option<String>, String> {
        let d = &self.deps;
        let list = format!("{}/api/emails/{}", d.base(), urlencode(&mailbox.id));
        poll_loop(d, timeout_ms, &list, |_| list.clone()).await
    }

    async fn delete_mailbox(&self, mailbox: &Mailbox) -> Result<(), String> {
        let d = &self.deps;
        let url = format!("{}/api/emails/{}", d.base(), urlencode(&mailbox.id));
        let r = fetch_channel(
            d.fetcher,
            d.req("DELETE", url.clone(), None),
            "moemail",
            "删邮箱",
        )
        .await?;
        require_ok(&r, "moemail", "删邮箱", "DELETE", &url)
    }
}

// ── 列域名（两家各一个端点，形状不同）────────────────────────────────────

/// YYDS：域名有自己的端点。
pub async fn yyds_list_domains(d: &MailDeps<'_>) -> Result<Vec<String>, String> {
    let url = format!("{}/v1/domains", d.base());
    let r = fetch_channel(
        d.fetcher,
        d.req("GET", url.clone(), None),
        "yyds",
        "取域名列表",
    )
    .await?;
    require_ok(&r, "yyds", "取域名列表", "GET", &url)?;
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    Ok(pick_list(&v, &["domains", "items", "list"]))
}

/// MoeMail：域名装在**站点配置**里，没有独立端点。
pub async fn moemail_list_domains(d: &MailDeps<'_>) -> Result<Vec<String>, String> {
    let url = format!("{}/api/config", d.base());
    let r = fetch_channel(
        d.fetcher,
        d.req("GET", url.clone(), None),
        "moemail",
        "取域名列表",
    )
    .await?;
    require_ok(&r, "moemail", "取域名列表", "GET", &url)?;
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    Ok(pick_list(&v, &["emailDomains", "domains"]))
}

// ── 轮询 ──────────────────────────────────────────────────────────────────

/// 两家共用的轮询循环。
///
/// # ⚠️ 这里被吞掉的请求失败**刻意不留任何日志**
///
/// 单次铸 key 光轮询就要打**约 40 次**请求，**瞬时错误是常态**。
/// 在这里留痕会让那 100 格的事件环**几秒清空一次**，
/// **把运维真正需要的诊断挤出去。**
///
/// # 🔴 抖动必须在这里被吞掉，不许穿出去
///
/// 请求本身失败（DNS / TCP reset / TLS / 单请求超时）与「HTTP 非 2xx」
///「正文不是 JSON」是**同一类瞬时故障，处置必须一致**：本轮跳过、继续轮询，
/// **只有整体超时耗尽才收工**。
/// 此前这里没有兜住，**一次网络抖动就会穿出去、被上层收成 `network_error` 作废整次铸 key** ——
/// 而那一刻验证码往往**已经在邮箱里**、窗口还剩一百多秒。
///
/// **代价明写、登记为已知缺口**：「地址对，但收信接口的路径不对」这一类故障
/// 仍然只表现为 `code_timeout`，而那条文案会把人指向 MX 记录 / 转发规则，**方向是反的**。
/// 可接受的依据是：地址填成带接口前缀的形态时，**列域名第一步就先失败了**，压根走不到这里。
async fn poll_loop(
    d: &MailDeps<'_>,
    timeout_ms: i64,
    list_url: &str,
    detail_url: impl Fn(&str) -> String,
) -> Result<Option<String>, String> {
    let start = (d.now)();
    let mut seen: HashSet<String> = HashSet::new();
    while (d.now)() - start < timeout_ms {
        // 整段都不许把错误抛出去 —— 见上面那段。
        if let Ok(r) =
            fetch_channel(d.fetcher, d.req("GET", list_url.to_string(), None), "", "").await
            && (200..300).contains(&r.status)
            && let Ok(v) = serde_json::from_str::<Value>(&r.body)
        {
            for m in messages_of(&v) {
                let Some(id) = m.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !seen.insert(id.to_string()) {
                    continue;
                }
                // 列表里就带正文的站点（MoeMail）直接用；不带的（YYDS）再取一次详情。
                let subject = m.get("subject").and_then(Value::as_str).unwrap_or_default();
                let inline = normalize_body(m.get("html").or_else(|| m.get("text")));
                if !inline.is_empty()
                    && let Some(c) = extract_code(subject, &inline)
                {
                    return Ok(Some(c));
                }
                if let Ok(dr) =
                    fetch_channel(d.fetcher, d.req("GET", detail_url(id), None), "", "").await
                    && (200..300).contains(&dr.status)
                    && let Ok(dv) = serde_json::from_str::<Value>(&dr.body)
                {
                    let sub = pick(&dv, "subject").unwrap_or_default().to_string();
                    let body = normalize_body(
                        dv.get("html")
                            .or_else(|| dv.get("text"))
                            .or_else(|| dv.get("data").and_then(|x| x.get("html")))
                            .or_else(|| dv.get("data").and_then(|x| x.get("text"))),
                    );
                    if let Some(c) = extract_code(&sub, &body) {
                        return Ok(Some(c));
                    }
                }
            }
        }
        (d.sleep)(POLL_INTERVAL_MS).await;
    }
    // **超时返回 `None` 而不是错误** —— 端口契约逐字写着。
    Ok(None)
}

/// 从各种形状里取出信件列表。
fn messages_of(v: &Value) -> Vec<Value> {
    for scope in [Some(v), v.get("data")].into_iter().flatten() {
        for n in ["messages", "items", "list", "emails"] {
            if let Some(a) = scope.get(n).and_then(Value::as_array) {
                return a.clone();
            }
        }
    }
    // 顶层直接就是数组。
    v.as_array().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests;
