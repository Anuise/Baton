//! 產品名稱的單一來源（spec D-01）。
//!
//! 改名只需修改這裡與 `src-tauri/tauri.conf.json` 的安裝設定；
//! `tests/product_name.rs` 會確認兩處一致。

/// 對使用者顯示的產品名稱。
pub const PRODUCT_NAME: &str = "Baton";

/// 本機資料目錄名稱（`%LOCALAPPDATA%\<名稱>\`，spec D-17）。
pub const DATA_DIR_NAME: &str = "Baton";

/// Runtime 預設監聽的 loopback 埠（spec D-23 的「既定埠」）。
pub const DEFAULT_RUNTIME_PORT: u16 = 47821;
