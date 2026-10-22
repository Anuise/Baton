//! shim 的情境測試：stdin／stdout 用 `tokio::io::duplex`，Runtime 用測試內自己起的 stub HTTP server。

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::Router;
use baton_domain::agent::{
    AGENT_TOKEN_ENV, AGENT_TOOLS_PREFIX, RUNTIME_URL_ENV, TOOL_GET_TASK_CONTRACT, TOOL_PERMISSION_PROMPT,
    TOOL_READ_INBOX, TOOL_REPORT_BLOCKED, TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION, TOOL_SAVE_CHECKPOINT,
    TOOL_SEND_MESSAGE,
};
use baton_domain::artifact::{ARTIFACT_MAX_BYTES, DESCRIPTION_MAX_CHARS, NAME_MAX_CHARS, TOOL_REGISTER_ARTIFACT};
use baton_domain::knowledge::{
    EVIDENCE_MAX, LESSON_FIELD_MAX_CHARS, QUERY_MAX_CHARS, TAGS_MAX, TITLE_MAX_CHARS, TOOL_RECORD_LESSON_CANDIDATE,
    TOOL_SEARCH_KNOWLEDGE,
};
use baton_domain::message::{INBOX_MAX_LIMIT, MAX_RECIPIENTS};
use baton_mcp_shim::{serve, ShimConfig};
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, BufReader};

const TOKEN: &str = "tok-SECRET-9f3a1c";
/// 沒有任何服務在聽的位址；只給不會真的發出 HTTP 請求的測試用。
const UNUSED_URL: &str = "http://127.0.0.1:1";

