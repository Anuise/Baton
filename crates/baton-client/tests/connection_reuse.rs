//! App 以 0.5 秒輪詢 Runtime：同一個 RuntimeClient 的連續請求要重用連線，不是每次都重新建立 HTTP 用戶端與連線。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use baton_client::{ClientError, RuntimeClient};
use baton_credential::write_desktop_credential;
use baton_harness::fakes::FakeSecretProtector;

/// 只回 401 的最小 HTTP/1.1 伺服器（保持連線），記下接受了幾條連線。
fn keep_alive_server() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
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
                    let body = r#"{"error":"credential_invalid","message":"測試"}"#;
                    let response = format!(
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
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

#[tokio::test]
async fn consecutive_requests_from_one_client_reuse_the_connection() {
    let (addr, accepted) = keep_alive_server();
    let data_dir = tempfile::tempdir().unwrap();
    write_desktop_credential(data_dir.path(), &FakeSecretProtector, &"a".repeat(64)).unwrap();
    let client = RuntimeClient::new(addr, data_dir.path().to_path_buf(), Arc::new(FakeSecretProtector));

    for _ in 0..3 {
        let error = client.health().await.unwrap_err();
        assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "credential_invalid"), "{error:?}");
    }

    assert_eq!(accepted.load(Ordering::SeqCst), 1, "三次請求應共用同一條連線");
}
