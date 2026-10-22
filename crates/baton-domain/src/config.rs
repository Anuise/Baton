//! Runtime 設定：資料目錄與監聽位址。
//!
//! 只做純解析；環境變數由呼叫端以 `from_lookup(|k| std::env::var(k).ok())` 注入，
//! 讓 Runtime 與桌面 App 共用同一套規則而 Domain 不碰 I/O。

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::product;

/// 覆寫資料目錄的環境變數（測試與多開發環境用）。
pub const ENV_DATA_DIR: &str = "BATON_DATA_DIR";
/// 覆寫監聽埠的環境變數。
pub const ENV_PORT: &str = "BATON_PORT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("無法決定資料目錄：未設定 {ENV_DATA_DIR}，也找不到 LOCALAPPDATA")]
    NoDataDir,
    #[error("{ENV_PORT} 不是合法的埠號：{0}")]
    BadPort(String),
}

impl RuntimeConfig {
    /// 以可注入的查詢函式讀取設定，方便測試。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let data_dir = match lookup(ENV_DATA_DIR) {
            Some(dir) => PathBuf::from(dir),
            // 預設位置：%LOCALAPPDATA%\<產品名>\（spec D-17）
            None => lookup("LOCALAPPDATA")
                .map(|base| PathBuf::from(base).join(product::DATA_DIR_NAME))
                .ok_or(ConfigError::NoDataDir)?,
        };
        let port = match lookup(ENV_PORT) {
            Some(raw) => raw.parse::<u16>().map_err(|_| ConfigError::BadPort(raw))?,
            None => product::DEFAULT_RUNTIME_PORT,
        };
        // 只綁 IPv4 loopback；IPv6 與其餘防護由票 04 處理
        Ok(Self { data_dir, bind: SocketAddr::from((Ipv4Addr::LOCALHOST, port)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    #[test]
    fn defaults_to_local_app_data_product_dir_and_default_port() {
        let config =
            RuntimeConfig::from_lookup(lookup(&[("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local")])).unwrap();
        assert_eq!(
            config.data_dir,
            PathBuf::from("C:\\Users\\u\\AppData\\Local").join(product::DATA_DIR_NAME)
        );
        assert_eq!(config.bind, SocketAddr::from((Ipv4Addr::LOCALHOST, product::DEFAULT_RUNTIME_PORT)));
    }

    #[test]
    fn env_overrides_data_dir_and_port() {
        let config = RuntimeConfig::from_lookup(lookup(&[(ENV_DATA_DIR, "D:\\x"), (ENV_PORT, "5000")])).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("D:\\x"));
        assert_eq!(config.bind.port(), 5000);
    }

    #[test]
    fn missing_data_dir_sources_is_an_error() {
        assert_eq!(RuntimeConfig::from_lookup(lookup(&[])), Err(ConfigError::NoDataDir));
    }

    #[test]
    fn invalid_port_is_rejected() {
        let result = RuntimeConfig::from_lookup(lookup(&[(ENV_DATA_DIR, "D:\\x"), (ENV_PORT, "abc")]));
        assert_eq!(result, Err(ConfigError::BadPort("abc".into())));
    }
}
