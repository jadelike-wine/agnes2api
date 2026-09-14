//! 两条邮箱通道的判据。
//!
//! 这里钉的大多是**真机实测出来的契约**（哪个接口用 address、哪个用 id），
//! 而它们一旦弄反，表现是「删邮箱每次 404、用完即删从未真正生效」——
//! **一个几天之后才以「补池零产出」暴露出来的故障**。

use super::*;
use crate::ports::fetcher::{BodyStream, FetchError, OutboundResponse};
use std::sync::Mutex;

// ── 夹具 ──────────────────────────────────────────────────────────────────

struct Route {
    method: String,
    /// URL 里必须出现的片段。
    contains: String,
    status: u16,
    body: String,
}

#[derive(Default)]
struct Fake {
    routes: Mutex<Vec<Route>>,
    seen: Mutex<Vec<(String, String)>>,
    /// 命中这个片段的请求直接判成「发不出去」。
    fail_on: Mutex<Option<String>>,
}
impl Fake {
    fn route(self, method: &str, contains: &str, status: u16, body: &str) -> Self {
        self.routes.lock().unwrap().push(Route {
            method: method.into(),
            contains: contains.into(),
            status,
            body: body.into(),
        });
        self
    }
    fn urls(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, u)| u.clone())
            .collect()
    }
    fn calls(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}
#[async_trait]
impl Fetcher for Fake {
    async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
        self.seen
            .lock()
            .unwrap()
            .push((req.method.clone(), req.url.clone()));
        if let Some(f) = self.fail_on.lock().unwrap().as_ref()
            && req.url.contains(f.as_str())
        {
            return Err(FetchError::Connect("网络抖动".into()));
        }
        let routes = self.routes.lock().unwrap();
        let hit = routes
            .iter()
            .find(|r| r.method == req.method && req.url.contains(&r.contains));
        let (status, body) = match hit {
            Some(r) => (r.status, r.body.clone()),
            None => (404, "{}".into()),
        };
        let s: BodyStream = Box::pin(futures_util::stream::iter(vec![Ok(Bytes::from(body))]));
        Ok(OutboundResponse {
            status,
            headers: vec![],
            body: s,
        })
    }
}

struct Ctx {
    events: Mutex<Vec<(&'static str, String)>>,
    clock: Mutex<i64>,
}
impl Default for Ctx {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            clock: Mutex::new(0),
        }
    }
}

fn deps<'a>(f: &'a Fake, ctx: &'a Ctx) -> MailDeps<'a> {
    MailDeps {
        fetcher: f,
        base_url: "https://m.test/".into(),
        api_key: "mk-secret".into(),
        now: Box::leak(Box::new({
            let c: &'a Mutex<i64> = &ctx.clock;
            move || *c.lock().unwrap()
        })),
        sleep: Box::leak(Box::new({
            let c: &'a Mutex<i64> = &ctx.clock;
            move |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
                *c.lock().unwrap() += ms;
                Box::pin(async {})
            }
        })),
        emit: Box::leak(Box::new({
            let e: &'a Mutex<Vec<(&'static str, String)>> = &ctx.events;
            move |x: MailEvent| e.lock().unwrap().push((x.event, x.msg))
        })),
    }
}

fn events(ctx: &Ctx) -> Vec<&'static str> {
    ctx.events.lock().unwrap().iter().map(|(e, _)| *e).collect()
}

// ── 🔴 真机实测的那条契约：收信按 address、删除按 id ─────────────────────

