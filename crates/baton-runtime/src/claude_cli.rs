//! 正式的 ClaudeEnvironment：以本機的 `claude` 與 `git` 命令列工具探測環境（spec D-20）。
//!
//! - 只執行 `--version`、`claude auth status` 與 `where`；不下載、不安裝、不執行任何腳本。
//! - Claude Code 以 Runtime 實際啟動 Worker 的同一個程式探測（`claude_program`），精靈的結果與之後的 Run 一致。
//! - 核對認證時沿用 Runtime 目前的環境變數，只覆寫 `CLAUDE_CONFIG_DIR`，
//!   所以環境變數或 credential helper 造成的覆蓋會如實出現在結果中（使用者故事 5）。
//! - 只取用 `claude auth status` 輸出中的認證方式、帳號與設定目錄；登入資料（token）從不經過這裡。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use baton_ports::{
    AuthReport, AuthSource, ClaudeEnvironment, PortError, PortResult, ProcessIdentity, TerminalLaunch, ToolProbe,
};
use serde::Deserialize;

/// 單一命令的等待上限；超過就視為探測失敗。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// 不建立主控台視窗（Runtime 本身沒有主控台）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct ClaudeCli;

/// Runtime 啟動 Claude Code 用的程式（Worker、Controller、語意 judge 與這裡的探測共用，審查 R152）。
///
/// PATH 上任何一處有 `claude.exe` 時是 `claude`：CreateProcessW 只會替它補上 `.exe` 並依 PATH 找到，
/// 啟動指令雜湊也與既有的相同。只有 npm 安裝的批次檔（`claude.cmd`／`claude.bat`）時是第一個批次檔的完整路徑，
/// 啟動端據此改經 cmd.exe 執行。都找不到時仍是 `claude`（啟動失敗會如實回報）。
/// 同一台機器、同一個 PATH 下結果固定，所以 Runtime 重建後組出相同的啟動指令。
pub fn claude_program() -> String {
    resolve_program("claude", &path_dirs()).unwrap_or_else(|| "claude".to_string())
}

/// 目前程序的 PATH 中的目錄（依序）。
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default()
}

