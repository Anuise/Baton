//! 正式的 BrowserDriver（`LocalBrowserDriver`，票 46）對真實 Playwright 與本機 HTTP 伺服器的行為。
//!
//! Playwright 不在 repo 中：受驗專案（這裡是暫存目錄）與 `BATON_PLAYWRIGHT_DIR` 都解析不到時印出原因並略過。

use std::time::Duration;

use axum::response::Html;
use baton_ports::{BrowserCommand, BrowserDriver, BrowserOutcome, BrowserSessionRequest, BrowserStep, PageSnapshot, PortError};
use baton_runtime::browser_driver::LocalBrowserDriver;

const PASSWORD: &str = "s3cr3t-Pass!";

const LOGIN_PAGE: &str = r#"<!doctype html><html><head><title>登入</title></head><body>
<h1>請登入</h1>
<form method="post" action="/login">
  <input id="user" name="user">
  <input id="password" name="password" type="password">
  <button id="submit" type="submit">送出</button>
</form>
<p id="account">帳號 4111-1111</p>
</body></html>"#;

#[derive(serde::Deserialize)]
struct Login {
    user: String,
    password: String,
}

async fn start_site() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/login", axum::routing::get(|| async { Html(LOGIN_PAGE) }))
        .route(
            "/login",
            axum::routing::post(|axum::extract::Form(login): axum::extract::Form<Login>| async move {
                let ok = login.password == PASSWORD;
                Html(format!(
                    "<!doctype html><title>{}</title><p id=\"greeting\">{}</p>",
                    if ok { "首頁" } else { "登入失敗" },
                    if ok { format!("歡迎，{}", login.user) } else { "密碼錯誤".to_string() }
                ))
            }),
        )
        .route("/missing", axum::routing::get(|| async { (axum::http::StatusCode::NOT_FOUND, Html("<title>沒有</title><p>找不到頁面</p>")) }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn step(command: BrowserCommand, visible: &[&str], mask: Option<&[&str]>) -> BrowserStep {
    BrowserStep {
        command,
        timeout: Duration::from_secs(10),
        visible: visible.iter().map(|s| s.to_string()).collect(),
        screenshot_mask: mask.map(|m| m.iter().map(|s| s.to_string()).collect()),
    }
}

fn done(outcome: BrowserOutcome) -> PageSnapshot {
    match outcome {
        BrowserOutcome::Done { page } => page,
        other => panic!("預期完成：{other:?}"),
    }
}

/// 環境中沒有 Node.js、Playwright 或其瀏覽器時略過（不是 Baton 的錯誤）。
fn unavailable(error: &PortError) -> bool {
    let message = error.to_string();
    ["找不到 Playwright", "無法啟動 Node.js", "Executable doesn't exist"].iter().any(|m| message.contains(m))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_drives_a_real_browser_and_returns_its_trace_and_video() {
    let site = start_site().await;
    let project = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let driver = LocalBrowserDriver::default();
    let request = BrowserSessionRequest {
        working_dir: project.path().to_path_buf(),
        scratch_dir: scratch.path().to_path_buf(),
        trace: true,
        video: true,
    };
    let session = match driver.open(request).await {
        Ok(session) => session,
        Err(error) if unavailable(&error) => {
            eprintln!("略過：{error}（可設定 BATON_PLAYWRIGHT_DIR 指向已安裝 playwright 的目錄）");
            return;
        }
        Err(error) => panic!("{error}"),
    };

    // 導覽：狀態碼、標題、可見文字、可見性與遮罩截圖
    let goto = BrowserCommand::Goto { url: format!("http://{site}/login") };
    let page = done(driver.step(&session, step(goto, &["#user", "#greeting"], Some(&["#account"]))).await.unwrap());
    assert_eq!((page.status, page.title.as_str(), page.visible.clone()), (Some(200), "登入", vec![true, false]));
    assert!(page.text.contains("請登入") && page.text.contains("帳號 4111-1111"), "{}", page.text);
    let png = page.screenshot.expect("有要求截圖");
    assert!(png.starts_with(b"\x89PNG"), "截圖是 PNG");

    // 填表並送出：點擊造成的導覽也更新狀態碼與畫面
    let fill = |selector: &str, text: &str| BrowserCommand::Fill { selector: selector.into(), text: text.into() };
    done(driver.step(&session, step(fill("#user", "alice"), &[], None)).await.unwrap());
    done(driver.step(&session, step(fill("#password", PASSWORD), &[], None)).await.unwrap());
    let click = BrowserCommand::Click { selector: "#submit".into() };
    let page = done(driver.step(&session, step(click, &["#greeting"], None)).await.unwrap());
    assert_eq!((page.title.as_str(), page.status, page.visible.clone()), ("首頁", Some(200), vec![true]));
    assert!(page.text.contains("歡迎，alice"), "{}", page.text);
    assert_eq!(page.screenshot, None, "沒有要求就不截圖");

    let missing = BrowserCommand::Goto { url: format!("http://{site}/missing") };
    assert_eq!(done(driver.step(&session, step(missing, &[], None)).await.unwrap()).status, Some(404));

    // 找不到元素：操作失敗並附上當前畫面；錯誤訊息不含輸入的文字
    let mut absent = step(fill("#nope", PASSWORD), &[], None);
    absent.timeout = Duration::from_millis(500);
    match driver.step(&session, absent).await.unwrap() {
        BrowserOutcome::ActionFailed { error, message, page } => {
            assert_eq!(error, "TimeoutError");
            assert!(!message.contains(PASSWORD), "{message}");
            assert_eq!(page.expect("附上當前畫面").title, "沒有");
        }
        other => panic!("{other:?}"),
    }

    // 連不上：拿一個剛釋放的埠
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let unreachable = BrowserCommand::Goto { url: format!("http://{closed}/") };
    let outcome = driver.step(&session, step(unreachable, &[], None)).await.unwrap();
    assert!(matches!(&outcome, BrowserOutcome::Unreachable { reason } if reason.contains("net::ERR_")), "{outcome:?}");

    let files = driver.close(&session).await.unwrap();
    let trace = files.iter().find(|f| f.kind == "trace").expect("有 trace");
    assert_eq!(trace.name, "trace.zip");
    assert!(trace.bytes.starts_with(b"PK"), "trace 是 zip");
    assert_no_password_in_trace(&trace.bytes, scratch.path());
    let video = files.iter().find(|f| f.kind == "video").expect("有錄影");
    assert!(video.name.ends_with(".webm") && !video.bytes.is_empty(), "{}", video.name);

    // 已關閉的 Session 不能再操作；重複關閉無害
    let again = driver.step(&session, step(BrowserCommand::Goto { url: format!("http://{site}/login") }, &[], None)).await;
    assert!(again.is_err());
    assert!(driver.close(&session).await.unwrap().is_empty());
}

/// 以 Windows 內建的 bsdtar 解開 trace，確認輸入過的密碼（含表單編碼）沒有留在任何項目中。
fn assert_no_password_in_trace(bytes: &[u8], scratch: &std::path::Path) {
    let tar = std::path::Path::new(r"C:\Windows\System32\tar.exe");
    if !tar.is_file() {
        eprintln!("略過 trace 內容檢查：沒有 {}", tar.display());
        return;
    }
    let zip = scratch.join("check-trace.zip");
    let out = scratch.join("check-trace");
    std::fs::write(&zip, bytes).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let status = std::process::Command::new(tar).arg("-xf").arg(&zip).arg("-C").arg(&out).status().unwrap();
    assert!(status.success(), "trace 是有效的 zip");
    let encoded = "s3cr3t-Pass%21";
    let mut entries = 0;
    let mut pending = vec![out];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            entries += 1;
            let content = std::fs::read(&path).unwrap();
            for secret in [PASSWORD, encoded] {
                assert!(!content.windows(secret.len()).any(|w| w == secret.as_bytes()), "{} 含有密碼", path.display());
            }
        }
    }
    assert!(entries > 0);
}

#[tokio::test]
async fn opening_without_playwright_in_the_project_explains_what_to_install() {
    if std::env::var_os("BATON_PLAYWRIGHT_DIR").is_some() {
        eprintln!("略過：設定了 BATON_PLAYWRIGHT_DIR");
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let request = BrowserSessionRequest {
        working_dir: project.path().to_path_buf(),
        scratch_dir: scratch.path().to_path_buf(),
        trace: false,
        video: false,
    };
    let driver = LocalBrowserDriver::default();
    match driver.open(request).await {
        Err(error) if error.to_string().contains("無法啟動 Node.js") => eprintln!("略過：{error}"),
        Err(error) => assert!(error.to_string().contains("找不到 Playwright"), "{error}"),
        Ok(session) => {
            eprintln!("略過：暫存目錄的上層目錄解析得到 Playwright");
            driver.close(&session).await.unwrap();
        }
    }
}
