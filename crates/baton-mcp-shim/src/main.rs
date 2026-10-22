use std::process::ExitCode;

use baton_mcp_shim::{serve, ShimConfig};
use tokio::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // 錯誤訊息只含變數名稱，不含憑證
    let config = match ShimConfig::from_lookup(|name| std::env::var(name).ok()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("baton-mcp-shim：{error}");
            return ExitCode::from(2);
        }
    };
    match serve(BufReader::new(tokio::io::stdin()), tokio::io::stdout(), config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("baton-mcp-shim：{error}");
            ExitCode::FAILURE
        }
    }
}
