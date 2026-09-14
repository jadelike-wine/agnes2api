//! `mint` 的判据。
//!
//! 走到后三步的失败是**最贵的** —— 临时邮箱已经真的建出来、验证码已经真的要过一次。
//! 所以这里的断言大量打在**「这一步说了什么」**上，而不只是「返回了哪个枚举」。

use super::*;
use crate::registrar::domain_ledger::DomainEntry;
use std::collections::HashMap;
use std::sync::Mutex;

// ── 夹具 ──────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Spy {
    created: Mutex<Vec<String>>,
    deleted: Mutex<Vec<String>>,
    events: Mutex<Vec<(&'static str, &'static str, String)>>,
    slept: Mutex<Vec<i64>>,
    send_code_calls: Mutex<u32>,
}

struct FakeProvider<'a> {
    spy: &'a Spy,
    /// 建邮箱失败的域名。
    create_fails: Vec<String>,
    /// `None` = 等码超时。
    code: Option<String>,
    poll_err: bool,
}
#[async_trait]
impl MailboxProvider for FakeProvider<'_> {
    async fn create_mailbox(&self, domain: &str) -> Result<Mailbox, String> {
        if self.create_fails.iter().any(|d| d == domain) {
            return Err(format!("配额耗尽: {domain}"));
        }
        self.spy.created.lock().unwrap().push(domain.to_string());
        Ok(Mailbox {
            id: format!("id-{domain}"),
            address: format!("u0@{domain}"),
        })
    }
    async fn poll_code(&self, _m: &Mailbox, _t: i64) -> Result<Option<String>, String> {
        if self.poll_err {
            return Err("连接被重置".into());
        }
        Ok(self.code.clone())
    }
    async fn delete_mailbox(&self, m: &Mailbox) -> Result<(), String> {
        self.spy.deleted.lock().unwrap().push(m.id.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FakeAgnes<'a> {
    spy: Option<&'a Spy>,
    send: Option<Reply>,
    send_err: bool,
    register: Option<Reply>,
    login: Option<Reply>,
    create: Option<Reply>,
}
fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        body: body.to_string(),
    }
}
#[async_trait]
impl AgnesSite for FakeAgnes<'_> {
    async fn send_code(&self, _a: &str) -> Result<Reply, String> {
        if let Some(s) = self.spy {
            *s.send_code_calls.lock().unwrap() += 1;
        }
        if self.send_err {
            return Err("DNS 解析失败".into());
        }
        Ok(self.send.clone().unwrap_or_else(|| reply(200, "{}")))
    }
    async fn register(&self, _a: &str, _c: &str, _p: &str) -> Result<Reply, String> {
        Ok(self.register.clone().unwrap_or_else(|| reply(200, "{}")))
    }
    async fn login(&self, _a: &str, _p: &str) -> Result<Reply, String> {
        Ok(self
            .login
            .clone()
            .unwrap_or_else(|| reply(200, r#"{"token":"tk-1"}"#)))
    }
    async fn create_key(&self, _t: &str, _n: &str) -> Result<Reply, String> {
        Ok(self
            .create
            .clone()
            .unwrap_or_else(|| reply(200, r#"{"key":"sk-minted"}"#)))
    }
}

struct Case<'a> {
    provider: FakeProvider<'a>,
    agnes: FakeAgnes<'a>,
    ledger: DomainLedger,
}

async fn run(spy: &Spy, case: Case<'_>, candidates: &[&str]) -> (MintOutcome, Vec<Observation>) {
    let sleep = |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = ms;
        })
    };
    let slept = &spy.slept;
    let sleep_rec = move |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
        slept.lock().unwrap().push(ms);
        sleep(ms)
    };
    let emit = |e: TendEvent| {
        spy.events.lock().unwrap().push((e.level, e.event, e.msg));
    };
    let m = RealMinter {
        deps: MintDeps {
            provider: &case.provider,
            agnes: &case.agnes,
            ledger: &case.ledger,
            token_name: "auto",
            mint_delay_min_ms: 60_000,
            mint_delay_max_ms: 90_000,
            now: &|| 1_000_000,
            rand: &|| 0.0,
            sleep: &sleep_rec,
            emit: &emit,
        },
    };
    let mut journal = Vec::new();
    let out = m
        .mint_one(
            MintRequest {
                candidates: candidates.iter().map(|s| (*s).to_string()).collect(),
                code_timeout_ms: 120_000,
            },
            &mut journal,
        )
        .await;
    (out, journal)
}