/// 🔴 **这一格钉的是那条真机实测出来的定位契约。**
///
/// ```text
/// GET    /v1/messages?address={address} → 200，换成 id → 404 inbox_not_found
/// DELETE /v1/accounts/{id}              → 204，换成 address → 404 account_not_found
/// ```
/// **此前 handle 存的是 address，删邮箱因此每次都 404，「用完即删」从未真正生效** ——
/// 而那是一个几天之后才以「活跃邮箱配额耗尽 ⇒ 补池零产出」暴露出来的故障。
#[tokio::test]
async fn yyds_reads_mail_by_address_and_deletes_by_id() {
    let f = Fake::default()
        .route(
            "POST",
            "/v1/accounts",
            200,
            r#"{"data":{"address":"u0@a.test","id":"acc-1"}}"#,
        )
        .route("GET", "/v1/messages", 200, r#"{"messages":[]}"#)
        .route("DELETE", "/v1/accounts/acc-1", 204, "");
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };

    let m = y.create_mailbox("a.test").await.unwrap();
    assert_eq!(m.address, "u0@a.test");
    assert_eq!(m.id, "acc-1", "🔴 handle 必须存 id —— 删除靠它定位");

    // 收信：URL 里要带 address，**不带 id**。
    // ⚠️ 传正数：窗口为 0 时循环一次都不进，那样这一格就什么都没验到。
    let _ = y.poll_code(&m, 1).await;
    // 删除：URL 里要带 id，**不带 address**。
    y.delete_mailbox(&m).await.unwrap();

    let urls = f.urls();
    let list = urls
        .iter()
        .find(|u| u.contains("/v1/messages"))
        .cloned()
        .unwrap_or_default();
    let del = urls
        .iter()
        .find(|u| u.starts_with("https://m.test/v1/accounts/"))
        .cloned()
        .unwrap_or_default();
    assert!(
        list.contains("address=u0%40a.test"),
        "收信没按 address 定位 —— 换成 id 会 404 inbox_not_found：{list}",
    );
    assert!(
        del.ends_with("/acc-1"),
        "删除没按 id 定位 —— 换成 address 会 404 account_not_found：{del}",
    );
    assert!(!del.contains("u0"), "删除 URL 里出现了地址：{del}");
}

/// 🔴 **建邮箱响应缺 id 时：不发那次注定失败的删除，改成诚实告警。**
///
/// 变异对象：加一次「用 `本地部分@域名` 兜底删」——
/// 真机实测**一定 404**，而且没有按 address 反查 id 的端点。
/// 那只会往「删邮箱失败」那条 warn 里**灌稳定的假 404**，
/// **稀释「邮箱正在堆积、配额即将耗尽」这个真信号** —— 运维看多了就会开始忽略这类日志。
#[tokio::test]
async fn an_unparseable_create_response_warns_honestly_instead_of_firing_a_doomed_delete() {
    let f = Fake::default().route(
        "POST",
        "/v1/accounts",
        200,
        r#"{"data":{"address":"u0@a.test"}}"#,
    );
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let out = y.create_mailbox("a.test").await;
    assert!(out.is_err(), "缺 id 却报成了成功 —— 后续调用会 100% 打空");
    assert!(
        !f.calls().iter().any(|(m, _)| m == "DELETE"),
        "🔴 发了一次注定 404 的删除 —— 那会把真信号稀释掉",
    );
    assert!(events(&ctx).contains(&"registrar.mailbox_create_unparseable"));
    let msg = ctx.events.lock().unwrap()[0].1.clone();
    assert!(msg.contains("24"), "没说清它什么时候自己消失：{msg}");
    assert!(msg.contains("活跃邮箱名额"), "没说清它占着什么：{msg}");
}

/// 删除返回非 2xx ⇒ **报错**。
///
/// ⚠️ **返回值的含义是「确认删掉了没有」，不是「抛没抛错」**（端口契约逐字写着）：
/// 非 2xx 会正常返回，只看抛不抛错的调用方**一律会读成「删掉了」**。
#[tokio::test]
async fn a_delete_that_returns_non_2xx_is_not_reported_as_deleted() {
    let f = Fake::default().route(
        "DELETE",
        "/v1/accounts/acc-1",
        404,
        r#"{"e":"account_not_found"}"#,
    );
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "acc-1".into(),
        address: "u0@a.test".into(),
    };
    assert!(
        y.delete_mailbox(&m).await.is_err(),
        "404 被读成了「删掉了」—— 那个邮箱会一直占着名额",
    );
}

// ── 轮询：抖动必须被吞掉 ─────────────────────────────────────────────────

