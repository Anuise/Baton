//! 產品名稱必須與安裝設定（Tauri 設定檔）一致（spec D-01）。

use baton_domain::product::PRODUCT_NAME;

fn tauri_conf() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../src-tauri/tauri.conf.json");
    let text = std::fs::read_to_string(path).expect("找不到 src-tauri/tauri.conf.json");
    serde_json::from_str(&text).expect("tauri.conf.json 不是合法 JSON")
}

#[test]
fn installer_product_name_matches_constant() {
    assert_eq!(tauri_conf()["productName"], PRODUCT_NAME);
}

#[test]
fn main_window_title_matches_constant() {
    assert_eq!(tauri_conf()["app"]["windows"][0]["title"], PRODUCT_NAME);
}

#[test]
fn installer_is_per_user_nsis() {
    let conf = tauri_conf();
    assert_eq!(conf["bundle"]["targets"], serde_json::json!(["nsis"]));
    assert_eq!(conf["bundle"]["windows"]["nsis"]["installMode"], "currentUser");
}

/// 更新以 Runtime 回報的版本與更新清單比較（票 34）：安裝程式的版本必須就是 workspace 的版本。
#[test]
fn installer_version_matches_the_workspace_version() {
    assert_eq!(tauri_conf()["version"], env!("CARGO_PKG_VERSION"));
}