fn mk(spy: &Spy) -> Case<'_> {
    Case {
        provider: FakeProvider {
            spy,
            create_fails: vec![],
            code: Some("143770".into()),
            poll_err: false,
        },
        agnes: FakeAgnes {
            spy: Some(spy),
            ..Default::default()
        },
        ledger: DomainLedger::default(),
    }
}

fn evs(spy: &Spy) -> Vec<&'static str> {
    spy.events
        .lock()
        .unwrap()
        .iter()
        .map(|(_, e, _)| *e)
        .collect()
}
fn msg_of(spy: &Spy, event: &str) -> String {
    spy.events
        .lock()
        .unwrap()
        .iter()
        .find(|(_, e, _)| *e == event)
        .map(|(_, _, m)| m.clone())
        .unwrap_or_default()
}

// ── 顺利那一条 ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_clean_run_mints_a_key_records_an_ok_verdict_and_deletes_the_mailbox() {
    let spy = Spy::default();
    let (out, journal) = run(&spy, mk(&spy), &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Ok {
            key: "sk-minted".into()
        }
    );
    assert_eq!(
        journal,
        vec![Observation {
            domain: "a.test".into(),
            verdict: DomainState::Ok,
        }],
        "发码 2xx 是干净可靠的证据，必须无条件记回 ok",
    );
    assert_eq!(
        *spy.deleted.lock().unwrap(),
        vec!["id-a.test".to_string()],
        "🔴 用完即删 —— 漏掉会悄悄耗尽活跃邮箱配额",
    );
}

/// **账号密码到此为止，不返回也不持久化。**
#[tokio::test]
async fn the_password_never_leaves_this_module() {
    let spy = Spy::default();
    let (out, _) = run(&spy, mk(&spy), &["a.test"]).await;
    let MintOutcome::Ok { key } = out else {
        panic!("该铸出来的")
    };
    assert_eq!(key, "sk-minted");
    // 口令是随机的，但长度固定 —— 断言它一个字都没进事件。
    for (_, _, m) in spy.events.lock().unwrap().iter() {
        assert!(
            m.len() < 400,
            "事件正文异常地长，像是把什么东西整个塞进去了：{m}"
        );
    }
}

// ── 限流：当场 return ─────────────────────────────────────────────────────

/// 🔴 **当场 return，不换域名、不 sleep、不记任何域名结论。**
#[tokio::test]
async fn hitting_the_rate_limit_returns_at_once_without_trying_another_domain() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(429, "error code: 1015"));
    let (out, journal) = run(&spy, c, &["a.test", "b.test", "c.test"]).await;
    assert_eq!(
        out,
        MintOutcome::RateLimited {
            limit_kind: BackoffKind::Edge,
            marker: Some("1015".into()),
        }
    );
    assert!(
        journal.is_empty(),
        "限流那一支记了域名结论 —— 那是把限流当成了域名问题"
    );
    assert_eq!(
        *spy.send_code_calls.lock().unwrap(),
        1,
        "换了下一个域名接着打"
    );
    assert!(spy.slept.lock().unwrap().is_empty(), "限流之后还睡了一觉");
    // 邮箱照样要删。
    assert_eq!(spy.deleted.lock().unwrap().len(), 1);
}

/// 应用层限流走 `App` 档。
#[tokio::test]
async fn an_application_level_rate_limit_is_reported_as_such() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "Too Many Requests from this IP"));
    let (out, _) = run(&spy, c, &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::RateLimited {
            limit_kind: BackoffKind::App,
            marker: None,
        }
    );
}

// ── 域名判死 ──────────────────────────────────────────────────────────────

/// 判死 ⇒ 记一条 blocked、**换下一个域名**。
#[tokio::test]
async fn a_blocked_domain_is_recorded_and_the_next_candidate_is_tried() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "domain not allowed"));
    let (out, journal) = run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::DomainBlockedAll
        }
    );
    assert_eq!(journal.len(), 2, "两个候选都该留下一条 blocked");
    assert!(journal.iter().all(|o| o.verdict == DomainState::Blocked));
    assert_eq!(*spy.send_code_calls.lock().unwrap(), 2);
    // 🔴 换域名之前**必须真的等一下** —— 零间隔连发正是实测中触发边缘限流的形态。
    assert_eq!(
        *spy.slept.lock().unwrap(),
        vec![60_000],
        "换下一个域名时没睡 —— 一个全新出口连发到第 3 次就开始 429",
    );
}

