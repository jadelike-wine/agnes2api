/**
 * 使用统计板块（`usage`）。
 *
 * 布局**照 kiro**（06 §3.2 ⑥）：
 *   `.stats-head` → 六张 `.stat-card`（`.usage-summary-grid`）→ `.usage-chart-panel`
 *   （手搓 inline SVG 柱图 + 浮动 tooltip）→ `.usage-table-panel`（日汇总表）
 *   → `.usage-drill`（下钻）。
 * 语义**照 agnes**：五态互斥、两种破折号、`≈`、`.usage-incomplete` 一条都不丢。
 *
 * # 🔴 四条贯穿本文件的纪律
 *
 * ① **一切来自接口的内容一律 `textContent`**（`el()` / `elI18n()` 内部就是它），
 *    全文件零 `innerHTML`。SVG 一律 `createElementNS`。
 *    ⚠️ kiro 的同一份文件里有 `els.chart.innerHTML = ''` 与 `tip.innerHTML = ''`
 *    两处 —— 那是它没有共用层、也没有 CSP 的结果。本文件用 `clear()`。
 *
 * ② 🔴 **「该显示哪一档」一个字都不在这里判。**
 *    后端 `GET /admin/api/usage` 的 `view` 块（`src/admin/usage.rs` 的
 *    `UsageViewWire`）已经把五态、六张卡的档位、每一行的状态全判好了。
 *    本文件只做「`kind` → 画什么」的搬运。
 *    ⚠️ 那套判定在 TS 版里是面板自己算的、**算错过五次**，每一次的形态都逐条记在
 *    `src/core/admin/view/usage.rs` 里。**别在这里写第二份**（哪怕只是一个
 *    `total.requests > 0 ? … : …`）。
 *
 * ③ 🔴 **三种「没有数字」必须画成三样东西**：
 *    · `value`          → 真的数字（**含 0**，那一次读成功了，我们确实知道是零）；
 *    · `none`           → EN DASH `–`（`.usage-none`）：读成功了，这一段真的没有样本；
 *    · `none-no-shards` → **同一根 EN DASH，换一句 tooltip**：一条分片都还没落盘，
 *                          「有没有样本」正是我们分不出来的那件事；
 *    · `unknown`        → EM DASH `—`（`.usage-unknown`）：这一块压根没读出来。
 *    ⚠️ **别把真零「修」成破折号**，那是反向的撒谎（`cell_kind` 的文档逐字写着）。
 *
 * ④ **板块内不许监听 `langchange`**：框架层会 `apply(document)` 之后重跑 `onShow()`。
 *
 * # ⚠️ 关掉的时候后端回 **200 + `tier:"off"`**，不是 404 不是 503
 *
 * 503 会让面板把「**运维没打开统计**」渲染成「**后端坏了**」，而这两件事的处置
 * 完全不同（去改一个环境变量 vs 去看容器日志）。⇒ 本文件**没有**「这条端点挂了」
 * 这种分支：`view.state === "off"` 是一条正常返回，画的是一句说明 + 六格 EM DASH。
 *
 * # ⚠️⚠️ 档位 → `(from, to)` 的映射：**唯一一个必须出现在前端的日长常数**
 *
 * 服务端**不认档位**（没有 `days` 参数），但两边必须对同一个映射有共识 ——
 * `src/admin/usage.rs::parse_range` 的文档把它钉死成
 * **`from = to − (N − 1) × 86400000`**（服务端按整 UTC 天取，而「N 天」含今天）。
 * 🔴 **差一天的后果不是「多一天数据」，是那句警告永久常驻**：发
 * `from = now − 30 × 86400000` 的话 `asked_from_day = 今天 − 30` 而保留期起点是
 * `今天 − 29` ⇒ **每一次点 30d 都 `clamped: true`**，面板于是每一次都说
 * 「这段时间早于保留期」——**说到运维不再看它**。
 * ⇒ [`DAY_MS`] 是这份共识的前端一半，不是一次随手的复制。
 *
 * # ⚠️ 「now」取的是**服务端**时钟，浏览器时钟只当第一次的种子
 *
 * 区间两端都由前端算，而 UTC 日的归属对**一天**这个粒度敏感：浏览器时钟偏一天
 * （虚拟机休眠、时区被改、用户手动调过）时，面板会去查一段**服务端根本没在看**
 * 的日子，然后如实报「这几天没有请求」——一句关于错误日子的真话。
 * ⇒ 第一次用 `Date.now()` 当种子，拿回响应后改用它自己的 `generatedAt`；
 *   若两者**落在不同的 UTC 日**，**补发一次**（每次进板块至多一次，见 `correcting`）。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n } from "./ui.js";

/* ============================================================
   常量
   ============================================================ */

/**
 * 一天。见文件头「档位 → (from, to) 的映射」那一段 —— 它是**契约的前端一半**，
 * 由 `src/admin/usage.rs::parse_range` 的文档与
 * `the_four_preset_ranges_are_not_clamped_but_an_off_by_one_day_always_is` 那格
 * 钉在另一头。⚠️ **别改成 `24 * 3600 * 1000` 之类的算式**：两边要能被同一次 grep 找到。
 */
const DAY_MS = 86400000;

/**
 * 四个档位。**与后端那格判据逐字同一组数**（`for n in [1i64, 3, 7, 30]`）。
 * 🔴 **没有「6h」这一档**（kiro 有）：本仓的用量桶粒度就是 **UTC 天**，
 * 一个 6 小时的按钮只能返回「今天」——装作能给出半天是撒谎。
 */
