/**
 * 「注册机」板块（裁定 P-5 / P-6 的第 8 个板块，排在 logs 与 settings 之间）。
 *
 * 🔴 **kiro2api 里没有任何可参照物** —— kiro 的账号是用户自己的登录凭据，
 * agnes 的账号是注册机铸出来的。用户明确要求**保留这个功能**，所以这里一格都不少；
 * 外观按 06 §2.12 的表用 kiro 的设计语言新造（`css/sections-registrar.css` 的
 * `.reg-` 一族 + 一堆现成原语），**没有一个新发明的类名**。
 *
 * ── ⚠️ 本轮后端还是占位，面板必须**诚实地**把这件事说出来 ────────────────────
 *
 * `src/admin/registrar_stub.rs` 的三条端点在 P1 恒回 `503 {reason:"not_wired"}`
 * （P3 才真接线）。于是本板块有**四种互斥的相位**，一种都不许并进另一种：
 *
 * · `loading`  —— 还在读。**不套危险色**：给一条还在进行中的提示配红字，
 *                 等于每次进板块都先闪一次红。
 * · `notWired` —— 「这个部署没有接注册机」。🔴 **这不是错误，也不是「功能正常」。**
 *                 画成错误会让运维去查网络与日志（查不到任何东西，因为什么都没坏）；
 *                 画成正常（例如摆一排空的通道卡 + 一颗点了没反应的「立即补池」）
 *                 是更坏的那一种 —— 它会让人以为补池在跑，而池子正在见底。
 *                 ⇒ 这一相位下**一颗动作按钮都不画**，只说那一句真话 + 一颗「重新检查」。
 * · `error`    —— 真的读失败了（断网 / 500 / 响应形状不对）。走 `.banner .banner-danger`。
 * · `ready`    —— 读到了一份状态（P3 之后才可能出现）。完整渲染。
 *
 * ⚠️⚠️ **横幅是两层，`.banner` 那一层不许漏**（本文件六处横幅曾经全部漏掉它）：
 * `css/base.css` 把它拆成 `.banner`（padding / border / 4px 左色条 / radius /
 * margin / 字号 —— 横幅之所以看起来是横幅，全在这一层）与 `.banner-warn|-info|-danger`
 * （只管 background + `border-left-color` + color）。只写后者的话拿到的是**一块没有
 * 内边距、没有边框、没有圆角、没有左色条的纯色文字**，与正文几乎分不开。
 * ⚠️ 其余六个板块（apikeys/settings/models/logs/usage/dashboard）**全部写的是
 * `"banner banner-danger"` 这种两层形态** —— 这里漏掉是抄漏，不是本板块的设计。
 *
 * ⚠️ **「一颗按钮都不画」不等于后端那条事件白写了**：`registrar.manual_tend_not_wired`
 * 仍然有真实触发路径 —— ① curl / 脚本直接打那条端点；② 面板开着的时候部署被换成
 * 一个没有注册机的版本，此时板块还停在 `ready` 相位、按钮还在，点下去拿到 503，
 * 本文件会弹一句诚实的话并把相位切回 `notWired`。
 *
 * ── 三条纪律 ────────────────────────────────────────────────────────────────
 *
 * ① **一切来自接口的内容一律 `textContent`**（`el()` 走的就是它）：补池历史里的
 *    `channel` 是配置来的字符串，`reason` 可能是这个面板版本**不认识**的值，
 *    两者都会被原样显示出来。
 * ② 🔴 **没有自动刷新。** 这个板块每刷新一次要付 3 次存储读（补池历史 + 补池锁 +
 *    护栏键），而配额账把面板的读算成「人点一下才发生」。要加轮询之前先去看
 *    05 §9 的「配额账」一节。
 * ③ ⚠️ **板块内不许监听 `langchange`**：框架层 `apply(document)` 之后会重跑 `onShow()`。
 *
 * ── 🔴 两条通道必须完全平级（agnes 设计 §10.3 第 2 条）────────────────────────
 *
 * 两张通道卡由**同一个** `channelCard()` 建出来。这不是省代码：
 * 两段各写各的迟早会分叉（一边多一行说明、一边少一颗按钮），而**任何不对称都会被
 * 读成排名**。从同一个函数建出来时，不对称在结构上就是不可表达的 —— 除非有人专门
 * 加一个 `if (channel === …)`，而那一行在评审里看得见。
 * ⚠️ 同一条线的 CSS 侧：`css/sections-registrar.css` 里**不许出现
 * `[data-channel="…"]` 选择器**。本文件因此**连那个属性都不发** —— 没有任何渲染
 * 需要它，而属性一旦在 DOM 上，写一条按通道分叉的样式就只差一行。
 * ⚠️ 顺序取自本文件的 `CHANNELS`（**字母序，唯一可辩护的中立规则**），
 * 不取响应里对象键的顺序 —— 后者改后端一行就能悄悄改变谁排在前面。
 */
import { api, ApiError } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, toast, openModal } from "./ui.js";

/* ============================================================
   常量
   ============================================================ */

/**
 * 两条邮箱通道。🔴 **字母序**，见文件头。
 * ⚠️ 06 §5.3 把这张表归进 A 档（该由 `GET /admin/api/registrar/status` 的 DTO 给），
 * 但 Rust 侧那份 DTO 在 P1 **结构上不存在**（`src/registrar/mod.rs` 文件头：
 * 不建任何占位类型）。⇒ 本轮它暂住在这里，**已如实登记为欠账**。
 */
const CHANNELS = ["moemail", "yyds"];

/**
 * 通道的**展示名**。🔴 **刻意不进 i18n 字典**：它们是两家服务的专名
 * （MoeMail / YYDS），五种语言下逐字相同 —— 而「未翻译泄漏」那格判据
 * （`src/webui/mod.rs` 的 `no_untranslated_leakage`）会把这种词条判红，
 * 豁免名单又是空的。把专名当数据渲染，比给它造五份假翻译诚实。
 */
const CHANNEL_LABEL = { moemail: "MoeMail", yyds: "YYDS" };

/**
 * 「地址事实」那一行。**它是两张卡之间唯一的不对称**（agnes 设计 §10.3 第 5 条），
 * 而这是**同一个字段位置上的两句不同事实**，不是「一边有一边没有」——
 * 后者才会被读成排名。
 */
const CHANNEL_FACT_KEY = {
  moemail: "section.reg.channel.fact.moemail",
  yyds: "section.reg.channel.fact.yyds",
};

/**
 * 一轮补池里单条失败的归因。
 *
 * 🔴 **表外的取值原样显示出来**（`section.reg.fail.unknownReason`），
 * 而不是写死一句「未知」：一条本来能被 grep 到的归因被折叠成「未知」之后，
 * 面板与事件板块之间那条线索就断了。
 */
