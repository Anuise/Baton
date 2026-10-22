//! Store：SQLite 存取。
//!
//! - 所有寫入都經過單一寫入佇列：一條專用執行緒擁有唯一的寫入連線，依序執行排進來的工作，
//!   每個工作是一個短交易（spec D-17；issue #3 §11.1）。
//! - 對外只暴露「在一個交易中套用 Domain 結果」與唯讀查詢；唯讀查詢使用另一條獨立的連線。
//! - 啟動時讀回驗證 PRAGMA；`schema_migrations` 記錄資料結構版本，不相容（較新）的版本拒絕寫入。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use baton_domain::auth::{AgentScope, CredentialRecord, Role};
use baton_domain::authorization::{
    self, Admission, AdmissionDenied, AuthorizationError, AuthorizationRevision, Capability, ConfirmOutcome,
    ConfirmRequest,
};
use baton_domain::event::DomainEvent;
use baton_domain::storage_location::{self, RejectedLocation};
use baton_domain::workspace::{self, Workspace, WorkspaceCommand, WorkspaceError, WorkspaceStatus};
use baton_domain::Timestamp;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use tokio::sync::oneshot;

mod agent;
mod artifacts;
pub mod backup;
mod decisions;
mod guards;
mod integration;
mod knowledge;
mod lifecycle;
mod messages;
mod profiles;
mod quota;
mod runs;
mod scheduling;
mod sessions;
mod shared_settings;
mod tasks;
mod verification;
mod workdirs;
pub use agent::AgentContext;
pub use artifacts::NewArtifactVersion;
pub use decisions::{AnswerResult, DecisionApplyError, DecisionList, DecisionOverview, PendingByWorkspace};
pub use guards::{GuardApplyError, GuardOverview};
pub use integration::{IntegrationApplyError, IntegrationWork};
pub use knowledge::{KnowledgeApplyError, KnowledgeAudience, NewKnowledgeRequest, ScopeRequest};
pub use lifecycle::{
    lifecycle_file_path, read_lifecycle_file, write_lifecycle_file, LifecycleApplyError, LifecycleSnapshot,
    WorkspaceLifecycleOutcome, LIFECYCLE_FILE_NAME,
};
pub use messages::{Inbox, MessageApplyError, SendOutcome};
pub use profiles::ProfileApplyError;
pub use quota::{AccountQuota, QuotaApplyError};
pub use runs::{ExecutingRun, PendingLaunch, RunApplyError};
pub use sessions::TranscriptProgress;
pub use scheduling::{
    QueuedRequest, ScheduleOutcome, ScheduleRequest, SchedulingSnapshot, StartedRun, WaitingTask,
};
pub use tasks::{TaskApplyError, TaskBoardData, TaskDetailData};
pub use verification::{AcceptanceApplyError, PendingVerification};
pub use workdirs::WorkdirCandidate;

/// 控制資料庫檔名，位於資料目錄之下。
pub const DB_FILE_NAME: &str = "control.sqlite";

/// 每個連線的 busy timeout；逾時就回報錯誤，不無限等待。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("無法建立資料目錄 {0}：{1}")]
    CreateDir(PathBuf, std::io::Error),
    #[error("資料庫錯誤：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("資料庫設定未生效：{0}")]
    PragmaNotApplied(String),
    #[error("資料庫工作被中斷")]
    Join,
    #[error("資料目錄 {path} 不能使用：{reason}。請改用本機磁碟上的目錄（預設為 %LOCALAPPDATA%\\Baton），或以 BATON_DATA_DIR 指定")]
    UnsupportedLocation { path: PathBuf, reason: RejectedLocation },
    #[error("套用 migration 之前的備份失敗，沒有套用 migration：{0}")]
    PreMigrationBackup(String),
    #[error("資料庫的 schema 版本 {found} 比這個程式支援的 {supported} 新，拒絕寫入；請升級程式，不要用舊版修改這份資料")]
    IncompatibleSchema { found: usize, supported: usize },
}

pub type StoreResult<T> = Result<T, StoreError>;

