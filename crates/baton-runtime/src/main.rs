//! Runtime 正式執行檔。

use baton_ports::Clock;
use baton_runtime::production_ports::{production_ports, SystemClock};
use baton_runtime::single_instance::InstanceGuard;
use baton_domain::guardian::GUARDIAN_ATTEMPT_ENV;
use baton_domain::login::{START_SOURCE_ENV, START_SOURCE_LOGIN};
use baton_guardian::files::{self, InstanceRecord};
use baton_runtime::{Runtime, RuntimeConfig, RuntimeOptions};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Runtime 啟動失敗：{message}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("restore") {
        return restore(&args[1..]);
    }
    let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok()).map_err(|e| e.to_string())?;

    // 單實例：已有 Runtime 在執行就直接結束，呼叫端連回既有的那個即可
    let Some(_guard) = InstanceGuard::acquire(&config.data_dir).map_err(|e| e.to_string())? else {
        println!("已有 Runtime 在執行，本程序結束");
        return Ok(());
    };

    let ports = production_ports(&config.data_dir, config.bind.port());
    // 記下本程序的身分，監護據此確認「是這個 Runtime 結束了」（票 32）
    let instance = InstanceRecord { pid: std::process::id(), recorded_at: ports.clock.now() };
    if let Err(error) = files::write_instance(&files::runtime_instance_path(&config.data_dir), &instance) {
        eprintln!("無法記下 Runtime 的程序身分（監護將無法監看這個 Runtime）：{error}");
    }
    // 監護拉起本程序時交付的嘗試識別（票 32）；登入啟動入口拉起時標明的啟動來源（票 33）
    let options = RuntimeOptions {
        guardian_attempt: std::env::var(GUARDIAN_ATTEMPT_ENV).ok().filter(|id| !id.is_empty()),
        login_start: std::env::var(START_SOURCE_ENV).ok().as_deref() == Some(START_SOURCE_LOGIN),
        ..RuntimeOptions::default()
    };
    let runtime = Runtime::start_with(config.clone(), ports, options).await.map_err(|e| e.to_string())?;
    println!("Runtime 已啟動：http://{}", runtime.local_addr());

    tokio::select! {
        _ = runtime.wait() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

/// `baton-runtime restore <備份目錄> <目標資料目錄>`：把一份備份（`backups/<備份識別>`）還原到乾淨的目錄（票 40）。
/// 還原後以 `BATON_DATA_DIR` 指向目標目錄啟動 Runtime；備份當下尚未結束的 Run 已被結束，不會自動重做。
fn restore(args: &[String]) -> Result<(), String> {
    let [backup_dir, target] = args else {
        return Err("用法：baton-runtime restore <備份目錄> <目標資料目錄>".into());
    };
    let now = SystemClock.now();
    let report = baton_store::backup::restore_backup(backup_dir.as_ref(), target.as_ref(), now).map_err(|e| e.to_string())?;
    for check in &report.checks {
        println!("{} {:?}：{}", if check.passed { "通過" } else { "失敗" }, check.kind, check.detail);
    }
    println!("已結束 {} 個備份當下尚未結束的 Run（不會自動重做）", report.interrupted_runs);
    if report.passed() {
        Ok(())
    } else {
        Err("還原完成，但有核對沒有通過；請不要使用這個目錄".into())
    }
}