const FAIL_KEY = {
  domain_blocked_all: "section.reg.fail.domainBlockedAll",
  upstream_error: "section.reg.fail.upstreamError",
  code_timeout: "section.reg.fail.codeTimeout",
  register_failed: "section.reg.fail.registerFailed",
  login_failed: "section.reg.fail.loginFailed",
  key_failed: "section.reg.fail.keyFailed",
  provider_error: "section.reg.fail.providerError",
  network_error: "section.reg.fail.networkError",
  rate_limited: "section.reg.fail.rateLimited",
  provider_missing: "section.reg.fail.providerMissing",
  round_crashed: "section.reg.fail.roundCrashed",
  key_suspicious: "section.reg.fail.keySuspicious",
  upstream_backoff: "section.reg.fail.upstreamBackoff",
};

/**
 * 后端**拒绝**一次调用时顶层那个 `reason`（02 §9.15 / §9.17 的拒绝支）。
 *
 * ⚠️⚠️ **`registrar_blocked` 绝不许并进 `registrar_disabled`**：后者逐字写着
 * 「请先在设置里打开它」，而这一档的开关**明明是开的**（是这份配置装不起来）——
 * 照那句话去做只会让运维把一个已经打开的开关再点一遍。
 * ⚠️ 出站探测护栏那两档（`probe_in_flight` / `probe_cooldown`）**必须都在表里**：
 * 漏掉的话它们退回一句通用文案，于是「刚测过，隔几秒再来」与「这条通道真的连不上」
 * 在面板上长得一模一样 —— 而运维恰恰会在失败之后立刻重试，也就是必然撞上这一格。
 */
const REFUSE_KEY = {
  registrar_disabled: "section.reg.refuse.disabled",
  registrar_blocked: "section.reg.refuse.blocked",
  unknown_channel: "section.reg.refuse.unknownChannel",
  channel_not_configured: "section.reg.refuse.channelNotConfigured",
  not_wired: "section.reg.refuse.notWired",
  tend_in_flight: "section.reg.refuse.tendInFlight",
  locked: "section.reg.refuse.locked",
  manual_cooldown: "section.reg.refuse.manualCooldown",
  write_budget_exhausted: "section.reg.refuse.budgetExhausted",
  probe_in_flight: "section.reg.refuse.probeInFlight",
  probe_cooldown: "section.reg.refuse.probeCooldown",
};

/**
 * 跨轮退避的三档。
 * ⚠️ **三档文案分开，而且三档都要明说「换邮箱通道逃不掉」**：限流发生在
 * 「出口 IP → 上游」这条边上，与用哪条邮箱通道无关。不说这一句，运维的第一反应
 * 就是去切通道 —— 切完照样被限。
 */
const BACKOFF_KEY = {
  edge: "section.reg.backoff.edge",
  app: "section.reg.backoff.app",
  cluster: "section.reg.backoff.cluster",
};

/* ============================================================
   状态
   ============================================================ */

let nodes = null;
/** `"loading"` | `"notWired"` | `"error"` | `"ready"`，见文件头。 */
let phase = "loading";
/** 最近一次成功的 `/registrar/status`（已窄化）；`null` = 还没有过一次成功。 */
let data = null;
let abort = null;
/**
 * 现在停在哪一页。**模块级状态，不从 DOM 现读**：`onShow()` 会在切板块、切语言时
 * 被重跑，那时要知道该拉哪一页的数据，而从 DOM 反推「哪颗 tab 的 `aria-selected`
 * 是 true」就是把同一份状态存了第二处。
 */
let activeTab = "status";
/** 设置分页那份配置卡（由设置板块供给，见 `mountSettings()`）。 */
let configPanel = null;
let configPanelTried = false;

/* ============================================================
   本地化格式（06 §5.3 的 B 档：`Intl` 只有浏览器有，留在 JS）
   ⚠️ **这几个函数里不许有「该显示哪一档」的分支** —— 那一半是 A 档。
   ⚠️ 已登记的欠账：`js/pure/format.mjs`（P-9 的 B 档归宿）本轮还不存在，
   所以这几个小函数暂住在板块文件里，`js/sec-accounts.js` 里有形近的另一份。
   ============================================================ */

function lang() { return document.documentElement.lang || "zh-CN"; }

function fmtInt(n) { return new Intl.NumberFormat(lang()).format(n); }

function fmtInstant(ms) { return new Date(ms).toLocaleString(lang()); }

function fmtDuration(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return t("section.reg.durationDays", { d, h });
  if (h > 0) return t("section.reg.durationHours", { h, m });
  return t("section.reg.durationMinutes", { m, s: s % 60 });
}

/**
 * 「没读出来」那一格（EM DASH，U-4）。
 * 🔴 **与「读成功但这一段真的没有样本」（EN DASH）不是同义词**，别合并：
 * 前者是一句关于我们自己的话，后者是一句关于数据的话，处置方向完全不同。
 */
function unknownSpan() {
  return el("span", { class: "usage-unknown", title: t("section.reg.unknownHint") }, "—");
}

/* ============================================================
   窄化（**读不出来一律 `null`，不是 0、不是空数组**）
   ============================================================ */

function asObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
}

/** 有限数才算数。`NaN` / `Infinity` / 字符串一律 `null`。 */
function num(v) { return typeof v === "number" && Number.isFinite(v) ? v : null; }

/** 严格布尔。缺字段 / 别的类型一律 `null`（**不是 `false`**）。 */
function bool(v) { return typeof v === "boolean" ? v : null; }

function str(v) { return typeof v === "string" ? v : null; }

/**
 * 把 `/registrar/status` 的响应窄化。
 *
 * ⚠️ 🔴 **逐块降级**（02 §9.16 的原话）：`pool` / `manual` / `domains` / `backoff` /
 * `history` 每一块各自可能是 `null`，**坏掉的那一块不许把整份响应判死**，
 * 也不许被伪造成一份空壳（一个 `{target:0,counted:0}` 在屏幕上与「目标就是 0」
 * 一模一样）。这与 playground 那边「一条格式不对就整份判死」的口径**相反**，
 * 理由也相反：那边整份是一个不可分的目录，这边整份本来就是几块独立的读。
 */
