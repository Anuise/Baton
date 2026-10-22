//! 正式版的更新來源（票 34；spec D-19）：Tauri 更新清單 + minisign 簽章。
//!
//! - **清單**：`BATON_UPDATE_ENDPOINT` 指向 Tauri 格式的 `latest.json`
//!   （`version`、`notes`、`pub_date`、`platforms["windows-x86_64-nsis" | "windows-x86_64"] = { url, signature }`）。
//! - **簽章**：`BATON_UPDATE_PUBKEY` 是 `tauri signer generate` 產生的公鑰（base64）；安裝程式下載後以它驗證
//!   `signature`（`tauri signer sign` 的輸出），通過才改名成正式檔名交給桌面殼執行，否則刪除暫存檔。
//! - 兩個值都在**建置時**以環境變數帶入（`pnpm build --bundle` 時設定）；沒有設定的版本視為「未設定更新來源」，
//!   Runtime 只顯示「未設定」，不會連線。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine as _;
use baton_domain::maintenance::ReleaseInfo;
use baton_ports::{PortError, PortResult, UpdateSource};
use serde::Deserialize;

/// 清單中 Windows x64 NSIS 安裝程式的平台名稱（先找含安裝程式種類的，再找一般的）。
const PLATFORM_KEYS: [&str; 2] = ["windows-x86_64-nsis", "windows-x86_64"];

/// 查詢清單的逾時；下載安裝程式另有較長的逾時。
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// 建置時設定的更新來源；沒有設定時為 `None`。
pub fn configured() -> Option<HttpUpdateSource> {
    let endpoint = option_env!("BATON_UPDATE_ENDPOINT").filter(|v| !v.trim().is_empty())?;
    let pubkey = option_env!("BATON_UPDATE_PUBKEY").filter(|v| !v.trim().is_empty())?;
    Some(HttpUpdateSource::new(endpoint, pubkey))
}

pub struct HttpUpdateSource {
    endpoint: String,
    pubkey: String,
    http: reqwest::Client,
}

impl HttpUpdateSource {
    pub fn new(endpoint: &str, pubkey: &str) -> Self {
        Self { endpoint: endpoint.to_string(), pubkey: pubkey.trim().to_string(), http: reqwest::Client::new() }
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    platforms: std::collections::HashMap<String, PlatformEntry>,
}

#[derive(Deserialize)]
struct PlatformEntry {
    url: String,
    signature: String,
}

/// 解析 Tauri 格式的更新清單，取出 Windows x64 安裝程式。
pub fn parse_manifest(body: &[u8]) -> PortResult<ReleaseInfo> {
    let manifest: Manifest =
        serde_json::from_slice(body).map_err(|e| PortError::Failed(format!("更新清單的格式不正確：{e}")))?;
    // 清單本身沒有簽章，版本又會拿來組下載目錄：只接受 semver 會用到的字元，擋掉路徑分隔與 `..`
    if !is_safe_version(&manifest.version) {
        return Err(PortError::Failed(format!("更新清單的版本不正確：{:?}", manifest.version)));
    }
    let entry = PLATFORM_KEYS
        .iter()
        .find_map(|key| manifest.platforms.get(*key))
        .ok_or_else(|| PortError::Failed("更新清單沒有 Windows x64 的安裝程式".to_string()))?;
    Ok(ReleaseInfo {
        version: manifest.version,
        notes: manifest.notes,
        published_at: manifest.pub_date,
        url: entry.url.clone(),
        signature: entry.signature.clone(),
    })
}

/// 版本只能由 `[0-9A-Za-z.+-]` 組成、不能為空、不能含 `..`（用作 `updates\<版本>` 目錄名稱時不會跳出資料目錄）。
fn is_safe_version(version: &str) -> bool {
    !version.is_empty()
        && !version.contains("..")
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// 以 base64 編碼的 minisign 公鑰驗證 base64 編碼的簽章（與 Tauri 更新程式相同的格式）。
pub fn verify_signature(data: &[u8], pubkey_base64: &str, signature_base64: &str) -> Result<(), String> {
    let decode = |what: &str, raw: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| format!("{what}不是有效的 base64 文字"))
    };
    let public_key = minisign_verify::PublicKey::decode(&decode("公鑰", pubkey_base64)?)
        .map_err(|e| format!("公鑰格式不正確：{e}"))?;
    let signature = minisign_verify::Signature::decode(&decode("簽章", signature_base64)?)
        .map_err(|e| format!("簽章格式不正確：{e}"))?;
    public_key.verify(data, &signature, true).map_err(|e| format!("簽章驗證失敗：{e}"))
}

/// 下載網址最後一段當檔名（只留安全的字元）；取不到時用固定名稱。
fn file_name_of(url: &str) -> String {
    let last = url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    let cleaned: String =
        last.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    if cleaned.to_ascii_lowercase().ends_with(".exe") { cleaned } else { "Baton-setup.exe".to_string() }
}

/// 驗證通過才把內容寫成正式檔名（暫存檔 + 改名）；不通過時不留下任何檔案。
pub fn store_verified(bytes: &[u8], release: &ReleaseInfo, pubkey: &str, dest_dir: &Path) -> PortResult<PathBuf> {
    verify_signature(bytes, pubkey, &release.signature).map_err(PortError::Failed)?;
    std::fs::create_dir_all(dest_dir).map_err(|e| PortError::Failed(format!("無法建立 {}：{e}", dest_dir.display())))?;
    let target = dest_dir.join(file_name_of(&release.url));
    let partial = target.with_extension("exe.part");
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, &target))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            PortError::Failed(format!("無法寫入安裝程式 {}：{e}", target.display()))
        })?;
    Ok(target)
}

