/**
 * 后端 DTO 交出来的徽章类名 → **这份 CSS 里真的存在的**类名。
 *
 * ── 为什么这一层必须存在 ────────────────────────────────────────────────────
 *
 * `src/core/admin/view/` 下那三处（`keys::badge_class` / `apikeys::ak_badge_class` /
 * `events::level_badge_class`）交出来的是
 * `badge` / `badge badge-danger` / `badge badge-warn` / `badge badge-ok`，
 * 而 `css/base.css` 里**根本没有 `.badge-ok` 与 `.badge-warn`**
 *（它有的是 `.badge-success` / `.badge-warning`）。
 * ⇒ 把 DTO 那个串原样挂上去的话，`.badge` 之外的那一半是个**不存在的选择器**，
 *   三档（可用 / 已过期 / 已停用）在屏幕上**完全同色**。
 * 🔴 而 Rust 那几格「N 档互不相同」的判据**只比较字符串**，照样全绿 ——
 *   这正是它自己注释里点名警告过的「在字符串上成立、在屏幕上不成立」。
 *
 * ── 为什么收在这里，而不是各板块各抄一张表 ──────────────────────────────────
 *
 * 这张表本来就是权宜之计；权宜之计**抄成三份**才是真正会漂的那件事：
 * 哪天 CSS 补了 `.badge-ok` 的别名、或者后端改了 DTO，改一份漏两份，
 * 而漏掉的那两个板块会以「某两档同色」的形态慢慢浮上来 —— 那恰恰是本文件要挡的病。
 * ⇒ **全站唯一一份**，`sec-apikeys.js` / `sec-accounts.js` / `sec-logs.js` 共用。
 *
 * ⚠️ **它仍然是欠账，不是设计**：同一个决定有了第二份实现。真正的收口是
 * 「view 层别再输出 CSS 类名」（应该交语义档位，由面板挑类），或者退一步让 CSS
 * 补两条别名。两者都不归本轮这一面改，已登记在交付说明里。
 *
 * 这个目录下的文件受三条硬规则约束：顶层只许 `export`、禁 `import`、
 * 禁出现 `document` / `window`。
 */

/**
 * 逐个类名的别名表。**按空格切开之后一个一个翻**，不是整串去匹配 ——
 * 整串匹配的那一版遇到 `"badge badge-ok is-dim"` 这类将来可能出现的组合会整条失配，
 * 悄悄退回「不存在的选择器」那个状态，而那正是本文件要挡的病。
 */
const ALIAS = {
  "badge-ok": "badge-success",
  "badge-warn": "badge-warning",
};

/**
 * 把 DTO 给的那一串翻成屏幕上真的有样式的那一串。
 *
 * 认不出来的类名**原样留着**（DTO 将来多给一档时，面板至少不会把它吃掉）；
 * 空 / 非字符串落到裸 `.badge`（中性灰，不冒充任何一档）。
 */
export function badgeClass(fromDto) {
  if (typeof fromDto !== "string") return "badge";
  const parts = fromDto.trim().split(/\s+/).filter((c) => c !== "");
  if (parts.length === 0) return "badge";
  return parts.map((c) => ALIAS[c] || c).join(" ");
}