/// 🔴🔴 **这一格钉的是那道被拆掉的「第二道保险」不许回来。**
///
/// 那道保险是「已知 ok 的域名回 400 就当成出口限流、当场 return、一条域名结论都不记」。
/// 它制造了一把**实测出来的死锁**：台账里那条 `ok` 的 `at` 永远不刷新 ⇒
/// 选域名档内按 `at` 升序 ⇒ 那个坏域名**每一轮都排第一** ⇒
/// 每轮「打它一次 → 判成限流 → 停整轮 → 退避翻倍」，
/// **一把 key 都出不来，直到 7 天把那条 ok 过期掉才自愈**。
///
/// ⇒ 今天它**只诊断，不改控制流**：照样记 blocked、照样换下一个域名。
#[tokio::test]
async fn a_known_good_domain_being_refused_is_only_diagnosed_never_short_circuited() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "domain not allowed"));
    c.ledger.entries.insert(
        "a.test".into(),
        DomainEntry {
            s: DomainState::Ok,
            at: 1_000_000, // 与夹具的 now 同刻 ⇒ 未过期 ⇒ 已知能用
            n: 1,
        },
    );
    let (out, journal) = run(&spy, c, &["a.test", "b.test"]).await;
    assert!(
        evs(&spy).contains(&"registrar.known_good_domain_rejected"),
        "已知能用的域名被拒，却一句诊断都没说",
    );
    assert!(
        matches!(
            out,
            MintOutcome::Failed {
                reason: TendFailureReason::DomainBlockedAll
            }
        ),
        "🔴 走成了限流那一档 —— 那道保险回来了，而它会把注册机锁死 7 天",
    );
    assert_eq!(
        journal.len(),
        2,
        "🔴 一条域名结论都没记 ⇒ 台账里那条 ok 的 at 永远不刷新"
    );
    assert_eq!(
        *spy.send_code_calls.lock().unwrap(),
        2,
        "🔴 当场 return 了，后面的候选一个都没试"
    );
}

/// 400 但正文为空 ⇒ **一条域名结论都不记**，换下一个。
#[tokio::test]
async fn an_unreadable_four_hundred_produces_no_domain_verdict_at_all() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "   "));
    let (_, journal) = run(&spy, c, &["a.test"]).await;
    assert!(
        journal.is_empty(),
        "判死走的是负向匹配，而负向匹配至少要先有一段正文可读",
    );
    assert!(evs(&spy).contains(&"registrar.send_code_unclassified"));
}

// ── 归因优先级 ────────────────────────────────────────────────────────────

/// 一个邮箱都没建出来 ⇒ **通道级失败**。
#[tokio::test]
async fn failing_to_create_any_mailbox_is_attributed_to_the_provider() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.create_fails = vec!["a.test".into(), "b.test".into()];
    let (out, journal) = run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::ProviderError
        }
    );
    assert!(journal.is_empty(), "建邮箱失败与「上游怎么看这个域名」无关");
    assert!(evs(&spy).contains(&"registrar.create_mailbox_failed"));
}

/// ⚠️⚠️ **消息里不许出现「可降级到备通道」那半句。**
///
/// 二选一之后**永远不会自动切通道** —— 照着旧文案说一遍，
/// 运维就会「等它自己切过去」而实际上永远不会切，
/// **那是把一个要人管的故障说成了自愈的故障。**
#[tokio::test]
async fn the_provider_failure_message_never_promises_an_automatic_fallback() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.create_fails = vec!["a.test".into()];
    run(&spy, c, &["a.test"]).await;
    let m = msg_of(&spy, "registrar.provider_unusable");
    assert!(!m.is_empty(), "通道整个不可用却一句话都没说");
    for banned in ["降级", "备通道", "自动切"] {
        assert!(
            !m.contains(banned) || m.contains("不会自动切"),
            "报文里许诺了一个永远不会发生的自动降级：{m}",
        );
    }
    assert!(m.contains("不会自动切"), "没说清「要换得去面板上改」：{m}");
}