struct Recorded {
    method: String,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// 起一個固定回應的 stub Runtime，回傳 base URL 與收到的請求紀錄。
async fn spawn_stub(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback({
        let log = log.clone();
        move |request: Request| {
            let log = log.clone();
            async move {
                let (parts, body_in) = request.into_parts();
                let bytes = axum::body::to_bytes(body_in, usize::MAX).await.unwrap().to_vec();
                log.lock().unwrap().push(Recorded {
                    method: parts.method.to_string(),
                    path: parts.uri.path().to_string(),
                    headers: parts.headers,
                    body: bytes,
                });
                (StatusCode::from_u16(status).unwrap(), [(CONTENT_TYPE, "application/json")], body)
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, log)
}

/// 已關閉的埠：連線一定失敗。
fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

/// 送完所有輸入行並關閉 stdin，等 `serve` 乾淨結束，回傳 stdout 的每一行；同時確認輸出不含憑證。
async fn exchange(runtime_url: &str, input_lines: &[String]) -> Vec<String> {
    let (mut client_write, server_read) = duplex(1 << 16);
    let (server_write, mut client_read) = duplex(1 << 16);
    for line in input_lines {
        client_write.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    }
    client_write.shutdown().await.unwrap();

    let config = ShimConfig::new(runtime_url.to_string(), TOKEN.to_string());
    serve(BufReader::new(server_read), server_write, config).await.unwrap();

    let mut output = String::new();
    client_read.read_to_string(&mut output).await.unwrap();
    assert!(!output.contains(TOKEN), "輸出不應含憑證：{output}");
    output.lines().map(str::to_string).collect()
}

/// 送一行原始輸入，期待恰好一行回應。
async fn call_raw(runtime_url: &str, line: &str) -> Value {
    let output = exchange(runtime_url, &[line.to_string()]).await;
    assert_eq!(output.len(), 1, "應恰好一行回應：{output:?}");
    serde_json::from_str(&output[0]).unwrap()
}

async fn call(runtime_url: &str, request: Value) -> Value {
    call_raw(runtime_url, &request.to_string()).await
}

fn tool_call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}

#[tokio::test]
async fn initialize_echoes_a_supported_protocol_version() {
    let reply = call(
        UNUSED_URL,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
    )
    .await;

    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(reply["result"]["capabilities"], json!({"tools":{}}));
    assert_eq!(reply["result"]["serverInfo"]["name"], "baton-mcp-shim");
    assert_eq!(reply["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn initialize_falls_back_for_an_unknown_protocol_version() {
    for params in [json!({"protocolVersion":"1999-01-01"}), json!({})] {
        let reply = call(UNUSED_URL, json!({"jsonrpc":"2.0","id":"a","method":"initialize","params":params})).await;
        assert_eq!(reply["id"], "a");
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    }
}

#[tokio::test]
async fn tools_list_has_the_eleven_tools_with_their_required_fields() {
    let reply = call(UNUSED_URL, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let tools = reply["result"]["tools"].as_array().unwrap();

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            TOOL_GET_TASK_CONTRACT,
            TOOL_REPORT_PROGRESS,
            TOOL_SAVE_CHECKPOINT,
            TOOL_REPORT_BLOCKED,
            TOOL_REQUEST_DECISION,
            TOOL_PERMISSION_PROMPT,
            TOOL_SEND_MESSAGE,
            TOOL_READ_INBOX,
            TOOL_REGISTER_ARTIFACT,
            TOOL_RECORD_LESSON_CANDIDATE,
            TOOL_SEARCH_KNOWLEDGE,
        ]
    );
    for tool in tools {
        assert!(!tool["description"].as_str().unwrap().is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
    let schema = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap()["inputSchema"].clone();
    assert_eq!(schema(TOOL_GET_TASK_CONTRACT), json!({"type":"object","properties":{}}));
    assert_eq!(schema(TOOL_REPORT_PROGRESS)["required"], json!(["summary"]));
    assert_eq!(schema(TOOL_SAVE_CHECKPOINT)["required"], json!(["summary"]));
    assert_eq!(schema(TOOL_SAVE_CHECKPOINT)["properties"]["details"]["type"], "string");
    assert_eq!(schema(TOOL_REPORT_BLOCKED)["required"], json!(["reason"]));

    let decision = schema(TOOL_REQUEST_DECISION);
    assert_eq!(decision["required"], json!(["question", "reason", "options"]));
    assert_eq!(decision["properties"]["options"]["type"], "array");
    assert_eq!(decision["properties"]["options"]["minItems"], 2);
    assert_eq!(decision["properties"]["options"]["maxItems"], 8);
    assert_eq!(decision["properties"]["options"]["items"]["required"], json!(["id", "label"]));
    assert_eq!(decision["properties"]["urgency"]["enum"], json!(["low", "normal", "high"]));
    assert_eq!(decision["properties"]["deadline_minutes"]["type"], "integer");
    for optional in ["recommendation", "evidence"] {
        assert!(decision["properties"].get(optional).is_some(), "缺少 {optional}");
    }
    let permission = schema(TOOL_PERMISSION_PROMPT);
    assert_eq!(permission["required"], json!(["tool_name"]));
    assert_eq!(permission["properties"]["input"]["type"], "object");
    assert!(permission["properties"].get("tool_use_id").is_some());
    let send = schema(TOOL_SEND_MESSAGE);
    assert_eq!(send["required"], json!(["to", "subject"]));
    assert_eq!(send["properties"]["to"]["type"], "array");
    assert_eq!(send["properties"]["to"]["minItems"], 1);
    assert_eq!(send["properties"]["to"]["maxItems"], MAX_RECIPIENTS);
    assert_eq!(send["properties"]["kind"]["enum"], json!(["note", "question", "answer", "result"]));
    for field in ["subject", "body", "message_id", "in_reply_to", "correlation_id"] {
        assert_eq!(send["properties"][field]["type"], "string", "{field}");
    }
    let inbox = schema(TOOL_READ_INBOX);
    assert!(inbox.get("required").is_none());
    assert_eq!(inbox["properties"]["include_read"]["type"], "boolean");
    assert_eq!(inbox["properties"]["limit"]["minimum"], 1);
    assert_eq!(inbox["properties"]["limit"]["maximum"], INBOX_MAX_LIMIT);
    let artifact = schema(TOOL_REGISTER_ARTIFACT);
    assert_eq!(artifact["required"], json!(["name"]));
    let properties = &artifact["properties"];
    for field in ["name", "description", "content", "content_base64"] {
        assert_eq!(properties[field]["type"], "string", "{field}");
    }
    assert_eq!(properties["name"]["maxLength"], NAME_MAX_CHARS);
    assert_eq!(properties["description"]["maxLength"], DESCRIPTION_MAX_CHARS);
    // 大小上限寫在說明文字裡（單位 MiB，由常數換算）
    let limit = format!("{} MiB", ARTIFACT_MAX_BYTES / (1024 * 1024));
    for field in ["content", "content_base64"] {
        assert!(properties[field]["description"].as_str().unwrap().contains(&limit), "{field}");
    }

    // 候選教訓：說明要講清楚只是候選、需使用者確認；上限取自 domain 常數
    let lesson_tool = tools.iter().find(|t| t["name"] == TOOL_RECORD_LESSON_CANDIDATE).unwrap();
    let description = lesson_tool["description"].as_str().unwrap();
    assert!(description.contains("候選") && description.contains("確認"), "{description}");
    let lesson = schema(TOOL_RECORD_LESSON_CANDIDATE);
    assert_eq!(lesson["required"], json!(["title"]));
    let properties = &lesson["properties"];
    assert_eq!(properties["title"]["maxLength"], TITLE_MAX_CHARS);
    for field in ["trigger", "root_cause", "resolution"] {
        assert_eq!(properties[field]["maxLength"], LESSON_FIELD_MAX_CHARS, "{field}");
    }
    for field in ["tags", "evidence"] {
        assert_eq!(properties[field]["type"], "array", "{field}");
        assert_eq!(properties[field]["items"]["type"], "string", "{field}");
    }
    assert_eq!(properties["tags"]["maxItems"], TAGS_MAX);
    assert_eq!(properties["evidence"]["maxItems"], EVIDENCE_MAX);
    assert_eq!(properties["scope"]["enum"], json!(["task", "goal", "workspace"]));
    assert_eq!(properties["scope"]["default"], "task");

    let search = schema(TOOL_SEARCH_KNOWLEDGE);
    assert_eq!(search["properties"]["query"]["type"], "string");
    assert_eq!(search["properties"]["query"]["maxLength"], QUERY_MAX_CHARS);
    assert!(search.get("required").is_none(), "query 可省略（等同空字串）");
}

#[tokio::test]
async fn register_artifact_is_forwarded_with_credentials_and_an_unchanged_body() {
    let created = r#"{"artifact_id":"a1","version":1,"name":"report.md","size_bytes":5,"sha256":"ab","created":true}"#;
    let (url, log) = spawn_stub(201, created).await;
    let arguments = json!({"name":"report.md","description":"期末報告","content":"你好\nworld"});

    let reply = call(&url, tool_call(11, TOOL_REGISTER_ARTIFACT, arguments.clone())).await;

    assert_eq!(reply["id"], 11);
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][0]["text"], created);

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 1);
    let request = &log[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_REGISTER_ARTIFACT}"));
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.headers["x-baton-client"], "baton-desktop");
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, arguments);
    // 歸屬由憑證決定，shim 不填 workspace_id／run_id
    assert!(body.get("workspace_id").is_none() && body.get("run_id").is_none());
}

#[tokio::test]
async fn register_artifact_runtime_errors_become_tool_errors_like_other_tools() {
    let cases = [
        (400, r#"{"error":"artifact_invalid","message":"成果名稱必須是檔名"}"#, "artifact_invalid：成果名稱必須是檔名"),
        (413, r#"{"error":"artifact_too_large","message":"成果內容過大"}"#, "artifact_too_large：成果內容過大"),
        (409, r#"{"error":"run_report_stale","message":"這個 Run 已結束"}"#, "run_report_stale：這個 Run 已結束"),
    ];
    for (status, body, expected) in cases {
        let (url, _) = spawn_stub(status, body).await;

        let reply = call(&url, tool_call(12, TOOL_REGISTER_ARTIFACT, json!({"name":"../x","content":"c"}))).await;

        assert_eq!(reply["result"]["isError"], true, "HTTP {status}");
        assert!(reply.get("error").is_none());
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(expected), "{text}");
        assert!(text.contains(&status.to_string()), "{text}");
    }
}

#[tokio::test]
async fn register_artifact_accepts_a_request_line_of_about_14_mib() {
    // 10 MiB 的內容以 base64 編碼後約 14 MiB；單行 JSON-RPC 不可有長度上限
    let content_base64 = "QUJD".repeat(ARTIFACT_MAX_BYTES / 3 + 1);
    let request = tool_call(13, TOOL_REGISTER_ARTIFACT, json!({"name":"big.bin","content_base64":content_base64}));
    let line = format!("{request}\n");
    assert!(line.len() > 13 * 1024 * 1024);
    let (url, log) = spawn_stub(201, r#"{"artifact_id":"a1","version":1}"#).await;

    // 輸入遠大於 duplex 緩衝，寫入端必須與 serve 並行
    let (mut client_write, server_read) = duplex(1 << 16);
    let (server_write, mut client_read) = duplex(1 << 16);
    let writer = tokio::spawn(async move {
        client_write.write_all(line.as_bytes()).await.unwrap();
        client_write.shutdown().await.unwrap();
    });
    let config = ShimConfig::new(url, TOKEN.to_string());
    serve(BufReader::new(server_read), server_write, config).await.unwrap();
    writer.await.unwrap();

    let mut output = String::new();
    client_read.read_to_string(&mut output).await.unwrap();
    let reply: Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let log = log.lock().unwrap();
    let body: Value = serde_json::from_slice(&log[0].body).unwrap();
    assert_eq!(body["content_base64"].as_str().unwrap().len(), ARTIFACT_MAX_BYTES / 3 * 4 + 4);
}

#[tokio::test]
async fn request_decision_is_forwarded_to_its_tool_path_with_unchanged_arguments() {
    let (url, log) = spawn_stub(200, r#"{"decision_request_id":"d1","status":"waiting_decision","existing":false,"message":"ok"}"#).await;
    let arguments = json!({
        "question": "用哪個資料庫？",
        "reason": "契約沒有指定",
        "options": [{"id":"a","label":"SQLite"},{"id":"b","label":"Postgres","tradeoff":"要另外架設"}],
        "recommendation": "a",
        "urgency": "high",
        "deadline_minutes": 30
    });

    let reply = call(&url, tool_call(8, TOOL_REQUEST_DECISION, arguments.clone())).await;

    assert_eq!(reply["result"]["isError"], false);
    assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("waiting_decision"));
    let log = log.lock().unwrap();
    assert_eq!(log[0].method, "POST");
    assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_REQUEST_DECISION}"));
    assert_eq!(log[0].headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), arguments);
}

#[tokio::test]
async fn permission_prompt_is_forwarded_and_a_waiting_deny_is_not_a_tool_error() {
    // Runtime 在等待裁定時回 HTTP 200 與 behavior=deny；這是正常的權限回應，不是工具錯誤
    let body = r#"{"behavior":"deny","message":"等待使用者裁定","status":"waiting_decision","decision_request_id":"d2"}"#;
    let (url, log) = spawn_stub(200, body).await;
    let arguments = json!({"tool_name":"Bash","input":{"command":"rm -rf x"},"tool_use_id":"toolu_1"});

    let reply = call(&url, tool_call(9, TOOL_PERMISSION_PROMPT, arguments.clone())).await;

    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][0]["text"], body);
    let log = log.lock().unwrap();
    assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_PERMISSION_PROMPT}"));
    assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), arguments);
}