const RANGES = [1, 3, 7, 30];

/**
 * 图表的三种度量。都取自同一个 `UsageBucket`，零额外请求。
 *
 * ⚠️⚠️ **文案键一律写成字面量，不许用模板串拼**（`` `section.usg.metric.${m}` ``）。
 * 理由是硬的：`src/webui/mod.rs` 的 `referenced_keys()` 扫的是**双引号字面量**，
 * 拼出来的 key 它一个都看不见 ⇒ 那几条词条会被「字典里每个 key 都被引用过」
 * 那一格判成孤儿而当场红；反过来，拼错一个前缀也不会被「面板里的 key 必须在
 * 字典里」那一格抓住。**下面几张表都是同一条理由。**
 */
const METRICS = [
  { id: "requests", labelKey: "section.usg.metric.requests" },
  { id: "tokens", labelKey: "section.usg.metric.tokens" },
  { id: "errors", labelKey: "section.usg.metric.errors" },
];

/** 四个档位的文案键。见 `METRICS` 上方那段。 */
const RANGE_KEY = {
  1: "section.usg.range.d1",
  3: "section.usg.range.d3",
  7: "section.usg.range.d7",
  30: "section.usg.range.d30",
};

/**
 * 下钻的四个维度。**kiro 只有一维**（逐请求记录表），这四维是 agnes 更强的一面。
 * `col` 是那一维第一列的表头文案。
 */
const DIMS = [
  { id: "hours", labelKey: "section.usg.dim.hours", colKey: "section.usg.dim.hours.col" },
  { id: "byModel", labelKey: "section.usg.dim.byModel", colKey: "section.usg.dim.byModel.col" },
  { id: "byProtocol", labelKey: "section.usg.dim.byProtocol", colKey: "section.usg.dim.byProtocol.col" },
  { id: "byApiKey", labelKey: "section.usg.dim.byApiKey", colKey: "section.usg.dim.byApiKey.col" },
];

/**
 * 后端 `view.apiKeyLabelKeys` 给的是**它自己那套**文案键
 * （`src/core/admin/view/usage.rs::api_key_row_label_key` 回的
 * `usage.key.master` / `usage.key.unattributed` / `usage.key.other`），
 * 而本仓字典的命名空间登记表（`src/webui/mod.rs` 的 `NAMESPACES`）里**没有
 * `usage.` 这一条** ⇒ 那三个串直接当 key 用的话，`t()` 会把它们原样画上屏幕。
 *
 * ⚠️ **这张表是权宜之计，不是设计**：它让同一个决定有了第二份实现。
 * 正确的收口是让两边用同一套命名空间（改 Rust 那三个串、或把 `usage.` 加进
 * `NAMESPACES`），而那两份文件都不归本面改 ⇒ **登记为欠账**，见交付说明。
 * 🔴 认不出来的一律**照实画原始 id**（下面 `apiKeyLabel`），不猜、也不标「已删除」：
 * 一个已经被删掉的密钥与一个「这一刻那张表读不出来」的密钥在面板手上完全同形。
 */
const API_KEY_LABEL = {
  "usage.key.master": "section.usg.key.master",
  "usage.key.unattributed": "section.usg.key.unattributed",
  "usage.key.other": "section.usg.key.other",
};

/**
 * `note` 的九条 code → 文案键。**逐条一句话，绝不并成「没有数据」。**
 * 🔴 全集与 `src/admin/usage.rs::USAGE_NOTES` 一一对应；后端加第十条时这里认不出来，
 * 走 `section.usg.note.unknown`（**说「面板还不认识这条」，不假装看懂了**）。
 */
const NOTE_KEY = {
  tier2_off: "section.usg.note.tier2Off",
  clock_unavailable: "section.usg.note.clockUnavailable",
  read_failed: "section.usg.note.readFailed",
  range_clamped: "section.usg.note.rangeClamped",
  date_out_of_retention: "section.usg.note.dateOutOfRetention",
  no_shards: "section.usg.note.noShards",
  all_malformed: "section.usg.note.allMalformed",
  partial_malformed: "section.usg.note.partialMalformed",
  no_request_detail: "section.usg.note.noRequestDetail",
};

/** `view.noteSeverity` → 横幅变体类。横幅是 agnes 独有的组件（U-3）。 */
const BANNER_CLASS = { error: "banner banner-danger", warn: "banner banner-warn", info: "banner banner-info" };

/* ============================================================
   本地化格式（06 §5.3 的 B 档：`Intl` 只有浏览器有，留在 JS）
   ⚠️ **这几个函数里不许有「该显示哪一档」的分支** —— 那一半是 A 档，
   由 `view.cards.*` / `view.dayRows[i].state` 给出。
   ⚠️ 已登记的欠账：`js/pure/format.mjs`（P-9 的 B 档归宿）本轮还不存在，
   所以这几个小函数暂住在板块文件里，别的 `sec-*.js` 里有形近的另一份。
   ============================================================ */

function langOf() { return document.documentElement.lang || "zh-CN"; }

/** 千分位整数。`Intl` 按当前界面语言给分隔符。 */
function fmtInt(n) { return new Intl.NumberFormat(langOf()).format(n); }

/** 百分比，一位小数。 */
function fmtPercent(ratio) { return `${(ratio * 100).toFixed(1)}%`; }

/** 毫秒。**单位跟着数字走**，不放在表头 —— 表头那一格在破折号那几档下会骗人。 */
function fmtMs(ms) { return t("section.usg.msValue", { n: fmtInt(ms) }); }

