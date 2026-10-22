//! Domain 核心：純邏輯、無 I/O。
//!
//! 目前只有產品識別與時間型別；實體狀態機由後續的票逐步加入。

pub mod config;
pub mod platform;
pub mod product;
pub mod time;

pub use time::Timestamp;
