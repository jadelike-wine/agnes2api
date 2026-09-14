/**
 * 仪表盘板块（← agnes 的 `overview`，裁定 P-6 改名为 kiro 的 `dashboard`）。
 *
 * 布局**完全照 kiro**（06 §3.2 ①）：
 *   `.stats-head` → 五张 `.stat-card` 一行 → `.card.dash-sysinfo`（信息栅格）
 *   → `.dash-uptime-row`（uptime 卡 + 两张 `.dash-qr-card`）。
 * 语义**按 agnes 重填**，并把 agnes 明确更强的三样并进去（06 §3.2 ① 末三条）：
 *   · **降级横幅** `.banner-danger`（kiro 没有横幅组件，U-3 保留）；
 *   · **新鲜度提示** → 信息面板下面两条 `.mng-inline-note`；
 *   · **被 env 锁死的字段数** → 信息栅格里的第 9 格，点一下跳设置页。
 *
 * # 🔴 三条贯穿本文件的纪律
 *
 * ① **一切来自接口的内容一律 `textContent`（`el()` / `elI18n()` 内部就是它），
 *    全文件零 `innerHTML`。**
 * ② 🔴 **读不出来的格子画 EM DASH（`.usage-unknown`），绝不伪造 0。**
 *    这是本仓的产品不变式，不是风格偏好：一个「读不出来」被画成 `0` 之后，
 *    「池子里有 0 把 key」与「这次没读到池子」长得一模一样，而两者的处置方向
 *    正好相反（去补 key / 去查存储）。`src/admin/overview.rs` 的文件头逐字写着
 *    同一句话 —— **后端已经把逐块降级做成 `null` 了，前端这一半不能把它擦掉。**
 *    与它配对的是 EN DASH（`.usage-none`）：**读成功了，但这一段真的没有样本**。
 * ③ **板块内不许监听 `langchange`**：框架层（`js/app.js`）会 `apply(document)`
 *    之后重跑一次 `onShow()`。
 *
 * # ⚠️ 与 kiro 仪表盘的四处**刻意不同**，逐条给理由
 *
 * · **没有「全局剩余积分」那张卡**（06 §9 R-4）：Agnes 上游到底有没有查余额接口，
 *   全仓零证据 —— 画一张恒为 `—` 的卡不是诚实，是占着一个固定五列栅格里的位置。
 *   第四张换成**今日成功率**（`/usage` 的 `view.cards.successRate` 是判好的）。
 * · **没有「今日花费」那张卡**：本仓**没有任何定价表**（`UsageBucket` 里一个
 *   金额字段都没有），算不出花费。06 §3.2 ① 那一格写的是「C3 定价表落地后」，
 *   而 C3 今天不存在。⚠️ **别用「token × 一个猜的单价」补上去** —— 那是在屏幕上
 *   印一个没有任何来源的数字，比空着坏得多。
 * · **`.dash-rotation-badge` 换了内容**：kiro 那枚画的是负载均衡策略，而本仓的
 *   `GatewayConfig` 里**没有这个概念**（全仓 grep 零命中）。那枚徽章改画**注册机
 *   状态**，并且 🔴 **`registrarEnabled` 与 `registrarBlocked` 一起判** ——
 *   `src/admin/overview.rs` 逐字警告过：只判前一格的话，面板会照旧声称有一个
 *   在工作的注册机，而补池其实一轮都没跑。
 * · **两张二维码卡换成「文档 / GitHub」**：本项目没有社群与赞助二维码，
 *   而 kiro 那两张的形态（`.dash-qr-card` + 虚线框 + emoji + 一行说明）原样留着。
 *   ⚠️ **别留空的虚线框**（06 §3.2 ① 末行）。
 *
 * # ⚠️⚠️ 「今日」是怎么取的 —— 一个刻意绕开的常数
 *
 * 用量按 **UTC 日**分桶（`src/admin/usage.rs`：按本地时区分桶会让同一份数据在改
 * 时区之后整体错位）。要取「今天」，最直觉的写法是
 * `from = Math.floor(serverTime / 86400000) * 86400000` ——
 * 🔴 **那会把 `USAGE_DAY_MS` 这个后端常量在前端复制一份**，而两处分叉时
 * 面板会安静地展示错一天的数据。
 * ⇒ 本文件改发 **`from = to = serverTime`**：`parse_range()` 把两端各自取整到
 * 所在的 UTC 日，于是 `from_day == to_day == 今天`，`days` 恰好一格、`total`
 * 就是今天的合计，而且 `clamped` 恒为假（今天一定在保留期内）。
 * **一个日长常数都不必出现在前端。**
 *
 * ⚠️ `serverTime` 用的是**服务端**时钟（`/overview` 给的），不是 `Date.now()`：
 * 两者不同源，浏览器时钟偏一天时会去查一个空日子并报「今天没有请求」。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, svgIcon } from "./ui.js";

/* ============================================================
   本地化格式（06 §5.3 的 B 档：`Intl` 只有浏览器有，留在 JS）
   ⚠️ **这几个函数里不许有「该显示哪一档」的分支** —— 那一半是 A 档，
   由端点的 DTO（`view.cards.*` / `null`）给出，见下面 `cell()`。
   ⚠️ 已登记的欠账：`js/pure/format.mjs`（P-9 的 B 档归宿）本轮还不存在，
   所以这几个小函数暂住在板块文件里，`js/sec-accounts.js` 里有形近的另一份。
   ============================================================ */

