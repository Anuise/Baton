//! 資料目錄位置的判定（spec D-17、issue #3 §11.1）：
//! 控制資料庫不可放在網路共享，也不可放在正在同步的雲端資料夾。
//!
//! 純字串判斷；磁碟實際位置（例如對應到網路的磁碟機代號）由呼叫端先解析成真實路徑再傳入。

use std::fmt;

/// 不適合放資料庫的位置種類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedLocation {
    /// UNC 路徑（`\\server\share\...`）。
    NetworkShare,
    /// 位於雲端同步資料夾；`provider` 是服務名稱（例如 OneDrive）。
    CloudSync { provider: String },
}

impl fmt::Display for RejectedLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RejectedLocation::NetworkShare => write!(
                f,
                "位於網路共享（UNC 路徑）。SQLite 需要可靠的檔案鎖與 WAL 共享記憶體，網路檔案系統無法保證，可能造成資料毀損"
            ),
            RejectedLocation::CloudSync { provider } => write!(
                f,
                "位於 {provider} 的同步資料夾。同步程式可能在寫入途中鎖定或覆寫資料庫檔案，可能造成資料毀損"
            ),
        }
    }
}

/// 已知雲端同步資料夾的目錄名稱前綴（小寫）與對應的服務名稱。
const SYNC_FOLDERS: &[(&str, &str)] = &[
    ("onedrive", "OneDrive"),
    ("dropbox", "Dropbox"),
    ("google drive", "Google Drive"),
    ("googledrive", "Google Drive"),
    ("icloud drive", "iCloud Drive"),
    ("iclouddrive", "iCloud Drive"),
    ("box sync", "Box"),
    ("nextcloud", "Nextcloud"),
];

/// 判定資料目錄的位置是否可接受。`sync_roots` 是呼叫端從環境取得的同步根目錄
/// （例如環境變數 `OneDrive` 的值），位於其下的路徑一律視為雲端同步。
pub fn check_data_location(path: &str, sync_roots: &[String]) -> Result<(), RejectedLocation> {
    let unified = path.replace('/', "\\");
    // \\?\UNC\server\share 是長路徑形式的 UNC；\\?\C:\... 是一般本機路徑
    if unified.starts_with("\\\\?\\UNC\\") || unified.starts_with("\\\\.\\UNC\\") {
        return Err(RejectedLocation::NetworkShare);
    }
    let is_device_path = unified.starts_with("\\\\?\\") || unified.starts_with("\\\\.\\");
    if unified.starts_with("\\\\") && !is_device_path {
        return Err(RejectedLocation::NetworkShare);
    }

    let lowered = unified.to_lowercase();
    for root in sync_roots {
        let root = root.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        if !root.is_empty() && (lowered == root || lowered.starts_with(&format!("{root}\\"))) {
            return Err(RejectedLocation::CloudSync { provider: provider_of(&root) });
        }
    }
    for part in lowered.split('\\') {
        if let Some((_, provider)) = SYNC_FOLDERS.iter().find(|(prefix, _)| part.starts_with(prefix)) {
            return Err(RejectedLocation::CloudSync { provider: (*provider).to_string() });
        }
    }
    Ok(())
}

/// 由同步根目錄的最後一段猜服務名稱；猜不到時以「雲端同步」概稱。
fn provider_of(root: &str) -> String {
    let last = root.rsplit('\\').next().unwrap_or(root);
    SYNC_FOLDERS
        .iter()
        .find(|(prefix, _)| last.starts_with(prefix))
        .map_or_else(|| "雲端同步".to_string(), |(_, provider)| (*provider).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(path: &str) -> Result<(), RejectedLocation> {
        check_data_location(path, &[])
    }

    #[test]
    fn default_local_app_data_location_is_accepted() {
        assert_eq!(check("C:\\Users\\me\\AppData\\Local\\Baton"), Ok(()));
        assert_eq!(check("D:\\data\\baton"), Ok(()));
        assert_eq!(check("\\\\?\\C:\\Users\\me\\AppData\\Local\\Baton"), Ok(()));
    }

    #[test]
    fn unc_paths_are_network_shares() {
        assert_eq!(check("\\\\server\\share\\baton"), Err(RejectedLocation::NetworkShare));
        assert_eq!(check("//server/share/baton"), Err(RejectedLocation::NetworkShare));
        assert_eq!(check("\\\\?\\UNC\\server\\share\\baton"), Err(RejectedLocation::NetworkShare));
    }

    #[test]
    fn known_sync_folder_names_are_rejected_with_provider() {
        let cases = [
            ("C:\\Users\\me\\OneDrive\\Baton", "OneDrive"),
            ("C:\\Users\\me\\OneDrive - Contoso\\Documents\\Baton", "OneDrive"),
            ("C:\\Users\\me\\Dropbox\\Baton", "Dropbox"),
            ("C:\\Users\\me\\Google Drive\\Baton", "Google Drive"),
            ("C:\\Users\\me\\iCloudDrive\\Baton", "iCloud Drive"),
        ];
        for (path, provider) in cases {
            assert_eq!(check(path), Err(RejectedLocation::CloudSync { provider: provider.into() }), "{path}");
        }
    }

    #[test]
    fn sync_folder_names_match_case_insensitively_per_path_segment() {
        assert!(check("c:\\users\\me\\onedrive\\x").is_err());
        // 只是名稱裡剛好含有字串、但不是以它開頭的目錄，不算同步資料夾
        assert_eq!(check("C:\\work\\my-onedrive-notes"), Ok(()));
    }

    #[test]
    fn configured_sync_roots_reject_anything_underneath_them() {
        let roots = vec!["D:\\Cloud\\Sync\\".to_string()];

        assert!(matches!(check_data_location("d:\\cloud\\sync\\Baton", &roots), Err(RejectedLocation::CloudSync { .. })));
        assert!(check_data_location("D:\\Cloud\\Sync", &roots).is_err());
        assert_eq!(check_data_location("D:\\Cloud\\Syncing\\Baton", &roots), Ok(()));
    }

    #[test]
    fn rejection_explains_why() {
        let text = RejectedLocation::CloudSync { provider: "OneDrive".into() }.to_string();
        assert!(text.contains("OneDrive") && text.contains("同步"));
        assert!(RejectedLocation::NetworkShare.to_string().contains("網路共享"));
    }
}