function narrow(body) {
  const b = asObject(body);
  if (b === null) return null;

  const chIn = asObject(b.channels);
  const channels = {};
  for (const c of CHANNELS) {
    const one = chIn === null ? null : asObject(chIn[c]);
    channels[c] = one === null
      ? { configured: null, selected: null }
      : { configured: bool(one.configured), selected: bool(one.selected) };
  }

  const poolIn = asObject(b.pool);
  const manualIn = asObject(b.manual);
  const domainsIn = asObject(b.domains);
  const backoffIn = asObject(b.backoff);
  const historyIn = asObject(b.history);

  return {
    enabled: bool(b.enabled),
    blocked: bool(b.blocked),
    channel: str(b.channel),
    channels,
    pool: poolIn === null ? null : {
      target: num(poolIn.target), counted: num(poolIn.counted),
      gap: num(poolIn.gap), fresh: num(poolIn.fresh), mintBatch: num(poolIn.mintBatch),
    },
    lockedUntil: num(b.lockedUntil),
    manual: manualIn === null ? null : {
      remaining: num(manualIn.remaining), perDay: num(manualIn.perDay),
      resetAt: num(manualIn.resetAt), cooldownUntil: num(manualIn.cooldownUntil),
      retryAfterMs: num(manualIn.retryAfterMs),
    },
    domains: domainsIn === null ? null : {
      ok: num(domainsIn.ok), blocked: num(domainsIn.blocked),
      suspect: num(domainsIn.suspect), unknown: num(domainsIn.unknown),
      updatedAt: num(domainsIn.updatedAt),
    },
    backoff: backoffIn === null ? null : {
      kind: str(backoffIn.kind), until: num(backoffIn.until),
      retryAfterMs: num(backoffIn.retryAfterMs),
    },
    history: historyIn === null ? null : {
      entries: Array.isArray(historyIn.entries) ? historyIn.entries : [],
      // ⚠️ 环里有几条**读不出来**的记录，是一句要说出去的话（见 `renderHistory`）。
      malformed: num(historyIn.malformed),
    },
  };
}

/* ============================================================
   相位与拒绝支
   ============================================================ */

/** 一次失败的接口调用是不是「这个部署没接注册机」。 */
function isNotWired(e) {
  return e instanceof ApiError && e.status === 503
    && asObject(e.body) !== null && e.body.reason === "not_wired";
}

/**
 * 从一次失败的接口调用里取出后端给的顶层 `reason` 对应的文案键。
 * **拿不到就 `null` ⇒ 调用方用一句通用文案，不猜。**
 */
function refuseKeyOf(e) {
  const b = e instanceof ApiError ? asObject(e.body) : null;
  if (b === null) return null;
  const key = REFUSE_KEY[b.reason];
  return key === undefined ? null : key;
}

/* ============================================================
   渲染 · 顶部 TAB
   ============================================================ */

/**
 * 收起 / 展开一个分页。
 *
 * ⚠️ **两条一起写**：`aria-hidden` 是 `css/sections-registrar.css` 认的那一条
 * （`.reg-pane[aria-hidden="false"] { display: block }`），`hidden` 属性是
 * **样式表没加载出来时**的兜底 —— 那一刻两页会同时铺在屏幕上，而
 * 「注册机的设置摊开在运行状态下面」看起来像一个真实的布局缺陷，很难联想到 CSS 404。
 */
function setPaneHidden(pane, hidden) {
  pane.setAttribute("aria-hidden", hidden ? "true" : "false");
  if (hidden) pane.setAttribute("hidden", "");
  else pane.removeAttribute("hidden");
}

/**
 * 板块顶部那条 TAB。
 *
 * 🔴 **组件复用 kiro 的 `.login-tabs/.login-tab/.login-pane`**（06 §2.12：它与
 * agnes 的 `.tabs/.tab` 是同一个组件、同一种视觉 —— 底边一条线，选中项用 2px
 * 主色下边框把那条线顶开），只换 `.reg-` 前缀，**别新造第二套**。
 *
 * ⚠️ **可及性按 WAI-ARIA 的 tabs 模式做全，不只是「两颗长得像标签的按钮」**：
 * `role="tablist"/"tab"/"tabpanel"` + `aria-selected` + `aria-controls` /
 * `aria-labelledby` 把「这是一组分页、现在选中哪一页、这一页对应哪块内容」
 * 告诉读屏器；**roving tabindex**（选中的那颗 `tabindex="0"`、其余 `-1`）
 * 让 Tab 键一次跳过整组，组内换页走左右方向键（外加 Home / End）。
 * 不这么做的话，纯键盘用户要按 N 次 Tab 才能走出这条标签栏。
 *
 * ⚠️ **只拦截这四个键，别的一概放行**：吞掉 Tab / Enter / Space 会把这条标签栏
 * 变成一个键盘出不去的陷阱。
 */
function buildTabs(host, tabs, onSelect) {
  const list = el("div", { class: "reg-tabs", role: "tablist" });
  const btns = [];
  const select = (idx, focus) => {
    const changed = btns.findIndex((b) => b.getAttribute("aria-selected") === "true") !== idx;
    btns.forEach((b, i) => {
      b.setAttribute("aria-selected", i === idx ? "true" : "false");
      b.setAttribute("tabindex", i === idx ? "0" : "-1");
      setPaneHidden(tabs[i].pane, i !== idx);
    });
    if (focus) btns[idx].focus();
    if (changed) onSelect(tabs[idx].name);
  };
  tabs.forEach((tab, i) => {
    const btn = elI18n("button", tab.labelKey, {
      type: "button", class: "reg-tab", role: "tab",
      id: `reg-tab-${tab.name}`, "aria-controls": `reg-pane-${tab.name}`,
      "aria-selected": i === 0 ? "true" : "false", tabindex: i === 0 ? "0" : "-1",
    });
    btn.addEventListener("click", () => { select(i, false); });
    btn.addEventListener("keydown", (ev) => {
      const step = { ArrowRight: 1, ArrowLeft: -1 }[ev.key];
      if (step !== undefined) {
        ev.preventDefault();
        select((i + step + tabs.length) % tabs.length, true);
        return;
      }
      if (ev.key === "Home") { ev.preventDefault(); select(0, true); }
      else if (ev.key === "End") { ev.preventDefault(); select(tabs.length - 1, true); }
    });
    btns.push(btn);
    list.appendChild(btn);
    tab.pane.setAttribute("class", "reg-pane");
    tab.pane.setAttribute("role", "tabpanel");
    tab.pane.setAttribute("id", `reg-pane-${tab.name}`);
    tab.pane.setAttribute("aria-labelledby", `reg-tab-${tab.name}`);
    setPaneHidden(tab.pane, i !== 0);
  });
  host.appendChild(list);
  for (const tab of tabs) host.appendChild(tab.pane);
}

/* ============================================================
   渲染 · 运行状态页
   ============================================================ */

function statTile(labelKey, value, tone, subNode) {
  const card = el("div", { class: `card acct-stat-card${tone ? ` ${tone}` : ""}` });
  card.appendChild(elI18n("div", labelKey, { class: "acct-stat-label" }));
  const n = el("div", { class: "acct-stat-num" });
  if (value === null) n.appendChild(unknownSpan());
  else n.textContent = fmtInt(value);
  card.appendChild(n);
  if (subNode) {
    const sub = el("div", { class: "acct-stat-sub" });
    sub.appendChild(subNode);
    card.appendChild(sub);
  }
  return card;
}