/// 在 `dirs` 中找 `name`：任一個目錄有 `{name}.exe` 回傳 `name`；否則回傳第一個 `{name}.cmd`／`{name}.bat` 的完整路徑。
fn resolve_program(name: &str, dirs: &[PathBuf]) -> Option<String> {
    if dirs.iter().any(|dir| dir.join(format!("{name}.exe")).is_file()) {
        return Some(name.to_string());
    }
    dirs.iter()
        // 同一個目錄內依 PATHEXT 的預設順序（.BAT 在 .CMD 之前）
        .flat_map(|dir| ["bat", "cmd"].map(|ext| dir.join(format!("{name}.{ext}"))))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

/// 程式是批次檔（要經 cmd.exe 執行）。
pub fn is_batch_file(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// 執行一個命令並收集輸出；`envs` 只追加或覆寫，其餘沿用目前的環境。
async fn run(program: &str, args: &[&str], envs: &[(&str, &Path)]) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for (key, value) in envs {
        command.env(key, value);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("{program} 在 {} 秒內沒有回應", COMMAND_TIMEOUT.as_secs()))?;
    output.map_err(|e| format!("無法執行 {program}：{e}"))
}

/// 工具是否在 PATH 上（`where` 會依 PATHEXT 找到 `.exe` 與 `.cmd`），以及 `--version` 的第一行。
async fn probe_tool(name: &str) -> PortResult<ToolProbe> {
    let located = run("where.exe", &[name], &[]).await.map_err(PortError::Failed)?;
    if !located.status.success() {
        return Ok(ToolProbe::NotFound);
    }
    // 透過 cmd 執行，PATHEXT 上的批次檔也能啟動；版本取不到不代表工具不存在
    let version_output = version_line(run("cmd.exe", &["/d", "/c", name, "--version"], &[]).await);
    Ok(ToolProbe::Found { version_output })
}

/// `--version` 輸出的第一個非空白行；執行失敗時為 `None`。
fn version_line(result: Result<std::process::Output, String>) -> Option<String> {
    match result {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

#[async_trait]
impl ClaudeEnvironment for ClaudeCli {
    /// 探測 Runtime 實際會啟動的同一個程式（`claude_program`），精靈與 Profile 核對的結果與之後的 Run 一致。
    /// 批次檔交給標準函式庫經 cmd.exe 執行（它負責批次檔的引數跳脫）。
    async fn probe_claude(&self) -> PortResult<ToolProbe> {
        let Some(program) = resolve_program("claude", &path_dirs()) else { return Ok(ToolProbe::NotFound) };
        Ok(ToolProbe::Found { version_output: version_line(run(&program, &["--version"], &[]).await) })
    }

    async fn probe_git(&self) -> PortResult<ToolProbe> {
        probe_tool("git").await
    }

    async fn auth_status(&self, config_dir: &Path) -> PortResult<AuthReport> {
        // 未登入時 claude 的結束碼不是 0，但仍會輸出 JSON，所以不看結束碼、只解析輸出
        let output = run(&claude_program(), &["auth", "status", "--json"], &[("CLAUDE_CONFIG_DIR", config_dir)])
            .await
            .map_err(PortError::Failed)?;
        parse_auth_status(&String::from_utf8_lossy(&output.stdout)).map_err(|reason| {
            let detail: String = String::from_utf8_lossy(&output.stderr).trim().chars().take(200).collect();
            PortError::Failed(if detail.is_empty() { reason } else { format!("{reason}（{detail}）") })
        })
    }

    async fn open_login_terminal(&self, config_dir: &Path) -> PortResult<()> {
        login_terminal_command(config_dir)
            .spawn()
            .map(drop)
            .map_err(|e| PortError::Failed(format!("無法開啟終端機：{e}")))
    }

    /// 以 `CREATE_NEW_CONSOLE` 開新主控台執行 Controller 宿主，外觀由 Windows 的「預設終端機應用程式」設定決定：
    /// Windows 11 預設是 Windows Terminal，沒有時是系統主控台。
    ///
    /// 不直接呼叫 `wt.exe`：wt.exe 把命令交給既有的 Windows Terminal 實例時，不保證帶上呼叫端的環境變數
    /// （Agent 憑證與 `CLAUDE_CONFIG_DIR` 只經環境變數交付），也拿不到宿主的程序身分，無法放進 Job 管理。
    #[cfg(windows)]
    async fn open_controller_terminal(&self, launch: &TerminalLaunch) -> PortResult<ProcessIdentity> {
        let launch = launch.clone();
        tokio::task::spawn_blocking(move || crate::windows_process::spawn_in_new_console(&launch.spawn, &launch.title))
            .await
            .map_err(|e| PortError::Failed(format!("無法開啟 Controller 終端機：背景工作失敗：{e}")))?
            .map_err(|e| PortError::Failed(format!("無法開啟 Controller 終端機：{e}")))
    }
}

/// 開啟登入終端機的命令：`start` 讓 cmd 取得自己的主控台（Runtime 的標準輸入輸出是空的，
/// 直接繼承會讓新視窗立刻結束），視窗以 `/k` 保持開啟，登入完成後由使用者自己關閉。
/// 這裡只執行 `claude auth login`，不下載、不安裝任何東西。
fn login_terminal_command(config_dir: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("cmd.exe");
    command
        .args(["/d", "/c", "start", "Baton - Claude login", "cmd.exe", "/k", "claude", "auth", "login"])
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `claude auth status --json` 輸出中我們用得到的欄位；其餘欄位一律忽略。
#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "loggedIn", default)]
    logged_in: bool,
    #[serde(rename = "authMethod", default)]
    auth_method: Option<String>,
    #[serde(rename = "apiProvider", default)]
    api_provider: Option<String>,
    #[serde(rename = "apiKeySource", default)]
    api_key_source: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "configDirectory", default)]
    config_directory: Option<String>,
}

/// 把 `claude auth status --json` 的輸出整理成 `AuthReport`。
///
/// 只有 `authMethod` 為 `claude.ai`（Profile 設定目錄中的原生登入）才算 `NativeLogin`；
/// API key（環境變數或 `apiKeyHelper`）、環境變數 token、第三方供應商與不認識的方式都算被覆蓋。
pub fn parse_auth_status(stdout: &str) -> Result<AuthReport, String> {
    let status: StatusJson =
        serde_json::from_str(stdout.trim()).map_err(|_| "claude auth status 的輸出不是預期的 JSON".to_string())?;
    let non_empty = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    let method = non_empty(status.auth_method.clone());
    let provider = non_empty(status.api_provider).filter(|p| p != "firstParty");

    let (source, source_detail) = if !status.logged_in {
        (AuthSource::None, None)
    } else if provider.is_some() {
        (AuthSource::ThirdPartyProvider, provider)
    } else {
        match method.as_deref() {
            Some("claude.ai") => (AuthSource::NativeLogin, None),
            Some("api_key") => (AuthSource::ApiKey, non_empty(status.api_key_source)),
            Some("oauth_token") => (AuthSource::OauthToken, None),
            _ => (AuthSource::Other, method),
        }
    };
    Ok(AuthReport {
        logged_in: status.logged_in,
        source,
        source_detail,
        account: non_empty(status.email),
        config_dir: non_empty(status.config_directory),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 以下範例取自 Claude Code 2.1.307 實際的 `claude auth status` 輸出（帳號與組織已改為範例值）
    const NATIVE: &str = r#"{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "analyticsDisabled": false,
  "projectsDirectory": "C:\\Baton\\profiles\\prof_1\\projects",
  "configDirectory": "C:\\Baton\\profiles\\prof_1",
  "email": "me@example.com",
  "orgId": "00000000-0000-0000-0000-000000000000",
  "orgName": "Example",
  "subscriptionType": "team"
}"#;

    const API_KEY: &str = r#"{
  "loggedIn": true,
  "authMethod": "api_key",
  "apiProvider": "firstParty",
  "configDirectory": "C:\\Baton\\profiles\\prof_1",
  "apiKeySource": "ANTHROPIC_API_KEY"
}"#;

    const OAUTH_TOKEN: &str = r#"{"loggedIn": true, "authMethod": "oauth_token", "apiProvider": "firstParty"}"#;

    const LOGGED_OUT: &str =
        r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty", "configDirectory": "C:\\Baton\\profiles\\prof_1"}"#;

    #[test]
    fn native_claude_ai_login_is_a_native_login_with_its_account_and_config_dir() {
        let report = parse_auth_status(NATIVE).unwrap();

        assert_eq!(
            report,
            AuthReport {
                logged_in: true,
                source: AuthSource::NativeLogin,
                source_detail: None,
                account: Some("me@example.com".into()),
                config_dir: Some("C:\\Baton\\profiles\\prof_1".into()),
            }
        );
    }

    #[test]
    fn api_key_and_token_overrides_are_not_native_logins_and_name_their_origin() {
        let api_key = parse_auth_status(API_KEY).unwrap();
        assert_eq!((api_key.source, api_key.source_detail.as_deref()), (AuthSource::ApiKey, Some("ANTHROPIC_API_KEY")));
        assert_eq!(api_key.account, None, "API key 沒有帳號身分");

        let token = parse_auth_status(OAUTH_TOKEN).unwrap();
        assert_eq!(token.source, AuthSource::OauthToken);
    }

    #[test]
    fn third_party_providers_and_unknown_methods_are_overrides() {
        let bedrock = parse_auth_status(r#"{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "bedrock"}"#).unwrap();
        assert_eq!((bedrock.source, bedrock.source_detail.as_deref()), (AuthSource::ThirdPartyProvider, Some("bedrock")));

        let unknown = parse_auth_status(r#"{"loggedIn": true, "authMethod": "sso", "apiProvider": "firstParty"}"#).unwrap();
        assert_eq!((unknown.source, unknown.source_detail.as_deref()), (AuthSource::Other, Some("sso")));
    }

    #[test]
    fn logged_out_output_reports_not_logged_in_without_a_source() {
        let report = parse_auth_status(LOGGED_OUT).unwrap();

        assert_eq!((report.logged_in, report.source, report.account), (false, AuthSource::None, None));
    }

    #[test]
    fn unparsable_output_is_an_error_that_does_not_echo_the_output() {
        let error = parse_auth_status("token=sk-ant-secret-value not json").unwrap_err();

        assert!(!error.contains("sk-ant"), "{error}");
    }

    #[test]
    fn login_terminal_runs_only_the_native_login_with_the_profiles_config_dir() {
        let command = login_terminal_command(Path::new("C:\\Baton\\profiles\\prof_1"));

        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(command.get_program(), "cmd.exe");
        assert_eq!(&args[args.len() - 4..], ["/k", "claude", "auth", "login"], "{args:?}");
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && *v == Some(std::ffi::OsStr::new("C:\\Baton\\profiles\\prof_1"))));
        assert!(!args.iter().any(|a| a.contains("http") || a.contains("irm") || a.contains("curl")), "不下載任何東西：{args:?}");
    }

    #[test]
    fn claude_exe_anywhere_on_the_path_is_launched_by_name_and_otherwise_the_first_batch_file_by_full_path() {
        let temp = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let dir = temp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        };
        let (npm, native, scoop, empty) = (dir("npm dir"), dir("native"), dir("scoop"), dir("empty"));
        std::fs::write(npm.join("claude.cmd"), "").unwrap();
        std::fs::write(native.join("claude.exe"), "").unwrap();
        std::fs::write(scoop.join("claude.bat"), "").unwrap();
        let full = |path: PathBuf| Some(path.to_string_lossy().into_owned());

        // CreateProcessW 依 PATH 找得到 claude.exe：維持以名稱啟動（與既有的啟動指令雜湊相同）
        assert_eq!(resolve_program("claude", &[npm.clone(), native.clone()]), Some("claude".to_string()));
        // 只有批次檔：第一個找到的完整路徑
        assert_eq!(resolve_program("claude", &[empty.clone(), npm.clone(), scoop.clone()]), full(npm.join("claude.cmd")));
        assert_eq!(resolve_program("claude", std::slice::from_ref(&scoop)), full(scoop.join("claude.bat")));
        assert_eq!(resolve_program("claude", &[empty]), None);
        assert!(is_batch_file(r"C:\npm\CLAUDE.CMD") && is_batch_file("x.bat") && !is_batch_file("claude"));
    }

    /// 在有安裝並登入 Claude Code 的機器上手動執行：`cargo test -p baton-runtime -- --ignored live_`。
    #[tokio::test]
    #[ignore = "需要本機已安裝 Claude Code 與 Git"]
    async fn live_probes_find_the_installed_tools() {
        let claude = ClaudeCli.probe_claude().await.unwrap();
        let git = ClaudeCli.probe_git().await.unwrap();

        assert!(matches!(claude, ToolProbe::Found { version_output: Some(_) }), "{claude:?}");
        assert!(matches!(git, ToolProbe::Found { .. }), "{git:?}");
    }

    /// 指定一個已登入的設定目錄（`BATON_LIVE_PROFILE_DIR`），確認核對得到原生登入與帳號。
    #[tokio::test]
    #[ignore = "需要設定 BATON_LIVE_PROFILE_DIR 指向已登入 Claude Code 的設定目錄"]
    async fn live_auth_status_of_a_logged_in_profile_is_a_native_login() {
        let dir = std::env::var("BATON_LIVE_PROFILE_DIR").expect("請設定 BATON_LIVE_PROFILE_DIR");

        let report = ClaudeCli.auth_status(Path::new(&dir)).await.unwrap();

        assert_eq!((report.logged_in, report.source), (true, AuthSource::NativeLogin), "{report:?}");
        assert!(report.account.is_some(), "{report:?}");
    }

    #[tokio::test]
    #[ignore = "需要本機已安裝 Claude Code"]
    async fn live_auth_status_of_an_empty_config_dir_is_not_logged_in() {
        let dir = std::env::temp_dir().join(format!("baton-live-empty-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let report = ClaudeCli.auth_status(&dir).await.unwrap();
        eprintln!("live auth_status: {report:?}");

        // 若環境變數覆蓋了認證，這裡會是 logged_in = true 且 source 不是 NativeLogin
        assert!(!report.logged_in || report.source != AuthSource::NativeLogin, "{report:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