/// 🔴 **这一格钉的是「抖动不许穿出去」。**
///
/// 请求本身失败与「非 2xx」「正文不是 JSON」是**同一类瞬时故障** ——
/// 本轮跳过、继续轮询，**只有整体超时耗尽才收工**。
/// 此前没兜住时，**一次网络抖动就会作废整次铸 key**，
/// 而那一刻验证码往往**已经在邮箱里**、窗口还剩一百多秒。
#[tokio::test]
async fn a_transient_failure_while_polling_does_not_abort_the_whole_attempt() {
    let f = Fake::default();
    *f.fail_on.lock().unwrap() = Some("/v1/messages".into());
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "acc-1".into(),
        address: "u0@a.test".into(),
    };
    let out = y.poll_code(&m, 10_000).await;
    assert_eq!(
        out,
        Ok(None),
        "🔴 抖动穿出去了 —— 上层会把它收成 network_error 作废整次铸 key",
    );
    // 确实轮了好几圈才收工（3 秒一圈、10 秒窗口）。
    assert!(
        f.calls().len() >= 3,
        "只打了 {} 次就放弃了",
        f.calls().len()
    );
}

/// ⚠️ **这里被吞掉的失败刻意不留任何日志。**
///
/// 单次铸 key 光轮询就要打约 40 次请求，瞬时错误是常态；
/// 在这里留痕会让那 100 格的事件环**几秒清空一次**，
/// **把运维真正需要的诊断挤出去。**
#[tokio::test]
async fn the_polling_loop_stays_silent_about_transient_failures() {
    let f = Fake::default();
    *f.fail_on.lock().unwrap() = Some("/v1/messages".into());
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "acc-1".into(),
        address: "u0@a.test".into(),
    };
    let _ = y.poll_code(&m, 10_000).await;
    assert!(
        events(&ctx).is_empty(),
        "轮询里留痕了 —— 它会把 100 格的事件环几秒清空一次：{:?}",
        events(&ctx),
    );
}

/// 超时返回 `None` 而不是错误（端口契约逐字写着）。
#[tokio::test]
async fn running_out_of_time_yields_none_not_an_error() {
    let f = Fake::default().route("GET", "/v1/messages", 200, r#"{"messages":[]}"#);
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "acc-1".into(),
        address: "u0@a.test".into(),
    };
    assert_eq!(y.poll_code(&m, 5_000).await, Ok(None));
}

/// 列表里就带正文时，**不再打第二次详情请求**。
#[tokio::test]
async fn a_list_that_already_carries_the_body_does_not_fetch_the_detail() {
    let f = Fake::default().route(
        "GET",
        "/api/emails/box-1",
        200,
        r#"{"messages":[{"id":"m1","subject":"验证码","html":"<p>143770</p>"}]}"#,
    );
    let ctx = Ctx::default();
    let mm = Moemail {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "box-1".into(),
        address: "u0@a.test".into(),
    };
    assert_eq!(mm.poll_code(&m, 10_000).await, Ok(Some("143770".into())));
    assert_eq!(f.calls().len(), 1, "正文已经在列表里了，却又打了一次详情");
}

/// 同一封信**只处理一次** —— 否则每一圈都会重新解析全部旧信。
#[tokio::test]
async fn each_message_is_only_examined_once() {
    let f = Fake::default().route(
        "GET",
        "/api/emails/box-1",
        200,
        r#"{"messages":[{"id":"m1","subject":"x","html":"<p>没有码</p>"}]}"#,
    );
    let ctx = Ctx::default();
    let mm = Moemail {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "box-1".into(),
        address: "u0@a.test".into(),
    };
    let _ = mm.poll_code(&m, 10_000).await;
    // 4 圈列表（3 秒一圈、10 秒窗口），**一次详情都不该打**（那封信只看一次）。
    assert!(f.calls().iter().all(|(m, _)| m == "GET"), "{:?}", f.calls(),);
}

// ── 列域名：两家形状不同 ─────────────────────────────────────────────────

/// YYDS 有独立端点。
#[tokio::test]
async fn yyds_lists_domains_from_its_own_endpoint() {
    let f = Fake::default().route(
        "GET",
        "/v1/domains",
        200,
        r#"{"domains":["a.test","b.test"]}"#,
    );
    let ctx = Ctx::default();
    let d = deps(&f, &ctx);
    assert_eq!(
        yyds_list_domains(&d).await.unwrap(),
        vec!["a.test".to_string(), "b.test".to_string()],
    );
}