#[tokio::test]
async fn message_tools_forward_to_their_paths_with_credentials_and_unchanged_arguments() {
    let cases = [
        (
            TOOL_SEND_MESSAGE,
            json!({"to":["task-b","task-c"],"kind":"question","subject":"介面要不要改？","body":"本文\n含換行與 \"引號\"","message_id":"m-1","unknown":{"x":[1]}}),
            r#"{"delivered":["task-b","task-c"]}"#,
        ),
        (TOOL_READ_INBOX, json!({"include_read":true,"limit":5}), r#"{"messages":[]}"#),
    ];
    for (tool, arguments, response) in cases {
        let (url, log) = spawn_stub(200, response).await;

        let reply = call(&url, tool_call(11, tool, arguments.clone())).await;

        assert_eq!(reply["id"], 11);
        assert_eq!(reply["result"]["isError"], false, "{tool}");
        assert_eq!(reply["result"]["content"][0]["text"], response);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1, "{tool}");
        let request = &log[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, format!("{AGENT_TOOLS_PREFIX}/{tool}"));
        assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
        assert_eq!(request.headers["x-baton-client"], "baton-desktop");
        assert!(request.headers["content-type"].to_str().unwrap().starts_with("application/json"));
        assert_eq!(serde_json::from_slice::<Value>(&request.body).unwrap(), arguments, "{tool}");
    }
}