/** 千分位整数。`Intl` 按当前界面语言给分隔符。 */
function fmtInt(n) {
  return new Intl.NumberFormat(document.documentElement.lang || "zh-CN").format(n);
}

/** 百分比，一位小数。 */
function fmtPercent(ratio) {
  return `${(ratio * 100).toFixed(1)}%`;
}

/**
 * 字节数 → 人读的串。
 * ⚠️ 用 **KiB / MiB / GiB**（1024 进位）而不是 MB：`rssBytes` 来自
 * `/proc/self/statm`（页数 × 页大小），它本来就是 1024 进位的量。
 */
function fmtBytes(bytes) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i += 1; }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`;
}

/** 一段毫秒 → 「N 天 N 小时」/「N 小时 N 分」/「N 分 N 秒」。 */
function fmtDuration(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return t("dash.durationDays", { d, h });
  if (h > 0) return t("dash.durationHours", { h, m });
  return t("dash.durationMinutes", { m, s: s % 60 });
}

/** 一段毫秒 → 整秒（新鲜度那两句话只需要秒）。 */
function secsOf(ms) { return Math.max(0, Math.round(ms / 1000)); }

/** 服务端时刻 → 本地时区的可读串。**时区是浏览器的**，这一格说的是「服务器现在几点」。 */
function fmtInstant(msEpoch) {
  return new Date(msEpoch).toLocaleString(document.documentElement.lang || "zh-CN");
}

/* ============================================================
   两种破折号 —— U-4，kiro 只治了一半的那个病
   ============================================================ */

/**
 * 把一根破折号**包进一个自己的 `<span>`** 再挂进宿主格子。
 *
 * 🔴🔴 **标记类绝不能与宿主格子的类并到同一个元素上。** 这是 U-4 在本板块上
 * 真的失效过的那条路径，值得逐字写下来：
 *   · `.usage-unknown`（`css/base.css`）与 `.dash-stat-value`
 *     （`css/sections-stats.css`）**特指度完全相同**（都是单个类），
 *     而 `sections-stats.css` 在 `index.html` 里**后加载**
 *     ⇒ 后者的 `color: var(--text-primary)` / `font-weight: 700`
 *     把前者的 `--warning` / 600 **整条覆盖**；
 *   · `.dash-info-value` / `.dash-uptime-value` 同样写死了 `color`，三格全中。
 * ⇒ 屏幕上「不知道」（EM DASH）与「没有」（EN DASH）**同色同粗**，两种状态
 *   只剩破折号长度这一点差别 —— 而 U-4 钉的正是「这两件事必须分得开」。
 * ⚠️ `js/sec-usage.js` 的 `unknownCell()` / `noneCell()` 一开始就是包子 span 的
 *    形态，所以那个板块没中这一枪。**这里照它的形态改，不另发明第二种。**
 *
 * 顺带收掉两件与它同源的小事：
 * ① 宿主的 `class` 不再被追加写 ⇒ 反复画同一格不会积出
 *    `dash-info-value usage-unknown usage-unknown`；
 * ② `title` 挂在 span 自己身上 ⇒ 下一次 `textContent = …` 把 span 连同提示一起
 *    换掉。挂在宿主上时它会**活过**那次赋值（`paintOverview()` 里
 *    `version` / `runtime` / `serverTime` 三格只写 textContent、不动 title），
 *    结果是一个已经有值的格子悬停仍在说「这一块没读出来」。
 */
function markText(node, cls, ch, titleKey) {
  node.textContent = "";
  node.removeAttribute("title");
  const mark = el("span", { class: cls, title: t(titleKey) }, ch);
  node.appendChild(mark);
  return mark;
}

/**
 * 把一格画成「没读出来」（EM DASH）。**返回那枚 span**，调用方要改提示语时改它。
 * 🔴 **它与 `noneText()` 不是同义词。** kiro 把两者都画成 `—`，而它自己的回归
 * 测试第 ⑦/⑧ 条正是「统计接口挂了却印出 0.0000 冒充真实零流量」——
 * 同一个病，它只治了一半。
 */
function unknownText(node) {
  return markText(node, "usage-unknown", "—", "dash.unknownHint");
}

/** 把一格画成「读成功了，但这一段真的没有样本」（EN DASH）。 */
function noneText(node) {
  return markText(node, "usage-none", "–", "dash.noneHint");
}

/**
 * 按 `/usage` 那份 DTO 里已经判好的档位画一张卡的数字。
 *
 * 🔴 **`kind` 一律取自 `view.cards.*`，绝不在这里从 `tier` / `shards` / `total`
 * 反推** —— `src/admin/usage.rs` 逐字写着：反推就是把那套五态判定复制了第二份，
 * 而那套判定在 TS 版里被面板算错过五次。
 *
 * 四档（`src/core/admin/view/usage.rs` 的闭集）：
 *   · `value`          —— 有数，画出来；
 *   · `none`           —— 读成功、分母是 0 / 没有样本 ⇒ EN DASH；
 *   · `none-no-shards` —— 读成功、这段时间一个分片都没写过 ⇒ EN DASH；
 *   · `unknown`        —— 🔴 **没读出来** ⇒ EM DASH。
 * ⚠️ 表外的值落到 `unknown`：一个这份面板还不认识的新档位，**最不武断的处置是
 * 说「不知道」**，绝不冒充成「有数」。
 */
function paintCard(node, kind, valueText, complete, state) {
  node.removeAttribute("title");
  node.setAttribute("class", "dash-stat-value");
  if (kind === "value") {
    node.textContent = valueText;
    if (complete === false) {
      // 读到的是一段**不完整**的数据：比「没读出来」轻一档，但不许冒充完整。
      node.setAttribute("class", "dash-stat-value usage-incomplete");
      node.setAttribute("title", t("dash.incompleteHint"));
    }
    return;
  }
  if (kind === "none" || kind === "none-no-shards") { noneText(node); return; }
  const mark = unknownText(node);
  // ⚠️ **「运维没打开统计」与「统计接口挂了」都画 EM DASH，但要说不同的话**：
  //    前者的处置是去设置页打开 Tier-2，后者是去查存储 —— 一句「不知道」盖住
  //    两种完全不同的下一步，运维只能猜。
  // ⚠️ 提示改的是**那枚 span**，不是宿主格子：宿主上的 title 会活过下一次
  //    `textContent = …`（见 `markText()`）。
  if (state === "off") mark.setAttribute("title", t("dash.usageOffHint"));
}

/* ============================================================
   图标（`js/ui.js` 的 `svgIcon()` 是全站唯一那份造 SVG 的实现）
   ============================================================ */

const ICON_TAG = "M 4 4 h 8 l 8 8 l -8 8 l -8 -8 z M 8.5 8.5 h .01";
const ICON_CPU = "M 6 6 h 12 v 12 h -12 z M 9 2 v 2 M 15 2 v 2 M 9 20 v 2 M 15 20 v 2 M 2 9 h 2 M 2 15 h 2 M 20 9 h 2 M 20 15 h 2";
const ICON_CLOCK = "M 12 3 a 9 9 0 1 0 0 18 a 9 9 0 1 0 0 -18 z M 12 7 v 5 l 3.5 2";
const ICON_DISK = "M 4 6 c 0 -1.7 3.6 -3 8 -3 s 8 1.3 8 3 v 12 c 0 1.7 -3.6 3 -8 3 s -8 -1.3 -8 -3 z M 4 6 c 0 1.7 3.6 3 8 3 s 8 -1.3 8 -3";
const ICON_MEMORY = "M 3 8 h 18 v 8 h -18 z M 7 16 v 3 M 12 16 v 3 M 17 16 v 3";
const ICON_HASH = "M 4 9 h 16 M 4 15 h 16 M 10 3 l -2 18 M 16 3 l -2 18";
const ICON_SERVER = "M 3 4 h 18 v 6 h -18 z M 3 14 h 18 v 6 h -18 z M 7 7 h .01 M 7 17 h .01";
const ICON_LOCK = "M 5 11 h 14 v 10 h -14 z M 8 11 V 7 a 4 4 0 0 1 8 0 v 4";

/* ============================================================
   骨架
   ============================================================ */

/** 五张卡的登记表。**顺序即屏幕上的顺序**（固定 5 列栅格，不是 auto-fit）。 */
const STAT_CARDS = [
  { id: "pool", labelKey: "dash.cardPool", emoji: "👤", tint: "tint-green" },
  { id: "requests", labelKey: "dash.cardRequests", emoji: "📨", tint: "" },
  { id: "tokens", labelKey: "dash.cardTokens", emoji: "🎟️", tint: "tint-amber" },
  { id: "successRate", labelKey: "dash.cardSuccessRate", emoji: "✅", tint: "tint-blue" },
  { id: "models", labelKey: "dash.cardModels", emoji: "🧩", tint: "tint-blue" },
];

/**
 * 信息栅格九格。**第 9 格是 agnes 独有的「被 env 锁死的字段数」**，
 * 它带一颗跳转（06 §3.2 ①）。前 8 格照 kiro 的 4 列 × 2 行。
 */
const INFO_ITEMS = [
  { id: "version", labelKey: "dash.version", icon: ICON_TAG },
  { id: "runtime", labelKey: "dash.runtime", icon: ICON_CPU },
  { id: "serverTime", labelKey: "dash.serverTime", icon: ICON_CLOCK },
  { id: "storage", labelKey: "dash.storage", icon: ICON_DISK },
  { id: "memory", labelKey: "dash.memory", icon: ICON_MEMORY },
  { id: "pid", labelKey: "dash.pid", icon: ICON_HASH },
  { id: "pool", labelKey: "dash.poolShape", icon: ICON_SERVER },
  { id: "quota", labelKey: "dash.poolStats", icon: ICON_TAG },
  { id: "envLocked", labelKey: "dash.envLocked", icon: ICON_LOCK },
];

/**
 * 两张二维码位的卡。**形态照 kiro，内容换成本项目真有的两样东西。**
 *
 * ⚠️ **三个 key 各有各的位置，一条都不许拿来顶另一条**（06 §3.2 ① 末行 +
 * `css/sections-stats.css` 的三条规则）：
 *   · `hintKey` → 虚线框**内**的 `.dash-qr-hint`（`.65rem` / `--text-tertiary`，
 *     装在一个 120×120 的定高盒子里）：kiro 那里放的是二维码图，本项目没有图，
 *     这一行就是**那张图的替身**，只够写下「去哪儿」，**必须短**；
 *   · `titleKey` → 框下的 `.dash-qr-title`（`.9rem` / 600）；
 *   · `descKey`  → 再下面的 `.dash-qr-desc`（`.8rem` / `--text-secondary`，整句话）。
 * 🔴 曾经 `hintKey` 不存在、框内框外**共用同一个 `descKey`** ⇒ 同一句话在屏幕上
 *    **各印两遍**（真浏览器实测：那句话在板块正文里出现 2 次）。框里那一份还被
 *    `.65rem` 挤成三到四行（zh 41px / en 54px 高），占满一个本该只放一行的替身位。
 */
const LINK_CARDS = [
  { emoji: "📘", titleKey: "dash.docs", hintKey: "dash.docsHint", descKey: "dash.docsDesc" },
  { emoji: "🐙", titleKey: "dash.repo", hintKey: "dash.repoHint", descKey: "dash.repoDesc" },
];

/** 板块内的节点表。**按 id 存，不靠 `querySelector` 的字符串拼接找。** */
const nodes = {
  root: null,
  banner: null,
  stats: {},
  info: {},
  sysinfoVer: null,
  registrarBadge: null,
  uptime: null,
  freshPool: null,
  freshConfig: null,
  refreshBtn: null,
};

function statCard(spec) {
  const card = el("div", { class: "stat-card" });
  card.appendChild(el("div", { class: `dash-stat-icon ${spec.tint}`.trim(), "aria-hidden": "true" }, spec.emoji));
  const body = el("div", { class: "dash-stat-body" });
  const value = el("p", { class: "dash-stat-value" }, "—");
  nodes.stats[spec.id] = value;
  body.appendChild(value);
  body.appendChild(elI18n("p", spec.labelKey, { class: "dash-stat-label" }));
  card.appendChild(body);
  return card;
}

function infoItem(spec) {
  const item = el("div", { class: "dash-info-item" });
  const label = el("span", { class: "dash-info-label" });
  const ico = el("span", { class: "dash-info-icon", "aria-hidden": "true" });
  ico.appendChild(svgOf(spec.icon));
  label.appendChild(ico);
  label.appendChild(elI18n("span", spec.labelKey));
  item.appendChild(label);
  const value = el("span", { class: "dash-info-value" }, "—");
  nodes.info[spec.id] = value;
  item.appendChild(value);
  return item;
}

/**
 * 16px 的行内图标。
 * 🔴 **转调 `js/ui.js` 的 `svgIcon()`，这里不另造一份 `createElementNS`**：
 * 那是全站唯一造 SVG 的地方，而它身上挂着两条不能各处重抄的决定
 *（`stroke-linecap` / `stroke-linejoin` 都是 `round`，缺省值会把这套细线短划
 * 图标渲染成一圈小方块）。
 */
function svgOf(d) {
  return svgIcon(d, 16);
}

function linkCard(spec) {
  const card = el("div", { class: "dash-qr-card" });
  const box = el("div", { class: "dash-qr-box" });
  box.appendChild(el("span", { class: "dash-qr-emoji", "aria-hidden": "true" }, spec.emoji));
  box.appendChild(elI18n("span", spec.hintKey, { class: "dash-qr-hint" }));
  card.appendChild(box);
  card.appendChild(elI18n("p", spec.titleKey, { class: "dash-qr-title" }));
  card.appendChild(elI18n("p", spec.descKey, { class: "dash-qr-desc" }));
  return card;
}

function build(root) {
  root.textContent = "";

  const head = el("div", { class: "stats-head" });
  head.appendChild(elI18n("h2", "dash.title"));
  const toolbar = el("div", { class: "stats-toolbar" });
  const refresh = elI18n("button", "dash.refresh", { type: "button", class: "btn btn-outline btn-sm" });
  refresh.addEventListener("click", () => { load(); });
  nodes.refreshBtn = refresh;
  toolbar.appendChild(refresh);
  head.appendChild(toolbar);
  root.appendChild(head);

  // 降级横幅（U-3）。**默认不占位**：没降级时它根本不在 DOM 里。
  nodes.banner = el("div", { class: "banner banner-danger hidden", role: "alert" });
  nodes.banner.appendChild(el("span", { "aria-hidden": "true" }, "⚠️"));
  nodes.banner.appendChild(elI18n("span", "dash.degraded"));
  root.appendChild(nodes.banner);

  const grid = el("div", { class: "stats-grid" });
  for (const spec of STAT_CARDS) grid.appendChild(statCard(spec));
  root.appendChild(grid);

  const sys = el("div", { class: "card dash-sysinfo" });
  const header = el("div", { class: "card-header" });
  const h3 = el("h3");
  const h3ico = el("span", { class: "dash-info-icon", "aria-hidden": "true" });
  h3ico.appendChild(svgOf(ICON_SERVER));
  h3.appendChild(h3ico);
  h3.appendChild(elI18n("span", "dash.systemInfo"));
  nodes.sysinfoVer = el("span", { class: "dash-sysinfo-ver" });
  h3.appendChild(nodes.sysinfoVer);
  header.appendChild(h3);
  // 🔴 注册机三态：**两格一起判**（见文件头）。
  const badge = el("span", { class: "dash-rotation-badge" });
  badge.appendChild(elI18n("span", "dash.registrar"));
  nodes.registrarBadge = el("span", { class: "dash-rotation-value" }, "—");
  badge.appendChild(nodes.registrarBadge);
  header.appendChild(badge);
  sys.appendChild(header);

  const infoGrid = el("div", { class: "dash-info-grid" });
  for (const spec of INFO_ITEMS) infoGrid.appendChild(infoItem(spec));
  sys.appendChild(infoGrid);

  // 新鲜度（agnes 独有）。两句话**各说各的**：一句是别的副本判定的冷却多久能看到，
  // 一句是刚改完配置多久能看见 —— `src/admin/overview.rs` 逐字警告过
  // 「两条上界就等于各自的 TTL，中间不许再加任何一项」。
  nodes.freshPool = el("p", { class: "mng-inline-note" });
  nodes.freshConfig = el("p", { class: "mng-inline-note" });
  sys.appendChild(nodes.freshPool);
  sys.appendChild(nodes.freshConfig);
  root.appendChild(sys);

  const uptimeRow = el("div", { class: "dash-uptime-row" });
  const uptimeCard = el("div", { class: "dash-uptime-card" });
  const uico = el("div", { class: "dash-uptime-icon", "aria-hidden": "true" });
  uico.appendChild(svgIcon(ICON_CLOCK, 32));
  uptimeCard.appendChild(uico);
  nodes.uptime = el("div", { class: "dash-uptime-value" }, "—");
  uptimeCard.appendChild(nodes.uptime);
  uptimeCard.appendChild(elI18n("div", "dash.uptime", { class: "dash-uptime-label" }));
  uptimeRow.appendChild(uptimeCard);
  const links = el("div", { class: "dash-qr-cards" });
  for (const spec of LINK_CARDS) links.appendChild(linkCard(spec));
  uptimeRow.appendChild(links);
  root.appendChild(uptimeRow);
}

/* ============================================================
   取数
   ============================================================ */

/**
 * 在飞请求的作废闸。
 * 🔴 **两件事共用一个世代号**：`onHide()`（切走了）与 `load()`（又点了一次刷新）。
 * 后回来的那一次要能认出「我已经不是最新那一次了」，否则一次慢响应会在用户
 * 切走之后把一张旧数据画回屏幕上。
 */
let generation = 0;
let abort = null;

/** uptime 的本地滴答。**只在板块可见时走**，切走要停。 */
let uptimeTimer = null;
let uptimeBaseMs = null;
let uptimeBaseAt = 0;

function paintUptime() {
  if (uptimeBaseMs === null) { unknownText(nodes.uptime); return; }
  nodes.uptime.setAttribute("class", "dash-uptime-value");
  nodes.uptime.removeAttribute("title");
  // **本地滴答只加「从拿到那一刻起又过了多久」**，基准恒是服务端给的那个数。
  nodes.uptime.textContent = fmtDuration(uptimeBaseMs + (Date.now() - uptimeBaseAt));
}

function startUptime() {
  if (uptimeTimer === null) uptimeTimer = setInterval(paintUptime, 1000);
}

function stopUptime() {
  if (uptimeTimer !== null) { clearInterval(uptimeTimer); uptimeTimer = null; }
}

/** 一块读失败时把它名下的每一格画成 EM DASH。**不留上一次的旧值。** */
function blankStats(ids) {
  for (const id of ids) unknownText(nodes.stats[id]);
}

function blankInfo(ids) {
  for (const id of ids) unknownText(nodes.info[id]);
}

/**
 * 画 `/overview` 那一块。
 *
 * ⚠️ **逐块判 `null`，一块一块画** —— 后端就是逐块降级的形状
 * （`pool` / `poolStats` / `process` 三处独立可空），前端合并成一个
 * 「整页读失败」会把「池子读不出来但进程指标好着」这种真实状态抹平。
 */
function paintOverview(body) {
  nodes.sysinfoVer.textContent = body.version ? ` ${body.version}` : "";
  nodes.info.version.textContent = body.version || "—";
  nodes.info.runtime.textContent = body.runtime && body.runtime.name ? body.runtime.name : "—";
  nodes.info.serverTime.textContent = fmtInstant(body.serverTime);

  // 存储：后端 + 可写性。**可写性是两态，各说各的话**，
  // 「只读」不是故障但会让一切写操作在点下去之后才失败。
  const st = body.storage;
  if (st) {
    nodes.info.storage.setAttribute("class", "dash-info-value");
    nodes.info.storage.removeAttribute("title");
    nodes.info.storage.textContent = `${st.backend} · ${t(st.writable ? "dash.storageWritable" : "dash.storageReadonly")}`;
  } else {
    unknownText(nodes.info.storage);
  }

  // 进程指标：**取不到就是 `null`**（非 Linux / 读 `/proc` 失败）。
  // 🔴 后端注释逐字写着「不许编一个 0 出来 —— 0 字节常驻内存是一句当场可证伪的假话」。
  if (body.process) {
    nodes.info.memory.setAttribute("class", "dash-info-value");
    nodes.info.memory.removeAttribute("title");
    nodes.info.memory.textContent = fmtBytes(body.process.rssBytes);
    nodes.info.pid.setAttribute("class", "dash-info-value");
    nodes.info.pid.removeAttribute("title");
    nodes.info.pid.textContent = String(body.process.pid);
    uptimeBaseMs = body.process.uptimeMs;
    uptimeBaseAt = Date.now();
  } else {
    blankInfo(["memory", "pid"]);
    uptimeBaseMs = null;
  }
  paintUptime();

  // 池子：整块 `null` = 没读出来。
  if (body.pool) {
    const p = body.pool;
    nodes.stats.pool.setAttribute("class", "dash-stat-value");
    nodes.stats.pool.removeAttribute("title");
    nodes.stats.pool.textContent = `${fmtInt(p.fresh)} / ${fmtInt(p.total)}`;
    nodes.info.pool.setAttribute("class", "dash-info-value");
    nodes.info.pool.removeAttribute("title");
    nodes.info.pool.textContent = t("dash.poolShapeValue", {
      cooling: p.cooling, evicted: p.evicted, disabled: p.disabled,
    });
  } else {
    unknownText(nodes.stats.pool);
    unknownText(nodes.info.pool);
  }

  // Tier-1 池级累计。**近似值必须打 `≈`**，而且那一格由响应里的 `approximate`
  // 驱动，🔴 不许在前端写死（`src/admin/overview.rs`：「这一格是那个 ≈ 的唯一驱动源」）。
  if (body.poolStats) {
    const s = body.poolStats;
    nodes.info.quota.setAttribute("class", "dash-info-value");
    nodes.info.quota.removeAttribute("title");
    nodes.info.quota.textContent = "";
    if (s.approximate) {
      const mark = el("span", { class: "approx", title: t("dash.approxHint") }, "≈ ");
      nodes.info.quota.appendChild(mark);
    }
    nodes.info.quota.appendChild(el("span", null, t("dash.poolStatsValue", {
      requests: fmtInt(s.requests), failed: fmtInt(s.failed),
    })));
  } else {
    unknownText(nodes.info.quota);
  }

  // 配置块**刻意不参与逐块降级**（`ConfigHolder::current` 同步读、永不抛），
  // 所以这里没有 `null` 分支 —— 写一个永远走不到的分支比没有分支更坏。
  const cfg = body.config;
  nodes.banner.classList.toggle("hidden", !cfg.degraded);
  nodes.info.envLocked.textContent = "";
  const lockedBtn = el("button", { type: "button", class: "btn btn-outline btn-xs" });
  lockedBtn.textContent = t("dash.envLockedValue", { n: cfg.envLocked.length });
  lockedBtn.setAttribute("title", cfg.envLocked.length > 0 ? cfg.envLocked.join(", ") : t("dash.envLockedNone"));
  // 跳设置页。**走导航按钮自己的 click，不另造一条路由**：`js/app.js` 的
  // `showSection()` 没有对外导出，而那几颗按钮上的 `data-section` 是 `index.html`
  // 与 `src/webui/mod.rs` 三方对齐钉住的公开契约。
  lockedBtn.addEventListener("click", () => {
    const nav = document.querySelector('.nav-item[data-section="settings"]');
    if (nav) nav.click();
  });
  nodes.info.envLocked.appendChild(lockedBtn);

  // 🔴 注册机三态，两格一起判。
  nodes.registrarBadge.textContent = t(
    !cfg.registrarEnabled ? "dash.registrarOff"
      : cfg.registrarBlocked ? "dash.registrarBlocked"
        : "dash.registrarOn",
  );

  const f = body.freshness;
  nodes.freshPool.textContent = t("dash.freshPool", { sec: secsOf(f.poolVisibilityUpperBoundMs) });
  nodes.freshConfig.textContent = t("dash.freshConfig", { sec: secsOf(f.configVisibilityUpperBoundMs) });
  return body.serverTime;
}

/** 画今日用量那三张卡。**档位一律取 DTO 里判好的 `view.cards.*`。** */
function paintUsage(body) {
  const v = body.view;
  const total = body.total || {};
  const complete = v.complete;
  const st = v.state;
  paintCard(nodes.stats.requests, v.cards.requests, fmtInt(total.requests || 0), complete, st);
  paintCard(
    nodes.stats.tokens,
    v.cards.tokens,
    fmtInt((total.tokensIn || 0) + (total.tokensOut || 0)),
    complete,
    st,
  );
  // 成功率的分母是请求数：`empty` 态下计数是真的 0，而比率**没有分母**
  // —— 后端已经把这个区别做进 `cards.successRate` 了，这里只照着画。
  const ratio = total.requests > 0 ? (total.success || 0) / total.requests : 0;
  paintCard(nodes.stats.successRate, v.cards.successRate, fmtPercent(ratio), complete, st);
}

async function load() {
  const gen = ++generation;
  if (abort) abort.abort();
  abort = new AbortController();
  const signal = abort.signal;
  if (nodes.refreshBtn) nodes.refreshBtn.disabled = true;

  let serverTime = null;
  try {
    const body = await api.get("/overview", { signal });
    if (gen !== generation) return;
    serverTime = paintOverview(body);
  } catch (e) {
    if (gen !== generation) return;
    // 🔴 **一整块没读出来 ⇒ 它名下每一格画 EM DASH**，不留上一次的旧值
    //    —— 屏幕上挂着一份已经不新鲜的数据，比一个诚实的「不知道」危险。
    blankStats(["pool"]);
    blankInfo(["version", "runtime", "serverTime", "storage", "memory", "pid", "pool", "quota", "envLocked"]);
    uptimeBaseMs = null;
    paintUptime();
  }

  // ⚠️ 两条独立的块**并行发、各自降级**：模型目录读不出来不该把用量那三张卡
  //    一起打成「不知道」，反过来同理。
  const jobs = [];
  jobs.push(
    api.get("/models", { signal })
      .then((body) => {
        if (gen !== generation) return;
        nodes.stats.models.setAttribute("class", "dash-stat-value");
        nodes.stats.models.removeAttribute("title");
        nodes.stats.models.textContent = fmtInt(body.models.length);
      })
      .catch(() => { if (gen === generation) blankStats(["models"]); }),
  );
  if (serverTime !== null) {
    // 见文件头：`from = to = serverTime` ⇒ 服务端把两端取整到同一个 UTC 日，
    // 于是这一次问的恰好是「今天」，而前端一个日长常数都不必知道。
    jobs.push(
      api.get(`/usage?from=${serverTime}&to=${serverTime}`, { signal })
        .then((body) => { if (gen === generation) paintUsage(body); })
        .catch(() => { if (gen === generation) blankStats(["requests", "tokens", "successRate"]); }),
    );
  } else {
    // 概览都没读出来 ⇒ 拿不到服务端时钟 ⇒ **不发那一次请求**，也不拿浏览器
    // 时钟去凑一个「今天」（那会安静地查错一天）。
    blankStats(["requests", "tokens", "successRate"]);
  }
  await Promise.all(jobs);
  if (gen === generation && nodes.refreshBtn) nodes.refreshBtn.disabled = false;
}

/* ============================================================
   板块契约
   ============================================================ */

export const dashboardSection = {
  init(node) {
    nodes.root = node;
    build(node);
  },
  onShow() {
    // ⚠️ 切语言时框架层会重跑 `onShow()` —— 整页重绘走的就是这条路
    //（板块自己不许监听 `langchange`）。这里重新 `build()` 一次能让所有靠
    // `t()` 拼进 textContent 的串（uptime、新鲜度、池子形态）跟着换语言。
    build(nodes.root);
    startUptime();
    load();
  },
  onHide() {
    // 🔴 **切走要作废在飞请求 + 停掉本地滴答。** 06 §3.2 ② 那三条护栏里的第 ③ 条
    //（另两条在账号板块）：一个切走了的板块不该继续往 DOM 上写东西，也不该
    // 留一个每秒跑一次的孤儿定时器。
    generation += 1;
    if (abort) { abort.abort(); abort = null; }
    stopUptime();
  },
};

export default dashboardSection;
