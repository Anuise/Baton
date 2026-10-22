//! 真實 stdio 程序測試：啟動 bin，以環境變數設定，確認協定輸出與秘密不外洩。

use std::io::Write;
use std::process::{Command, Output, Stdio};

use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use serde_json::{json, Value};

const TOKEN: &str = "tok-PROCESS-SECRET-77c2";

fn shim() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_baton-mcp-shim"));
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

fn run_with_input(mut command: Command, input: &str) -> Output {
    let mut child = command.spawn().unwrap();
    // 寫完後 stdin 隨即關閉，shim 應乾淨結束（缺環境變數時 shim 可能先退出，寫入失敗可忽略）
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn speaks_mcp_over_stdio_and_leaks_no_token() {
    // 已關閉的埠：工具呼叫一定連線失敗
    let closed_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut command = shim();
    command.env(AGENT_TOKEN_ENV, TOKEN).env(RUNTIME_URL_ENV, format!("http://127.0.0.1:{closed_port}"));
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_task_contract","arguments":{}}}),
    ]
    .map(|line| format!("{line}\n"))
    .concat();

    let output = run_with_input(command, &input);

    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let replies: Vec<Value> = stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(), [json!(1), json!(2), json!(3)]);
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 31);
    assert_eq!(replies[2]["result"]["isError"], true);
    assert!(!stdout.contains(TOKEN));
    assert!(!String::from_utf8(output.stderr).unwrap().contains(TOKEN));
}

#[test]
fn exits_with_code_2_when_a_variable_is_missing() {
    let mut command = shim();
    command.env(AGENT_TOKEN_ENV, TOKEN).env_remove(RUNTIME_URL_ENV);
    let output = run_with_input(command, "");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(RUNTIME_URL_ENV));
    assert!(!stderr.contains(TOKEN));
    assert!(output.stdout.is_empty());

    let mut command = shim();
    command.env_remove(AGENT_TOKEN_ENV).env_remove(RUNTIME_URL_ENV);
    assert_eq!(run_with_input(command, "").status.code(), Some(2));
}