#[tokio::test]
async fn message_tools_turn_runtime_4xx_into_tool_errors_with_the_error_code() {
    for (tool, status, body) in [
        (TOOL_SEND_MESSAGE, 400, r#"{"error":"message_invalid","message":"主旨不能是空的"}"#),
        (TOOL_READ_INBOX, 403, r#"{"error":"forbidden","message":"憑證無效"}"#),
    ] {
        let (url, _) = spawn_stub(status, body).await;

        let reply = call(&url, tool_call(12, tool, json!({"subject":""}))).await;

        assert_eq!(reply["id"], 12);
        assert_eq!(reply["result"]["isError"], true, "{tool}");
        assert!(reply.get("error").is_none());
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        let code = if tool == TOOL_SEND_MESSAGE { "message_invalid" } else { "forbidden" };
        assert!(text.contains(code), "{text}");
        assert!(text.contains(&status.to_string()), "{text}");
    }
}

#[tokio::test]
async fn tools_call_forwards_to_the_tool_path_with_credentials_and_unchanged_arguments() {
    let (url, log) = spawn_stub(200, r#"{"ok":true,"lease":"renewed"}"#).await;
    let arguments = json!({"summary":"完成一半","unknown_field":{"nested":[1,2]}});

    let reply = call(&url, tool_call(7, TOOL_REPORT_PROGRESS, arguments.clone())).await;

    assert_eq!(reply["id"], 7);
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][0]["type"], "text");
    assert_eq!(reply["result"]["content"][0]["text"], r#"{"ok":true,"lease":"renewed"}"#);

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 1);
    let request = &log[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_REPORT_PROGRESS}"));
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.headers["x-baton-client"], "baton-desktop");
    assert!(request.headers["content-type"].to_str().unwrap().starts_with("application/json"));
    assert_eq!(serde_json::from_slice::<Value>(&request.body).unwrap(), arguments);
}