/// MoeMail 是**自建**服务，域名装在站点配置里 —— 没有独立端点。
#[tokio::test]
async fn moemail_lists_domains_out_of_the_site_config() {
    let f = Fake::default().route("GET", "/api/config", 200, r#"{"emailDomains":["x.test"]}"#);
    let ctx = Ctx::default();
    let d = deps(&f, &ctx);
    assert_eq!(
        moemail_list_domains(&d).await.unwrap(),
        vec!["x.test".to_string()]
    );
}

/// 域名装在 `{ "domain": "x" }` 里的站点也要认。
#[tokio::test]
async fn a_list_of_objects_is_flattened_to_domain_names() {
    let f = Fake::default().route(
        "GET",
        "/api/config",
        200,
        r#"{"emailDomains":[{"domain":"x.test"},{"domain":""},"y.test"]}"#,
    );
    let ctx = Ctx::default();
    let d = deps(&f, &ctx);
    assert_eq!(
        moemail_list_domains(&d).await.unwrap(),
        vec!["x.test".to_string(), "y.test".to_string()],
        "空域名该被滤掉",
    );
}

/// 非 2xx ⇒ 报错，而且地址是**脱敏**的。
#[tokio::test]
async fn a_rejected_list_call_reports_a_redacted_address() {
    let f = Fake::default().route("GET", "/v1/domains", 401, r#"{"e":"unauthorized"}"#);
    let ctx = Ctx::default();
    let mut d = deps(&f, &ctx);
    d.base_url = "https://u:topsecret@m.test".into();
    let e = yyds_list_domains(&d).await.unwrap_err();
    assert!(e.contains("HTTP 401"), "状态码丢了：{e}");
    assert!(!e.contains("topsecret"), "🔴 口令漏进了诊断：{e}");
}

// ── 凭据走请求头 ─────────────────────────────────────────────────────────

/// 🔴 凭据走 `authorization` 头，**绝不进 URL**。
///
/// 进 URL 就会落进浏览器历史 / Referer / 反代访问日志 ——
/// 而脱敏只挡得住我们自己拼的那一份诊断，挡不住反代那一侧。
#[tokio::test]
async fn the_api_key_never_appears_in_any_url() {
    let f = Fake::default()
        .route(
            "POST",
            "/v1/accounts",
            200,
            r#"{"data":{"address":"u0@a.test","id":"acc-1"}}"#,
        )
        .route("GET", "/v1/domains", 200, r#"{"domains":[]}"#)
        .route("DELETE", "/v1/accounts/acc-1", 204, "");
    let ctx = Ctx::default();
    let d = deps(&f, &ctx);
    let _ = yyds_list_domains(&d).await;
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = y.create_mailbox("a.test").await.unwrap();
    y.delete_mailbox(&m).await.unwrap();
    for u in f.urls() {
        assert!(!u.contains("mk-secret"), "🔴 凭据进了 URL：{u}");
    }
}

// ── 地址转义 ─────────────────────────────────────────────────────────────

/// 地址里的 `+` 与 `@` 必须转义 —— 不转义时上游查的是**另一个邮箱**，
/// 表现成「等码超时」而真因在编码上。
#[tokio::test]
async fn the_address_is_percent_encoded_into_the_query() {
    let f = Fake::default().route("GET", "/v1/messages", 200, r#"{"messages":[]}"#);
    let ctx = Ctx::default();
    let y = Yyds {
        deps: deps(&f, &ctx),
    };
    let m = Mailbox {
        id: "acc-1".into(),
        address: "u0+tag@a.test".into(),
    };
    let _ = y.poll_code(&m, 1).await;
    // 窗口是 1ms、时钟不自己走 ⇒ 至少进了一圈。
    let u = f.urls().first().cloned().unwrap_or_default();
    assert!(u.contains("u0%2Btag%40a.test"), "{u}");
}

/// 末尾斜杠不许拼出双斜杠。
#[tokio::test]
async fn a_trailing_slash_on_the_base_url_does_not_produce_a_double_slash() {
    let f = Fake::default().route("GET", "/v1/domains", 200, r#"{"domains":[]}"#);
    let ctx = Ctx::default();
    let mut d = deps(&f, &ctx);
    d.base_url = "https://m.test///".into();
    let _ = yyds_list_domains(&d).await;
    assert_eq!(f.urls()[0], "https://m.test/v1/domains");
}