/// 套用 Workspace 命令的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceApplyError {
    #[error(transparent)]
    Rejected(#[from] WorkspaceError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for WorkspaceApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 確認授權的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum AuthorizationApplyError {
    #[error(transparent)]
    Rejected(#[from] AuthorizationError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// 准入檢查的失敗：被拒絕（附原因）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum AdmissionCheckError {
    #[error(transparent)]
    Denied(#[from] AdmissionDenied),
    #[error(transparent)]
    Store(#[from] StoreError),
}

struct Migration {
    name: &'static str,
    sql: &'static str,
}

/// 依序套用；第 N 個的版本號是 N（從 1 起）。已發布的項目不可修改，只能在後面追加。
const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "runtime_boot",
        sql: "CREATE TABLE runtime_boot (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at INTEGER NOT NULL
        );",
    },
    // 只存 token 的雜湊；role 預留 Agent 憑證（spec D-22）
    Migration {
        name: "credential",
        sql: "CREATE TABLE credential (
            token_hash TEXT PRIMARY KEY,
            role TEXT NOT NULL CHECK (role IN ('desktop', 'agent')),
            issued_at INTEGER NOT NULL,
            expires_at INTEGER
        );",
    },
    // 時間一律是 UTC 毫秒（spec D-33）；event 是追加式，禁止更新與刪除
    Migration {
        name: "workspace_and_event",
        sql: "CREATE TABLE workspace (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
            version INTEGER NOT NULL CHECK (version >= 1),
            created_at INTEGER NOT NULL,
            archived_at INTEGER
        );
        CREATE UNIQUE INDEX workspace_active_path ON workspace (lower(path)) WHERE status = 'active';
        CREATE TABLE event (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            aggregate_type TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            aggregate_version INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            occurred_at INTEGER NOT NULL,
            payload TEXT NOT NULL CHECK (json_valid(payload)),
            UNIQUE (aggregate_type, aggregate_id, aggregate_version)
        );
        CREATE TRIGGER event_no_update BEFORE UPDATE ON event
        BEGIN SELECT RAISE(ABORT, 'event 是追加式資料表，不可更新'); END;
        CREATE TRIGGER event_no_delete BEFORE DELETE ON event
        BEGIN SELECT RAISE(ABORT, 'event 是追加式資料表，不可刪除'); END;",
    },
    // 每次確認授權追加一個 revision，之後不可修改（改授權 = 追加新的 revision）
    Migration {
        name: "authorization_revision",
        sql: "CREATE TABLE authorization_revision (
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            revision INTEGER NOT NULL CHECK (revision >= 1),
            grants TEXT NOT NULL CHECK (json_valid(grants)),
            source TEXT NOT NULL CHECK (json_valid(source)),
            confirmed_at INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, revision)
        );
        CREATE TRIGGER authorization_revision_no_update BEFORE UPDATE ON authorization_revision
        BEGIN SELECT RAISE(ABORT, 'authorization_revision 是追加式資料表，不可更新'); END;
        CREATE TRIGGER authorization_revision_no_delete BEFORE DELETE ON authorization_revision
        BEGIN SELECT RAISE(ABORT, 'authorization_revision 是追加式資料表，不可刪除'); END;",
    },
    // Goal、Task 與六項最小契約；契約每次變更追加一個版本，之後不可修改
    Migration {
        name: "goal_task_contract",
        sql: "CREATE TABLE goal (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            title TEXT NOT NULL,
            version INTEGER NOT NULL CHECK (version >= 1),
            created_at INTEGER NOT NULL
        );
        CREATE TABLE task (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            goal_id TEXT NOT NULL REFERENCES goal (id),
            title TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'ready', 'running', 'waiting_decision',
                'waiting_resource', 'blocked', 'verifying', 'completed', 'cancelled')),
            version INTEGER NOT NULL CHECK (version >= 1),
            contract_version INTEGER NOT NULL CHECK (contract_version >= 1),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX task_workspace ON task (workspace_id);
        CREATE TABLE task_contract_version (
            task_id TEXT NOT NULL REFERENCES task (id),
            version INTEGER NOT NULL CHECK (version >= 1),
            content TEXT NOT NULL CHECK (json_valid(content)),
            note TEXT,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (task_id, version)
        );
        CREATE TRIGGER task_contract_version_no_update BEFORE UPDATE ON task_contract_version
        BEGIN SELECT RAISE(ABORT, 'task_contract_version 是追加式資料表，不可更新'); END;
        CREATE TRIGGER task_contract_version_no_delete BEFORE DELETE ON task_contract_version
        BEGIN SELECT RAISE(ABORT, 'task_contract_version 是追加式資料表，不可刪除'); END;
        CREATE TABLE task_dependency (
            task_id TEXT NOT NULL REFERENCES task (id),
            depends_on TEXT NOT NULL REFERENCES task (id),
            PRIMARY KEY (task_id, depends_on),
            CHECK (task_id <> depends_on)
        );",
    },
    // Run、execution lease 與派工 Outbox（票 10）。
    // 同一個 Task 任何時刻最多一個尚未釋放的 lease 由唯一索引保證，不只靠應用程式邏輯
    Migration {
        name: "run_lease_outbox",
        sql: "CREATE TABLE run (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            status TEXT NOT NULL CHECK (status IN ('queued', 'starting', 'running', 'succeeded', 'failed',
                'interrupted', 'cancelled')),
            version INTEGER NOT NULL CHECK (version >= 1),
            generation INTEGER NOT NULL CHECK (generation >= 1),
            contract_version INTEGER NOT NULL,
            authorization_revision INTEGER,
            command_id TEXT,
            pid INTEGER,
            process_created_at INTEGER,
            command_hash TEXT,
            cancel_requested_at INTEGER,
            end_reason TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            started_at INTEGER,
            ended_at INTEGER,
            UNIQUE (task_id, generation)
        );
        CREATE INDEX run_workspace ON run (workspace_id);
        CREATE UNIQUE INDEX run_command_id ON run (command_id) WHERE command_id IS NOT NULL;
        CREATE TABLE execution_lease (
            run_id TEXT PRIMARY KEY REFERENCES run (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            generation INTEGER NOT NULL,
            acquired_at INTEGER NOT NULL,
            last_heartbeat_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            released_at INTEGER,
            release_reason TEXT
        );
        CREATE UNIQUE INDEX execution_lease_one_held_per_task ON execution_lease (task_id)
            WHERE released_at IS NULL;
        CREATE TABLE outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            run_id TEXT NOT NULL REFERENCES run (id),
            payload TEXT NOT NULL CHECK (json_valid(payload)),
            created_at INTEGER NOT NULL,
            delivered_at INTEGER,
            UNIQUE (kind, run_id)
        );",
    },
    // 只保存核對結果（登入與否、認證來源、帳號身分），不保存任何登入資料；設定目錄不重複（不分大小寫）
    Migration {
        name: "account_profile_and_setup_state",
        sql: "CREATE TABLE account_profile (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            config_dir TEXT NOT NULL,
            expected_account TEXT,
            status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
            verification TEXT CHECK (verification IS NULL OR json_valid(verification)),
            version INTEGER NOT NULL CHECK (version >= 1),
            created_at INTEGER NOT NULL,
            archived_at INTEGER
        );
        CREATE UNIQUE INDEX account_profile_config_dir ON account_profile (lower(config_dir));
        CREATE TABLE setup_state (
            key TEXT PRIMARY KEY,
            acknowledged_at INTEGER NOT NULL
        );",
    },
    // Agent 憑證的範圍（票 11）：Workspace、Run 與世代；桌面憑證這三欄為空。每個 Run 最多一個憑證
    Migration {
        name: "agent_credential_scope",
        sql: "ALTER TABLE credential ADD COLUMN workspace_id TEXT;
        ALTER TABLE credential ADD COLUMN run_id TEXT REFERENCES run (id);
        ALTER TABLE credential ADD COLUMN generation INTEGER;
        CREATE UNIQUE INDEX credential_one_per_run ON credential (run_id) WHERE run_id IS NOT NULL;",
    },
    // Worker 回報的追加式稽核（票 11）：accepted = 0 是來自已不是寫入所有者的世代，內容保留但沒有改變任何狀態
    Migration {
        name: "agent_report",
        sql: "CREATE TABLE agent_report (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            run_id TEXT NOT NULL REFERENCES run (id),
            generation INTEGER NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('progress', 'checkpoint', 'blocked')),
            accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
            content TEXT NOT NULL CHECK (json_valid(content)),
            received_at INTEGER NOT NULL
        );
        CREATE INDEX agent_report_run ON agent_report (run_id, kind, accepted);
        CREATE TRIGGER agent_report_no_update BEFORE UPDATE ON agent_report
        BEGIN SELECT RAISE(ABORT, 'agent_report 是追加式資料表，不可更新'); END;
        CREATE TRIGGER agent_report_no_delete BEFORE DELETE ON agent_report
        BEGIN SELECT RAISE(ABORT, 'agent_report 是追加式資料表，不可刪除'); END;",
    },
    // 多帳號並行排程（票 19）：Run 記下使用的帳號；派工請求佇列（ready 的 Task 等名額）；容量預設只有一列。
    // 佇列只存「誰在排隊」，等待原因每次由目前狀態重新判定，不落地
    Migration {
        name: "scheduling",
        sql: "ALTER TABLE run ADD COLUMN profile_id TEXT;
        CREATE INDEX run_profile ON run (profile_id);
        CREATE TABLE dispatch_request (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL UNIQUE REFERENCES task (id),
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            priority INTEGER NOT NULL DEFAULT 0,
            requested_at INTEGER NOT NULL
        );
        CREATE TABLE scheduler_setting (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL CHECK (mode IN ('reserve_handoff', 'higher_concurrency')),
            version INTEGER NOT NULL CHECK (version >= 1),
            updated_at INTEGER NOT NULL
        );",
    },
    // 成果與它的不可變版本 manifest（票 12）：同 Workspace、同 Task、同名是同一個成果；版本只追加、不可更新或刪除，
    // storage_path 唯一，所以兩個版本不可能指向同一個檔案（新版本不覆寫舊版本）
    Migration {
        name: "artifact",
        sql: "CREATE TABLE artifact (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            UNIQUE (workspace_id, task_id, name)
        );
        CREATE TABLE artifact_version (
            artifact_id TEXT NOT NULL REFERENCES artifact (id),
            version INTEGER NOT NULL CHECK (version >= 1),
            description TEXT NOT NULL,
            size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
            sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
            storage_path TEXT NOT NULL UNIQUE,
            run_id TEXT NOT NULL REFERENCES run (id),
            generation INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (artifact_id, version)
        );
        CREATE TRIGGER artifact_no_update BEFORE UPDATE ON artifact
        BEGIN SELECT RAISE(ABORT, 'artifact 是追加式資料表，不可更新'); END;
        CREATE TRIGGER artifact_no_delete BEFORE DELETE ON artifact
        BEGIN SELECT RAISE(ABORT, 'artifact 是追加式資料表，不可刪除'); END;
        CREATE TRIGGER artifact_version_no_update BEFORE UPDATE ON artifact_version
        BEGIN SELECT RAISE(ABORT, 'artifact_version 是追加式資料表，不可更新'); END;
        CREATE TRIGGER artifact_version_no_delete BEFORE DELETE ON artifact_version
        BEGIN SELECT RAISE(ABORT, 'artifact_version 是追加式資料表，不可刪除'); END;",
    },
    // 共同設定的每個採用版本不可變（改設定 = 採用新版本）；Run 與版本的綁定一經建立也不可更改（spec D-09）
    Migration {
        name: "shared_settings_version",
        sql: "CREATE TABLE shared_settings_version (
            version INTEGER PRIMARY KEY CHECK (version >= 1),
            content_hash TEXT NOT NULL,
            bundle TEXT NOT NULL CHECK (json_valid(bundle)),
            sources TEXT NOT NULL CHECK (json_valid(sources)),
            exclusions TEXT NOT NULL CHECK (json_valid(exclusions)),
            adopted_at INTEGER NOT NULL
        );
        CREATE TRIGGER shared_settings_version_no_update BEFORE UPDATE ON shared_settings_version
        BEGIN SELECT RAISE(ABORT, 'shared_settings_version 是追加式資料表，不可更新'); END;
        CREATE TRIGGER shared_settings_version_no_delete BEFORE DELETE ON shared_settings_version
        BEGIN SELECT RAISE(ABORT, 'shared_settings_version 是追加式資料表，不可刪除'); END;
        CREATE TABLE run_settings_snapshot (
            run_id TEXT PRIMARY KEY REFERENCES run (id),
            version INTEGER NOT NULL REFERENCES shared_settings_version (version),
            content_hash TEXT NOT NULL,
            directory TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TRIGGER run_settings_snapshot_no_update BEFORE UPDATE ON run_settings_snapshot
        BEGIN SELECT RAISE(ABORT, 'run_settings_snapshot 是追加式資料表，不可更新'); END;
        CREATE TRIGGER run_settings_snapshot_no_delete BEFORE DELETE ON run_settings_snapshot
        BEGIN SELECT RAISE(ABORT, 'run_settings_snapshot 是追加式資料表，不可刪除'); END;",
    },
    // 額度觀測（票 27）：每筆觀測的來源、時間、可信度與原始資料參照；本機 Run 回報的用量（推估的依據）；
    // 使用者設定的方案上限；外部觀測器的輪詢時間。觀測不到的值存 NULL（未知），不填零。
    // 超過七日視窗的觀測與用量在輪詢時刪除（保留 8 天）
    Migration {
        name: "quota",
        sql: "CREATE TABLE quota_observation (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id TEXT NOT NULL,
            origin TEXT NOT NULL CHECK (origin IN ('adapter', 'estimate', 'observer')),
            source TEXT NOT NULL CHECK (source IN ('observed', 'estimated')),
            quota_window TEXT CHECK (quota_window IN ('five_hour', 'seven_day')),
            status TEXT CHECK (status IN ('allowed', 'warning', 'exhausted')),
            used REAL CHECK (used IS NULL OR used >= 0),
            tokens INTEGER CHECK (tokens IS NULL OR tokens >= 0),
            limit_tokens INTEGER CHECK (limit_tokens IS NULL OR limit_tokens > 0),
            reset_at INTEGER,
            observed_at INTEGER NOT NULL,
            confidence TEXT NOT NULL CHECK (confidence IN ('observed', 'estimated', 'unknown')),
            raw_ref TEXT NOT NULL,
            raw TEXT
        );
        CREATE INDEX quota_observation_latest ON quota_observation (profile_id, origin, quota_window, seq);
        CREATE INDEX quota_observation_time ON quota_observation (observed_at);
        CREATE TABLE quota_usage (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            tokens INTEGER NOT NULL CHECK (tokens >= 0),
            reported_at INTEGER NOT NULL,
            raw TEXT NOT NULL
        );
        CREATE INDEX quota_usage_profile ON quota_usage (profile_id, reported_at);
        CREATE TABLE quota_plan_limit (
            profile_id TEXT PRIMARY KEY,
            five_hour_tokens INTEGER CHECK (five_hour_tokens IS NULL OR five_hour_tokens > 0),
            seven_day_tokens INTEGER CHECK (seven_day_tokens IS NULL OR seven_day_tokens > 0),
            version INTEGER NOT NULL CHECK (version >= 1),
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE quota_poll (
            profile_id TEXT PRIMARY KEY,
            polled_at INTEGER NOT NULL
        );",
    },
    // Worker 間訊息與每個收件 Task 的交付（票 18）。訊息本身追加式；交付只會填入 delivered_at、read_at。
    // 交付以 (workspace, message_id, 收件 Task) 為主鍵：同一則訊息對同一個收件人不可能有第二份
    Migration {
        name: "message_and_delivery",
        sql: "CREATE TABLE message (
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            message_id TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            correlation_id TEXT NOT NULL,
            in_reply_to TEXT,
            kind TEXT NOT NULL CHECK (kind IN ('note', 'question', 'answer', 'result')),
            subject TEXT NOT NULL,
            body TEXT NOT NULL,
            sender_run_id TEXT NOT NULL REFERENCES run (id),
            sender_task_id TEXT NOT NULL REFERENCES task (id),
            sender_generation INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, message_id)
        );
        CREATE TRIGGER message_no_update BEFORE UPDATE ON message
        BEGIN SELECT RAISE(ABORT, 'message 是追加式資料表，不可更新'); END;
        CREATE TRIGGER message_no_delete BEFORE DELETE ON message
        BEGIN SELECT RAISE(ABORT, 'message 是追加式資料表，不可刪除'); END;
        CREATE TABLE message_delivery (
            workspace_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            recipient_task_id TEXT NOT NULL REFERENCES task (id),
            delivered_at INTEGER,
            read_at INTEGER,
            PRIMARY KEY (workspace_id, message_id, recipient_task_id),
            FOREIGN KEY (workspace_id, message_id) REFERENCES message (workspace_id, message_id)
        );
        CREATE INDEX message_delivery_inbox ON message_delivery (recipient_task_id, read_at);
        CREATE TRIGGER message_delivery_no_delete BEFORE DELETE ON message_delivery
        BEGIN SELECT RAISE(ABORT, 'message_delivery 不可刪除'); END;",
    },
    // 裁定請求與裁定（票 22）。請求只能從 pending 改成 answered（其餘欄位不可變）、不可刪除；
    // 裁定與單次批准的消耗紀錄是追加式；decided_by 在資料庫層就只允許桌面憑證
    Migration {
        name: "decision_request_and_decision",
        sql: "CREATE TABLE decision_request (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            run_id TEXT NOT NULL REFERENCES run (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            kind TEXT NOT NULL CHECK (kind IN ('agent', 'permission')),
            status TEXT NOT NULL CHECK (status IN ('pending', 'answered')),
            question TEXT NOT NULL,
            reason TEXT NOT NULL,
            options TEXT NOT NULL CHECK (json_valid(options)),
            recommendation TEXT,
            evidence TEXT NOT NULL CHECK (json_valid(evidence)),
            urgency TEXT NOT NULL CHECK (urgency IN ('low', 'normal', 'high')),
            deadline_at INTEGER,
            permission TEXT CHECK (permission IS NULL OR json_valid(permission)),
            created_at INTEGER NOT NULL
        );
        CREATE INDEX decision_request_workspace ON decision_request (workspace_id, status);
        CREATE TRIGGER decision_request_only_answer BEFORE UPDATE ON decision_request
        WHEN NOT (OLD.status = 'pending' AND NEW.status = 'answered' AND NEW.id = OLD.id
                  AND NEW.question = OLD.question AND NEW.options = OLD.options)
        BEGIN SELECT RAISE(ABORT, 'decision_request 只能從 pending 改成 answered'); END;
        CREATE TRIGGER decision_request_no_delete BEFORE DELETE ON decision_request
        BEGIN SELECT RAISE(ABORT, 'decision_request 不可刪除'); END;
        CREATE TABLE decision_request_block (
            request_id TEXT NOT NULL REFERENCES decision_request (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            PRIMARY KEY (request_id, task_id)
        );
        CREATE INDEX decision_request_block_task ON decision_request_block (task_id);
        CREATE TABLE decision (
            request_id TEXT PRIMARY KEY REFERENCES decision_request (id),
            option_id TEXT NOT NULL,
            note TEXT,
            decided_by TEXT NOT NULL CHECK (decided_by = 'desktop'),
            decided_at INTEGER NOT NULL
        );
        CREATE TRIGGER decision_no_update BEFORE UPDATE ON decision
        BEGIN SELECT RAISE(ABORT, 'decision 是追加式資料表，不可更新'); END;
        CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
        BEGIN SELECT RAISE(ABORT, 'decision 是追加式資料表，不可刪除'); END;
        CREATE TABLE decision_approval_use (
            request_id TEXT PRIMARY KEY REFERENCES decision (request_id),
            run_id TEXT NOT NULL REFERENCES run (id),
            used_at INTEGER NOT NULL
        );
        CREATE TRIGGER decision_approval_use_no_update BEFORE UPDATE ON decision_approval_use
        BEGIN SELECT RAISE(ABORT, 'decision_approval_use 是追加式資料表，不可更新'); END;
        CREATE TRIGGER decision_approval_use_no_delete BEFORE DELETE ON decision_approval_use
        BEGIN SELECT RAISE(ABORT, 'decision_approval_use 是追加式資料表，不可刪除'); END;",
    },
    Migration {
        // 票 13：成果接受與驗證紀錄。同一個 Run 最多一筆紀錄、同一個 Task 最多一筆「通過」
        name: "task_verification",
        sql: "CREATE TABLE task_verification (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            run_id TEXT NOT NULL UNIQUE REFERENCES run (id),
            generation INTEGER NOT NULL,
            contract_version INTEGER NOT NULL,
            method TEXT NOT NULL CHECK (method IN ('checks', 'manual')),
            outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed')),
            evidence TEXT NOT NULL,
            decided_at INTEGER NOT NULL
        );
        CREATE INDEX task_verification_workspace ON task_verification (workspace_id);
        CREATE UNIQUE INDEX task_verification_one_pass ON task_verification (task_id) WHERE outcome = 'passed';
        CREATE TRIGGER task_verification_no_update BEFORE UPDATE ON task_verification
        BEGIN SELECT RAISE(ABORT, 'task_verification 是追加式資料表，不可更新'); END;
        CREATE TRIGGER task_verification_no_delete BEFORE DELETE ON task_verification
        BEGIN SELECT RAISE(ABORT, 'task_verification 是追加式資料表，不可刪除'); END;",
    },
    // 知識條目與教訓（票 37，第 17 個 migration）：決策、教訓、一般知識同一張表，以 kind 區分。條目列保存目前狀態與目前內容的檢索文字，
    // 內容的每個版本另存於追加式的 knowledge_entry_version（修正只追加，不覆寫）。全文檢索索引（FTS5）不在 migration 中，
    // 而是每次開啟時依可用性建立並從 search_text 重建（spec D-26）
    Migration {
        name: "knowledge",
        sql: "CREATE TABLE knowledge_entry (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL CHECK (kind IN ('decision', 'lesson', 'fact')),
            scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace', 'goal', 'task')),
            workspace_id TEXT REFERENCES workspace (id),
            goal_id TEXT REFERENCES goal (id),
            task_id TEXT REFERENCES task (id),
            status TEXT NOT NULL CHECK (status IN ('candidate', 'verified', 'revoked', 'retired', 'superseded')),
            source TEXT NOT NULL CHECK (json_valid(source)),
            valid_from INTEGER NOT NULL,
            valid_until INTEGER,
            version INTEGER NOT NULL CHECK (version >= 1),
            revision INTEGER NOT NULL CHECK (revision >= 1),
            supersedes TEXT REFERENCES knowledge_entry (id),
            superseded_by TEXT REFERENCES knowledge_entry (id),
            status_reason TEXT,
            search_text TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            CHECK ((scope_kind = 'global') = (workspace_id IS NULL))
        );
        CREATE INDEX knowledge_entry_workspace ON knowledge_entry (workspace_id);
        CREATE TABLE knowledge_entry_version (
            entry_id TEXT NOT NULL REFERENCES knowledge_entry (id),
            version INTEGER NOT NULL CHECK (version >= 1),
            content TEXT NOT NULL CHECK (json_valid(content)),
            note TEXT,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (entry_id, version)
        );
        CREATE TRIGGER knowledge_entry_version_no_update BEFORE UPDATE ON knowledge_entry_version
        BEGIN SELECT RAISE(ABORT, 'knowledge_entry_version 是追加式資料表，不可更新'); END;
        CREATE TRIGGER knowledge_entry_version_no_delete BEFORE DELETE ON knowledge_entry_version
        BEGIN SELECT RAISE(ABORT, 'knowledge_entry_version 是追加式資料表，不可刪除'); END;
        CREATE TRIGGER knowledge_entry_no_delete BEFORE DELETE ON knowledge_entry
        BEGIN SELECT RAISE(ABORT, 'knowledge_entry 不可刪除：撤銷或淘汰只改變狀態'); END;",
    },
    // 桌面生命週期與完全退出（票 25）：Runtime 的停止意圖與最近一次退出的經過只有一列；
    // 每個 Workspace 的「完全退出後自動續跑」與暫停狀態（沒有列的 Workspace 等同預設：關閉、未暫停）
    Migration {
        name: "lifecycle",
        sql: "CREATE TABLE runtime_lifecycle (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            phase TEXT NOT NULL CHECK (phase IN ('running', 'stopping', 'stopped')),
            version INTEGER NOT NULL CHECK (version >= 1),
            last_exit TEXT CHECK (last_exit IS NULL OR json_valid(last_exit)),
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE workspace_lifecycle (
            workspace_id TEXT PRIMARY KEY REFERENCES workspace (id),
            auto_resume_after_exit INTEGER NOT NULL CHECK (auto_resume_after_exit IN (0, 1)),
            paused_since INTEGER,
            version INTEGER NOT NULL CHECK (version >= 1),
            updated_at INTEGER NOT NULL
        );",
    },
    // 票 14：Workspace 依角色的模型覆寫（spec D-30）與 Run 綁定的原生 Session
    Migration {
        name: "model_overrides_and_native_session",
        sql: "ALTER TABLE workspace ADD COLUMN model_overrides TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(model_overrides));
        ALTER TABLE run ADD COLUMN native_session_id TEXT;",
    },
    // Guard 與重犯事件（票 38）：Guard 只能停用、不能刪除；重犯事件是追加式。
    // 被 Guard 攔下的重犯以「Guard + Task + 契約版本」決定識別，佇列重試不會重複記錄
    Migration {
        name: "guard",
        sql: "CREATE TABLE guard (
            id TEXT PRIMARY KEY,
            lesson_id TEXT NOT NULL REFERENCES knowledge_entry (id),
            rule TEXT NOT NULL CHECK (json_valid(rule)),
            status TEXT NOT NULL CHECK (status IN ('enabled', 'disabled')),
            revision INTEGER NOT NULL CHECK (revision >= 1),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX guard_lesson ON guard (lesson_id);
        CREATE TRIGGER guard_no_delete BEFORE DELETE ON guard
        BEGIN SELECT RAISE(ABORT, 'guard 不可刪除：只能停用'); END;
        CREATE TABLE lesson_recurrence (
            id TEXT PRIMARY KEY,
            lesson_id TEXT NOT NULL REFERENCES knowledge_entry (id),
            guard_id TEXT REFERENCES guard (id),
            workspace_id TEXT REFERENCES workspace (id),
            task_id TEXT REFERENCES task (id),
            contract_version INTEGER,
            kind TEXT NOT NULL CHECK (kind IN ('prevented', 'reported')),
            message TEXT NOT NULL,
            occurred_at INTEGER NOT NULL,
            CHECK ((kind = 'prevented') = (guard_id IS NOT NULL))
        );
        CREATE INDEX lesson_recurrence_lesson ON lesson_recurrence (lesson_id);
        CREATE TRIGGER lesson_recurrence_no_update BEFORE UPDATE ON lesson_recurrence
        BEGIN SELECT RAISE(ABORT, 'lesson_recurrence 是追加式資料表，不可更新'); END;
        CREATE TRIGGER lesson_recurrence_no_delete BEFORE DELETE ON lesson_recurrence
        BEGIN SELECT RAISE(ABORT, 'lesson_recurrence 是追加式資料表，不可刪除'); END;",
    },
    // 票 20（第 21 個 migration）：Task 種類、平台登記的工作目錄（worktree／輸出目錄）與單一整合佇列。
    // 同一個 Task 一個工作目錄、可用的目錄路徑不重複；整合佇列任何時刻最多一個處理中的項目
    Migration {
        name: "task_workdir_and_integration_queue",
        sql: "ALTER TABLE task ADD COLUMN kind TEXT NOT NULL DEFAULT 'research'
            CHECK (kind IN ('development', 'research', 'document'));
        CREATE TABLE task_workdir (
            task_id TEXT PRIMARY KEY REFERENCES task (id),
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            kind TEXT NOT NULL CHECK (kind IN ('worktree', 'output_dir')),
            path TEXT NOT NULL,
            branch TEXT,
            base_branch TEXT,
            status TEXT NOT NULL CHECK (status IN ('ready', 'failed')),
            failure TEXT,
            version INTEGER NOT NULL CHECK (version >= 1),
            attempted_at INTEGER NOT NULL,
            ready_at INTEGER
        );
        CREATE INDEX task_workdir_workspace ON task_workdir (workspace_id);
        CREATE UNIQUE INDEX task_workdir_one_path ON task_workdir (path COLLATE NOCASE) WHERE status = 'ready';
        CREATE TABLE integration_entry (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            run_id TEXT NOT NULL UNIQUE REFERENCES run (id),
            generation INTEGER NOT NULL,
            branch TEXT NOT NULL,
            target_branch TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('queued', 'merging', 'verifying', 'advancing', 'integrated', 'failed')),
            version INTEGER NOT NULL CHECK (version >= 1),
            merge_commit TEXT,
            target_tip TEXT,
            checks TEXT NOT NULL CHECK (json_valid(checks)),
            failure TEXT,
            queued_at INTEGER NOT NULL,
            started_at INTEGER,
            finished_at INTEGER
        );
        CREATE INDEX integration_entry_workspace ON integration_entry (workspace_id);
        CREATE UNIQUE INDEX integration_entry_one_active
            ON integration_entry ((status IN ('merging', 'verifying', 'advancing')))
            WHERE status IN ('merging', 'verifying', 'advancing');",
    },
    // 完整裁定中心（票 35，第 22 個 migration）：裁定請求加上目標版本、內容版本、延後、合併與過期，裁定加上處理方式與批次識別。
    // SQLite 不能修改既有的 CHECK 約束，所以重建票 22 的四張表：這四張表只互相參照、沒有其他表參照它們，
    // 先建新表、搬資料、由子到父刪除舊表，再改名（外鍵參照會隨改名更新）。既有請求的目標版本取該 Task 目前的契約版本
    // （權限提示另取 Workspace 目前的授權 revision）
    Migration {
        name: "decision_center_full",
        sql: "CREATE TABLE decision_request_next (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            run_id TEXT NOT NULL REFERENCES run (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            kind TEXT NOT NULL CHECK (kind IN ('agent', 'permission', 'authority')),
            status TEXT NOT NULL CHECK (status IN ('pending', 'answered', 'superseded', 'merged')),
            question TEXT NOT NULL,
            reason TEXT NOT NULL,
            options TEXT NOT NULL CHECK (json_valid(options)),
            recommendation TEXT,
            evidence TEXT NOT NULL CHECK (json_valid(evidence)),
            urgency TEXT NOT NULL CHECK (urgency IN ('low', 'normal', 'high')),
            deadline_at INTEGER,
            permission TEXT CHECK (permission IS NULL OR json_valid(permission)),
            authority TEXT CHECK (authority IS NULL OR json_valid(authority)),
            target_task_id TEXT NOT NULL REFERENCES task (id),
            target_contract_version INTEGER NOT NULL,
            target_authorization_revision INTEGER,
            revision INTEGER NOT NULL CHECK (revision >= 1),
            deferred_until INTEGER,
            merged_into TEXT REFERENCES decision_request_next (id),
            status_reason TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            CHECK ((status = 'merged') = (merged_into IS NOT NULL))
        );
        INSERT INTO decision_request_next
            (id, workspace_id, run_id, task_id, kind, status, question, reason, options, recommendation, evidence, urgency,
             deadline_at, permission, authority, target_task_id, target_contract_version, target_authorization_revision,
             revision, deferred_until, merged_into, status_reason, created_at, updated_at)
        SELECT r.id, r.workspace_id, r.run_id, r.task_id, r.kind, r.status, r.question, r.reason, r.options,
               r.recommendation, r.evidence, r.urgency, r.deadline_at, r.permission, NULL, r.task_id,
               (SELECT t.contract_version FROM task t WHERE t.id = r.task_id),
               CASE r.kind WHEN 'permission' THEN
                   (SELECT MAX(a.revision) FROM authorization_revision a WHERE a.workspace_id = r.workspace_id) END,
               CASE r.status WHEN 'pending' THEN 1 ELSE 2 END, NULL, NULL, NULL, r.created_at, r.created_at
        FROM decision_request r ORDER BY r.rowid;
        CREATE TABLE decision_request_block_next (
            request_id TEXT NOT NULL REFERENCES decision_request_next (id),
            task_id TEXT NOT NULL REFERENCES task (id),
            PRIMARY KEY (request_id, task_id)
        );
        INSERT INTO decision_request_block_next (request_id, task_id)
        SELECT request_id, task_id FROM decision_request_block ORDER BY rowid;
        CREATE TABLE decision_next (
            request_id TEXT PRIMARY KEY REFERENCES decision_request_next (id),
            action TEXT NOT NULL CHECK (action IN ('accept', 'modify', 'answer', 'investigate', 'reject')),
            option_id TEXT,
            note TEXT,
            decided_by TEXT NOT NULL CHECK (decided_by = 'desktop'),
            batch_id TEXT,
            decided_at INTEGER NOT NULL,
            CHECK ((action IN ('accept', 'modify')) = (option_id IS NOT NULL)),
            CHECK (action NOT IN ('modify', 'answer', 'investigate') OR note IS NOT NULL)
        );
        INSERT INTO decision_next (request_id, action, option_id, note, decided_by, batch_id, decided_at)
        SELECT request_id, 'accept', option_id, note, decided_by, NULL, decided_at FROM decision ORDER BY rowid;
        CREATE TABLE decision_approval_use_next (
            request_id TEXT PRIMARY KEY REFERENCES decision_next (request_id),
            run_id TEXT NOT NULL REFERENCES run (id),
            used_at INTEGER NOT NULL
        );
        INSERT INTO decision_approval_use_next (request_id, run_id, used_at)
        SELECT request_id, run_id, used_at FROM decision_approval_use ORDER BY rowid;
        DROP TABLE decision_approval_use;
        DROP TABLE decision;
        DROP TABLE decision_request_block;
        DROP TABLE decision_request;
        ALTER TABLE decision_request_next RENAME TO decision_request;
        ALTER TABLE decision_request_block_next RENAME TO decision_request_block;
        ALTER TABLE decision_next RENAME TO decision;
        ALTER TABLE decision_approval_use_next RENAME TO decision_approval_use;
        CREATE INDEX decision_request_workspace ON decision_request (workspace_id, status);
        CREATE INDEX decision_request_block_task ON decision_request_block (task_id);
        -- 只有待處理的請求能變動，而且只能改可變欄位、內容版本只增不減；結束的狀態不能再改
        CREATE TRIGGER decision_request_guard BEFORE UPDATE ON decision_request
        WHEN NOT (OLD.status = 'pending' AND NEW.revision > OLD.revision AND NEW.id = OLD.id
                  AND NEW.workspace_id = OLD.workspace_id AND NEW.run_id = OLD.run_id AND NEW.task_id = OLD.task_id
                  AND NEW.kind = OLD.kind AND NEW.question = OLD.question AND NEW.reason = OLD.reason
                  AND NEW.options = OLD.options AND NEW.target_task_id = OLD.target_task_id
                  AND NEW.target_contract_version = OLD.target_contract_version
                  AND NEW.target_authorization_revision IS OLD.target_authorization_revision
                  AND NEW.created_at = OLD.created_at)
        BEGIN SELECT RAISE(ABORT, 'decision_request 只有待處理時能變動，且只能改狀態、延後、合併與原因'); END;
        CREATE TRIGGER decision_request_no_delete BEFORE DELETE ON decision_request
        BEGIN SELECT RAISE(ABORT, 'decision_request 不可刪除'); END;
        CREATE TRIGGER decision_request_block_no_delete BEFORE DELETE ON decision_request_block
        BEGIN SELECT RAISE(ABORT, 'decision_request_block 不可刪除'); END;
        CREATE TRIGGER decision_no_update BEFORE UPDATE ON decision
        BEGIN SELECT RAISE(ABORT, 'decision 是追加式資料表，不可更新'); END;
        CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
        BEGIN SELECT RAISE(ABORT, 'decision 是追加式資料表，不可刪除'); END;
        CREATE TRIGGER decision_approval_use_no_update BEFORE UPDATE ON decision_approval_use
        BEGIN SELECT RAISE(ABORT, 'decision_approval_use 是追加式資料表，不可更新'); END;
        CREATE TRIGGER decision_approval_use_no_delete BEFORE DELETE ON decision_approval_use
        BEGIN SELECT RAISE(ABORT, 'decision_approval_use 是追加式資料表，不可刪除'); END;
        -- 相似或手動連結的請求（雙向，較小的識別在前）
        CREATE TABLE decision_request_link (
            request_id TEXT NOT NULL REFERENCES decision_request (id),
            linked_id TEXT NOT NULL REFERENCES decision_request (id),
            created_at INTEGER NOT NULL,
            PRIMARY KEY (request_id, linked_id),
            CHECK (request_id < linked_id)
        );
        CREATE TRIGGER decision_request_link_no_delete BEFORE DELETE ON decision_request_link
        BEGIN SELECT RAISE(ABORT, 'decision_request_link 不可刪除'); END;
        -- Workspace 的決策權限表設定（spec D-15）：只保存被收窄的事項
        CREATE TABLE decision_authority_policy (
            workspace_id TEXT PRIMARY KEY REFERENCES workspace (id),
            revision INTEGER NOT NULL CHECK (revision >= 1),
            narrowed TEXT NOT NULL CHECK (json_valid(narrowed)),
            updated_at INTEGER NOT NULL
        );",
    },
    // 啟動准入（票 24，第 23 個 migration，R1-a）：准入時間與准入時固定的共同設定版本。准入時 `authorization_revision`
    // 改寫為准入當下生效的 revision；票 24 之前的 Run 兩欄都是 NULL（派工時記下的 revision 照舊）
    Migration {
        name: "run_admission",
        sql: "ALTER TABLE run ADD COLUMN admitted_at INTEGER;
        ALTER TABLE run ADD COLUMN settings_version INTEGER;",
    },
    // 票 15（第 24 個 migration）：平台 Session、原生 Session 與程序身分分開保存（issue §3.1～3.2）。
    // 原生識別的唯一性以「執行工具 + 帳號」為範圍；同一個平台 Session 任何時刻最多一個未結束的 Run 驅動它。
    // session_events 是事件表中 Session 聚合的檢視；transcript_segments 只記位置、雜湊與保存進度，內容在成果儲存
    Migration {
        name: "agent_session_and_transcript",
        sql: "CREATE TABLE agent_session (
            id TEXT PRIMARY KEY,
            runtime_type TEXT NOT NULL,
            workspace_id TEXT NOT NULL REFERENCES workspace (id),
            role TEXT NOT NULL CHECK (role IN ('controller', 'worker', 'reviewer')),
            task_id TEXT REFERENCES task (id),
            conversation_id TEXT,
            profile_id TEXT NOT NULL,
            working_directory TEXT NOT NULL,
            native_session_id TEXT,
            state TEXT NOT NULL CHECK (state IN ('allocating', 'active', 'suspended', 'closed', 'unavailable')),
            state_reason TEXT,
            version INTEGER NOT NULL CHECK (version >= 1),
            driver_run_id TEXT,
            predecessor_session_id TEXT REFERENCES agent_session (id),
            replacement_reason TEXT,
            history_gap TEXT,
            resume_capability TEXT CHECK (resume_capability IN ('resumable', 'not_resumable', 'unknown')),
            capability_checked_at INTEGER,
            created_at INTEGER NOT NULL,
            last_activity_at INTEGER NOT NULL,
            CHECK ((driver_run_id IS NOT NULL) = (state IN ('allocating', 'active')))
        );
        CREATE INDEX agent_session_workspace ON agent_session (workspace_id);
        CREATE UNIQUE INDEX agent_session_native ON agent_session (runtime_type, profile_id, native_session_id)
            WHERE native_session_id IS NOT NULL;
        CREATE UNIQUE INDEX agent_session_one_driver ON agent_session (driver_run_id) WHERE driver_run_id IS NOT NULL;
        ALTER TABLE run ADD COLUMN agent_session_id TEXT REFERENCES agent_session (id);
        ALTER TABLE run ADD COLUMN resume_native_session_id TEXT;
        CREATE UNIQUE INDEX run_one_active_per_session ON run (agent_session_id)
            WHERE agent_session_id IS NOT NULL AND status IN ('starting', 'running');
        CREATE VIEW session_events AS
            SELECT seq, aggregate_id AS agent_session_id, aggregate_version, event_type, occurred_at, payload
            FROM event WHERE aggregate_type = 'agent_session';
        CREATE TABLE transcript_segment (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_session_id TEXT NOT NULL REFERENCES agent_session (id),
            run_id TEXT NOT NULL REFERENCES run (id),
            first_position INTEGER NOT NULL CHECK (first_position >= 1),
            last_position INTEGER NOT NULL CHECK (last_position >= first_position),
            availability TEXT NOT NULL CHECK (availability IN ('stored', 'missing')),
            storage_path TEXT,
            sha256 TEXT,
            byte_len INTEGER,
            entry_count INTEGER NOT NULL,
            redactions INTEGER NOT NULL,
            recorded_at INTEGER NOT NULL,
            CHECK ((availability = 'stored') = (storage_path IS NOT NULL AND sha256 IS NOT NULL)),
            UNIQUE (run_id, first_position)
        );
        CREATE INDEX transcript_segment_session ON transcript_segment (agent_session_id);",
    },
];

