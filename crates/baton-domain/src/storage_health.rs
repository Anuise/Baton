//! 控制資料庫的可寫狀態（票 26，使用者故事 85）：磁碟滿或資料庫忙碌時停止新派工，並告訴使用者原因與處理方式。
//!
//! 純資料與文字；判定哪一種 SQLite 錯誤屬於哪一類在 Store（它才看得到錯誤代碼）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageFaultKind {
    /// 磁碟（或資料庫大小上限）已滿：SQLITE_FULL。
    DiskFull,
    /// 資料庫被其他程序鎖住，等待逾時：SQLITE_BUSY／SQLITE_LOCKED。
    Busy,
    /// 其他無法寫入的情形：I/O 錯誤、唯讀。
    Unwritable,
}

impl StorageFaultKind {
    /// 穩定的機器可讀代碼；API 拒絕寫入時的錯誤代碼也是它。
    pub fn code(self) -> &'static str {
        match self {
            StorageFaultKind::DiskFull => "storage_disk_full",
            StorageFaultKind::Busy => "storage_busy",
            StorageFaultKind::Unwritable => "storage_unwritable",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            StorageFaultKind::DiskFull => "資料目錄所在的磁碟空間不足，控制資料庫無法寫入",
            StorageFaultKind::Busy => "控制資料庫被其他程式鎖住，寫入等待逾時",
            StorageFaultKind::Unwritable => "控制資料庫無法寫入（磁碟 I/O 錯誤或唯讀）",
        }
    }

    /// 使用者可以怎麼處理。
    pub fn action(self) -> &'static str {
        match self {
            StorageFaultKind::DiskFull => "請釋放資料目錄所在磁碟的空間（例如刪除不需要的檔案或舊備份）；空間足夠後 Baton 會自動恢復派工",
            StorageFaultKind::Busy => "請關閉正在開啟控制資料庫的其他程式（例如資料庫檢視工具、備份或防毒掃描）；鎖解除後 Baton 會自動恢復派工",
            StorageFaultKind::Unwritable => "請確認資料目錄所在的磁碟仍然連接、沒有被設為唯讀，且有寫入權限；恢復後 Baton 會自動恢復派工",
        }
    }
}

/// 目前的寫入故障。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageFault {
    pub kind: StorageFaultKind,
    pub code: String,
    pub summary: String,
    pub action: String,
    /// SQLite 的原始訊息（診斷用）。
    pub detail: String,
}

impl StorageFault {
    pub fn new(kind: StorageFaultKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            code: kind.code().to_string(),
            summary: kind.summary().to_string(),
            action: kind.action().to_string(),
            detail: detail.into(),
        }
    }
}

/// 對外顯示的可寫狀態：有故障時新派工停止（`dispatch_halted`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageHealth {
    pub dispatch_halted: bool,
    pub fault: Option<StorageFault>,
}

impl StorageHealth {
    pub fn from_fault(fault: Option<StorageFault>) -> Self {
        Self { dispatch_halted: fault.is_some(), fault }
    }
}