/** `YYYY-MM-DD`（UTC）原样画。**不转本地时区**：桶就是按 UTC 日分的。 */
function fmtDate(s) { return s; }

/** 清空一个节点。**不用 `innerHTML = ""`**（见文件头纪律 ①）。 */
function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

/* ============================================================
   三种「没有数字」——纪律 ③ 的落点
   ============================================================ */

/**
 * 一格数字。`kind` 直接来自后端 DTO，本函数**不做任何判定**。
 *
 * `fmt` 只在 `value` 那一档被调用 —— 其余三档根本没有数字可格式化，
 * 传一个 0 进去格式化再丢掉，是把「伪造 0」这条路留在代码里。
 */
function cell(kind, value, fmt) {
  if (kind === "value") return el("span", null, fmt(value));
  if (kind === "unknown") return unknownCell();
  // `none` 与 `none-no-shards` **共用同一根 EN DASH，字形一个像素都不动**
  //（`cell_kind` 文档逐字写着这一条），只换 tooltip：前者是「确实没有样本」，
  // 后者是「有没有样本这件事我们不知道」。
  const node = el("span", { class: "usage-none" }, "–");
  node.setAttribute("title", t(kind === "none-no-shards" ? "section.usg.cell.noneNoShards" : "section.usg.cell.none"));
  return node;
}

/** 「这一块压根没读出来」。EM DASH + 警示色 + 字距（非颜色线索）。 */
function unknownCell() {
  const node = el("span", { class: "usage-unknown" }, "—");
  node.setAttribute("title", t("section.usg.cell.unknown"));
  return node;
}

/**
 * 计数类的 `≈`。**后端 `approximate` 恒为真**（Tier-2 的槽位是 last-write-wins，
 * 超过 2 个副本时同槽位互相覆盖），⇒ 每一个计数都要带这个标记。
 * 🔴 **只给计数类带，不给比率类带**：两个都带 `≈` 的比值仍然是一个有意义的比值。
 */
function approx(node, on) {
  if (!on) return node;
  const wrap = el("span", null);
  wrap.appendChild(el("span", { class: "approx" }, "≈"));
  wrap.appendChild(node);
  return wrap;
}

/* ============================================================
   状态
   ============================================================ */

const state = {
  rangeDays: 1,
  metric: "requests",
  /** 汇总响应整包（含 `view`）。`null` = 还没拉到 / 上一次失败。 */
  summary: null,
  /** 请求失败（网络 / 5xx / 400）时那句话的 key。**与 `tier:"off"` 不是一回事。** */
  failedKey: null,
  /**
   * 服务端时钟。第一次是 `null`（用 `Date.now()` 当种子），
   * 之后一律用上一次响应的 `generatedAt` —— 见文件头最后一段。
   */
  serverNow: null,
  /** 这一次进板块里补发过没有。**每次 `onShow()` 归零**，防止两个时钟来回打架。 */
  corrected: false,
  drill: { date: null, dim: "hours", data: null, failedKey: null, loading: false },
};

/** 在飞请求的作废控制。切板块 / 换档位 / 重新加载都顶掉上一发。 */
let listCtl = null;
let drillCtl = null;

const nodes = {};

/* ============================================================
   骨架
   ============================================================ */

/** 一张汇总卡。`tint` 走 `base.css` 的 `.stat-icon.primary/.success/...`。 */
function statCard(id, labelKey, emoji, tint) {
  const card = el("div", { class: "stat-card" });
  card.appendChild(el("div", { class: `stat-icon ${tint}`, "aria-hidden": "true" }, emoji));
  const info = el("div", { class: "stat-info" });
  const h3 = el("h3", { id });
  h3.appendChild(unknownCell());
  info.appendChild(h3);
  info.appendChild(elI18n("p", labelKey));
  card.appendChild(info);
  return card;
}