#[async_trait]
impl UpdateSource for HttpUpdateSource {
    async fn latest(&self) -> PortResult<ReleaseInfo> {
        let response = self
            .http
            .get(&self.endpoint)
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| PortError::Failed(format!("無法取得更新清單：{e}")))?;
        let body = response.bytes().await.map_err(|e| PortError::Failed(format!("無法讀取更新清單：{e}")))?;
        parse_manifest(&body)
    }

    async fn download(&self, release: &ReleaseInfo, dest_dir: &Path) -> PortResult<PathBuf> {
        let response = self
            .http
            .get(&release.url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| PortError::Failed(format!("無法下載安裝程式：{e}")))?;
        let bytes = response.bytes().await.map_err(|e| PortError::Failed(format!("無法下載安裝程式：{e}")))?;
        let (release, pubkey, dest_dir) = (release.clone(), self.pubkey.clone(), dest_dir.to_path_buf());
        tokio::task::spawn_blocking(move || store_verified(&bytes, &release, &pubkey, &dest_dir))
            .await
            .map_err(|e| PortError::Failed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 測試用的金鑰與簽章：以 `tauri signer generate`／`tauri signer sign` 產生，私鑰沒有保存。
    fn fixture(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update").join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("讀不到 {}：{e}", path.display()))
    }

    fn text(name: &str) -> String {
        String::from_utf8(fixture(name)).unwrap()
    }

    fn release(signature: String) -> ReleaseInfo {
        ReleaseInfo {
            version: "0.2.0".into(),
            notes: None,
            published_at: None,
            url: "https://example.invalid/download/Baton_0.2.0_x64-setup.exe?x=1".into(),
            signature,
        }
    }

    #[test]
    fn a_signature_made_with_the_matching_key_verifies() {
        verify_signature(&fixture("fixture.bin"), &text("test.key.pub"), &text("fixture.bin.sig")).unwrap();
    }

    #[test]
    fn tampered_content_or_another_key_is_rejected() {
        let mut tampered = fixture("fixture.bin");
        tampered[0] ^= 1;
        assert!(verify_signature(&tampered, &text("test.key.pub"), &text("fixture.bin.sig")).is_err());
        assert!(verify_signature(&fixture("fixture.bin"), &text("test.key.pub"), "不是簽章").is_err());
        // 另一把公鑰（從 minisign 官方文件的範例公鑰改寫）
        let other = base64::engine::general_purpose::STANDARD
            .encode("untrusted comment: minisign public key\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n");
        assert!(verify_signature(&fixture("fixture.bin"), &other, &text("fixture.bin.sig")).is_err());
    }

    #[test]
    fn only_verified_installers_are_kept_under_their_download_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_verified(&fixture("fixture.bin"), &release(text("fixture.bin.sig")), &text("test.key.pub"), dir.path())
            .unwrap();
        assert_eq!(path.file_name().unwrap(), "Baton_0.2.0_x64-setup.exe");
        assert_eq!(std::fs::read(&path).unwrap(), fixture("fixture.bin"));

        let rejected = tempfile::tempdir().unwrap();
        let mut tampered = fixture("fixture.bin");
        tampered[0] ^= 1;
        let result = store_verified(&tampered, &release(text("fixture.bin.sig")), &text("test.key.pub"), rejected.path());
        assert!(matches!(result, Err(PortError::Failed(message)) if message.contains("簽章")));
        assert_eq!(std::fs::read_dir(rejected.path()).unwrap().count(), 0, "驗證不通過不留下任何檔案");
    }

    #[test]
    fn the_tauri_manifest_yields_the_windows_installer() {
        let body = r#"{
            "version": "0.2.0",
            "notes": "修正",
            "pub_date": "2026-10-22T00:00:00Z",
            "platforms": {
                "darwin-aarch64": { "url": "https://x/mac.tar.gz", "signature": "m" },
                "windows-x86_64": { "url": "https://x/Baton_0.2.0_x64-setup.exe", "signature": "w" }
            }
        }"#;
        let release = parse_manifest(body.as_bytes()).unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.url, "https://x/Baton_0.2.0_x64-setup.exe");
        assert_eq!(release.signature, "w");
        assert_eq!(release.notes.as_deref(), Some("修正"));
        assert!(parse_manifest(br#"{ "version": "0.2.0", "platforms": {} }"#).is_err(), "沒有 Windows 安裝程式");
        assert!(parse_manifest(b"not json").is_err());
    }

    #[test]
    fn a_version_that_could_escape_the_download_directory_is_rejected() {
        let manifest = |version: &str| {
            serde_json::json!({
                "version": version,
                "platforms": { "windows-x86_64": { "url": "https://x/Baton-setup.exe", "signature": "w" } }
            })
            .to_string()
        };
        for version in ["..", "../../evil", "0.2.0/../..", r"C:\evil", "/evil", "0..2", "", "0.2.0 beta", r"0.2.0\x"] {
            assert!(parse_manifest(manifest(version).as_bytes()).is_err(), "應拒絕版本 {version:?}");
        }
        for version in ["0.2.0", "1.0.0-rc.1", "1.0.0+build.5"] {
            assert_eq!(parse_manifest(manifest(version).as_bytes()).unwrap().version, version);
        }
    }

    #[test]
    fn download_names_are_sanitized() {
        assert_eq!(file_name_of("https://x/a/Baton_0.2.0_x64-setup.exe?token=1"), "Baton_0.2.0_x64-setup.exe");
        assert_eq!(file_name_of("https://x/a/..%5C..%5Cevil.exe"), "..5C..5Cevil.exe");
        assert_eq!(file_name_of("https://x/download"), "Baton-setup.exe");
    }
}