function svcRow(labelKey, valueNode) {
  const row = el("div", { class: "svc-row" });
  const left = el("div", { class: "svc-row-left" });
  left.appendChild(elI18n("div", labelKey, { class: "svc-row-label" }));
  const val = el("div", { class: "svc-row-val" });
  val.appendChild(valueNode);
  left.appendChild(val);
  row.appendChild(left);
  return row;
}

/**
 * 一张通道卡。🔴 **两条通道走同一条代码路径**，见文件头。
 *
 * 卡里恰好三行 + 一句地址事实 + 一颗「测试连接」+ 一条恒占位的结果行。
 */
function channelCard(channel) {
  const card = el("div", { class: "card svc-card" });
  const head = el("div", { class: "svc-card-head" });
  // 专名当数据渲染（见 `CHANNEL_LABEL` 上方）。
  head.appendChild(el("span", null, CHANNEL_LABEL[channel]));
  card.appendChild(head);

  const view = data === null ? { configured: null, selected: null } : data.channels[channel];

  // 「本次使用 / 未使用」。**从前叫「角色」**（主通道 / 备用通道）——
  // 「角色」这个词本身就在暗示排名，而两条通道是二选一。
  const inUse = el("span");
  if (view.selected === null) inUse.appendChild(unknownSpan());
  else inUse.textContent = t(view.selected ? "section.reg.channel.inUse" : "section.reg.channel.unused");
  card.appendChild(svcRow("section.reg.channel.inUseLabel", inUse));

  const creds = el("span");
  if (view.configured === null) creds.appendChild(unknownSpan());
  else creds.textContent = t(view.configured ? "section.reg.channel.credsYes" : "section.reg.channel.credsNo");
  card.appendChild(svcRow("section.reg.channel.creds", creds));

  card.appendChild(elI18n("p", CHANNEL_FACT_KEY[channel], { class: "mng-inline-note" }));

  // 🔴 **恒占一行高度**（`.reg-test-result` 的 `min-height`），哪怕还没测过：
  //    不占位的话测完一条之后两张对称的卡会一高一低，而「哪条更高」在视觉上
  //    就是一次排序暗示。
  const result = el("p", { class: "reg-test-result", role: "status" });
  const test = elI18n("button", "section.reg.channel.test", { type: "button", class: "btn btn-sm btn-outline" });
  test.addEventListener("click", () => { void runChannelTest(channel, test, result); });
  card.appendChild(test);
  card.appendChild(result);
  return card;
}

/**
 * 跑一次通道连通性测试。**这颗按钮每点一次就向邮箱服务发一次真实请求。**
 *
 * ⚠️ **在飞期间禁用它**，理由不是「后端没有护栏」——后端有（在途去重 + 最小间隔，
 * 同一条通道在最小间隔内的第二次点击拿到 429）。准确的说法是**各自还挡住了对方
 * 挡不住的那一半**：护栏挡得住脚本（前端禁用挡不住），前端禁用挡得住那句无谓的
 * 「请稍后再试」（护栏挡不住，而运维什么错都没犯）。
 * ⚠️ **两者都不是「连点已经解决了」**：护栏是进程内的，跨副本无效。
 */
async function runChannelTest(channel, button, resultNode) {
  button.disabled = true;
  resultNode.className = "reg-test-result";
  resultNode.textContent = t("section.reg.channel.testing");
  try {
    const res = await api.post(`/registrar/channels/${encodeURIComponent(channel)}/test`);
    const view = channelTestResult(res);
    resultNode.className = `reg-test-result ${view.tone}`;
    // 失败态另配一个**非颜色**线索（WCAG 1.4.1）：色条只是加强。
    resultNode.textContent = (view.tone === "is-bad" ? "✕ " : view.tone === "is-ok" ? "✓ " : "")
      + t(view.key, view.params);
  } catch (e) {
    const key = refuseKeyOf(e);
    resultNode.className = "reg-test-result is-bad";
    resultNode.textContent = `✕ ${key === null ? t("section.reg.channel.testError") : t(key)}`;
    // 🔴 这条端点回 `not_wired` 说明整个部署的相位就是 `notWired`，
    //    把板块切过去 —— 否则屏幕上会同时有「补池在跑」与「没接注册机」两句话。
    if (isNotWired(e)) { phase = "notWired"; data = null; render(); }
  } finally {
    button.disabled = false;
  }
}

/**
 * 一次通道测试的结果该说哪句话（02 §9.17 的响应体）。
 *
 * ⚠️ **`domains === 0` 与「读不到几个」是两回事**：只有前者才能得出
 * 「这条通道现在补不了池」这个结论，后者只是我们没数出来。
 * ⚠️ **默认那一档是「没验」**：`credentials === "accepted"` 是唯一进得了绿灯的取值
 * （判据写成 `!== "accepted"` 而不是 `=== "rejected"`，缺字段同样不许被读成验过了）。
 * 🔴 **建出来了却删不掉（`cleaned !== true`）要报黄并说出去**：那个临时邮箱占着
 * 这条通道的活跃邮箱名额，而名额是补池能不能继续工作的前提；静默的残留会在几天后
 * 以「补池突然全失败」的形态炸出来，那时没人会想到是这颗按钮留下的。
 * ⚠️ `not_attempted` **自成一档**：一次上游请求都没发出去，要去看的是本网关自己的
 * 配置，与下面三档「上游怎么答的」排查方向正好相反。🔴 不许并进任何上游档。
 */
function channelTestResult(res) {
  const r = asObject(res);
  if (r === null) return { key: "section.reg.channel.testError", params: {}, tone: "is-bad" };
  const latency = num(r.latencyMs);
  const ms = latency === null ? "—" : latency;

  if (r.ok === true) {
    const domains = num(r.domains);
    if (domains === 0) {
      return { key: "section.reg.channel.testOkNoDomains", params: { ms }, tone: "is-bad" };
    }
    const params = { domains: domains === null ? "—" : domains, ms };
    if (r.credentials !== "accepted") {
      return { key: "section.reg.channel.testOkUnverified", params, tone: "" };
    }
    if (r.cleaned !== true) {
      return { key: "section.reg.channel.testOkDirty", params, tone: "" };
    }
    return { key: "section.reg.channel.testOk", params, tone: "is-ok" };
  }

  const status = num(r.status);
  if (r.reason === "not_attempted") {
    return { key: "section.reg.channel.testNotAttempted", params: { ms }, tone: "is-bad" };
  }
  if (r.reason === "credentials_rejected") {
    return { key: "section.reg.channel.testRejected", params: { status: status === null ? "—" : status }, tone: "is-bad" };
  }
  if (r.reason === "rate_limited") {
    return { key: "section.reg.channel.testRateLimited", params: {}, tone: "is-bad" };
  }
  // ⚠️⚠️ 后端拿不到状态码时**不带** `status`（它不伪造兜底值），两句话因此分开。
  //    **这一档不等于「请求压根没走通」**：两个适配器都在 2xx 之后解析正文，
  //    上游回 200 + 非 JSON 正文（反代/CDN 错误页那一族）时同样没有 `status`，
  //    而那一次请求发出去了、上游也答了。文案里那张清单因此不是穷尽的候选。
  if (status === null) {
    return { key: "section.reg.channel.testFailedNoStatus", params: { ms }, tone: "is-bad" };
  }
  return { key: "section.reg.channel.testFailed", params: { status, ms }, tone: "is-bad" };
}

