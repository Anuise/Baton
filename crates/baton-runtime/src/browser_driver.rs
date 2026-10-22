//! 正式的 BrowserDriver（票 46）：以受驗專案的 Playwright 執行瀏覽器操作。

use async_trait::async_trait;
use baton_ports::{BrowserDriver, BrowserFile, BrowserOutcome, BrowserSession, BrowserSessionRequest, BrowserStep, PortError, PortResult};

#[derive(Debug, Default)]
pub struct LocalBrowserDriver;

#[async_trait]
impl BrowserDriver for LocalBrowserDriver {
    async fn open(&self, _request: BrowserSessionRequest) -> PortResult<BrowserSession> {
        Err(PortError::NotImplemented("BrowserDriver"))
    }

    async fn step(&self, _session: &BrowserSession, _step: BrowserStep) -> PortResult<BrowserOutcome> {
        Err(PortError::NotImplemented("BrowserDriver"))
    }

    async fn close(&self, _session: &BrowserSession) -> PortResult<Vec<BrowserFile>> {
        Err(PortError::NotImplemented("BrowserDriver"))
    }
}