function build(node) {
  clear(node);

  const head = el("div", { class: "stats-head" });
  head.appendChild(elI18n("h2", "section.usg.title"));
  const toolbar = el("div", { class: "stats-toolbar" });
  nodes.refreshBtn = elI18n("button", "section.usg.refresh", { type: "button", class: "btn btn-sm btn-outline" });
  nodes.refreshBtn.addEventListener("click", () => load());
  toolbar.appendChild(nodes.refreshBtn);
  head.appendChild(toolbar);
  node.appendChild(head);

  // 横幅（note）。**恒在，靠 hidden 控制显隐** —— 每次重建一个横幅会让它在
  // 刷新的一瞬间跳一下，而它承载的正是「有件事你得知道」。
  nodes.banner = el("div", { class: "banner banner-info", hidden: true });
  node.appendChild(nodes.banner);

  // ── 六张汇总卡 ──────────────────────────────────────────────────────────
  const grid = el("div", { class: "stats-grid usage-summary-grid" });
  grid.appendChild(statCard("usgRequests", "section.usg.card.requests", "📨", "primary"));
  grid.appendChild(statCard("usgSuccess", "section.usg.card.successRate", "✅", "success"));
  grid.appendChild(statCard("usgErrors", "section.usg.card.errorRate", "⚠️", "danger"));
  grid.appendChild(statCard("usgLatency", "section.usg.card.avgLatency", "⏱️", "info"));
  grid.appendChild(statCard("usgTokens", "section.usg.card.tokens", "🔢", "warning"));
  grid.appendChild(statCard("usgStreaming", "section.usg.card.streaming", "📡", "info"));
  node.appendChild(grid);
  nodes.cards = {
    requests: grid.querySelector("#usgRequests"),
    successRate: grid.querySelector("#usgSuccess"),
    errorRate: grid.querySelector("#usgErrors"),
    avgLatency: grid.querySelector("#usgLatency"),
    tokens: grid.querySelector("#usgTokens"),
    streaming: grid.querySelector("#usgStreaming"),
  };

  // ── 图表面板 ────────────────────────────────────────────────────────────
  const panel = el("div", { class: "usage-chart-panel" });
  const controls = el("div", { class: "chart-controls" });

  const metricGroup = el("div", { class: "control-group" });
  metricGroup.appendChild(elI18n("label", "section.usg.metric"));
  nodes.metricBtns = el("div", { class: "btn-group" });
  for (const spec of METRICS) {
    const m = spec.id;
    const b = elI18n("button", spec.labelKey, { type: "button", class: "btn btn-sm btn-outline" });
    b.setAttribute("data-metric", m);
    b.addEventListener("click", () => {
      if (state.metric === m) return;
      state.metric = m;
      // 🔴 **换度量只重画，不重新请求**：三种度量取的是同一份 `UsageBucket`。
      paintControls();
      renderChart();
    });
    nodes.metricBtns.appendChild(b);
  }
  metricGroup.appendChild(nodes.metricBtns);
  controls.appendChild(metricGroup);

  const rangeGroup = el("div", { class: "control-group" });
  rangeGroup.appendChild(elI18n("label", "section.usg.timeRange"));
  nodes.rangeBtns = el("div", { class: "btn-group" });
  for (const n of RANGES) {
    const b = elI18n("button", RANGE_KEY[n], { type: "button", class: "btn btn-sm btn-outline" });
    b.setAttribute("data-range", String(n));
    b.addEventListener("click", () => {
      if (state.rangeDays === n) return;
      state.rangeDays = n;
      // 换档位要**收掉下钻**：那张表说的是某一天，而那一天可能已经不在新区间里。
      closeDrill();
      paintControls();
      load();
    });
    nodes.rangeBtns.appendChild(b);
  }
  rangeGroup.appendChild(nodes.rangeBtns);
  controls.appendChild(rangeGroup);
  panel.appendChild(controls);

  // `≈` 的那句说明。**恒显示**（后端 `approximate` 恒为真），不是一条偶发提示。
  nodes.approxNote = el("div", { class: "usage-fallback-note" });
  nodes.approxNote.appendChild(elI18n("span", "section.usg.approxNote"));
  panel.appendChild(nodes.approxNote);

  nodes.chart = el("div", { class: "chart-container" });
  panel.appendChild(nodes.chart);
  node.appendChild(panel);

  // ── 日汇总表 + 下钻 ─────────────────────────────────────────────────────
  const tablePanel = el("div", { class: "usage-table-panel" });
  tablePanel.appendChild(elI18n("h3", "section.usg.dailyTable", { class: "usage-panel-title" }));
  const wrap = el("div", { class: "usage-table-wrap" });
  const table = el("table", { class: "data-table usage-daily-table" });
  const thead = el("thead");
  const hrow = el("tr");
  hrow.appendChild(elI18n("th", "section.usg.col.date"));
  hrow.appendChild(elI18n("th", "section.usg.col.requests", { class: "num" }));
  hrow.appendChild(elI18n("th", "section.usg.col.success", { class: "num" }));
  hrow.appendChild(elI18n("th", "section.usg.col.errors", { class: "num" }));
  hrow.appendChild(elI18n("th", "section.usg.col.tokens", { class: "num" }));
  hrow.appendChild(elI18n("th", "section.usg.col.streaming", { class: "num" }));
  thead.appendChild(hrow);
  table.appendChild(thead);
  nodes.dailyBody = el("tbody");
  table.appendChild(nodes.dailyBody);
  wrap.appendChild(table);
  tablePanel.appendChild(wrap);

  nodes.drill = el("div", { class: "usage-drill", hidden: true });
  tablePanel.appendChild(nodes.drill);
  node.appendChild(tablePanel);

  // 浮动 tooltip。挂在板块根上而不是 `<body>`：切板块时它跟着一起被隐藏。
  nodes.tooltip = el("div", { class: "usage-tooltip" });
  node.appendChild(nodes.tooltip);

  paintControls();
}

/** 两组按钮的选中态。`.active` 由 `.chart-controls .btn-group .btn.active` 上色 + 加粗。 */
function paintControls() {
  for (const b of nodes.metricBtns.children) {
    b.classList.toggle("active", b.getAttribute("data-metric") === state.metric);
    b.setAttribute("aria-pressed", b.getAttribute("data-metric") === state.metric ? "true" : "false");
  }
  for (const b of nodes.rangeBtns.children) {
    const on = Number(b.getAttribute("data-range")) === state.rangeDays;
    b.classList.toggle("active", on);
    b.setAttribute("aria-pressed", on ? "true" : "false");
  }
}

/* ============================================================
   取数
   ============================================================ */

/** 这一次请求用哪个时刻当「now」。见文件头最后一段。 */
function clockBase() {
  return state.serverNow === null ? Date.now() : state.serverNow;
}

/**
 * 拉汇总。
 *
 * 🔴 **`from` / `to` 两端都发**，不省 `to`：省掉 `to` 的话服务端用自己的 now，
 * 而 `from` 是前端按自己那个 now 算的 —— 两个不同源的时刻夹出来的区间，
 * 跨度会比档位多一天或少一天，而多出来的那一天正好把 `clamped` 点亮。
 */
