//! RuntimeClient 的傳輸行為，以一個最小的 HTTP/1.1 伺服器代替 Runtime：
//! - App 以 0.5 秒輪詢 Runtime：同一個 RuntimeClient 的連續請求要重用連線。
//! - 派工會在 Runtime 端同步準備工作目錄（正式版是 `git worktree add`），不能套用一般請求的 2 秒上限。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use baton_client::{ClientError, DispatchOutcome, RuntimeClient};
use baton_credential::write_desktop_credential;
use baton_harness::fakes::FakeSecretProtector;

/// 對每個請求都等 `delay` 之後回 `status` 與 `body` 的伺服器（保持連線），記下接受了幾條連線。
fn fake_runtime(delay: Duration, status: &'static str, body: &'static str) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    // 讀到一個完整的請求標頭（這些請求都沒有本文）
                    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    buffer.drain(..end);
                    std::thread::sleep(delay);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if stream.write_all(response.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, accepted)
}

fn client_of(addr: std::net::SocketAddr, data_dir: &std::path::Path) -> RuntimeClient {
    write_desktop_credential(data_dir, &FakeSecretProtector, &"a".repeat(64)).unwrap();
    RuntimeClient::new(addr, data_dir.to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn consecutive_requests_from_one_client_reuse_the_connection() {
    let body = r#"{"error":"credential_invalid","message":"測試"}"#;
    let (addr, accepted) = fake_runtime(Duration::ZERO, "401 Unauthorized", body);
    let data_dir = tempfile::tempdir().unwrap();
    let client = client_of(addr, data_dir.path());

    for _ in 0..3 {
        let error = client.health().await.unwrap_err();
        assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "credential_invalid"), "{error:?}");
    }

    assert_eq!(accepted.load(Ordering::SeqCst), 1, "三次請求應共用同一條連線");
}

#[tokio::test]
async fn dispatch_waits_beyond_the_plain_request_limit_while_the_runtime_prepares_the_workdir() {
    // 負載高或 repo 大時，Runtime 準備工作目錄（建立 worktree、寫入並 fsync `.baton/.gitignore`）超過 2 秒
    let body = r#"{"queued":true,"task_id":"task_1","priority":0,"reasons":[]}"#;
    let (addr, _) = fake_runtime(Duration::from_millis(2_500), "202 Accepted", body);
    let data_dir = tempfile::tempdir().unwrap();
    let client = client_of(addr, data_dir.path());

    let outcome = client.dispatch_task("ws_1", "task_1").await;

    assert!(matches!(outcome, Ok(DispatchOutcome::Queued(_))), "派工不應在 Runtime 回應前就被當成連不上：{outcome:?}");
}
