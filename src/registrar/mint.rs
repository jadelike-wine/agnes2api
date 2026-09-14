//! 单次铸号：拿一批候选域名，尽力铸出**一把** key。
//!
//! 移植自 TS 版 `src/core/registrar/mint.ts`（388 行），规则真源
//! `docs/porting/01-core-rules.md §10.6`。
//!
//! # 走到后三步的失败是**最贵的**
//!
//! 走到「注册 / 登录 / 建 key」时：临时邮箱已经**真的建出来**、验证码已经**真的向 Agnes 要过一次**
//!（吃掉的是全网关共享的限流预算），**上游那边甚至可能已经把账号建好了**。
//! 而上一版那三条告警**只有邮箱地址** —— **四种真因在面板上长得一模一样，处置却完全不同。**
//! ⇒ 那三步各有自己的诊断，见 [`step_message`]。

use super::backoff::BackoffKind;
use super::domain_ledger::{
    DomainLedger, DomainState, OK_TTL_MS, Observation, SendCodeClass, classify_send_code,
    edge_marker, upstream_message,
};
use super::tender::{MintOutcome, MintRequest, Minter, TendEvent, TendFailureReason};
use async_trait::async_trait;
use serde_json::Value;

/// 一个临时邮箱。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub address: String,
}

/// 一次上游回话。
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// 邮箱通道。
#[async_trait]
pub trait MailboxProvider: Send + Sync {
    async fn create_mailbox(&self, domain: &str) -> Result<Mailbox, String>;
    /// 轮询验证码。**超时返回 `None`**，不是错误。
    async fn poll_code(&self, mailbox: &Mailbox, timeout_ms: i64)
    -> Result<Option<String>, String>;
    /// **用完即删。** 失败只记一条 warn。
    async fn delete_mailbox(&self, mailbox: &Mailbox) -> Result<(), String>;
}

/// Agnes 站点的四步。
#[async_trait]
pub trait AgnesSite: Send + Sync {
    async fn send_code(&self, address: &str) -> Result<Reply, String>;
    async fn register(&self, address: &str, code: &str, password: &str) -> Result<Reply, String>;
    async fn login(&self, address: &str, password: &str) -> Result<Reply, String>;
    async fn create_key(&self, token: &str, name: &str) -> Result<Reply, String>;
}

/// 后三步各自要从 2xx 正文里挖出来的那个字段。
const TOKEN_FIELDS: [&str; 2] = ["token", "access_token"];
const KEY_FIELDS: [&str; 2] = ["key", "api_key"];

/// 从 `{...}` 或 `{"data": {...}}` 里挖一个字段。
fn pick<'a>(v: &'a Value, names: &[&str]) -> Option<&'a str> {
    let scopes = [Some(v), v.get("data")];
    for scope in scopes.into_iter().flatten() {
        for n in names {
            if let Some(s) = scope.get(*n).and_then(Value::as_str)
                && !s.is_empty()
            {
                return Some(s);
            }
        }
    }
    None
}

