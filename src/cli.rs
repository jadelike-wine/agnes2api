//! 命令行参数。
//!
//! # 🔴 一条从 kiro2api 学来的硬规矩：**不设 `default_value`**
//!
//! 有默认值就**无法区分「用户显式指定」与「没给」**，于是 CLI 那一层会把优先级
//! 更高的环境变量一起压掉。默认值必须在**用完所有层之后**才兜底，
//! 而不是在解析参数的那一刻就填上。
//!
//! 优先级：`CLI > 环境变量 > 内置默认`。

use crate::core::config_provenance::EnvMap;
use clap::Parser;

/// Agnes-AI 多协议中转网关。
#[derive(Parser, Debug, Default)]
#[command(name = "agnes2api", version, about, long_about = None)]
pub struct Cli {
    /// 数据目录（`store.json` 就落在它下面）。不给则看 `DATA_DIR`，再不给用 `/app/data`。
    #[arg(short = 'd', long)]
    pub data_dir: Option<String>,

    /// 监听端口。不给则看 `PORT`，再不给用 8080。
    #[arg(short = 'p', long)]
    pub port: Option<u16>,
}

/// 空串**视同没设**。
///
/// # 🔴 它只用在 `DATA_DIR` / `PORT` 这两个进程级开关上，**不是全局规则**
///
/// 起因是一条 shell 与程序对空串**不同义**造成的静默失败：
/// `.env.example` 是给 `cp .env.example .env` + compose 的 `env_file:` 直接用的，
/// 那条路径上一个留空的键送进来的是**空字符串**（不是「未设置」）；
/// 而容器那一侧同一个变量走的是 shell 的 `${DATA_DIR:-/app/data}` —— `:-` 把空串
/// **当未设**。两边一分叉：
///
/// - `DATA_DIR=` ⇒ entrypoint 照旧准备并 chown `/app/data`，而进程这边把 `store.json`
///   落在**工作目录**下。那里可写 ⇒ 启动探测通过、`/health` 报 healthy、面板一切正常，
///   而挂载卷从头到尾是空的 ⇒ **容器一重建，整池 key 与配置静默消失**。
/// - `PORT=` ⇒ compose 仍按 `${PORT:-8080}` 发布 8080，容器内却监听到别处
///   ⇒ 端口映射打空。
///
/// ⚠️ **别把这条推广成「全仓空串都当没设」**：数值型配置项（`MAX_STRIKES` 等）
/// 留空时**拒绝启动是有意的** fail-fast。这两个之所以归一，是因为它们的失败方向
/// 相反 —— 不响，而且要等到容器重建那一刻才看得见。
fn or_unset(v: Option<&String>) -> Option<&str> {
    v.map(String::as_str).filter(|s| !s.is_empty())
}

/// 数据目录：`CLI > DATA_DIR > /app/data`。
pub fn data_dir(cli: &Cli, env: &EnvMap) -> String {
    if let Some(d) = or_unset(cli.data_dir.as_ref()) {
        return d.to_string();
    }
    or_unset(env.get("DATA_DIR"))
        .unwrap_or("/app/data")
        .to_string()
}

/// 监听端口：`CLI > PORT > 8080`。
///
/// **显式的 `PORT=0` 仍然是「让内核挑一个」**，测试就靠它。
/// 只有空串才是「没设」。
///
/// `PORT` 不是数字时**拒绝启动**：与 `MAX_STRIKES` 那一族同一条理由 ——
/// 部署时错误必须让运维立刻看得见，而不是悄悄监听到一个谁都想不到的端口。
pub fn port(cli: &Cli, env: &EnvMap) -> Result<u16, String> {
    if let Some(p) = cli.port {
        return Ok(p);
    }
    match or_unset(env.get("PORT")) {
        None => Ok(8080),
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|_| format!("环境变量 PORT 不是合法端口: {raw}")),
    }
}

/// 抓一份环境变量快照。**这是全仓唯一读 `std::env` 的地方。**
///
/// `core` 一层收的是这份快照，于是整个配置体系在测试里就是「给一张表，看返回什么」。
pub fn env_snapshot() -> EnvMap {
    std::env::vars().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// 三层优先级：CLI > env > 内置默认。
    #[test]
    fn data_dir_priority() {
        let e = env(&[("DATA_DIR", "/from-env")]);
        assert_eq!(
            data_dir(
                &Cli {
                    data_dir: Some("/from-cli".into()),
                    port: None
                },
                &e
            ),
            "/from-cli"
        );
        assert_eq!(data_dir(&Cli::default(), &e), "/from-env");
        assert_eq!(data_dir(&Cli::default(), &EnvMap::new()), "/app/data");
    }

    /// 🔴 **`DATA_DIR=` 视同没设。**
    ///
    /// 回归对象（静默且不可逆）：空串被当成一个合法目录时，数据落在工作目录下，
    /// 那里**可写**，于是启动探测通过、`/health` 报 healthy、面板一切正常 ——
    /// 而挂载卷是空的，容器一重建整池 key 与配置全没了。
    #[test]
    fn an_empty_data_dir_is_treated_as_unset() {
        assert_eq!(
            data_dir(&Cli::default(), &env(&[("DATA_DIR", "")])),
            "/app/data"
        );
        assert_eq!(
            data_dir(
                &Cli {
                    data_dir: Some(String::new()),
                    port: None
                },
                &EnvMap::new()
            ),
            "/app/data"
        );
    }

    /// 端口的三层优先级。
    #[test]
    fn port_priority() {
        let e = env(&[("PORT", "9000")]);
        assert_eq!(
            port(
                &Cli {
                    data_dir: None,
                    port: Some(1234)
                },
                &e
            )
            .unwrap(),
            1234
        );
        assert_eq!(port(&Cli::default(), &e).unwrap(), 9000);
        assert_eq!(port(&Cli::default(), &EnvMap::new()).unwrap(), 8080);
    }

    /// `PORT=` 是「没设」，而 `PORT=0` 是「让内核挑一个」—— 两者不许混。
    #[test]
    fn empty_port_is_unset_but_explicit_zero_is_meaningful() {
        assert_eq!(port(&Cli::default(), &env(&[("PORT", "")])).unwrap(), 8080);
        assert_eq!(port(&Cli::default(), &env(&[("PORT", "0")])).unwrap(), 0);
    }

    /// `PORT` 不是数字 ⇒ 拒绝启动（部署时错误必须立刻可见）。
    #[test]
    fn a_non_numeric_port_refuses_to_start() {
        let err = port(&Cli::default(), &env(&[("PORT", "八千")])).unwrap_err();
        assert!(err.contains("PORT"), "报错必须点名是哪个变量: {err}");
    }

    /// clap 的定义本身能解析（防止属性写错到运行期才发现）。
    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// 🔴 **不许给这两个参数设 `default_value`。**
    ///
    /// 设了就分不出「用户显式指定」与「没给」，会把优先级更高的环境变量压掉。
    /// 这一格直接查 clap 的定义，而不是查行为 —— 行为那一侧已经由上面几格覆盖，
    /// 这一条防的是「有人图省事加了个 default_value，而三层优先级悄悄塌成两层」。
    #[test]
    fn neither_argument_declares_a_default_value() {
        use clap::CommandFactory;
        for arg in Cli::command().get_arguments() {
            assert!(
                arg.get_default_values().is_empty(),
                "{} 设了 default_value，优先级体系会当场塌掉",
                arg.get_id()
            );
        }
    }
}
