//! Domain 核心：純邏輯、無 I/O。
//!
//! 目前有產品識別、時間型別、本機憑證邏輯、Workspace 與 account_profile 狀態機、首次啟動精靈判定；其餘實體的狀態機由後續的票逐步加入。

pub mod auth;
pub mod config;
pub mod event;
pub mod platform;
pub mod product;
pub mod profile;
pub mod setup;
pub mod storage_location;
pub mod time;
pub mod workspace;

pub use time::Timestamp;