async function load() {
  if (listCtl) listCtl.abort();
  listCtl = new AbortController();
  const ctl = listCtl;

  const base = clockBase();
  const to = base;
  const from = to - (state.rangeDays - 1) * DAY_MS;
  setChartPlaceholder("section.usg.loading");
  try {
    const body = await api.get(`/usage?from=${from}&to=${to}`, { signal: ctl.signal });
    if (ctl !== listCtl) return; // 后一次赢
    state.summary = body;
    state.failedKey = null;

    // ── 时钟自纠：**每次进板块至多一次** ─────────────────────────────────
    // 判据是「两个时刻落在不同的 UTC 日」，不是「差了多少毫秒」：
    // 差 2 毫秒但恰好跨在日界上要纠，差 23 小时但同一天不必纠。
    const serverNow = typeof body.generatedAt === "number" ? body.generatedAt : null;
    const skewedDay = serverNow !== null && Math.floor(serverNow / DAY_MS) !== Math.floor(base / DAY_MS);
    state.serverNow = serverNow;
    if (skewedDay && !state.corrected) {
      state.corrected = true;
      render();       // 先把手上这份画出来，别让屏幕空一拍
      load();         // 再按服务端时钟补发一次
      return;
    }
  } catch (e) {
    if (ctl !== listCtl) return;
    if (e && e.status === 401) return; // 已经被送回登录闸了
    state.summary = null;
    // ⚠️ **这一档与 `tier:"off"` 不是一回事**：那一档是 200，说的是「没开」；
    //    这一档是请求本身没成功，说的是「我们不知道」。
    state.failedKey = e && typeof e.status === "number" && e.status > 0
      ? "section.usg.loadFailedHttp"
      : "section.usg.loadFailedNetwork";
  }
  render();
}

/* ============================================================
   渲染
   ============================================================ */

function render() {
  paintBanner();
  paintCards();
  renderChart();
  renderDaily();
}

/** 顶部横幅：请求失败 / `note` 两条来源，**失败优先**。 */
function paintBanner() {
  const b = nodes.banner;
  clear(b);
  if (state.failedKey) {
    b.className = "banner banner-danger";
    b.appendChild(elI18n("span", state.failedKey));
    b.hidden = false;
    return;
  }
  const s = state.summary;
  const severity = s && s.view ? s.view.noteSeverity : null;
  if (!s || !severity) { b.hidden = true; return; }
  b.className = BANNER_CLASS[severity] || BANNER_CLASS.warn;
  const key = NOTE_KEY[s.note] || "section.usg.note.unknown";
  b.appendChild(elI18n("span", key));
  // 🔴 **`complete: false` 单独再说一句**：「伪造的不只是 0，还有『这份数据是全的』
  //    这个印象」。它与 note 是两件事（partial_malformed 时两句都要出现）。
  if (s.view && s.view.complete === false) {
    b.appendChild(elI18n("span", "section.usg.incompleteNote", { class: "usage-incomplete" }));
  }
  b.hidden = false;
}

/** 六张卡。**档位一律取 `view.cards.*`**，本函数零判定（纪律 ②）。 */
function paintCards() {
  const s = state.summary;
  const view = s && s.view ? s.view : null;
  const total = s && s.total ? s.total : null;
  const cards = view ? view.cards : null;
  const incomplete = !!(view && view.complete === false);
  const isApprox = !!(s && s.approximate);

  const put = (name, kind, value, fmt, counting) => {
    const host = nodes.cards[name];
    clear(host);
    // 请求本身失败时六格全是 EM DASH：那一刻连 `view` 都没有。
    const node = cards ? cell(kind, value, fmt) : unknownCell();
    host.appendChild(counting && kind === "value" ? approx(node, isApprox) : node);
    host.classList.toggle("usage-incomplete", incomplete && kind === "value");
  };

  const req = total ? total.requests : 0;
  put("requests", cards && cards.requests, req, fmtInt, true);
  put("successRate", cards && cards.successRate, total ? total.success / (req || 1) : 0, fmtPercent, false);
  put("errorRate", cards && cards.errorRate, total ? total.errors / (req || 1) : 0, fmtPercent, false);
  // 🔴 平均延迟那个数**由后端算好**（`avgLatencyMs`）：`null` = 没有样本，不是零延迟。
  put("avgLatency", cards && cards.avgLatency, cards ? cards.avgLatencyMs : null, fmtMs, false);
  put("tokens", cards && cards.tokens, total ? total.tokensIn + total.tokensOut : 0, fmtInt, true);
  put("streaming", cards && cards.streaming, total ? total.streamingRequests : 0, fmtInt, true);
}

/* ---------- 图表（手搓 inline SVG，零依赖、零 innerHTML） ---------- */

const SVG_NS = "http://www.w3.org/2000/svg";

function svgEl(tag, attrs) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs || {})) node.setAttribute(k, String(v));
  return node;
}

function setChartPlaceholder(key) {
  if (!nodes.chart) return;
  clear(nodes.chart);
  const box = el("div", { class: "chart-loading" });
  box.appendChild(el("span", { class: "stats-spinner", "aria-hidden": "true" }));
  box.appendChild(elI18n("span", key));
  nodes.chart.appendChild(box);
}

/** 空图。`.empty-chart` 是 kiro 的那一族（大字形 + 一句说明）。 */
function setChartEmpty(key) {
  clear(nodes.chart);
  const box = el("div", { class: "empty-chart" });
  box.appendChild(el("div", { class: "empty-chart-icon", "aria-hidden": "true" }, "📊"));
  box.appendChild(elI18n("p", key));
  nodes.chart.appendChild(box);
}

