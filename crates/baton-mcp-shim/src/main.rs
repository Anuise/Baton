use std::ffi::OsString;
use std::process::ExitCode;

use baton_domain::controller_cli::HOST_SUBCOMMAND;
use baton_domain::controller_wait::WAIT_SUBCOMMAND;
use baton_mcp_shim::{run_controller_host, run_wait, serve, ShimConfig};
use tokio::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|first| first == HOST_SUBCOMMAND) {
        // 結束碼沿用子程序的（可能超出 u8），所以用 process::exit
        let code = run_controller_host(&args[1..], |name| std::env::var(name).ok()).await;
        std::process::exit(code);
    }
    if args.first().is_some_and(|first| first == WAIT_SUBCOMMAND) {
        let code = run_wait(&args[1..], |name| std::env::var(name).ok(), &mut std::io::stdout()).await;
        std::process::exit(code);
    }
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
