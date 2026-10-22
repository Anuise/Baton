//! Domain 核心：純邏輯、無 I/O。
//!
//! 目前有產品識別、時間型別與本機憑證邏輯；實體狀態機由後續的票逐步加入。

pub mod auth;
pub mod config;
pub mod product;
pub mod time;

pub use time::Timestamp;