/** 当前度量那一条登记。认不出来时回落第一条（**不返回 `undefined` 去炸渲染**）。 */
function metricSpec() {
  return METRICS.find((m) => m.id === state.metric) || METRICS[0];
}

/** 这根柱子画哪个数。三种度量取的都是同一个桶里的字段。 */
function metricValue(bucket) {
  if (state.metric === "tokens") return bucket.tokensIn + bucket.tokensOut;
  if (state.metric === "errors") return bucket.errors;
  return bucket.requests;
}

/** 纵轴上界取一个「整齐」的数（1 / 2 / 5 × 10^k）。 */
function niceCeil(v) {
  if (v <= 0) return 1;
  const mag = 10 ** Math.floor(Math.log10(v));
  const norm = v / mag;
  const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
  return nice * mag;
}

function shortNum(v) {
  if (v >= 1000) return `${Math.round(v / 100) / 10}k`;
  return String(Math.round(v * 10) / 10);
}

function renderChart() {
  const s = state.summary;
  if (state.failedKey) { setChartEmpty("section.usg.chart.unavailable"); return; }
  if (!s || !s.view) { setChartPlaceholder("section.usg.loading"); return; }
  // 🔴 五态里的前两档**没有柱子可画**，而两句话不一样。
  if (s.view.state === "off") { setChartEmpty("section.usg.chart.off"); return; }
  if (s.view.state === "unavailable") { setChartEmpty("section.usg.chart.unavailable"); return; }
  const days = Array.isArray(s.days) ? s.days : [];
  if (days.length === 0) { setChartEmpty("section.usg.chart.empty"); return; }

  // viewBox 几何。**只给 viewBox，宽度交给 CSS** ⇒ 天然响应式、零 resize 监听。
  const W = 720, H = 320;
  const padL = 55, padR = 24, padT = 24, padB = 40;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;

  let maxV = 0;
  for (const d of days) maxV = Math.max(maxV, metricValue(d.total));
  const ceil = niceCeil(maxV);

  const n = days.length;
  const slot = plotW / n;
  const barW = Math.max(2, Math.min(28, slot * 0.7));

  const svg = svgEl("svg", {
    viewBox: `0 0 ${W} ${H}`,
    preserveAspectRatio: "xMidYMid meet",
    role: "img",
  });
  // 读屏器读不了一堆 `<rect>`，给它一句总结。
  const title = svgEl("title", {});
  title.textContent = t("section.usg.chart.alt", { metric: t(metricSpec().labelKey), n: String(n) });
  svg.appendChild(title);

  const steps = 4;
  for (let g = 0; g <= steps; g += 1) {
    const yv = ceil * (g / steps);
    const y = padT + plotH - (yv / ceil) * plotH;
    svg.appendChild(svgEl("line", { class: "usage-chart-gridline", x1: padL, x2: W - padR, y1: y, y2: y }));
    const label = svgEl("text", { class: "usage-chart-ylabel", x: padL - 8, y: y + 4, "text-anchor": "end" });
    label.textContent = shortNum(yv);
    svg.appendChild(label);
  }

  const labelEvery = Math.max(1, Math.ceil(n / 8));
  const rows = s.view.dayRows || [];
  days.forEach((d, i) => {
    const rowState = rows[i] ? rows[i].state : "data";
    const v = metricValue(d.total);
    const h = (v / ceil) * plotH;
    const x = padL + slot * i + (slot - barW) / 2;
    const y = padT + plotH - h;
    // ⚠️ **这一行读不出来时不画柱子**（而不是画一根 0 高的柱子）：
    //    一根零高柱与「这一天真的是 0」在屏幕上完全同形。
    //    ⚠️ 监听器也跟着不挂 —— 挂在一个没进 DOM 的节点上的监听器永远不会触发，
    //    留着只会让人以为「那一天也有 tooltip」。
    if (rowState !== "unavailable") {
      const rect = svgEl("rect", {
        class: "usage-chart-bar",
        x, y: Math.min(y, padT + plotH), width: barW, height: Math.max(0, h), rx: 2,
      });
      rect.addEventListener("mousemove", (ev) => showTip(ev, d, rowState));
      rect.addEventListener("mouseleave", hideTip);
      svg.appendChild(rect);
    }

    if (i % labelEvery === 0 || i === n - 1) {
      const xl = svgEl("text", {
        class: "usage-chart-label", x: x + barW / 2, y: padT + plotH + 16, "text-anchor": "middle",
      });
      // 只画 `MM-DD`：日期串是 UTC 的 `YYYY-MM-DD`，切掉年份不改变含义。
      xl.textContent = d.date.slice(5);
      svg.appendChild(xl);
    }
  });

  clear(nodes.chart);
  nodes.chart.appendChild(svg);
}

function tipRow(label, node) {
  const row = el("div", { class: "tt-row" });
  row.appendChild(el("span", null, label));
  const b = el("b");
  b.appendChild(node);
  row.appendChild(b);
  return row;
}

