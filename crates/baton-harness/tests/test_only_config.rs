//! 測試專用的 Runtime 組態（fake Port 的 Harness 與 UI E2E 的伺服器、driver，票 53）不會進入正式建置或安裝程式。
//!
//! 正式建置是 `scripts/build.mjs`：建置 `baton-runtime`、`baton-guardian`、`baton-mcp-shim` 與桌面殼 `baton-desktop`，
//! NSIS 安裝程式另外打包 `tauri.bundle.conf.json` 的 `externalBin` 與 `tauri.conf.json` 的 `frontendDist`（`web/dist`）。
//! 這裡以 `cargo metadata` 與設定檔證明：這些程式的一般與建置相依都到不了 `baton-harness`；
//! `baton-harness` 沒有可執行檔、UI E2E 只是它的測試目標；安裝程式只打包那三個 sidecar 與 Web UI，沒有其他資源。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

const HARNESS: &str = "baton-harness";
const DESKTOP: &str = "baton-desktop";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read_json(relative: &str) -> Value {
    let path = repo_root().join(relative);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("無法讀取 {}：{e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", path.display()))
}

/// workspace 的套件（不解析外部相依；只看宣告）。
fn workspace_packages() -> Vec<Value> {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline", "--manifest-path"])
        .arg(repo_root().join("Cargo.toml"))
        .output()
        .expect("無法執行 cargo metadata");
    assert!(output.status.success(), "cargo metadata 失敗：{}", String::from_utf8_lossy(&output.stderr));
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata 的輸出不是 JSON");
    metadata["packages"].as_array().expect("cargo metadata 沒有 packages").clone()
}

/// 安裝程式打包的 sidecar 名稱（`binaries/<名稱>`）。
fn bundled_sidecars() -> Vec<String> {
    let conf = read_json("src-tauri/tauri.bundle.conf.json");
    conf["bundle"]["externalBin"]
        .as_array()
        .expect("tauri.bundle.conf.json 沒有 externalBin")
        .iter()
        .map(|bin| bin.as_str().unwrap().strip_prefix("binaries/").expect("sidecar 放在 binaries/ 之下").to_string())
        .collect()
}

fn bin_owner<'a>(packages: &'a [Value], bin: &str) -> &'a Value {
    packages
        .iter()
        .find(|p| p["targets"].as_array().unwrap().iter().any(|t| t["name"] == bin && t["kind"] == serde_json::json!(["bin"])))
        .unwrap_or_else(|| panic!("workspace 中沒有可執行檔 {bin}"))
}

/// 從 `roots` 出發，沿一般與建置相依（不含 dev 相依）走得到的 workspace 套件。
fn production_closure(packages: &[Value], roots: &[String]) -> BTreeSet<String> {
    let by_name: HashMap<&str, &Value> = packages.iter().map(|p| (p["name"].as_str().unwrap(), p)).collect();
    let mut reached = BTreeSet::new();
    let mut pending: Vec<String> = roots.to_vec();
    while let Some(name) = pending.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        let Some(package) = by_name.get(name.as_str()) else { continue };
        for dependency in package["dependencies"].as_array().unwrap() {
            let kind = dependency["kind"].as_str();
            if kind != Some("dev") && by_name.contains_key(dependency["name"].as_str().unwrap()) {
                pending.push(dependency["name"].as_str().unwrap().to_string());
            }
        }
    }
    reached
}

#[test]
fn the_shipped_programs_do_not_depend_on_the_harness() {
    let packages = workspace_packages();
    let mut roots: Vec<String> =
        bundled_sidecars().iter().map(|bin| bin_owner(&packages, bin)["name"].as_str().unwrap().to_string()).collect();
    roots.push(DESKTOP.to_string());
    // Runtime 從自己的資料夾啟動 baton-mcp-shim.exe（Controller CLI 的宿主與 Worker 的 MCP 伺服器），安裝程式必須一起打包
    assert_eq!(roots, ["baton-runtime", "baton-guardian", "baton-mcp-shim", DESKTOP], "安裝程式打包的程式");

    let closure = production_closure(&packages, &roots);
    assert!(closure.contains("baton-runtime") && closure.contains("baton-api"), "相依圖有展開：{closure:?}");
    assert!(!closure.contains(HARNESS), "正式建置的程式相依到 {HARNESS}：{closure:?}");
}

#[test]
fn the_harness_has_no_executable_and_the_ui_e2e_is_only_a_test_target() {
    let packages = workspace_packages();
    let harness = packages.iter().find(|p| p["name"] == HARNESS).expect("workspace 中有 baton-harness");
    let targets = harness["targets"].as_array().unwrap();
    let kinds: BTreeSet<String> =
        targets.iter().flat_map(|t| t["kind"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string())).collect();
    assert_eq!(kinds, BTreeSet::from(["lib".to_string(), "test".to_string()]), "baton-harness 只有函式庫與測試目標");
    let e2e = targets.iter().find(|t| t["name"] == "ui_e2e").expect("UI E2E 的測試目標");
    assert_eq!(e2e["kind"], serde_json::json!(["test"]));
}

#[test]
fn the_installer_bundles_only_the_sidecars_and_the_web_ui() {
    let base = read_json("src-tauri/tauri.conf.json");
    let bundle = read_json("src-tauri/tauri.bundle.conf.json");
    assert_eq!(base["build"]["frontendDist"], "../web/dist", "安裝程式內的 Web UI 是 web/dist");
    for conf in [&base, &bundle] {
        assert!(conf["bundle"]["resources"].is_null(), "安裝程式不打包額外資源：{conf}");
    }
    assert_eq!(bundle["bundle"].as_object().unwrap().keys().collect::<Vec<_>>(), ["externalBin"]);
    // web/dist 只由 web/index.html 與 web/src 建置；Web UI 原始碼不引用 E2E 的任何東西
    for entry in std::fs::read_dir(repo_root().join("web").join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains("ui_e2e") && !text.contains(HARNESS), "{} 引用了測試專用組態", path.display());
    }
}