/**
 * 状态卡：三态 + 通道 + 锁 + 配额 + 退避。
 *
 * 🔴 **三态，不是两态**：关 / 开着在跑 / **开着但这份配置装不起来**。
 * 第三态压进前两态里的任何一个都是撒谎 —— 压成「已关闭」是对着一个亮着的开关说
 * 没打开，压成「已启用」是声称有一个在工作的注册机而补池一轮都没跑。
 */
function statusCard() {
  const card = el("div", { class: "card" });
  const head = el("div", { class: "card-header" });
  head.appendChild(elI18n("h3", "section.reg.state.title"));

  const badge = el("div", { class: "status-badge" });
  badge.appendChild(el("span", { class: "status-dot" }));
  const text = el("span", { class: "status-text" });
  if (data === null || data.enabled === null) {
    badge.className = "status-badge is-unknown";
    text.textContent = t("section.reg.state.unknown");
  } else if (data.enabled === false) {
    badge.className = "status-badge is-unknown";
    text.textContent = t("section.reg.state.off");
  } else if (data.blocked === true) {
    badge.className = "status-badge is-degraded";
    text.textContent = t("section.reg.state.blocked");
  } else {
    badge.className = "status-badge";
    text.textContent = t("section.reg.state.on");
  }
  badge.appendChild(text);
  head.appendChild(badge);
  card.appendChild(head);

  const grid = el("div", { class: "info-grid" });
  const item = (labelKey, node) => {
    const box = el("div", { class: "info-item" });
    box.appendChild(elI18n("div", labelKey, { class: "info-label" }));
    const v = el("div", { class: "info-value" });
    v.appendChild(node);
    box.appendChild(v);
    return box;
  };

  const chan = el("span");
  if (data === null) chan.appendChild(unknownSpan());
  else if (data.channel === null) chan.textContent = t("section.reg.channel.none");
  else chan.textContent = CHANNEL_LABEL[data.channel] || data.channel;
  grid.appendChild(item("section.reg.channel.label", chan));

  // 手动补池配额：⚠️ **`remaining` 在点之前就显示**，不是等到点不动了才说 ——
  // 后者等于让运维毫不知情地撞上一堵墙。
  const quota = el("span");
  const m = data === null ? null : data.manual;
  if (m === null || m.remaining === null || m.perDay === null) quota.appendChild(unknownSpan());
  else quota.textContent = t("section.reg.quota", { remaining: fmtInt(m.remaining), perDay: fmtInt(m.perDay) });
  grid.appendChild(item("section.reg.quotaLabel", quota));
  card.appendChild(grid);

  // 「开着却一条通道都没选」。**只在这一种组合下出现**：注册机整个关着时该说的是
  // 「已关闭」，而不是催人去选通道。
  if (data !== null && data.enabled === true && data.channel === null) {
    card.appendChild(elI18n("p", "section.reg.emptyChannel", { class: "banner banner-warn" }));
  }

  // 🔴 **倒计时用相对量、几点恢复用绝对量，两个都由服务端给** ——
  //    绝不拿本地时钟去减服务端时刻（两者不同源，浏览器差一分钟就会印出负数）。
  if (m !== null && m.cooldownUntil !== null && m.retryAfterMs !== null) {
    card.appendChild(el("p", { class: "mng-inline-note" },
      t("section.reg.cooldown", { at: fmtInstant(m.cooldownUntil), left: fmtDuration(m.retryAfterMs) })));
  }
  if (data !== null && data.lockedUntil !== null) {
    card.appendChild(el("p", { class: "mng-inline-note" },
      t("section.reg.locked", { at: fmtInstant(data.lockedUntil) })));
  }

  // 退避横幅**只在真的还在退避里时出现**：给一个已经过去的时刻会渲染出一个恒为 0
  // 的假倒计时（与上面那条冷却同一条纪律）。
  const b = data === null ? null : data.backoff;
  if (b !== null && b.until !== null && b.retryAfterMs !== null && BACKOFF_KEY[b.kind] !== undefined) {
    card.appendChild(el("p", { class: "banner banner-warn reg-backoff-note" },
      t(BACKOFF_KEY[b.kind], { left: fmtDuration(b.retryAfterMs), at: fmtInstant(b.until) })));
  }
  return card;
}

/**
 * 池子四格。
 * 🔴 **`counted` 刻意不叫 `available`**（02 §9.16）：它数的是**占名额数**，
 * 被停用的与正在冷却的 key 都算在里面，而这两种恰恰都不能打上游。
 * 真正的可用数是并列的 `fresh` —— 两格都画，合并成一格就是把两个不同的事实
 * 说成同一个。
 */
function poolGrid() {
  const p = data === null ? null : data.pool;
  const grid = el("div", { class: "accounts-stat-grid" });
  grid.appendChild(statTile("section.reg.pool.target", p === null ? null : p.target, ""));
  grid.appendChild(statTile("section.reg.pool.counted", p === null ? null : p.counted, "",
    elI18n("span", "section.reg.pool.countedTip")));
  grid.appendChild(statTile("section.reg.pool.gap", p === null ? null : p.gap,
    p !== null && p.gap !== null && p.gap > 0 ? "warn" : ""));
  grid.appendChild(statTile("section.reg.pool.fresh", p === null ? null : p.fresh,
    p !== null && p.fresh !== null && p.fresh > 0 ? "ok" : ""));
  return grid;
}

/**
 * 域名台账四格。
 * ⚠️ **「判死」用的是中性措辞**：那是**我们**按一张启发式词表做的判定，不是上游的
 * 声明，而那张词表会误判。
 * ⚠️ 06 §2.12 的表写的是 `.key-stat-grid`，但那条规则是 `repeat(5, 1fr)`，
 * 而这里恰好四格 ⇒ 改用同族的 `.accounts-stat-grid`（`repeat(4, 1fr)`），
 * **已如实登记为对规格的一处偏离**。两者的卡片原语（padding / hover / 数字标度）
 * 逐条相同，并排看分不出。
 */