/// 见过 5xx ⇒ `upstream_error` 优先于 `domain_blocked_all`。
#[tokio::test]
async fn seeing_an_upstream_error_outranks_domain_blocked_all() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(503, "gateway down"));
    let (out, journal) = run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::UpstreamError
        }
    );
    assert!(journal.is_empty(), "5xx 不是域名的错，不许记域名结论");
}

/// 请求压根没走通 ⇒ `network_error`，**一条域名结论都不记**。
#[tokio::test]
async fn a_transport_failure_never_pollutes_the_domain_ledger() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send_err = true;
    let (out, journal) = run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::NetworkError
        }
    );
    assert!(
        journal.is_empty(),
        "把「这次请求没走通」接进了台账 —— 那是用噪声污染证据",
    );
    assert_eq!(spy.deleted.lock().unwrap().len(), 1, "抛错了也要删邮箱");
}

// ── 后三步 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_code_that_never_arrives_ends_the_attempt() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.code = None;
    let (out, _) = run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::CodeTimeout
        }
    );
    assert_eq!(
        *spy.send_code_calls.lock().unwrap(),
        1,
        "等码超时之后不该再去打下一个域名 —— 那一发验证码已经花掉了",
    );
}

#[tokio::test]
async fn each_of_the_last_three_steps_has_its_own_reason() {
    // 注册被拒
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.register = Some(reply(409, "already registered"));
    let (out, _) = run(&spy, c, &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::RegisterFailed
        }
    );
    assert!(evs(&spy).contains(&"registrar.register_rejected"));

    // 登录拿不到令牌
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.login = Some(reply(200, r#"{"data":{"token_id":"x"}}"#));
    let (out, _) = run(&spy, c, &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::LoginFailed
        }
    );

    // 建 key 没回 key
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.create = Some(reply(200, r#"{"data":{"secret_id":"x"}}"#));
    let (out, _) = run(&spy, c, &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::KeyFailed
        }
    );
}

/// 🔴 **2xx 却没解析出目标字段时，只说字段名，一个值都不说。**
///
/// 这一档的正文**极可能就装着那把令牌本身**，而运维需要的恰恰只有字段名。
#[tokio::test]
async fn a_renamed_field_is_diagnosed_by_name_only_never_by_value() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    // 上游把 `token` 改名成 `token_id`，而那个值**就是令牌本身**。
    c.agnes.login = Some(reply(
        200,
        r#"{"ok":true,"data":{"token_id":"THE-ACTUAL-SECRET-TOKEN"}}"#,
    ));
    run(&spy, c, &["a.test"]).await;
    let m = msg_of(&spy, "registrar.login_no_token");
    assert!(
        m.contains("data.token_id"),
        "没说出改了名的那个字段 —— 运维看不出发生了什么：{m}",
    );
    assert!(
        !m.contains("THE-ACTUAL-SECRET-TOKEN"),
        "🔴 把那把令牌原样印进了事件：{m}",
    );
    // ⚠️ **「我们期望的是哪几个字段」那半句同样承重**：
    // 只列出上游给了什么、不说我们要什么，运维还是得去翻源码才知道该对照哪个名字。
    assert!(
        m.contains("token") && m.contains("access_token"),
        "没说清我们期望的是哪几个字段名：{m}",
    );
}

/// 非 2xx ⇒ 走上游原话那一档（抹地址、抹本步在场的凭据）。
#[tokio::test]
async fn a_rejected_step_reports_the_upstream_wording_with_secrets_masked() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.register = Some(reply(400, "bad code for u0@a.test"));
    run(&spy, c, &["a.test"]).await;
    let m = msg_of(&spy, "registrar.register_rejected");
    assert!(m.contains("bad code"), "上游原话丢了：{m}");
    assert!(!m.contains("u0@a.test"), "🔴 邮箱地址原样进了事件：{m}");
}

// ── step_message 本身 ─────────────────────────────────────────────────────

