//! Domain 核心：純邏輯、無 I/O。
//!
//! 目前有產品識別、時間型別、本機憑證邏輯、Workspace 狀態機、Workspace 授權與 Goal／Task（含六項最小派工契約）；其餘實體的狀態機由後續的票逐步加入。

pub mod auth;
pub mod authorization;
pub mod config;
pub mod event;
pub mod platform;
pub mod product;
pub mod storage_location;
pub mod task;
pub mod time;
pub mod workspace;

pub use time::Timestamp;