function domainGrid() {
  const d = data === null ? null : data.domains;
  const grid = el("div", { class: "accounts-stat-grid" });
  grid.appendChild(statTile("section.reg.domains.ok", d === null ? null : d.ok, "ok"));
  grid.appendChild(statTile("section.reg.domains.blocked", d === null ? null : d.blocked, "warn"));
  grid.appendChild(statTile("section.reg.domains.suspect", d === null ? null : d.suspect, "warn"));
  grid.appendChild(statTile("section.reg.domains.unknown", d === null ? null : d.unknown, ""));
  return grid;
}

/** 一轮补池的结果该说哪句话。🔴 **四条路必须分开说，不许合并成「失败」**。 */
function roundOutcome(r) {
  const skipped = bool(r.skipped);
  const attempted = num(r.attempted);
  const minted = num(r.minted);
  if (skipped === true) return { key: "section.reg.row.skipped", params: {} };
  if (attempted === null || minted === null) return { key: "section.reg.row.unreadable", params: {} };
  // ⚠️ **`attempted === 0` 不许说成「跑完了，0/0」**：它是「根本没跑起来」
  //    （池子已满 / 还在退避窗口里），而那是一句完全不同的话。
  if (attempted === 0) return { key: "section.reg.row.noAttempt", params: {} };
  return { key: "section.reg.row.minted", params: { minted: fmtInt(minted), attempted: fmtInt(attempted) } };
}

/** 这一轮的失败归因。**表外的取值原样显示，不冒充任何一档已知归因。** */
function roundFailures(r) {
  if (!Array.isArray(r.failures)) return [];
  const out = [];
  for (const item of r.failures) {
    const f = asObject(item);
    if (f === null) continue;
    const reason = str(f.reason);
    out.push({
      channel: str(f.channel) || "",
      key: reason === null ? null : (FAIL_KEY[reason] ?? null),
      reason: reason === null ? "" : reason,
    });
  }
  return out;
}

function failureBadge(f) {
  const text = f.key === null
    ? t("section.reg.fail.unknownReason", { reason: f.reason })
    : t(f.key);
  return el("span", { class: "badge badge-warning" }, f.channel === "" ? text : `${f.channel} · ${text}`);
}

function historyCard() {
  const card = el("div", { class: "card" });
  const head = el("div", { class: "card-header" });
  head.appendChild(elI18n("h3", "section.reg.history.title"));
  card.appendChild(head);

  const h = data === null ? null : data.history;
  if (h === null) {
    card.appendChild(elI18n("p", "section.reg.history.unreadable", { class: "empty-state text-danger" }));
    return card;
  }
  // 🔴 **环里有几条读不出来的记录要说出去**：静默跳过它们会让「补池最近很少跑」
  //    与「补池跑了但我们读不懂它留下的记录」在屏幕上变成同一句话。
  if (h.malformed !== null && h.malformed > 0) {
    card.appendChild(el("p", { class: "banner banner-warn" },
      t("section.reg.history.malformed", { count: fmtInt(h.malformed) })));
  }
  if (h.entries.length === 0) {
    card.appendChild(elI18n("p", "section.reg.history.empty", { class: "empty-state" }));
    return card;
  }

  const wrap = el("div", { class: "table-container" });
  const table = el("table", { class: "data-table" });
  const thead = el("thead");
  const hr = el("tr");
  for (const k of [
    "section.reg.col.at", "section.reg.col.trigger", "section.reg.col.channel",
    "section.reg.col.result", "section.reg.col.duration", "section.reg.col.failures",
  ]) hr.appendChild(elI18n("th", k));
  thead.appendChild(hr);
  table.appendChild(thead);

  const tbody = el("tbody");
  for (const item of h.entries) {
    const r = asObject(item);
    const tr = el("tr");
    if (r === null) {
      // 一条**整个读不出来**的记录：占一行，说清楚，不跳过。
      const td = el("td", { colspan: "6" });
      td.appendChild(elI18n("span", "section.reg.row.unreadable", { class: "reg-reason" }));
      tr.appendChild(td);
      tbody.appendChild(tr);
      continue;
    }

    const at = num(r.at);
    const atCell = el("td", { class: "kv-mono" });
    if (at === null) atCell.appendChild(unknownSpan());
    else atCell.textContent = fmtInstant(at);
    tr.appendChild(atCell);

    const trigger = str(r.trigger);
    const tgCell = el("td");
    if (trigger === "manual") tgCell.textContent = t("section.reg.trigger.manual");
    else if (trigger === "cron") tgCell.textContent = t("section.reg.trigger.cron");
    else tgCell.appendChild(unknownSpan());
    tr.appendChild(tgCell);

    // `primaryChannel` 是**这一轮实际用的那条**（手动补池可以指定单条通道），
    // 不是设置里选中的那条。空串 = 注册机当时关着。
    const pc = str(r.primaryChannel);
    const chCell = el("td");
    if (pc === null || pc === "") chCell.appendChild(unknownSpan());
    else chCell.textContent = CHANNEL_LABEL[pc] || pc;
    tr.appendChild(chCell);

    const outcome = roundOutcome(r);
    const resCell = el("td");
    resCell.appendChild(el("div", null, t(outcome.key, outcome.params)));
    tr.appendChild(resCell);

    const dur = num(r.durationMs);
    const durCell = el("td", { class: "kv-mono" });
    if (dur === null) durCell.appendChild(unknownSpan());
    else durCell.textContent = fmtDuration(dur);
    tr.appendChild(durCell);

    const failCell = el("td", { class: "table-actions" });
    const failures = roundFailures(r);
    if (failures.length === 0) failCell.appendChild(unknownSpan());
    else for (const f of failures) failCell.appendChild(failureBadge(f));
    tr.appendChild(failCell);

    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  wrap.appendChild(table);
  card.appendChild(wrap);
  return card;
}

/* ============================================================
   「立即补池」
   ============================================================ */

/**
 * 这一次点击最多消耗什么。**算不出来就 `null`，弹窗会如实说「说不准」而不是编一个数字。**
 * ⚠️ `pool.mintBatch` 报的是 `min(配置值, MANUAL_MINT_BATCH)`（02 §9.15），
 * 所以它就是**手动这一轮**的上界，不是定时那一轮的。
 */
function tendCost() {
  const p = data === null ? null : data.pool;
  if (p === null || p.mintBatch === null) return null;
  // 每铸一把 key 要建一个临时邮箱 ⇒ 两个数同源，但**分两格说**：
  // 邮箱名额与 key 数是两种不同的资源，耗尽时的处置也不同。
  return { keys: p.mintBatch, mailboxes: p.mintBatch };
}

/**
 * 「立即补池」的确认弹窗。🔴 **必须明示消耗**（agnes 设计 §10.2 第 3 条护栏）。
 *
 * ⚠️ **通道下拉初始是占位符，两条通道都不预选** —— 预选任何一条都会被读成排名。
 * 占位符本身是一个**有意义的选项**（「按设置里选中的那条」），不是空白项。
 * ⚠️ **显式写死初始值 `""`**，不靠「第一个 option 恰好是占位符」这个巧合：
 * 有人往 `CHANNELS` 前面插一条通道时，那个巧合会静默地把它变成预选项。
 */
function confirmAndTend() {
  const body = el("div");
  const cost = tendCost();
  body.appendChild(el("p", { class: "modal-desc" },
    cost === null
      ? t("section.reg.tend.confirmUnknown")
      : t("section.reg.tend.confirmMsg", { keys: fmtInt(cost.keys), mailboxes: fmtInt(cost.mailboxes) })));
  // 这颗按钮**会挂到整轮跑完**（最长约两分钟），用户点确认之前必须知道这件事。
  body.appendChild(elI18n("p", "section.reg.tend.confirmWait", { class: "modal-desc" }));

  const group = el("div", { class: "form-group" });
  group.appendChild(elI18n("label", "section.reg.tend.channelLabel", { class: "form-label" }));
  const select = el("select", { class: "form-control" });
  select.appendChild(el("option", { value: "" }, t("section.reg.tend.channelAny")));
  for (const c of CHANNELS) select.appendChild(el("option", { value: c }, CHANNEL_LABEL[c]));
  select.value = "";
  group.appendChild(select);
  body.appendChild(group);

  openModal("section.reg.tend.confirmTitle", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "common.confirm",
      onClick: () => { void startTend(select.value === "" ? null : select.value); },
    },
  ]);
}