function showTip(ev, day, rowState) {
  const tip = nodes.tooltip;
  clear(tip);
  tip.appendChild(el("div", { class: "tt-date" }, fmtDate(day.date)));
  // 🔴 tooltip 里同样是三档，不是「反正有个数就画出来」。
  const kind = rowState === "unavailable" ? "unknown" : rowState === "no-shards" ? "none-no-shards" : "value";
  tip.appendChild(tipRow(t("section.usg.col.requests"), cell(kind, day.total.requests, fmtInt)));
  tip.appendChild(tipRow(t("section.usg.col.errors"), cell(kind, day.total.errors, fmtInt)));
  tip.appendChild(tipRow(t("section.usg.col.tokens"), cell(kind, day.total.tokensIn + day.total.tokensOut, fmtInt)));
  tip.classList.add("show");
  const pad = 14;
  let x = ev.clientX + pad;
  let y = ev.clientY + pad;
  if (x + tip.offsetWidth > window.innerWidth) x = ev.clientX - tip.offsetWidth - pad;
  if (y + tip.offsetHeight > window.innerHeight) y = ev.clientY - tip.offsetHeight - pad;
  tip.style.left = `${x}px`;
  tip.style.top = `${y}px`;
}

function hideTip() { if (nodes.tooltip) nodes.tooltip.classList.remove("show"); }

/* ---------- 日汇总表 ---------- */

function emptyRow(key, cols) {
  const tr = el("tr");
  const td = el("td", { colspan: String(cols) });
  td.appendChild(elI18n("div", key, { class: "empty-state" }));
  tr.appendChild(td);
  return tr;
}

function renderDaily() {
  const body = nodes.dailyBody;
  clear(body);
  const s = state.summary;
  if (state.failedKey) { body.appendChild(emptyRow(state.failedKey, 6)); return; }
  if (!s || !s.view) { body.appendChild(emptyRow("section.usg.loading", 6)); return; }

  const days = Array.isArray(s.days) ? s.days : [];
  if (days.length === 0) {
    // 🔴 **白名单判据**（`read_succeeded`）：只有确知读成功了才说「没有可列的日子」。
    //    黑名单（「不是 unavailable 就当空」）方向是反的 —— 将来多一档状态会默认说假话。
    body.appendChild(emptyRow(s.view.readSucceeded ? "section.usg.table.none" : "section.usg.table.unavailable", 6));
    return;
  }

  const rows = s.view.dayRows || [];
  days.forEach((d, i) => {
    const rowState = rows[i] ? rows[i].state : "data";
    const kind = rowState === "unavailable" ? "unknown" : rowState === "no-shards" ? "none-no-shards" : "value";
    const tr = el("tr");
    tr.setAttribute("tabindex", "0");
    tr.setAttribute("role", "button");
    tr.setAttribute("aria-label", t("section.usg.drill.open", { date: d.date }));
    const dateTd = el("td");
    dateTd.appendChild(el("span", { class: "kv-mono" }, fmtDate(d.date)));
    tr.appendChild(dateTd);
    const num = (v) => {
      const td = el("td", { class: "num" });
      td.appendChild(cell(kind, v, fmtInt));
      return td;
    };
    tr.appendChild(num(d.total.requests));
    tr.appendChild(num(d.total.success));
    tr.appendChild(num(d.total.errors));
    tr.appendChild(num(d.total.tokensIn + d.total.tokensOut));
    tr.appendChild(num(d.total.streamingRequests));
    const open = () => toggleDrill(d.date);
    tr.addEventListener("click", open);
    // 整行可点 ⇒ **键盘也得点得动**（kiro 的日表只有鼠标）。
    tr.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); open(); }
    });
    body.appendChild(tr);
  });
}

/* ---------- 下钻（四维，kiro 只有一维） ---------- */

function closeDrill() {
  if (drillCtl) { drillCtl.abort(); drillCtl = null; }
  state.drill = { date: null, dim: state.drill.dim, data: null, failedKey: null, loading: false };
  if (nodes.drill) { clear(nodes.drill); nodes.drill.hidden = true; }
}

function toggleDrill(date) {
  if (state.drill.date === date) { closeDrill(); return; }
  loadDrill(date);
}

async function loadDrill(date) {
  if (drillCtl) drillCtl.abort();
  drillCtl = new AbortController();
  const ctl = drillCtl;
  state.drill = { date, dim: state.drill.dim, data: null, failedKey: null, loading: true };
  renderDrill();
  try {
    // ⚠️ **这条端点只认日期串**（`YYYY-MM-DD`，UTC），与汇总那条的 epoch 毫秒
    //    **口径刻意不同、两者不通用**（`src/admin/usage.rs::by_date` 文件头）。
    const body = await api.get(`/usage/${encodeURIComponent(date)}`, { signal: ctl.signal });
    if (ctl !== drillCtl) return;
    state.drill.data = body;
  } catch (e) {
    if (ctl !== drillCtl) return;
    if (e && e.status === 401) return;
    state.drill.failedKey = e && typeof e.status === "number" && e.status > 0
      ? "section.usg.loadFailedHttp"
      : "section.usg.loadFailedNetwork";
  }
  state.drill.loading = false;
  renderDrill();
}

/** 一个桶键该显示成什么。认不出来**照实画原始 id**，见 `API_KEY_LABEL` 的说明。 */
function apiKeyLabel(id, labelKeys) {
  const serverKey = labelKeys ? labelKeys[id] : undefined;
  const panelKey = serverKey ? API_KEY_LABEL[serverKey] : undefined;
  return panelKey ? t(panelKey) : id;
}

