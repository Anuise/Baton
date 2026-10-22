//! Domain 核心：純邏輯、無 I/O。
//!
//! 目前有產品識別、時間型別、本機憑證邏輯、Workspace 狀態機、Workspace 授權與 Goal／Task（含六項最小派工契約）、account_profile 狀態機與首次啟動精靈判定，Run／execution lease／派工 Outbox，多帳號排程與容量、額度觀測與判斷，備份的觸發、保留與還原演練核對、桌面生命週期與完全退出，以及 Task 工作目錄（worktree／輸出目錄）與整合佇列；其餘實體的狀態機由後續的票逐步加入。

pub mod agent;
pub mod approval;
pub mod artifact;
pub mod auth;
pub mod authority;
pub mod claude_code;
pub mod cleanup;
pub mod authorization;
pub mod backup;
pub mod budget;
pub mod config;
pub mod context;
pub mod conversation;
pub mod decision;
pub mod event;
pub mod external_operation;
pub mod guard;
pub mod guardian;
pub mod handoff;
pub mod integration;
pub mod knowledge;
pub mod lifecycle;
pub mod login;
pub mod maintenance;
pub mod message;
pub mod metrics;
pub mod permission;
pub mod platform;
pub mod product;
pub mod profile;
pub mod quota;
pub mod recovery;
pub mod review;
pub mod setup;
pub mod run;
pub mod scheduler;
pub mod session;
pub mod shared_settings;
pub mod storage_health;
pub mod storage_location;
pub mod task;
pub mod time;
pub mod timeline;
pub mod understanding;
pub mod verification;
pub mod verification_profile;
pub mod workdir;
pub mod workflow;
pub mod workspace;

pub use time::Timestamp;