/**
 * 真的补一轮。
 *
 * 🔴 **`outcome.kind` 三种不许被读成同一种「失败」**（02 §9.15）：
 * · `done` 且真的开跑过 —— 报真实的 铸出/尝试 数；
 * · `done` 但 `attempted === 0` —— **根本没跑起来**（池子已满 / 还在退避窗口里）；
 * · `skipped` —— 一次上游请求都没发（配置在两步之间被改掉）；
 * · `crashed` —— 整轮抛错，**可能已经建出临时邮箱**，处置与上面几条都不同。
 *
 * ⚠️ **网络错误那一支既不许渲染成成功、也不许渲染成失败**：请求断了不等于那一轮
 * 没跑（兜底网还攥着它）⇒ 说一句「结果未知，去补池历史里看」。
 * ⚠️ **拒绝的原因必须留在屏幕上（sticky）**：里面有「今天的额度用完了」这种
 * 四秒钟根本读不完、而且读漏了会让人反复点的信息。
 */
async function startTend(channel) {
  try {
    const r = await api.post("/registrar/tend", channel === null ? {} : { channel });
    const o = asObject(r) === null ? null : asObject(r.outcome);
    if (o === null) {
      toast(t("section.reg.tend.unknownOutcome"), "warn", { sticky: true });
    } else if (o.kind === "skipped") {
      toast(t("section.reg.tend.skipped"), "warn", { sticky: true });
    } else if (o.kind === "crashed") {
      toast(t("section.reg.tend.crashed"), "warn", { sticky: true });
    } else if (o.kind === "done") {
      const res = asObject(o.result);
      const out = res === null ? { key: "section.reg.row.unreadable", params: {} } : roundOutcome(res);
      const noRun = res !== null && num(res.attempted) === 0;
      toast(t(out.key, out.params), noRun ? "warn" : "ok", noRun ? { sticky: true } : undefined);
    } else {
      toast(t("section.reg.tend.unknownOutcome"), "warn", { sticky: true });
    }
  } catch (e) {
    if (isNotWired(e)) {
      // 诚实：这不是「补池失败了」，是「这个部署没有注册机」。
      toast(t("section.reg.notWired.toast"), "warn", { sticky: true });
      phase = "notWired";
      data = null;
      render();
      return;
    }
    const key = refuseKeyOf(e);
    if (key !== null) toast(t(key), "warn", { sticky: true });
    else if (e instanceof ApiError && e.status === 401) return; // 壳层已经接管
    else if (!(e instanceof ApiError)) toast(t("section.reg.tend.unknownResult"), "warn", { sticky: true });
    else toast(t("section.reg.tend.failed"), "warn", { sticky: true });
  }
  // 无论成败都重新拉一次：成功那一支的 `remaining` 与冷却时刻都变了，
  // 失败那一支也可能是因为别的副本刚抢走了锁。
  void load();
}

/* ============================================================
   渲染 · 总装
   ============================================================ */

function statusToolbar() {
  const bar = el("div", { class: "mng-toolbar" });
  const refresh = elI18n("button", "section.reg.refresh", { type: "button", class: "btn btn-sm btn-outline" });
  refresh.addEventListener("click", () => { void load(); });
  bar.appendChild(refresh);
  bar.appendChild(el("div", { class: "mng-spacer" }));
  const tend = elI18n("button", "section.reg.tend.button", { type: "button", class: "btn btn-primary" });
  tend.addEventListener("click", () => { confirmAndTend(); });
  bar.appendChild(tend);
  return bar;
}

/**
 * 🔴 「这个部署没有接注册机」那一屏。**不是错误，也不是功能正常。**
 *
 * 用 `.banner-info`（中性档，kiro 没有横幅组件，这是 agnes 明确更强的一条，U-3）
 * 而不是 `.banner-danger`：红色会让运维去查网络与日志，而那里什么都查不到 ——
 * 因为什么都没坏。
 * **这一屏上一颗动作按钮都没有**，理由见文件头。
 */
function notWiredPane() {
  const card = el("div", { class: "card" });
  const banner = el("div", { class: "banner banner-info" });
  banner.appendChild(elI18n("span", "section.reg.notWired.title"));
  card.appendChild(banner);
  card.appendChild(elI18n("p", "section.reg.notWired.body", { class: "mng-inline-note" }));
  card.appendChild(elI18n("p", "section.reg.notWired.next", { class: "mng-inline-note" }));
  const again = elI18n("button", "section.reg.notWired.recheck", { type: "button", class: "btn btn-sm btn-outline" });
  again.addEventListener("click", () => { void load(); });
  card.appendChild(again);
  return card;
}