/// 注册链后三步的诊断，**两档**。
///
/// - **非 2xx** ⇒ 上游拒了，正文是它的裁决词 ⇒ 走 [`upstream_message`]
///   （抹邮箱地址 + 抹**本步在场的已知凭据** + 抹 `sk-` 片段 + 截断 + 回查）。
/// - **2xx 却没解析出目标字段** ⇒ 🔴 **只说字段名，一个值都不说。**
///
/// # 🔴 第二档为什么一个值都不许说
///
/// 这一档的正文**极可能就装着那把令牌 / key 本身**（「上游把字段改名了」正是它要说的话），
/// 而运维在这一档需要的**恰恰只有字段名**：**看到 `data.token_id` 他就知道名字变了。**
///
/// ⚠️ 正文不是 JSON 对象时**落回上一档** —— 那种正文里不会有我们没认出来的令牌。
pub fn step_message(
    status: u16,
    body: &str,
    address: &str,
    secrets: &[String],
    expected: &[&str],
) -> String {
    if !(200..300).contains(&status) {
        return upstream_message(body, address, secrets);
    }
    match serde_json::from_str::<Value>(body) {
        Ok(v) if v.is_object() => {
            let mut names: Vec<String> = Vec::new();
            collect_field_names(&v, &mut names);
            format!(
                "上游回了 2xx，但正文里没有我们认识的字段（期望 {}）。正文顶层与 data 下一层的字段名：{}。\
                 **这里一个值都不显示** —— 这一档的正文极可能就装着那把令牌本身，\
                 而你需要的只有字段名：看到一个眼生的名字就说明上游改名了。",
                expected.join(" / "),
                if names.is_empty() {
                    "（一个都没有）".to_string()
                } else {
                    names.join(", ")
                },
            )
        }
        // 正文不是 JSON 对象 ⇒ 落回上一档。那种正文里不会有我们没认出来的令牌。
        _ => upstream_message(body, address, secrets),
    }
}

/// 顶层 + `data` 下一层的字段名，**最多 20 个**。
///
/// 🔴 **只走两层、只取 20 个**：再深就等于把整份响应结构画出来，
/// 而这条诊断只需要认出「名字变了」。
fn collect_field_names(v: &Value, out: &mut Vec<String>) {
    const MAX: usize = 20;
    if let Some(o) = v.as_object() {
        for k in o.keys() {
            if out.len() >= MAX {
                return;
            }
            out.push(k.clone());
        }
        if let Some(d) = o.get("data").and_then(Value::as_object) {
            for k in d.keys() {
                if out.len() >= MAX {
                    return;
                }
                out.push(format!("data.{k}"));
            }
        }
    }
}

/// 口令来自 [`super::agnes::random_password`]。
///
/// ⚠️⚠️ **移植时我在这里写错过一次，留个路标。** 我自己造了一份 24 位、
/// 字母表里带着 `o`、**末尾没有 `Q!` 后缀**的实现，而正典那一份是
/// 「14 位 + 固定后缀 `Q!`」—— 那个后缀满足上游的**复杂度要求**（至少一个大写 + 一个特殊字符）。
/// 少了它的后果**不是「口令弱一点」，而是注册那一步被上游拒掉**，
/// 而故障会显示成 `register_failed` —— **没人看得出真因是口令不合格。**
/// ⇒ 口令的唯一真源在 `agnes`，这里只 `use`。
pub use super::agnes::random_password;

/// 这个域名在台账里是不是「已知能用」。
fn is_known_good(ledger: &DomainLedger, domain: &str, now: i64) -> bool {
    ledger
        .entries
        .get(domain)
        .is_some_and(|e| e.s == DomainState::Ok && now - e.at < OK_TTL_MS)
}

/// 一次铸号的全部外部依赖。
pub struct MintDeps<'a> {
    pub provider: &'a dyn MailboxProvider,
    pub agnes: &'a dyn AgnesSite,
    pub ledger: &'a DomainLedger,
    pub token_name: &'a str,
    pub mint_delay_min_ms: i64,
    pub mint_delay_max_ms: i64,
    pub now: &'a (dyn Fn() -> i64 + Sync),
    pub rand: &'a (dyn Fn() -> f64 + Sync),
    pub sleep: &'a (dyn Fn(i64) -> futures_core::future::BoxFuture<'static, ()> + Sync),
    pub emit: &'a (dyn Fn(TendEvent) + Sync),
}

fn ev(
    level: &'static str,
    event: &'static str,
    msg: impl Into<String>,
    fields: Vec<(&'static str, String)>,
) -> TendEvent {
    TendEvent {
        level,
        event,
        msg: msg.into(),
        fields,
    }
}

/// 一个域名试完之后的去向。
enum PerDomain {
    /// 整个铸号到此为止（成功、限流、或走到后三步失败了）。
    Done(MintOutcome),
    /// 换下一个域名。
    Next,
}

/// 真正的铸号器。
pub struct RealMinter<'a> {
    pub deps: MintDeps<'a>,
}

