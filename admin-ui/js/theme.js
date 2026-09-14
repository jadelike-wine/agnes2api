/**
 * 主题。`boot.js` 已经在 body 绘制之前把 `data-theme` 落到 `<html>` 上了（防闪白），
 * 这里只负责运行期的读写与广播。
 *
 * light 时**移除属性**而不是设成 `"light"`：CSS 里 `:root` 是亮色全量 token，
 * `[data-theme="dark"]` 整体覆盖，多一个 light 值只会多一条永远匹配不上的规则。
 */
import { THEME_STORE as STORE } from "./pure/storage-keys.mjs";

export function getTheme() {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}

/**
 * ⚠️ **下面这两个色值是 `css/base.css` 里 `--bg-secondary` 的第二份拷贝**
 *（`index.html` 里那条初始 `<meta name="theme-color">` 是第三份）。
 * 对账由 `src/webui/mod.rs` 的 `theme_color_matches_the_css_token` 那一格钉着
 * ——它从这里与 index.html 抠出色值，跟 base.css 的亮暗 `--bg-secondary` 逐字比
 * ⇒ **改这一行而忘了改 base.css（或反过来）会当场红**，但拷贝本身消不掉：
 *
 * · 不能写成 `var(--bg-secondary)`：`<meta name="theme-color">` 的 content 是给浏览器
 *   chrome（移动端地址栏 / 标题栏）读的**字符串**，不走 CSS 变量解析；
 * · 也不能在这里 `getComputedStyle` 去读：那一步要等样式表加载完，而这段代码在
 *   切主题的**同一帧**里跑，读到的可能是上一套值。
 */
const THEME_COLOR_LIGHT = "#f8fafc";
const THEME_COLOR_DARK = "#0f172a";

export function setTheme(next) {
  const dark = next === "dark";
  if (dark) document.documentElement.setAttribute("data-theme", "dark");
  else document.documentElement.removeAttribute("data-theme");
  try { localStorage.setItem(STORE, dark ? "dark" : "light"); } catch (e) { /* 隐私模式 */ }
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", dark ? THEME_COLOR_DARK : THEME_COLOR_LIGHT);
  document.dispatchEvent(new CustomEvent("themechange", { detail: { theme: dark ? "dark" : "light" } }));
}

export function toggleTheme() { setTheme(getTheme() === "dark" ? "light" : "dark"); }