#[tokio::test]
async fn missing_arguments_are_forwarded_as_an_empty_object() {
    let (url, log) = spawn_stub(200, r#"{"goal":"x"}"#).await;

    let reply = call(&url, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":TOOL_GET_TASK_CONTRACT}})).await;

    assert_eq!(reply["result"]["isError"], false);
    let log = log.lock().unwrap();
    assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_GET_TASK_CONTRACT}"));
    assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), json!({}));
}

#[tokio::test]
async fn runtime_errors_become_tool_errors_with_code_and_message() {
    for status in [401, 422, 500] {
        let (url, _) = spawn_stub(status, r#"{"error":"report_invalid","message":"摘要不能是空的"}"#).await;

        let reply = call(&url, tool_call(4, TOOL_REPORT_PROGRESS, json!({"summary":""}))).await;

        assert_eq!(reply["id"], 4);
        assert_eq!(reply["result"]["isError"], true, "HTTP {status}");
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("report_invalid：摘要不能是空的"), "{text}");
        assert!(text.contains(&status.to_string()), "{text}");
    }
}

#[tokio::test]
async fn a_non_json_runtime_response_becomes_a_tool_error() {
    for (status, body) in [(200, "<html>nope</html>"), (502, "bad gateway")] {
        let (url, _) = spawn_stub(status, body).await;

        let reply = call(&url, tool_call(5, TOOL_GET_TASK_CONTRACT, json!({}))).await;

        assert_eq!(reply["result"]["isError"], true, "HTTP {status}");
        assert!(reply.get("error").is_none());
    }
}

