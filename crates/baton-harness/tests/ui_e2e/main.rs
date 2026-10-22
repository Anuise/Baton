//! UI E2E（spec Testing Decisions 3，票 53）：以 Playwright 操作真實的 Web UI（`web/dist`），
//! 經真實 HTTP 連到以 fake Port 啟動的真實 Runtime 與 API。
//!
//! 測試專用的 Runtime 組態只存在於這個測試目標中（`baton-harness` 的 `tests/`），不會進入正式建置或安裝程式
//! （由 `tests/test_only_config.rs` 檢查）：
//! - Runtime：`Harness`（真實 Domain、Store、Scheduler／Supervisor 與 Axum API，Port 全是 fake）。
//! - `server`：在 loopback 上提供 `web/dist`，並把 `/api/` 轉給 Runtime，轉送時帶上桌面憑證與固定標頭、
//!   去掉瀏覽器的 Origin；也就是扮演桌面殼的角色，Runtime 的 API 防護完全不放寬。
//! - `browser`：以 JSON 行協定驅動 `driver.mjs`（Playwright + Chromium）。
//!
//! 全部測試都標為 `#[ignore]`：一般的 `cargo test` 顯示為 ignored（不是通過）。
//! 以 `node scripts/test-e2e.mjs`（或設定了 `BATON_PLAYWRIGHT_DIR` 的 `pnpm test`）執行；找不到 Playwright 時那兩個腳本標為略過，
//! 直接以 `--ignored` 執行卻找不到 Playwright 時測試失敗。

mod browser;
mod decision_center_batch;
mod dispatch_to_completion;
mod server;

use std::path::PathBuf;

use baton_domain::Timestamp;
use baton_harness::Harness;

use crate::browser::Browser;
use crate::server::WebUiServer;

/// fake Clock 的起點（2023-11-14 22:13:20.123 UTC）。
pub const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

/// 已建置的 Web UI。
pub fn web_dist() -> PathBuf {
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(dist.join("index.html").is_file(), "找不到 {}：請先執行 pnpm --filter web build", dist.display());
    dist
}

/// 一次 UI E2E 的全部組件：fake Port 的 Runtime、提供 Web UI 的伺服器、瀏覽器。
pub struct UiE2e {
    pub harness: Harness,
    pub server: WebUiServer,
    pub browser: Browser,
}

impl UiE2e {
    /// 啟動 Runtime（已有一個可派工的帳號）與伺服器，開啟瀏覽器並載入 Web UI。
    pub async fn start() -> Self {
        let harness = Harness::builder().clock_at(CLOCK_START).start().await;
        // 派工需要一個可派工的帳號；帳號的新增與登入由首次精靈的旅程涵蓋（smoke-desktop），這裡直接備妥
        harness.add_account("主要帳號", "main@example.com").await;
        let server = WebUiServer::start(&harness, web_dist()).await;
        let browser = Browser::open().await;
        browser.goto(&server.url("/")).await;
        // Web UI 連上 Runtime：狀態列顯示運作中
        browser.wait_visible(&[browser::text("Runtime 運作中")]).await;
        Self { harness, server, browser }
    }

    /// 關閉瀏覽器並確認它的程序都已結束（Runtime 與資料目錄隨 Harness 結束）。
    pub async fn finish(self) {
        self.browser.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "UI E2E：需要 Playwright，以 node scripts/test-e2e.mjs 執行"]
async fn the_web_ui_reaches_the_runtime_through_the_test_only_shell() {
    let e2e = UiE2e::start().await;
    // 經伺服器轉送的請求通過 Runtime 的 API 防護：畫面上的產品名稱取自 Runtime 回報
    let product = baton_domain::product::PRODUCT_NAME;
    e2e.browser.wait_visible(&[browser::role("heading", product)]).await;
    // 伺服器確實帶上了桌面憑證、去掉了瀏覽器的 Origin：Runtime 的日誌沒有任何被防護拒絕的請求
    let log = e2e.harness.log_text();
    let denied: Vec<&str> = log.lines().filter(|l| l.contains(" -> 401 ") || l.contains(" -> 403 ")).collect();
    assert!(log.contains("GET /api/v1/health -> 200 ok"), "{log}");
    assert_eq!(denied, Vec::<&str>::new());
    e2e.finish().await;
}