function renderStatusPane() {
  const host = nodes.statusPane;
  host.textContent = "";

  if (phase === "loading") {
    const empty = el("div", { class: "empty-state" });
    // ⚠️ **「还在读」不套危险色**：否则每次进板块都先闪一次红。
    empty.appendChild(elI18n("span", "section.reg.loading", { class: "text-muted" }));
    host.appendChild(empty);
    return;
  }

  if (phase === "notWired") { host.appendChild(notWiredPane()); return; }

  if (phase === "error") {
    const card = el("div", { class: "card" });
    card.appendChild(elI18n("div", "section.reg.loadFailed", { class: "banner banner-danger" }));
    const again = elI18n("button", "section.reg.retry", { type: "button", class: "btn btn-sm btn-outline" });
    again.addEventListener("click", () => { void load(); });
    card.appendChild(again);
    host.appendChild(card);
    return;
  }

  host.appendChild(statusToolbar());
  host.appendChild(statusCard());
  host.appendChild(elI18n("h3", "section.reg.pool.title", { class: "mng-section-title" }));
  host.appendChild(poolGrid());

  host.appendChild(elI18n("h3", "section.reg.channels.title", { class: "mng-section-title" }));
  host.appendChild(elI18n("p", "section.reg.channels.hint", { class: "mng-inline-note" }));
  const channels = el("div", { class: "reg-channels" });
  for (const c of CHANNELS) channels.appendChild(channelCard(c));
  host.appendChild(channels);

  host.appendChild(elI18n("h3", "section.reg.domains.title", { class: "mng-section-title" }));
  host.appendChild(elI18n("p", "section.reg.domains.note", { class: "mng-inline-note" }));
  host.appendChild(domainGrid());
  if (data !== null && data.domains !== null && data.domains.updatedAt !== null) {
    host.appendChild(el("p", { class: "mng-inline-note" },
      t("section.reg.domains.updatedAt", { at: fmtInstant(data.domains.updatedAt) })));
  }

  host.appendChild(historyCard());
}

function render() {
  if (nodes === null) return;
  renderStatusPane();
}

/* ============================================================
   设置分页
   ============================================================ */

/**
 * 注册机那张**配置卡**由设置板块那份表单实现供给（`registrarConfigPanel`）：
 * 屏幕上它在本板块的「设置」分页里，而**实现只有一份** —— 同一个字段在整个面板里
 * 只有一个控件（06 §2.12 末行）。
 *
 * ⚠️ **动态 `import()` + 优雅回落，不是顶层 `import`**：本轮 Rust 侧
 * **根本没有注册机配置**（`src/registrar/mod.rs`：不建任何占位类型，
 * 一个空的 `RegistrarConfig` 会让「注册机配好了吗」在面板上得到一个假答案），
 * 所以设置板块很可能不导出这个符号。顶层 `import` 在那种情况下会让
 * **整个板块加载失败**（`app.js` 只会画一句「未接线」，连运行状态那一页也没了）——
 * 一处缺失连坐掉一整块功能。
 */
async function mountSettings() {
  if (configPanelTried) {
    // ⚠️ **挂载只做一次，`onShow()` 每次都要重跑。** 早一版这里直接 `return`，
    //    于是配置卡只在第一次切到这一页时拉过数据 —— 之后切走再回来、或者切一次
    //    语言（框架层会重跑 `onShow()`），看到的都是第一次那份快照，
    //    而屏幕上没有任何东西说它是旧的。
    if (configPanel !== null && configPanel.onShow) configPanel.onShow();
    return;
  }
  configPanelTried = true;
  try {
    const mod = await import("/admin/js/sec-settings.js");
    if (mod && mod.registrarConfigPanel && typeof mod.registrarConfigPanel.init === "function") {
      configPanel = mod.registrarConfigPanel;
    }
  } catch (e) {
    configPanel = null;
  }
  const host = nodes.settingsPane;
  host.textContent = "";
  if (configPanel === null) {
    // 🔴 诚实：**不画一张空表单**。一排能填能存、而后端根本没有这些字段的输入框，
    //    是本板块最容易造出来的那种假象。
    const card = el("div", { class: "card" });
    const banner = el("div", { class: "banner banner-info" });
    banner.appendChild(elI18n("span", "section.reg.settings.absent"));
    card.appendChild(banner);
    card.appendChild(elI18n("p", "section.reg.settings.absentBody", { class: "mng-inline-note" }));
    host.appendChild(card);
    return;
  }
  configPanel.init(host);
  if (configPanel.onShow) configPanel.onShow();
}

/* ============================================================
   网络
   ============================================================ */

async function load() {
  // **作废在飞请求**：不作废的话切回来时旧响应可能盖掉新数据。
  if (abort !== null) abort.abort();
  abort = new AbortController();
  const mine = abort;
  if (data === null) phase = "loading";
  render();
  try {
    const body = await api.get("/registrar/status", { signal: mine.signal });
    const narrowed = narrow(body);
    if (mine !== abort) return;
    if (narrowed === null) {
      // 读回来了但形状不对 ⇒ 这是一次**失败**，不是一份空状态。
      data = null;
      phase = "error";
    } else {
      data = narrowed;
      phase = "ready";
    }
  } catch (e) {
    if (e && e.name === "AbortError") return;
    if (mine !== abort) return;
    // **不伪造上一次的数据**：读失败就清空，不留着旧值假装它是新的。
    data = null;
    phase = isNotWired(e) ? "notWired" : "error";
  }
  render();
}

/* ============================================================
   板块契约
   ============================================================ */

export const registrarSection = {
  init(host) {
    host.textContent = "";
    const header = el("div", { class: "section-header" });
    header.appendChild(elI18n("h2", "section.reg.title", { class: "section-title" }));
    host.appendChild(header);
    host.appendChild(elI18n("p", "section.reg.desc", { class: "mng-inline-note" }));

    const statusPane = el("div");
    const settingsPane = el("div");
    buildTabs(host, [
      { name: "status", labelKey: "section.reg.tab.status", pane: statusPane },
      { name: "settings", labelKey: "section.reg.tab.settings", pane: settingsPane },
    ], (name) => {
      activeTab = name;
      // **换页之后立刻拉那一页自己的数据**：两页各有各的来源，不拉的话第二页
      // 第一次打开是空的，而运维只会以为「读不出来」。
      if (name === "settings") void mountSettings();
      else void load();
    });

    nodes = { statusPane, settingsPane };
  },

  onShow() {
    // **只拉当前这一页的数据**：站在「设置」分页上时拉一次 `/registrar/status`
    // 是一次白花的存储读，而本板块那条纪律（没有自动刷新）算的就是
    // 「人点一下才发生」。
    if (activeTab === "settings") { void mountSettings(); return; }
    void load();
  },

  onHide() {
    if (abort !== null) { abort.abort(); abort = null; }
    // 两页各有各的在飞请求，**两条都要作废** —— 只掐当前这一页的话，另一页那条
    // 晚到的响应会画到一棵已经切走的子树上。
    if (configPanel !== null && configPanel.onHide) configPanel.onHide();
  },
};