function renderDrill() {
  const host = nodes.drill;
  clear(host);
  const d = state.drill;
  if (!d.date) { host.hidden = true; return; }
  host.hidden = false;

  const head = el("div", { class: "usage-drill-head" });
  head.appendChild(el("h3", null, t("section.usg.drill.title", { date: d.date })));
  const close = elI18n("button", "section.usg.drill.close", { type: "button", class: "btn btn-sm btn-outline" });
  close.addEventListener("click", closeDrill);
  head.appendChild(close);
  host.appendChild(head);

  if (d.loading) {
    const box = el("div", { class: "chart-loading" });
    box.appendChild(el("span", { class: "stats-spinner", "aria-hidden": "true" }));
    box.appendChild(elI18n("span", "section.usg.loading"));
    host.appendChild(box);
    return;
  }
  if (d.failedKey) { host.appendChild(elI18n("div", d.failedKey, { class: "empty-state" })); return; }
  const body = d.data;
  if (!body || !body.view) { host.appendChild(elI18n("div", "section.usg.loadFailedNetwork", { class: "empty-state" })); return; }

  // 下钻自己的 note（常态是 `no_request_detail` —— 那句指路的话）。
  if (body.view.noteSeverity) {
    const banner = el("div", { class: BANNER_CLASS[body.view.noteSeverity] || BANNER_CLASS.warn });
    banner.appendChild(elI18n("span", NOTE_KEY[body.note] || "section.usg.note.unknown"));
    host.appendChild(banner);
  }

  // 四维 TAB。**用 `.login-tabs` 那一族**（06 §3.2 ⑥：四张表之间用 `.login-tab`
  // 形态的小 TAB 切）——kiro 里已经有这个组件，别新造。
  const tabs = el("div", { class: "login-tabs", role: "tablist" });
  for (const spec of DIMS) {
    const b = elI18n("button", spec.labelKey, { type: "button", class: "login-tab", role: "tab" });
    // 🔴 选中态钉 `aria-selected`：读屏器读的是那个属性，`.active` 只是给 CSS 看的。
    b.setAttribute("aria-selected", spec.id === d.dim ? "true" : "false");
    b.classList.toggle("active", spec.id === d.dim);
    b.addEventListener("click", () => { state.drill.dim = spec.id; renderDrill(); });
    tabs.appendChild(b);
  }
  host.appendChild(tabs);

  // 🔴 **整块读不出来时四张表都读不出来**，一格都不许画 0。
  if (body.view.state !== "data") {
    host.appendChild(elI18n("div", body.view.state === "off" ? "section.usg.drill.off" : "section.usg.drill.unavailable",
      { class: "empty-state" }));
    return;
  }

  const map = body[d.dim] || {};
  // 行序：小时那一维**由后端给**（`view.hourOrder`）—— JSON 对象没有「数值序」
  // 这回事，照 map 的自然序读出来是 0,1,10,11,…,2,20,… 一张乱序的时间轴。
  // 另外三维刻意留在字典序（后端已经排好，渲染层不自己排）。
  const order = d.dim === "hours" ? (body.view.hourOrder || []) : Object.keys(map);
  if (order.length === 0) {
    host.appendChild(elI18n("div", "section.usg.drill.empty", { class: "empty-state" }));
    return;
  }

  const wrap = el("div", { class: "table-container" });
  const table = el("table", { class: "data-table usage-records-table" });
  const thead = el("thead");
  const hr = el("tr");
  const dimSpec = DIMS.find((x) => x.id === d.dim) || DIMS[0];
  hr.appendChild(elI18n("th", dimSpec.colKey));
  hr.appendChild(elI18n("th", "section.usg.col.requests", { class: "num" }));
  hr.appendChild(elI18n("th", "section.usg.col.success", { class: "num" }));
  hr.appendChild(elI18n("th", "section.usg.col.errors", { class: "num" }));
  hr.appendChild(elI18n("th", "section.usg.col.tokens", { class: "num" }));
  thead.appendChild(hr);
  table.appendChild(thead);
  const tbody = el("tbody");
  for (const key of order) {
    const bucket = map[key];
    if (!bucket) continue;
    const tr = el("tr");
    const first = el("td", { class: "mono" });
    if (d.dim === "hours") first.textContent = t("section.usg.hourLabel", { h: key });
    else if (d.dim === "byApiKey") first.textContent = apiKeyLabel(key, body.view.apiKeyLabelKeys);
    else first.textContent = key;
    tr.appendChild(first);
    const num = (v) => {
      const td = el("td", { class: "num" });
      td.appendChild(approx(el("span", null, fmtInt(v)), !!body.approximate));
      return td;
    };
    tr.appendChild(num(bucket.requests));
    tr.appendChild(num(bucket.success));
    tr.appendChild(num(bucket.errors));
    tr.appendChild(num(bucket.tokensIn + bucket.tokensOut));
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  wrap.appendChild(table);
  host.appendChild(wrap);
}

/* ============================================================
   板块契约
   ============================================================ */

function stopAll() {
  if (listCtl) { listCtl.abort(); listCtl = null; }
  if (drillCtl) { drillCtl.abort(); drillCtl = null; }
  hideTip();
}

export const usageSection = {
  init(node) {
    nodes.root = node;
  },
  onShow() {
    stopAll();
    // ⚠️ 切语言时框架层会重跑 `onShow()` —— 整页重绘走的就是这条路（板块自己
    //    不许监听 `langchange`）。重新 `build()` 能让所有靠 `t()` 拼进 textContent
    //    的串（档位标签、tooltip、破折号的 title）跟着换语言。
    build(nodes.root);
    closeDrill();
    // 时钟自纠的配额**按「进一次板块」发**：标签页开一整天、中途系统校了时，
    // 下一次进板块还能再纠一次。
    state.corrected = false;
    load();
  },
  onHide() {
    // 🔴 切走作废一切在飞请求（06 §3.2 ② 第 ③ 条护栏，本板块同样适用）。
    stopAll();
  },
};

export default usageSection;