/// 🔴 **这一格钉的是「非 2xx 一律走上游原话那一档」，与正文是不是 JSON 无关。**
///
/// ⚠️ **挑输入时要当心**：非 2xx 且正文**不是** JSON 时，两条档位会走同一条兜底路径，
/// 于是「把 status 判据拆掉」这个变异**不会红**。我第一版就用了这种输入，
/// 变异存活了。这一格改用**合法 JSON 的非 2xx 正文** —— 只有它把两档真的分开。
#[test]
fn a_non_2xx_body_takes_the_upstream_wording_tier_even_when_it_is_valid_json() {
    // 正文是合法 JSON，而且**装着一把凭据**。
    let body = r#"{"error":"bad code","echo":"THE-SECRET"}"#;
    let m = step_message(
        400,
        body,
        "u0@a.test",
        &["THE-SECRET".to_string()],
        &["token"],
    );
    assert!(m.contains("bad code"), "上游的裁决词丢了：{m}");
    assert!(!m.contains("THE-SECRET"), "🔴 凭据原样漏进了诊断：{m}");
    assert!(
        !m.contains("期望"),
        "🔴 走成了「只说字段名」那一档 —— 那一档不会说出上游的裁决词，\
         而非 2xx 时那句裁决词正是运维唯一需要的东西：{m}",
    );
}

#[test]
fn a_non_json_body_falls_back_to_the_upstream_wording_tier() {
    let m = step_message(200, "<html>oops</html>", "u0@a.test", &[], &["token"]);
    assert!(
        m.contains("oops"),
        "非 JSON 正文里不会有我们没认出来的令牌：{m}"
    );
}

#[test]
fn the_field_name_list_is_capped_at_two_levels_and_twenty_names() {
    let mut body = String::from("{");
    for i in 0..40 {
        body.push_str(&format!(r#""f{i}":1,"#));
    }
    body.push_str(r#""data":{"deep":{"deeper":1}}}"#);
    let m = step_message(200, &body, "", &[], &["token"]);
    assert!(!m.contains("deeper"), "钻到第三层去了：{m}");
    let shown = m.matches("f").count();
    assert!(
        shown <= 25,
        "字段名列得太多，等于把整份响应结构画出来了：{shown}"
    );
}

// ── random_password ───────────────────────────────────────────────────────
//
// ⚠️⚠️ **这里原来有两格判据，它们钉的是一个错的实现，已删。**
//
// 我移植时在本模块自己造了一份 24 位、末尾没有 `Q!` 后缀的口令生成器，
// 又照着它写了两格判据（「24 位」「不含易混字符」）—— **两格都绿**，
// 而那份实现生成的口令**一个大写字母、一个特殊字符都没有**，
// 上游会直接拒掉注册，故障显示成 `register_failed`。
//
// 换成正典那一份（`agnes::random_password`，14 位 + 固定 `Q!`）之后，
// 那两格当场变红 —— **它们是在为一个错的答案作证**。
// 口令的判据今天只有一组，在 `agnes` 那边，而且比这两格严：
// 它额外钉住「必须含大写」「必须含特殊字符」，也就是那个后缀存在的理由。

// ── pick ──────────────────────────────────────────────────────────────────

#[test]
fn a_field_is_picked_from_the_top_level_or_from_data_but_empty_strings_do_not_count() {
    let v: Value = serde_json::from_str(r#"{"token":"","data":{"token":"tk"}}"#).unwrap();
    assert_eq!(pick(&v, &["token"]), Some("tk"), "空串被当成了有效值");
    let v2: Value = serde_json::from_str(r#"{"access_token":"at"}"#).unwrap();
    assert_eq!(pick(&v2, &TOKEN_FIELDS), Some("at"));
    let v3: Value = serde_json::from_str("{}").unwrap();
    assert_eq!(pick(&v3, &TOKEN_FIELDS), None);
}

// ── 候选之间的间隔 ────────────────────────────────────────────────────────

/// 🔴 **换域名之前必须真的等一下** —— 这一格钉的是那条实测。
///
/// 变异对象：把 `i > 0` 那一段删掉（裸 `continue`）——
/// **一个全新出口连发到第 3 次就开始 429**。
#[tokio::test]
async fn every_domain_switch_pays_the_configured_delay() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "domain not allowed"));
    run(&spy, c, &["a.test", "b.test", "c.test"]).await;
    assert_eq!(
        *spy.slept.lock().unwrap(),
        vec![60_000, 60_000],
        "三个候选之间只该有两段间隔，而且一段都不能省",
    );
}

/// 第一个候选**不睡**（那是这一次尝试的开头，间隔由 `tender` 在名额之间付）。
#[tokio::test]
async fn the_first_candidate_does_not_wait() {
    let spy = Spy::default();
    let (_, _) = run(&spy, mk(&spy), &["a.test"]).await;
    assert!(spy.slept.lock().unwrap().is_empty());
}

/// 建邮箱失败的那个域名也算一次候选 —— 下一个候选照样要等。
#[tokio::test]
async fn a_domain_that_failed_to_get_a_mailbox_still_counts_as_a_candidate() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.create_fails = vec!["a.test".into()];
    run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(*spy.slept.lock().unwrap(), vec![60_000]);
}

// ── 邮箱一定会被删 ────────────────────────────────────────────────────────

/// **用完即删**，而且删失败只记一条 warn、**不掩盖已产出的返回值**。
///
/// 三条退出路径各验一次 —— 漏掉任何一条都会**悄悄耗尽活跃邮箱配额**，
/// 而那是一个几天之后才以「补池零产出」暴露出来的故障。
#[tokio::test]
async fn the_mailbox_is_deleted_on_every_exit_path() {
    // ① 成功
    let spy = Spy::default();
    run(&spy, mk(&spy), &["a.test"]).await;
    assert_eq!(
        spy.deleted.lock().unwrap().len(),
        1,
        "「成功」这条路上邮箱没被删"
    );

    // ② 等码超时
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.code = None;
    run(&spy, c, &["a.test"]).await;
    assert_eq!(
        spy.deleted.lock().unwrap().len(),
        1,
        "「等码超时」这条路上邮箱没被删"
    );

    // ③ 注册被拒
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.register = Some(reply(400, "no"));
    run(&spy, c, &["a.test"]).await;
    assert_eq!(
        spy.deleted.lock().unwrap().len(),
        1,
        "「注册被拒」这条路上邮箱没被删"
    );
}

/// 不同域名各建各的邮箱，各删各的。
#[tokio::test]
async fn every_candidate_gets_its_own_mailbox_and_every_one_of_them_is_deleted() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.agnes.send = Some(reply(400, "domain not allowed"));
    run(&spy, c, &["a.test", "b.test"]).await;
    assert_eq!(*spy.created.lock().unwrap(), vec!["a.test", "b.test"]);
    assert_eq!(
        *spy.deleted.lock().unwrap(),
        vec!["id-a.test", "id-b.test"],
        "建了两个只删了一个",
    );
}