/// 開啟 Store 的選項；預設值就是正式執行的行為。
#[derive(Debug, Clone, Copy)]
pub struct StoreOptions {
    /// 是否嘗試使用全文檢索（FTS5 + trigram）。關閉時視同 FTS5 不可用，搜尋退回 LIKE；
    /// 測試以它重現「FTS5 不可用」的降級情境（bundled SQLite 一定有 FTS5）。
    pub full_text_search: bool,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self { full_text_search: true }
    }
}

/// 一個連線上實際生效的 PRAGMA（啟動時讀回驗證的對象）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaSettings {
    pub journal_mode: String,
    /// SQLite 的數值：2 = FULL。
    pub synchronous: i64,
    pub foreign_keys: bool,
    pub busy_timeout_ms: i64,
}

/// 寫入連線與唯讀連線各自的設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPragmas {
    pub writer: PragmaSettings,
    pub reader: PragmaSettings,
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

struct Inner {
    /// 寫入佇列的入口；擁有者消失（佇列關閉）後，寫入執行緒處理完剩餘工作就結束。
    writer: Option<Sender<Job>>,
    writer_thread: Option<JoinHandle<()>>,
    reader: Arc<Mutex<Connection>>,
    /// 資料目錄；備份寫在其下的 `backups/`。
    data_dir: PathBuf,
    /// 全文檢索（FTS5 + trigram）在開啟時確認可用並已重建索引；否則搜尋退回 LIKE（spec D-26）。
    full_text: bool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // 先關閉佇列，再等寫入執行緒結束，資料庫檔案才會確實釋放（重建 Runtime 與清理暫存目錄都依賴這點）
        drop(self.writer.take());
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl Store {
    /// 在 `data_dir` 開啟（必要時建立）控制資料庫並套用 migration。
    ///
    /// `now` 記錄在 `schema_migrations.applied_at`（UTC，spec D-33）。
    /// 資料目錄位於網路共享或雲端同步資料夾時拒絕（spec D-17）。
    pub async fn open(data_dir: &Path, now: Timestamp) -> StoreResult<Self> {
        Self::open_with(data_dir, now, StoreOptions::default()).await
    }

    /// 以指定選項開啟；見 [`StoreOptions`]。
    pub async fn open_with(data_dir: &Path, now: Timestamp, options: StoreOptions) -> StoreResult<Self> {
        let data_dir = data_dir.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&data_dir, now, options))
            .await
            .map_err(|_| StoreError::Join)?
    }

    fn open_blocking(data_dir: &Path, now: Timestamp, options: StoreOptions) -> StoreResult<Self> {
        check_data_dir(data_dir)?;
        std::fs::create_dir_all(data_dir).map_err(|e| StoreError::CreateDir(data_dir.to_path_buf(), e))?;
        let db_path = data_dir.join(DB_FILE_NAME);

        let mut writer = Connection::open(&db_path)?;
        apply_and_verify(&writer)?;
        migrate(&mut writer, now, Some(data_dir))?;
        // 全文檢索不可用（或被選項關閉）不阻擋啟動：搜尋退回 LIKE 並標示降級
        let full_text = options.full_text_search && knowledge::prepare_full_text(&mut writer).is_ok();

        let reader = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        apply_and_verify(&reader)?;
        // 這條連線只用來查詢；就算有 bug 也不會寫入，寫入一律走佇列
        reader.pragma_update(None, "query_only", "ON")?;

        let (sender, receiver) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("baton-store-writer".into())
            .spawn(move || {
                let mut conn = writer;
                for job in receiver {
                    job(&mut conn);
                }
            })
            .map_err(|_| StoreError::Join)?;
        Ok(Self {
            inner: Arc::new(Inner {
                writer: Some(sender),
                writer_thread: Some(thread),
                reader: Arc::new(Mutex::new(reader)),
                data_dir: data_dir.to_path_buf(),
                full_text,
            }),
        })
    }

    /// 把工作排進單一寫入佇列，在一個短交易中執行；回傳錯誤或 panic 時整個交易回滾。
    async fn write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> StoreResult<T> + Send + 'static,
    ) -> StoreResult<T> {
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let outcome = catch_unwind(AssertUnwindSafe(|| run_in_transaction(conn, work)));
            let _ = reply.send(outcome.unwrap_or(Err(StoreError::Join)));
        });
        self.inner.writer.as_ref().ok_or(StoreError::Join)?.send(job).map_err(|_| StoreError::Join)?;
        result.await.map_err(|_| StoreError::Join)?
    }

    /// 唯讀查詢：使用獨立的唯讀連線，不經過寫入佇列。
    async fn read<T: Send + 'static>(
        &self,
        query: impl FnOnce(&Connection) -> StoreResult<T> + Send + 'static,
    ) -> StoreResult<T> {
        let reader = self.inner.reader.clone();
        tokio::task::spawn_blocking(move || {
            let conn = reader.lock().unwrap_or_else(|e| e.into_inner());
            query(&conn)
        })
        .await
        .map_err(|_| StoreError::Join)?
    }

    /// 寫入連線與唯讀連線目前生效的 PRAGMA。
    pub async fn pragma_settings(&self) -> StoreResult<ConnectionPragmas> {
        let writer = self.write(|tx| read_pragmas(tx)).await?;
        let reader = self.read(|conn| read_pragmas(conn)).await?;
        Ok(ConnectionPragmas { writer, reader })
    }

    /// 記錄一次 Runtime 啟動，回傳累計的啟動次數（含這一次）。
    pub async fn record_boot(&self, now: Timestamp) -> StoreResult<u64> {
        self.write(move |tx| {
            tx.execute("INSERT INTO runtime_boot (started_at) VALUES (?1)", [now.as_millis()])?;
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM runtime_boot", [], |r| r.get(0))?;
            Ok(count as u64)
        })
        .await
    }

    /// 簽發憑證，不影響既有的憑證。
    pub async fn issue_credential(&self, record: CredentialRecord) -> StoreResult<()> {
        self.write_credential(record, false).await
    }

    /// 輪替：撤銷同角色的全部既有憑證，只留下新的。
    pub async fn rotate_credential(&self, record: CredentialRecord) -> StoreResult<()> {
        self.write_credential(record, true).await
    }

    async fn write_credential(&self, record: CredentialRecord, revoke_same_role: bool) -> StoreResult<()> {
        self.write(move |tx| {
            if revoke_same_role {
                tx.execute("DELETE FROM credential WHERE role = ?1", [record.role.as_str()])?;
            }
            let scope = record.scope.as_ref();
            tx.execute(
                "INSERT INTO credential (token_hash, role, issued_at, expires_at, workspace_id, run_id, generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    record.token_hash,
                    record.role.as_str(),
                    record.issued_at.as_millis(),
                    record.expires_at.map(|t| t.as_millis()),
                    scope.map(|s| s.workspace_id.as_str()),
                    scope.map(|s| s.run_id.as_str()),
                    scope.map(|s| s.generation),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// 以 token 雜湊查詢憑證。
    pub async fn find_credential(&self, token_hash: &str) -> StoreResult<Option<CredentialRecord>> {
        let token_hash = token_hash.to_string();
        self.read(move |conn| {
            let row = conn
                .query_row(
                    "SELECT role, issued_at, expires_at, workspace_id, run_id, generation
                     FROM credential WHERE token_hash = ?1",
                    [&token_hash],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, Option<i64>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<i64>>(5)?,
                        ))
                    },
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            Ok(row.and_then(|(role, issued_at, expires_at, workspace_id, run_id, generation)| {
                let scope = match (workspace_id, run_id, generation) {
                    (Some(workspace_id), Some(run_id), Some(generation)) => {
                        Some(AgentScope { workspace_id, run_id, generation })
                    }
                    _ => None,
                };
                Some(CredentialRecord {
                    token_hash,
                    role: Role::parse(&role)?,
                    issued_at: Timestamp::from_millis(issued_at),
                    expires_at: expires_at.map(Timestamp::from_millis),
                    scope,
                })
            }))
        })
        .await
    }

    /// 套用一個 Workspace 命令：在同一個交易中載入目前狀態、交給 Domain 判定，
    /// 再寫入新狀態並追加事件。Domain 拒絕時不寫入任何東西。
    pub async fn apply_workspace_command(
        &self,
        command: WorkspaceCommand,
        now: Timestamp,
    ) -> Result<Workspace, WorkspaceApplyError> {
        let outcome = self
            .write(move |tx| {
                let existing = load_workspaces(tx)?;
                match workspace::decide(&existing, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(decision) => {
                        save_workspace(tx, &decision.workspace)?;
                        for event in &decision.events {
                            append_event(tx, event)?;
                        }
                        Ok(Ok(decision.workspace))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 確認並保存 Workspace 授權：同一個交易中載入目前生效的 revision、交給 Domain 判定，
    /// 再追加新的 revision 與事件。Domain 拒絕或內容沒有變動時不寫入任何東西。
    pub async fn confirm_authorization(
        &self,
        workspace_id: String,
        request: ConfirmRequest,
        now: Timestamp,
    ) -> Result<ConfirmOutcome, AuthorizationApplyError> {
        let outcome = self
            .write(move |tx| {
                let Some(workspace) = load_workspaces(tx)?.into_iter().find(|w| w.id == workspace_id) else {
                    return Ok(Err(AuthorizationError::WorkspaceNotFound(workspace_id)));
                };
                let current = load_authorization_revisions(tx, &workspace_id)?.pop();
                match authorization::confirm(&workspace, current.as_ref(), request, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(outcome) => {
                        if let ConfirmOutcome::Created { revision, event } = &outcome {
                            save_authorization_revision(tx, revision)?;
                            append_event(tx, event)?;
                        }
                        Ok(Ok(outcome))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// Workspace 的全部授權 revision，由舊到新；最後一個是目前生效的授權。
    pub async fn authorization_revisions(&self, workspace_id: &str) -> StoreResult<Vec<AuthorizationRevision>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(load_authorization_revisions(conn, &workspace_id)?)).await
    }

    /// Run 的准入檢查：依目前生效的授權判定 `required` 能力是否都已授權。
    /// 通過時回傳這次依據的授權 revision；被拒絕時回傳原因。
    /// 走寫入佇列（不寫任何東西）：結果與授權更新依同一個持久化順序排列，不會讀到佇列中尚未提交的舊狀態（票 24）。
    pub async fn check_admission(
        &self,
        workspace_id: &str,
        required: Vec<Capability>,
    ) -> Result<Admission, AdmissionCheckError> {
        let workspace_id = workspace_id.to_string();
        let verdict = self
            .write(move |conn| {
                let workspace = load_workspaces(conn)?.into_iter().find(|w| w.id == workspace_id);
                let current = load_authorization_revisions(conn, &workspace_id)?.pop();
                Ok(authorization::admit(&workspace_id, workspace.as_ref(), current.as_ref(), &required))
            })
            .await?;
        Ok(verdict?)
    }

    /// 全部 Workspace（含已封存），依建立順序排列（以寫入順序為準，同一毫秒建立的也不會亂序）。
    pub async fn list_workspaces(&self) -> StoreResult<Vec<Workspace>> {
        self.read(|conn| Ok(load_workspaces(conn)?)).await
    }
}

fn run_in_transaction<T>(
    conn: &mut Connection,
    work: impl FnOnce(&Transaction<'_>) -> StoreResult<T>,
) -> StoreResult<T> {
    // IMMEDIATE：一開始就取得寫入鎖，不會在交易中途才因為升級鎖而失敗
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_compatible(&tx)?;
    let value = work(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// 資料庫被較新的版本升級過就拒絕寫入（每次寫入都檢查，連開啟之後才被升級的情況也擋得住）。
fn ensure_compatible(conn: &Connection) -> StoreResult<()> {
    let found = applied_version(conn)?;
    if found > MIGRATIONS.len() {
        return Err(StoreError::IncompatibleSchema { found, supported: MIGRATIONS.len() });
    }
    Ok(())
}

fn applied_version(conn: &Connection) -> StoreResult<usize> {
    let max: Option<i64> = conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))?;
    Ok(max.unwrap_or(0).max(0) as usize)
}

fn load_workspaces(conn: &Connection) -> rusqlite::Result<Vec<Workspace>> {
    let mut statement = conn.prepare(
        "SELECT id, name, path, status, version, created_at, archived_at, model_overrides FROM workspace ORDER BY rowid",
    )?;
    let rows = statement.query_map([], |r| {
        let status: String = r.get(3)?;
        Ok(Workspace {
            id: r.get(0)?,
            name: r.get(1)?,
            path: r.get(2)?,
            status: WorkspaceStatus::parse(&status).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, "未知的 Workspace 狀態".into())
            })?,
            version: r.get(4)?,
            created_at: Timestamp::from_millis(r.get(5)?),
            archived_at: r.get::<_, Option<i64>>(6)?.map(Timestamp::from_millis),
            model_overrides: json_column(7, &r.get::<_, String>(7)?)?,
        })
    })?;
    rows.collect()
}

fn save_workspace(tx: &Transaction<'_>, workspace: &Workspace) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO workspace (id, name, path, status, version, created_at, archived_at, model_overrides)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name, path = excluded.path, status = excluded.status,
           version = excluded.version, archived_at = excluded.archived_at,
           model_overrides = excluded.model_overrides",
        rusqlite::params![
            workspace.id,
            workspace.name,
            workspace.path,
            workspace.status.as_str(),
            workspace.version,
            workspace.created_at.as_millis(),
            workspace.archived_at.map(|t| t.as_millis()),
            serde_json::to_string(&workspace.model_overrides).expect("ModelOverrides 可序列化"),
        ],
    )?;
    Ok(())
}

fn append_event(tx: &Transaction<'_>, event: &DomainEvent) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO event
         (aggregate_type, aggregate_id, aggregate_version, event_type, occurred_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            event.aggregate_type,
            event.aggregate_id,
            event.aggregate_version,
            event.event_type,
            event.occurred_at.as_millis(),
            event.payload.to_string(),
        ],
    )?;
    Ok(())
}

fn json_column<T: serde::de::DeserializeOwned>(index: usize, raw: &str) -> rusqlite::Result<T> {
    serde_json::from_str(raw)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

/// 由舊到新（revision 遞增）。
fn load_authorization_revisions(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<AuthorizationRevision>> {
    let mut statement = conn.prepare(
        "SELECT revision, grants, source, confirmed_at FROM authorization_revision
         WHERE workspace_id = ?1 ORDER BY revision",
    )?;
    let rows = statement.query_map([workspace_id], |r| {
        Ok(AuthorizationRevision {
            workspace_id: workspace_id.to_string(),
            revision: r.get(0)?,
            grants: json_column(1, &r.get::<_, String>(1)?)?,
            source: json_column(2, &r.get::<_, String>(2)?)?,
            confirmed_at: Timestamp::from_millis(r.get(3)?),
        })
    })?;
    rows.collect()
}

fn save_authorization_revision(tx: &Transaction<'_>, revision: &AuthorizationRevision) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let grants = serde_json::to_string(&revision.grants).expect("grants 可序列化");
    let source = serde_json::to_string(&revision.source).expect("source 可序列化");
    tx.execute(
        "INSERT INTO authorization_revision (workspace_id, revision, grants, source, confirmed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![revision.workspace_id, revision.revision, grants, source, revision.confirmed_at.as_millis()],
    )?;
    Ok(())
}

/// 套用 PRAGMA 並讀回驗證，任何一項沒有生效就失敗（issue #3 §11.1）。
fn apply_and_verify(conn: &Connection) -> StoreResult<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::PragmaNotApplied(format!("journal_mode={mode}（需要 wal）")));
    }
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let settings = read_pragmas(conn)?;
    let expected_timeout = BUSY_TIMEOUT.as_millis() as i64;
    if settings.synchronous != 2 {
        return Err(StoreError::PragmaNotApplied(format!("synchronous={}（需要 2＝FULL）", settings.synchronous)));
    }
    if !settings.foreign_keys {
        return Err(StoreError::PragmaNotApplied("foreign_keys=OFF（需要 ON）".into()));
    }
    if settings.busy_timeout_ms != expected_timeout {
        return Err(StoreError::PragmaNotApplied(format!(
            "busy_timeout={}（需要 {expected_timeout}）",
            settings.busy_timeout_ms
        )));
    }
    Ok(())
}

fn read_pragmas(conn: &Connection) -> StoreResult<PragmaSettings> {
    Ok(PragmaSettings {
        journal_mode: conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?,
        synchronous: conn.pragma_query_value(None, "synchronous", |r| r.get(0))?,
        foreign_keys: conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))? == 1,
        busy_timeout_ms: conn.pragma_query_value(None, "busy_timeout", |r| r.get(0))?,
    })
}

/// 套用尚未套用的 migration，並把版本記進 `schema_migrations`。
/// 資料庫比程式新時不動它；之後的寫入會被 `ensure_compatible` 拒絕。
///
/// `backup_data_dir` 有值時，對既有的資料庫套用 migration 之前先在該資料目錄建立備份（spec D-18）；
/// 備份失敗就不套用 migration。還原時不需要（還原的來源本身就是備份）。
fn migrate(conn: &mut Connection, now: Timestamp, backup_data_dir: Option<&Path>) -> StoreResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );",
    )?;
    adopt_legacy_user_version(conn, now)?;

    let applied = applied_version(conn)?;
    if applied > MIGRATIONS.len() {
        return Ok(());
    }
    if let Some(data_dir) = backup_data_dir.filter(|_| applied > 0 && applied < MIGRATIONS.len()) {
        backup::backup_before_migration(conn, data_dir, now)
            .map_err(|e| StoreError::PreMigrationBackup(e.to_string()))?;
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![index + 1, migration.name, now.as_millis()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// 票 01 到 04 以 `PRAGMA user_version` 記錄版本；第一次看到這樣的資料庫時，把已套用的版本補記進 `schema_migrations`。
fn adopt_legacy_user_version(conn: &mut Connection, now: Timestamp) -> StoreResult<()> {
    let legacy: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if legacy == 0 || applied_version(conn)? > 0 {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (index, migration) in MIGRATIONS.iter().enumerate().take(legacy) {
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![index + 1, migration.name, now.as_millis()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 檢查資料目錄不在網路共享或雲端同步資料夾（spec D-17）。
///
/// 同時檢查使用者給的路徑與解析後的真實路徑：對應到網路的磁碟機代號，
/// 解析後會變成 `\\?\UNC\...`。目錄尚未建立時，解析最近的既有上層目錄。
/// 桌面殼在啟動 Runtime 之前也呼叫它，才能把原因直接顯示給使用者。
pub fn check_data_dir(data_dir: &Path) -> StoreResult<()> {
    let sync_roots: Vec<String> = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect();
    let mut candidates = vec![data_dir.to_path_buf()];
    if let Some(resolved) = data_dir.ancestors().find_map(|p| std::fs::canonicalize(p).ok()) {
        candidates.push(resolved);
    }
    for candidate in candidates {
        storage_location::check_data_location(&candidate.to_string_lossy(), &sync_roots)
            .map_err(|reason| StoreError::UnsupportedLocation { path: data_dir.to_path_buf(), reason })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use baton_domain::workspace::WorkspaceStatus;

    const T0: Timestamp = Timestamp::from_millis(500);

    async fn open(dir: &Path) -> Store {
        Store::open(dir, T0).await.unwrap()
    }

    fn create(id: &str, path: &str) -> WorkspaceCommand {
        WorkspaceCommand::Create { id: id.into(), name: None, path: path.into() }
    }

    /// 直接用另一個連線讀資料庫檔案（測試用，驗證實際落地的內容）。
    fn raw(dir: &Path) -> Connection {
        Connection::open(dir.join(DB_FILE_NAME)).unwrap()
    }

    #[tokio::test]
    async fn boot_count_accumulates_across_reopen_of_same_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(dir.path()).await;
        assert_eq!(first.record_boot(Timestamp::from_millis(1)).await.unwrap(), 1);
        drop(first);

        let second = open(dir.path()).await;
        assert_eq!(second.record_boot(Timestamp::from_millis(2)).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn separate_data_dirs_do_not_share_state() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_a = open(a.path()).await;
        store_a.record_boot(Timestamp::from_millis(1)).await.unwrap();
        let store_b = open(b.path()).await;
        assert_eq!(store_b.record_boot(Timestamp::from_millis(1)).await.unwrap(), 1);
    }

    fn desktop_record(hash: &str, expires_at: Option<i64>) -> CredentialRecord {
        CredentialRecord {
            token_hash: hash.into(),
            role: Role::Desktop,
            issued_at: Timestamp::from_millis(10),
            expires_at: expires_at.map(Timestamp::from_millis),
            scope: None,
        }
    }

    #[tokio::test]
    async fn issued_credential_can_be_found_by_hash_with_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        let record = CredentialRecord { role: Role::Agent, ..desktop_record("h1", Some(99)) };

        store.issue_credential(record.clone()).await.unwrap();

        assert_eq!(store.find_credential("h1").await.unwrap(), Some(record));
        assert_eq!(store.find_credential("other").await.unwrap(), None);
    }

    #[tokio::test]
    async fn rotating_revokes_same_role_credentials_but_not_other_roles() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        let agent = CredentialRecord { role: Role::Agent, ..desktop_record("agent", None) };
        store.issue_credential(desktop_record("old", None)).await.unwrap();
        store.issue_credential(agent.clone()).await.unwrap();

        store.rotate_credential(desktop_record("new", None)).await.unwrap();

        assert_eq!(store.find_credential("old").await.unwrap(), None);
        assert!(store.find_credential("new").await.unwrap().is_some());
        assert_eq!(store.find_credential("agent").await.unwrap(), Some(agent));
    }

    #[tokio::test]
    async fn credentials_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.issue_credential(desktop_record("h", None)).await.unwrap();
        drop(store);

        let reopened = open(dir.path()).await;
        assert!(reopened.find_credential("h").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn open_creates_missing_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        open(&nested).await;
        assert!(nested.join(DB_FILE_NAME).exists());
    }

    // --- PRAGMA ---

    #[tokio::test]
    async fn both_connections_run_with_the_verified_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let pragmas = store.pragma_settings().await.unwrap();

        let expected =
            PragmaSettings { journal_mode: "wal".into(), synchronous: 2, foreign_keys: true, busy_timeout_ms: 5000 };
        assert_eq!(pragmas.writer, expected);
        assert_eq!(pragmas.reader, expected);
    }

    #[test]
    fn startup_fails_when_wal_cannot_be_enabled() {
        // 記憶體資料庫無法使用 WAL：驗證必須抓到，而不是靜默繼續
        let conn = Connection::open_in_memory().unwrap();

        let error = apply_and_verify(&conn).unwrap_err();

        assert!(matches!(&error, StoreError::PragmaNotApplied(m) if m.contains("journal_mode")), "{error}");
    }

    #[tokio::test]
    async fn wal_mode_is_persisted_in_the_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let _store = open(dir.path()).await;

        let mode: String = raw(dir.path()).pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();

        assert_eq!(mode, "wal");
    }

    // --- schema_migrations ---

    #[tokio::test]
    async fn applied_migrations_are_recorded_with_name_and_utc_time_once() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open(dir.path(), Timestamp::from_millis(1_000)).await.unwrap());
        drop(Store::open(dir.path(), Timestamp::from_millis(9_999)).await.unwrap());

        let rows: Vec<(i64, String, i64)> = raw(dir.path())
            .prepare("SELECT version, name, applied_at FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert_eq!(rows.len(), MIGRATIONS.len());
        assert_eq!(rows[0], (1, "runtime_boot".to_string(), 1_000));
        assert!(rows.iter().all(|(_, _, applied_at)| *applied_at == 1_000), "第二次開啟不可重新套用");
    }

    #[tokio::test]
    async fn database_from_older_builds_with_user_version_is_adopted_without_data_loss() {
        let dir = tempfile::tempdir().unwrap();
        {
            let old = raw(dir.path());
            old.execute_batch(
                "CREATE TABLE runtime_boot (id INTEGER PRIMARY KEY AUTOINCREMENT, started_at INTEGER NOT NULL);
                 CREATE TABLE credential (token_hash TEXT PRIMARY KEY, role TEXT NOT NULL CHECK (role IN ('desktop', 'agent')),
                   issued_at INTEGER NOT NULL, expires_at INTEGER);
                 INSERT INTO runtime_boot (started_at) VALUES (7);
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let store = open(dir.path()).await;

        assert_eq!(store.record_boot(Timestamp::from_millis(8)).await.unwrap(), 2);
        assert!(store.list_workspaces().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn newer_schema_version_rejects_every_write_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        drop(open(dir.path()).await);
        raw(dir.path())
            .execute("INSERT INTO schema_migrations (version, name, applied_at) VALUES (999, 'future', 1)", [])
            .unwrap();

        let store = open(dir.path()).await;
        let boot = store.record_boot(Timestamp::from_millis(1)).await;
        let credential = store.issue_credential(desktop_record("h", None)).await;
        let workspace = store.apply_workspace_command(create("w1", "C:\\work"), Timestamp::from_millis(1)).await;

        assert!(matches!(boot, Err(StoreError::IncompatibleSchema { found: 999, .. })), "{boot:?}");
        assert!(matches!(credential, Err(StoreError::IncompatibleSchema { .. })));
        assert!(matches!(workspace, Err(WorkspaceApplyError::Store(StoreError::IncompatibleSchema { .. }))));
        let conn = raw(dir.path());
        for table in ["runtime_boot", "credential", "workspace", "event"] {
            let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap();
            assert_eq!(count, 0, "{table} 不應有任何寫入");
        }
    }

    #[tokio::test]
    async fn schema_upgraded_by_another_process_after_open_is_also_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.record_boot(Timestamp::from_millis(1)).await.unwrap();

        raw(dir.path())
            .execute("INSERT INTO schema_migrations (version, name, applied_at) VALUES (999, 'future', 1)", [])
            .unwrap();

        assert!(matches!(
            store.record_boot(Timestamp::from_millis(2)).await,
            Err(StoreError::IncompatibleSchema { .. })
        ));
    }

    // --- 單一寫入佇列 ---

    #[tokio::test]
    async fn failed_write_rolls_back_everything_it_did() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let result: StoreResult<()> = store
            .write(|tx| {
                tx.execute("INSERT INTO runtime_boot (started_at) VALUES (1)", [])?;
                Err(StoreError::Join)
            })
            .await;

        assert!(result.is_err());
        assert_eq!(store.record_boot(Timestamp::from_millis(2)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn panicking_write_rolls_back_and_the_queue_keeps_working() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let result: StoreResult<()> = store
            .write(|tx| {
                tx.execute("INSERT INTO runtime_boot (started_at) VALUES (1)", [])?;
                panic!("測試用的 panic");
            })
            .await;

        assert!(matches!(result, Err(StoreError::Join)));
        assert_eq!(store.record_boot(Timestamp::from_millis(2)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_writers_are_serialized_without_busy_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let tasks: Vec<_> = (0..30)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move {
                    store
                        .apply_workspace_command(create(&format!("w{i}"), &format!("C:\\work\\{i}")), Timestamp::from_millis(i))
                        .await
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        assert_eq!(store.list_workspaces().await.unwrap().len(), 30);
        let events: i64 = raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 30);
    }

    #[tokio::test]
    async fn concurrent_creates_of_the_same_directory_let_exactly_one_win() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let tasks: Vec<_> = (0..10)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move {
                    store.apply_workspace_command(create(&format!("w{i}"), "C:\\work\\same"), Timestamp::from_millis(1)).await
                })
            })
            .collect();
        let mut wins = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(_) => wins += 1,
                Err(error) => assert!(matches!(error, WorkspaceApplyError::Rejected(WorkspaceError::AlreadyRegistered { .. }))),
            }
        }

        assert_eq!(wins, 1);
        assert_eq!(store.list_workspaces().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dropping_the_store_releases_the_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.record_boot(Timestamp::from_millis(1)).await.unwrap();

        drop(store);

        // 所有連線都關閉後，Windows 才允許刪除檔案
        std::fs::remove_file(dir.path().join(DB_FILE_NAME)).unwrap();
    }

    // --- Workspace 與事件 ---

    #[tokio::test]
    async fn create_persists_workspace_and_event_in_one_transaction_with_utc_millis() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;

        let created = store
            .apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1_700_000_000_123))
            .await
            .unwrap();

        assert_eq!(store.list_workspaces().await.unwrap(), vec![created.clone()]);
        let conn = raw(dir.path());
        let (created_at, status): (i64, String) = conn
            .query_row("SELECT created_at, status FROM workspace WHERE id = 'w1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((created_at, status.as_str()), (1_700_000_000_123, "active"));
        let event: (String, String, i64, i64, String) = conn
            .query_row(
                "SELECT event_type, aggregate_id, aggregate_version, occurred_at, payload FROM event",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((event.0.as_str(), event.1.as_str(), event.2, event.3), ("workspace_created", "w1", 1, 1_700_000_000_123));
        assert!(event.4.contains("C:\\\\work\\\\notes"));
    }

    #[tokio::test]
    async fn archive_updates_state_and_appends_a_second_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();

        let archived = store
            .apply_workspace_command(WorkspaceCommand::Archive { id: "w1".into() }, Timestamp::from_millis(2))
            .await
            .unwrap();

        assert_eq!((archived.status, archived.version, archived.archived_at), (WorkspaceStatus::Archived, 2, Some(Timestamp::from_millis(2))));
        assert_eq!(store.list_workspaces().await.unwrap(), vec![archived]);
        let types: Vec<String> = raw(dir.path())
            .prepare("SELECT event_type FROM event ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(types, ["workspace_created", "workspace_archived"]);
    }

    #[tokio::test]
    async fn rejected_command_writes_neither_state_nor_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();

        let duplicate = store.apply_workspace_command(create("w2", "c:\\WORK\\notes"), Timestamp::from_millis(2)).await;
        let unknown = store.apply_workspace_command(WorkspaceCommand::Archive { id: "zz".into() }, Timestamp::from_millis(3)).await;

        assert!(matches!(duplicate, Err(WorkspaceApplyError::Rejected(WorkspaceError::AlreadyRegistered { .. }))));
        assert!(matches!(unknown, Err(WorkspaceApplyError::Rejected(WorkspaceError::NotFound(_)))));
        assert_eq!(store.list_workspaces().await.unwrap().len(), 1);
        let events: i64 = raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn workspaces_created_in_the_same_millisecond_keep_creation_order_even_after_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        let now = Timestamp::from_millis(1);
        // id 刻意不依字母順序，排序不可依賴 id
        for (id, path) in [("zz", r"C:\a"), ("aa", r"C:\b"), ("mm", r"C:\c")] {
            store.apply_workspace_command(create(id, path), now).await.unwrap();
        }
        store.apply_workspace_command(WorkspaceCommand::Archive { id: "zz".into() }, now).await.unwrap();

        let ids: Vec<String> = store.list_workspaces().await.unwrap().into_iter().map(|w| w.id).collect();

        assert_eq!(ids, ["zz", "aa", "mm"]);
    }

    #[tokio::test]
    async fn workspaces_and_events_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        let created = store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        drop(store);

        let reopened = open(dir.path()).await;

        assert_eq!(reopened.list_workspaces().await.unwrap(), vec![created]);
    }

    #[tokio::test]
    async fn event_table_is_append_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        let conn = raw(dir.path());

        let update = conn.execute("UPDATE event SET event_type = 'x'", []);
        let delete = conn.execute("DELETE FROM event", []);

        assert!(update.is_err() && delete.is_err());
    }

    // --- 授權 revision ---

    fn confirm_request(template_id: Option<&str>, grants: Vec<authorization::Grant>) -> ConfirmRequest {
        ConfirmRequest { template_id: template_id.map(Into::into), grants, acknowledged_risk: true, acknowledged_expansion: true }
    }

    fn research_grants(path: &str) -> Vec<authorization::Grant> {
        authorization::templates(path).into_iter().find(|t| t.id == authorization::TEMPLATE_RESEARCH_DOCS).unwrap().grants
    }

    #[tokio::test]
    async fn confirmed_authorization_is_persisted_with_event_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        let grants = research_grants("C:\\work\\notes");

        let outcome = store
            .confirm_authorization(
                "w1".into(),
                confirm_request(Some(authorization::TEMPLATE_RESEARCH_DOCS), grants),
                Timestamp::from_millis(5),
            )
            .await
            .unwrap();
        drop(store);

        let ConfirmOutcome::Created { revision, .. } = outcome else { panic!("{outcome:?}") };
        let reopened = open(dir.path()).await;
        assert_eq!(reopened.authorization_revisions("w1").await.unwrap(), vec![revision]);
        let event: (String, i64, i64) = raw(dir.path())
            .query_row(
                "SELECT event_type, aggregate_version, occurred_at FROM event WHERE aggregate_type = 'workspace_authorization'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(event, ("authorization_confirmed".to_string(), 1, 5));
    }

    #[tokio::test]
    async fn each_change_appends_a_revision_and_unchanged_or_rejected_confirmations_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        let grants = research_grants("C:\\work\\notes");
        let confirm = |grants: Vec<authorization::Grant>, at: i64| {
            store.confirm_authorization("w1".into(), confirm_request(None, grants), Timestamp::from_millis(at))
        };

        confirm(grants.clone(), 2).await.unwrap();
        let same = confirm(grants.clone(), 3).await.unwrap();
        let emptied = confirm(vec![], 4).await; // 空授權是合法的修改（撤回全部能力）
        let rejected_duplicate = confirm(vec![grants[0].clone(), grants[0].clone()], 5).await;

        assert!(matches!(same, ConfirmOutcome::Unchanged(_)));
        assert!(emptied.is_ok());
        assert!(matches!(
            rejected_duplicate,
            Err(AuthorizationApplyError::Rejected(AuthorizationError::DuplicateCapability(_)))
        ));
        let revisions = store.authorization_revisions("w1").await.unwrap();
        assert_eq!(revisions.iter().map(|r| r.revision).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(revisions.iter().map(|r| r.confirmed_at.as_millis()).collect::<Vec<_>>(), [2, 4]);
        let events: i64 = raw(dir.path())
            .query_row("SELECT COUNT(*) FROM event WHERE aggregate_type = 'workspace_authorization'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 2);
    }

    #[tokio::test]
    async fn authorization_of_unknown_or_archived_workspace_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        store.apply_workspace_command(WorkspaceCommand::Archive { id: "w1".into() }, Timestamp::from_millis(2)).await.unwrap();

        let unknown = store.confirm_authorization("zz".into(), confirm_request(None, vec![]), T0).await;
        let archived = store.confirm_authorization("w1".into(), confirm_request(None, vec![]), T0).await;

        assert!(matches!(unknown, Err(AuthorizationApplyError::Rejected(AuthorizationError::WorkspaceNotFound(_)))));
        assert!(matches!(archived, Err(AuthorizationApplyError::Rejected(AuthorizationError::WorkspaceArchived(_)))));
    }

    #[tokio::test]
    async fn admission_follows_the_latest_confirmed_revision() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();

        let before = store.check_admission("w1", vec![Capability::WriteFiles]).await;
        store
            .confirm_authorization("w1".into(), confirm_request(None, research_grants("C:\\work\\notes")), T0)
            .await
            .unwrap();
        let granted = store.check_admission("w1", vec![Capability::WriteFiles]).await.unwrap();
        let outside = store.check_admission("w1", vec![Capability::RunBuildTest]).await;
        store.confirm_authorization("w1".into(), confirm_request(None, vec![]), T0).await.unwrap();
        let revoked = store.check_admission("w1", vec![Capability::WriteFiles]).await;

        assert!(matches!(before, Err(AdmissionCheckError::Denied(AdmissionDenied::AuthorizationUnconfirmed(_)))));
        assert_eq!(granted.authorization_revision, Some(1));
        assert!(matches!(outside, Err(AdmissionCheckError::Denied(AdmissionDenied::CapabilityNotGranted { .. }))));
        assert!(matches!(revoked, Err(AdmissionCheckError::Denied(AdmissionDenied::CapabilityNotGranted { revision: 2, .. }))));
    }

    #[tokio::test]
    async fn authorization_revisions_are_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        store.apply_workspace_command(create("w1", "C:\\work\\notes"), Timestamp::from_millis(1)).await.unwrap();
        store
            .confirm_authorization("w1".into(), confirm_request(None, research_grants("C:\\work\\notes")), T0)
            .await
            .unwrap();
        let conn = raw(dir.path());

        let update = conn.execute("UPDATE authorization_revision SET grants = '[]'", []);
        let delete = conn.execute("DELETE FROM authorization_revision", []);

        assert!(update.is_err() && delete.is_err());
    }

    // --- 資料位置 ---

    #[tokio::test]
    async fn network_share_data_dir_is_rejected_with_an_explanation_and_nothing_is_created() {
        let error = Store::open(Path::new("\\\\fileserver\\share\\baton"), T0).await.err().unwrap();

        assert!(matches!(&error, StoreError::UnsupportedLocation { reason: RejectedLocation::NetworkShare, .. }));
        let message = error.to_string();
        assert!(message.contains("網路共享") && message.contains("fileserver"), "{message}");
    }

    #[tokio::test]
    async fn cloud_sync_folder_is_rejected_before_creating_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("OneDrive - Contoso").join("Baton");

        let error = Store::open(&target, T0).await.err().unwrap();

        assert!(matches!(&error, StoreError::UnsupportedLocation { reason: RejectedLocation::CloudSync { provider }, .. } if provider == "OneDrive"));
        assert!(!target.exists());
        assert!(error.to_string().contains("本機磁碟"));
    }
}
