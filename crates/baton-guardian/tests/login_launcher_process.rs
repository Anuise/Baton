//! 以真實的 baton-guardian 執行檔驗證登入啟動入口的抑制（票 33）：
//! 生命週期檔有停止抑制、維護抑制或內容毀損時，`--login` 直接以成功碼結束，不啟動 Runtime 也不當監護。

use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use baton_domain::lifecycle::{LifecycleFile, MaintenanceSuppression, StopSuppression, LIFECYCLE_FILE_NAME, LIFECYCLE_FILE_VERSION};
use baton_domain::login::{LAUNCHER_DATA_DIR_ARG, LAUNCHER_LOGIN_ARG, LAUNCHER_PORT_ARG};
use baton_domain::Timestamp;
use baton_guardian::files;

const GUARDIAN_EXE: &str = env!("CARGO_BIN_EXE_baton-guardian");
/// 受抑制時應該很快結束；超過這個時間就當作沒有被抑制。
const EXIT_WITHIN: Duration = Duration::from_secs(10);

fn unused_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
}

fn lifecycle(stop: bool, maintenance: bool) -> Vec<u8> {
    let file = LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: stop.then(|| StopSuppression { requested_at: Timestamp::from_millis(1), reason: "full_exit".into() }),
        maintenance: maintenance
            .then(|| MaintenanceSuppression { kind: "update".into(), requested_at: Timestamp::from_millis(1) }),
        updated_at: Timestamp::from_millis(1),
    };
    serde_json::to_vec(&file).unwrap()
}

/// 執行 `baton-guardian.exe --login --data-dir <data_dir> --port <未使用的埠>`，等它結束並回傳是否成功。
fn run_login_launcher(data_dir: &Path) -> bool {
    let mut child = Command::new(GUARDIAN_EXE)
        .arg(LAUNCHER_LOGIN_ARG)
        .arg(LAUNCHER_DATA_DIR_ARG)
        .arg(data_dir)
        .arg(LAUNCHER_PORT_ARG)
        .arg(unused_port().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + EXIT_WITHIN;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.success();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("登入啟動入口在 {EXIT_WITHIN:?} 內沒有結束（沒有被抑制）");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn suppressed_login_launch_exits_without_starting_runtime_or_guarding() {
    let cases: [(&str, Vec<u8>); 3] = [
        ("停止抑制", lifecycle(true, false)),
        ("維護抑制", lifecycle(false, true)),
        ("內容毀損", b"{broken".to_vec()),
    ];
    for (name, content) in cases {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(data_dir.path().join(LIFECYCLE_FILE_NAME), content).unwrap();

        assert!(run_login_launcher(data_dir.path()), "{name}：應以成功碼結束");

        assert!(!files::guardian_instance_path(data_dir.path()).exists(), "{name}：不應當監護");
        assert!(!files::runtime_instance_path(data_dir.path()).exists(), "{name}：不應啟動 Runtime");
        assert!(!data_dir.path().join("control.sqlite").exists(), "{name}：不應開啟資料庫");
    }
}