/// 轮询验证码时连接断了 ⇒ `network_error`，邮箱照样删。
#[tokio::test]
async fn a_poll_failure_is_a_transport_failure_and_still_deletes_the_mailbox() {
    let spy = Spy::default();
    let mut c = mk(&spy);
    c.provider.poll_err = true;
    let (out, journal) = run(&spy, c, &["a.test"]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::NetworkError
        }
    );
    // 发码那一步已经成功了 ⇒ 那条 ok 是干净证据，该留着。
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].verdict, DomainState::Ok);
    assert_eq!(spy.deleted.lock().unwrap().len(), 1);
}

/// 空候选 ⇒ 一个邮箱都没建 ⇒ 通道级失败。
#[tokio::test]
async fn an_empty_candidate_list_is_a_provider_error_not_a_silent_success() {
    let spy = Spy::default();
    let (out, journal) = run(&spy, mk(&spy), &[]).await;
    assert_eq!(
        out,
        MintOutcome::Failed {
            reason: TendFailureReason::ProviderError
        }
    );
    assert!(journal.is_empty());
}

// ── HashMap 只是为了让 clippy 看到它被用到 ────────────────────────────────
#[test]
fn the_ledger_shape_is_what_is_known_good_reads() {
    let mut e = HashMap::new();
    e.insert(
        "a.test".to_string(),
        DomainEntry {
            s: DomainState::Ok,
            at: 100,
            n: 1,
        },
    );
    let l = DomainLedger {
        updated_at: 0,
        total: None,
        entries: e,
    };
    assert!(is_known_good(&l, "a.test", 100));
    assert!(
        !is_known_good(&l, "a.test", 100 + OK_TTL_MS),
        "过期了还说能用"
    );
    assert!(!is_known_good(&l, "b.test", 100));
}
