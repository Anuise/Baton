//! CLI／API 驗證 Adapter 的正式 Port 實作（票 45）：子程序、HTTP（reqwest）、Git HEAD 與狀態目錄。
//!
//! 受驗服務以子程序啟動，停止時只終止這個直接啟動的程序；服務自己再開的子孫程序不受控
//! （與驗證命令相同，不放進 Job Object，見 AI 決策）。Runtime 結束時仍在執行的服務隨之被終止（`kill_on_drop`）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use baton_ports::{
    CommandOutput, CommandRequest, CommandRunner, HttpOutcome, HttpRequest, PortError, PortResult, ServiceHandle,
    ServiceRequest, VerificationDriver,
};

use crate::production_ports::LocalCommandRunner;

/// 保留的回應本文長度（位元組，取開頭）；斷言需要的 JSON 通常在開頭就完整。
const BODY_MAX_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub struct LocalVerificationDriver {
    services: Mutex<HashMap<String, tokio::process::Child>>,
    next_service: AtomicU64,
}

fn failed(message: String) -> PortError {
    PortError::Failed(message)
}

#[async_trait]
impl VerificationDriver for LocalVerificationDriver {
    async fn run_command(&self, request: CommandRequest) -> PortResult<CommandOutput> {
        LocalCommandRunner.run(request).await
    }

    async fn send_http(&self, request: HttpRequest) -> PortResult<HttpOutcome> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|e| failed(format!("HTTP 方法「{}」不正確：{e}", request.method)))?;
        // 不跟隨轉址：斷言看的是受驗介面實際的回應
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(request.timeout)
            .build()
            .map_err(|e| failed(format!("無法建立 HTTP 用戶端：{e}")))?;
        let mut builder = client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        match builder.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                match response.bytes().await {
                    Ok(bytes) => {
                        let body = String::from_utf8_lossy(&bytes[..bytes.len().min(BODY_MAX_BYTES)]).into_owned();
                        Ok(HttpOutcome::Responded { status, body })
                    }
                    Err(error) if error.is_timeout() => Ok(HttpOutcome::TimedOut),
                    Err(error) => Ok(HttpOutcome::Responded { status, body: format!("（無法讀取回應本文：{error}）") }),
                }
            }
            Err(error) if error.is_timeout() => Ok(HttpOutcome::TimedOut),
            Err(error) if error.is_connect() || error.is_request() => Ok(HttpOutcome::Unreachable { reason: error.to_string() }),
            Err(error) => Err(failed(format!("HTTP 請求失敗：{error}"))),
        }
    }

    async fn start_service(&self, request: ServiceRequest) -> PortResult<ServiceHandle> {
        let mut command = tokio::process::Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(&request.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let child = command.spawn().map_err(|e| failed(format!("無法啟動「{}」：{e}", request.program)))?;
        let id = format!("svc-{}", self.next_service.fetch_add(1, Ordering::Relaxed));
        self.services.lock().expect("服務表不會中毒").insert(id.clone(), child);
        Ok(ServiceHandle(id))
    }

    async fn stop_service(&self, handle: &ServiceHandle) -> PortResult<()> {
        let child = self.services.lock().expect("服務表不會中毒").remove(&handle.0);
        let Some(mut child) = child else { return Ok(()) };
        // 已經自行結束的服務不必再終止
        if child.try_wait().map_err(|e| failed(format!("無法查詢服務狀態：{e}")))?.is_some() {
            return Ok(());
        }
        child.kill().await.map_err(|e| failed(format!("無法停止服務：{e}")))
    }

    async fn pause(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    async fn revision(&self, dir: &Path) -> PortResult<Option<String>> {
        let mut command = tokio::process::Command::new("git");
        command.args(["rev-parse", "--verify", "HEAD"]).current_dir(dir).kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let output = command.output().await.map_err(|e| failed(format!("無法執行 git：{e}")))?;
        if !output.status.success() {
            return Ok(None);
        }
        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!head.is_empty()).then_some(head))
    }

    async fn prepare_state_dir(&self, dir: &Path) -> PortResult<()> {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            std::fs::create_dir_all(&dir)
        })
        .await
        .map_err(|e| failed(format!("狀態目錄的工作被中斷：{e}")))?
        .map_err(|e| failed(format!("無法建立狀態目錄：{e}")))
    }

    async fn remove_state_dir(&self, dir: &Path) -> PortResult<()> {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        })
        .await
        .map_err(|e| failed(format!("狀態目錄的工作被中斷：{e}")))?
        .map_err(|e| failed(format!("無法移除狀態目錄：{e}")))
    }
}