#[async_trait]
impl Minter for RealMinter<'_> {
    async fn mint_one(&self, req: MintRequest, journal: &mut Vec<Observation>) -> MintOutcome {
        let d = &self.deps;
        let mut created_any = false;
        let mut saw_upstream_error = false;

        for (i, domain) in req.candidates.iter().enumerate() {
            // 🔴 **必须真的等一下。**
            //
            // 从前这里是裸 `continue`（零间隔连打），而**实测的边缘限流正是被
            // 「无间隔连发」触发的**：一个全新出口连发到第 3 次就开始 429。
            if i > 0 {
                let span = (d.mint_delay_max_ms - d.mint_delay_min_ms).max(0);
                let delay = d.mint_delay_min_ms + ((d.rand)() * span as f64) as i64;
                if delay > 0 {
                    (d.sleep)(delay).await;
                }
            }

            let mailbox = match d.provider.create_mailbox(domain).await {
                Ok(m) => m,
                Err(e) => {
                    // ⚠️ **不记域名结论**：建邮箱失败说的是我们这条通道的事，
                    // 与「上游怎么看这个域名」无关。
                    (d.emit)(ev(
                        "warn",
                        "registrar.create_mailbox_failed",
                        e,
                        vec![("domain", domain.clone())],
                    ));
                    continue;
                }
            };
            created_any = true;

            let outcome = self
                .one_domain(domain, &mailbox, &req, journal, &mut saw_upstream_error)
                .await;

            // **用完即删**（相当于 TS 那边的 `finally`）。
            // 失败只记一条 warn —— **不掩盖已产出的返回值**。
            if let Err(e) = d.provider.delete_mailbox(&mailbox).await {
                (d.emit)(ev(
                    "warn",
                    "registrar.delete_mailbox_failed",
                    e,
                    vec![("domain", domain.clone())],
                ));
            }

            match outcome {
                PerDomain::Done(o) => return o,
                PerDomain::Next => continue,
            }
        }

        // ── 循环结束后的归因优先级 ────────────────────────────────────
        //
        // ⚠️ **限流不在这里** —— 它当场 return，根本走不到这一行。
        //
        // ⚠️⚠️ **消息里不许出现「可降级到备通道」那半句**：它会渲染进面板事件板块、
        // 进容器 stdout、进事件下载。**二选一之后永远不会自动切通道，
        // 照着旧文案说一遍，运维就会「等它自己切过去」而实际上永远不会切 ——
        // 那是把一个要人管的故障说成了自愈的故障。**
        let reason = if !created_any {
            // **通道级失败**：凭据失效、活跃邮箱配额耗尽、邮箱服务挂了。
            (d.emit)(ev(
                "error",
                "registrar.provider_unusable",
                "这一次尝试里一个临时邮箱都没建出来 —— 多半是通道凭据失效、活跃邮箱配额耗尽，\
                 或者邮箱服务本身挂了。**注册机不会自动切到另一条通道**（今天是二选一），\
                 要换得去面板上改。",
                vec![],
            ));
            TendFailureReason::ProviderError
        } else if saw_upstream_error {
            TendFailureReason::UpstreamError
        } else {
            TendFailureReason::DomainBlockedAll
        };
        MintOutcome::Failed { reason }
    }
}

