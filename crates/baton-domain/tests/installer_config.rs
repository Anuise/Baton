//! 安裝程式設定（票 34）：單使用者安裝（免系統管理員），覆蓋安裝與解除安裝前由掛勾停止 Runtime 與監護。

use std::path::PathBuf;

fn src_tauri() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../src-tauri"))
}

fn nsis_conf() -> serde_json::Value {
    let text = std::fs::read_to_string(src_tauri().join("tauri.conf.json")).expect("找不到 src-tauri/tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(&text).expect("tauri.conf.json 不是合法 JSON");
    conf["bundle"]["windows"]["nsis"].clone()
}

fn hooks() -> String {
    let path = nsis_conf()["installerHooks"].as_str().expect("未設定 installerHooks").to_string();
    std::fs::read_to_string(src_tauri().join(path)).expect("installerHooks 指向的檔案不存在")
}

/// 取出 `!macro <name>` 到 `!macroend` 之間的內容。
fn macro_body(text: &str, name: &str) -> String {
    let start = text.find(&format!("!macro {name}")).unwrap_or_else(|| panic!("缺少掛勾 {name}"));
    let rest = &text[start..];
    rest[..rest.find("!macroend").expect("掛勾沒有 !macroend")].to_string()
}

#[test]
fn installer_is_per_user_without_admin() {
    assert_eq!(nsis_conf()["installMode"], "currentUser");
}

#[test]
fn preinstall_hook_prepares_update() {
    let body = macro_body(&hooks(), "NSIS_HOOK_PREINSTALL");
    assert!(body.contains(r#"$INSTDIR\baton-runtime.exe" maintenance prepare-update"#), "{body}");
    assert!(body.contains("nsExec::Exec"), "應隱藏執行：{body}");
}

#[test]
fn preuninstall_hook_prepares_uninstall_unless_updating() {
    let body = macro_body(&hooks(), "NSIS_HOOK_PREUNINSTALL");
    assert!(body.contains(r#"$INSTDIR\baton-runtime.exe" maintenance prepare-uninstall"#), "{body}");
    assert!(body.contains("${If} $UpdateMode <> 1"), "更新模式不應解除登記：{body}");
    assert!(body.contains("nsExec::Exec"), "應隱藏執行：{body}");
}