#[tokio::test]
async fn an_unreachable_runtime_becomes_a_tool_error() {
    let reply = call(&closed_url(), tool_call(6, TOOL_REPORT_BLOCKED, json!({"reason":"卡住"}))).await;

    assert_eq!(reply["id"], 6);
    assert_eq!(reply["result"]["isError"], true);
    assert!(!reply["result"]["content"][0]["text"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_tools_methods_and_garbage_get_json_rpc_errors() {
    let reply = call(UNUSED_URL, tool_call(1, "delete_everything", json!({}))).await;
    assert_eq!(reply["error"]["code"], -32602);
    assert_eq!(reply["id"], 1);

    let reply = call(UNUSED_URL, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}})).await;
    assert_eq!(reply["error"]["code"], -32602);

    let reply = call(UNUSED_URL, json!({"jsonrpc":"2.0","id":3,"method":"resources/list"})).await;
    assert_eq!(reply["error"]["code"], -32601);
    assert_eq!(reply["id"], 3);

    let reply = call_raw(UNUSED_URL, "{not json").await;
    assert_eq!(reply["error"]["code"], -32700);
    assert_eq!(reply["id"], Value::Null);
}

#[tokio::test]
async fn notifications_produce_no_output_and_ping_gets_an_empty_result() {
    let input = [
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}),
        json!({"jsonrpc":"2.0","id":9,"method":"ping"}),
    ]
    .map(|line| line.to_string());

    let output = exchange(UNUSED_URL, &input).await;

    assert_eq!(output.len(), 1, "{output:?}");
    let reply: Value = serde_json::from_str(&output[0]).unwrap();
    assert_eq!(reply, json!({"jsonrpc":"2.0","id":9,"result":{}}));
}

#[test]
fn config_requires_both_variables_and_never_shows_the_token() {
    let both = |name: &str| match name {
        AGENT_TOKEN_ENV => Some(TOKEN.to_string()),
        RUNTIME_URL_ENV => Some(UNUSED_URL.to_string()),
        _ => None,
    };
    let config = ShimConfig::from_lookup(both).unwrap();
    assert_eq!(config.runtime_url, UNUSED_URL);
    assert_eq!(config.token, TOKEN);
    assert!(!format!("{config:?}").contains(TOKEN));

    let no_token = ShimConfig::from_lookup(|name| both(name).filter(|_| name != AGENT_TOKEN_ENV)).unwrap_err();
    assert!(no_token.to_string().contains(AGENT_TOKEN_ENV));
    let no_url = ShimConfig::from_lookup(|name| both(name).filter(|_| name != RUNTIME_URL_ENV)).unwrap_err();
    assert!(no_url.to_string().contains(RUNTIME_URL_ENV));
    assert!(ShimConfig::from_lookup(|_| Some(String::new())).is_err());
}