impl RealMinter<'_> {
    async fn one_domain(
        &self,
        domain: &str,
        mailbox: &Mailbox,
        req: &MintRequest,
        journal: &mut Vec<Observation>,
        saw_upstream_error: &mut bool,
    ) -> PerDomain {
        let d = &self.deps;
        let addr = mailbox.address.as_str();

        // ── ① 发码 ───────────────────────────────────────────────────
        let r = match d.agnes.send_code(addr).await {
            Ok(r) => r,
            Err(e) => return self.network_error(domain, &e),
        };
        match classify_send_code(r.status, &r.body) {
            // 🔴 **当场 return，不换域名、不 sleep、不记任何域名结论。**
            SendCodeClass::RateLimitedEdge | SendCodeClass::RateLimitedApp => {
                let kind = if matches!(
                    classify_send_code(r.status, &r.body),
                    SendCodeClass::RateLimitedEdge
                ) {
                    BackoffKind::Edge
                } else {
                    BackoffKind::App
                };
                return PerDomain::Done(MintOutcome::RateLimited {
                    limit_kind: kind,
                    marker: edge_marker(&r.body).map(str::to_string),
                });
            }
            SendCodeClass::DomainBlocked => {
                let msg = upstream_message(&r.body, addr, &[]);
                // ⚠️ **只诊断，不改控制流。**
                //
                // 🔴🔴 这一支曾经有第二道保险（「已知 ok 的域名回 400 就当成出口限流、
                // 当场 return、一条域名结论都不记」），**它被拆掉了，理由是实测出来的一条死锁**：
                // 在已知好域名**真的**被上游拉黑时 ——
                // ① 一条结论都不记 ⇒ 台账里那条 `ok` 的 `at` **永远不刷新**；
                // ② 当场 return ⇒ 同一次尝试里后面的候选一个都不试；
                // ③ `tender` 据此中止整轮并记指数退避。
                // 而选域名档内按 `at` 升序 ⇒ 那个 `at` 冻住的坏域名**每一轮都排第一**
                // ⇒ 每轮「打它一次 → 判成限流 → 停整轮 → 退避翻倍」，
                // **一把 key 都出不来，直到 7 天把那条 ok 过期掉才自愈**。
                // 实测探针：连着 6 轮 minted 全 0，而台账里另外三个好域名一次都没被派出去。
                //
                // 🔴 **拆掉它没有丢掉「防误判」**：`commit_journal` 的**两跳规则**本来就在防同一件事，
                // 外加一轮最多学 1 条的钳位、以及排序永不 filter。
                // **保险与两跳规则重复，却额外制造了上面那把死锁。**
                if is_known_good(d.ledger, domain, (d.now)()) {
                    (d.emit)(ev(
                        "warn",
                        "registrar.known_good_domain_rejected",
                        format!("台账里标着能用的域名这次被上游拒了。上游原话：{msg}"),
                        vec![("domain", domain.to_string())],
                    ));
                }
                journal.push(Observation {
                    domain: domain.to_string(),
                    verdict: DomainState::Blocked,
                });
                return PerDomain::Next;
            }
            SendCodeClass::Unreadable => {
                // ⚠️ **不产生任何域名判定**：判死走的是负向匹配，
                // 而负向匹配至少要先有一段正文可读。
                (d.emit)(ev(
                    "warn",
                    "registrar.send_code_unclassified",
                    "上游回了 400 但正文是空的 —— 分不出是「这个域名被屏蔽」还是「发得太频繁」，\
                     所以这一次**一条域名结论都不记**。",
                    vec![("domain", domain.to_string())],
                ));
                return PerDomain::Next;
            }
            SendCodeClass::UpstreamError => {
                *saw_upstream_error = true;
                (d.emit)(ev(
                    "warn",
                    "registrar.send_code_bad_status",
                    upstream_message(&r.body, addr, &[]),
                    vec![
                        ("domain", domain.to_string()),
                        ("status", r.status.to_string()),
                    ],
                ));
                return PerDomain::Next;
            }
            // ← **干净可靠的证据，无条件覆盖回 ok。**
            SendCodeClass::Ok => journal.push(Observation {
                domain: domain.to_string(),
                verdict: DomainState::Ok,
            }),
        }

        // ── ② 等码 ───────────────────────────────────────────────────
        let code = match d.provider.poll_code(mailbox, req.code_timeout_ms).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                (d.emit)(ev(
                    "warn",
                    "registrar.code_timeout",
                    "发码那一步上游回了 2xx，但在超时之前一封信都没收到。",
                    vec![
                        ("domain", domain.to_string()),
                        ("timeoutMs", req.code_timeout_ms.to_string()),
                    ],
                ));
                return PerDomain::Done(MintOutcome::Failed {
                    reason: TendFailureReason::CodeTimeout,
                });
            }
            Err(e) => return self.network_error(domain, &e),
        };

        // ── ③④⑤ 注册 / 登录 / 建 key —— **最贵的三步** ──────────────
        let password = random_password(d.rand);

        let reg = match d.agnes.register(addr, &code, &password).await {
            Ok(r) => r,
            Err(e) => return self.network_error(domain, &e),
        };
        if !(200..300).contains(&reg.status) {
            (d.emit)(ev(
                "warn",
                "registrar.register_rejected",
                step_message(
                    reg.status,
                    &reg.body,
                    addr,
                    std::slice::from_ref(&code),
                    &["ok"],
                ),
                vec![("domain", domain.to_string())],
            ));
            return PerDomain::Done(MintOutcome::Failed {
                reason: TendFailureReason::RegisterFailed,
            });
        }

        let lo = match d.agnes.login(addr, &password).await {
            Ok(r) => r,
            Err(e) => return self.network_error(domain, &e),
        };
        let token = match serde_json::from_str::<Value>(&lo.body)
            .ok()
            .as_ref()
            .and_then(|v| pick(v, &TOKEN_FIELDS).map(str::to_string))
            .filter(|_| (200..300).contains(&lo.status))
        {
            Some(t) => t,
            None => {
                (d.emit)(ev(
                    "warn",
                    "registrar.login_no_token",
                    step_message(
                        lo.status,
                        &lo.body,
                        addr,
                        std::slice::from_ref(&password),
                        &TOKEN_FIELDS,
                    ),
                    vec![("domain", domain.to_string())],
                ));
                return PerDomain::Done(MintOutcome::Failed {
                    reason: TendFailureReason::LoginFailed,
                });
            }
        };

        let created = match d.agnes.create_key(&token, d.token_name).await {
            Ok(r) => r,
            Err(e) => return self.network_error(domain, &e),
        };
        match serde_json::from_str::<Value>(&created.body)
            .ok()
            .as_ref()
            .and_then(|v| pick(v, &KEY_FIELDS).map(str::to_string))
            .filter(|_| (200..300).contains(&created.status))
        {
            // **账号密码到此为止，不返回也不持久化。**
            Some(key) => PerDomain::Done(MintOutcome::Ok { key }),
            None => {
                (d.emit)(ev(
                    "warn",
                    "registrar.key_not_returned",
                    step_message(
                        created.status,
                        &created.body,
                        addr,
                        std::slice::from_ref(&token),
                        &KEY_FIELDS,
                    ),
                    vec![("domain", domain.to_string())],
                ));
                PerDomain::Done(MintOutcome::Failed {
                    reason: TendFailureReason::KeyFailed,
                })
            }
        }
    }

    /// 请求压根没走通。
    ///
    /// ⚠️ **一条域名结论都不记**：它说明的是「这次请求没走通」，
    /// **不是「上游怎么看这个域名」。把它接进台账等于用噪声污染证据。**
    fn network_error(&self, domain: &str, e: &str) -> PerDomain {
        (self.deps.emit)(ev(
            "warn",
            "registrar.network_error",
            e.to_string(),
            vec![("domain", domain.to_string())],
        ));
        PerDomain::Done(MintOutcome::Failed {
            reason: TendFailureReason::NetworkError,
        })
    }
}

#[cfg(test)]
mod tests;
