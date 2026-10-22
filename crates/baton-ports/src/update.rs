//! 簽章更新的來源（票 34；spec D-19）。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use baton_domain::maintenance::ReleaseInfo;

use crate::PortResult;

/// 更新來源：查詢最新的發行版本，並下載安裝程式、驗證簽章。
/// 這個版本沒有設定更新來源時，兩個方法都回傳 `PortError::NotImplemented`（Runtime 視為「未設定」）。
#[async_trait]
pub trait UpdateSource: Send + Sync {
    /// 更新來源公布的最新版本（不一定比目前新，由 Domain 比較）。
    async fn latest(&self) -> PortResult<ReleaseInfo>;

    /// 下載 `release` 的安裝程式到 `dest_dir` 並驗證簽章；只有簽章正確才回傳檔案路徑，否則不留下任何檔案。
    async fn download(&self, release: &ReleaseInfo, dest_dir: &Path) -> PortResult<PathBuf>;
}
