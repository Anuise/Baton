//! Runtime 正式執行檔。

use baton_runtime::production_ports::production_ports;
use baton_runtime::single_instance::InstanceGuard;
use baton_runtime::{Runtime, RuntimeConfig};

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
    let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok()).map_err(|e| e.to_string())?;

    // 單實例：已有 Runtime 在執行就直接結束，呼叫端連回既有的那個即可
    let Some(_guard) = InstanceGuard::acquire(&config.data_dir).map_err(|e| e.to_string())? else {
        println!("已有 Runtime 在執行，本程序結束");
        return Ok(());
    };

    let runtime = Runtime::start(config.clone(), production_ports(&config.data_dir)).await.map_err(|e| e.to_string())?;
    println!("Runtime 已啟動：http://{}", runtime.local_addr());

    tokio::select! {
        _ = runtime.wait() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
