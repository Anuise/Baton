//! shim 的情境測試：stdin／stdout 用 `tokio::io::duplex`，Runtime 用測試內自己起的 stub HTTP server。

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::Router;
use baton_domain::external_operation::{TOOL_BEGIN_EXTERNAL_OPERATION, TOOL_RECORD_EXTERNAL_OPERATION_RESULT};
use baton_domain::agent::{
    AGENT_TOKEN_ENV, AGENT_TOOLS_PREFIX, BLOCKING_MAX_TASKS, RUNTIME_URL_ENV, TOOL_ADD_DEPENDENCY, TOOL_CANCEL_TASK,
    TOOL_CONFIRM_HANDOFF, TOOL_CONTROLLER_ACTION, TOOL_DISPATCH_TASK, TOOL_GET_TASK_CONTRACT,
    TOOL_PERMISSION_PROMPT, TOOL_PROPOSE_DECISION_REQUEST, TOOL_PROPOSE_TASK,
    TOOL_READ_INBOX, TOOL_REPORT_BLOCKED, TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION, TOOL_SAVE_CHECKPOINT,
    TOOL_SEND_MESSAGE, TOOL_UPDATE_TASK,
};
use baton_domain::artifact::{ARTIFACT_MAX_BYTES, DESCRIPTION_MAX_CHARS, NAME_MAX_CHARS, TOOL_REGISTER_ARTIFACT};
use baton_domain::knowledge::{
    EVIDENCE_MAX, LESSON_FIELD_MAX_CHARS, QUERY_MAX_CHARS, TAGS_MAX, TITLE_MAX_CHARS, TOOL_RECORD_LESSON_CANDIDATE,
    TOOL_SEARCH_KNOWLEDGE,
};
use baton_domain::message::{INBOX_MAX_LIMIT, MAX_RECIPIENTS};
use baton_domain::review::{
    CONTRACTS_MAX, DESIGN_MAX_CHARS, FINDINGS_MAX, PATHS_MAX, TOOL_CLASSIFY_FINDING, TOOL_GET_REVIEWS,
    TOOL_REPORT_DESIGN_DEVIATION, TOOL_RESOLVE_DESIGN_DEVIATION, TOOL_SUBMIT_REVIEW,
};
use baton_domain::understanding::{TOOL_GET_PROJECT_UNDERSTANDING, TOOL_RECORD_UNDERSTANDING};
use baton_domain::workflow::{
    EVIDENCE_MAX as WORKFLOW_EVIDENCE_MAX, NOTE_MAX_CHARS as WORKFLOW_NOTE_MAX_CHARS,
    TEXT_MAX_CHARS as WORKFLOW_TEXT_MAX_CHARS, TOOL_GET_WORKFLOWS, TOOL_RECORD_STAGE_RESULT, TOOL_START_WORKFLOW,
};
use baton_domain::verification::TOOL_ACCEPT_ARTIFACT;
use baton_domain::verification_profile::{
    Verdict, LIST_MAX as PROFILE_LIST_MAX, TEXT_MAX_CHARS as PROFILE_TEXT_MAX_CHARS, TOOL_GET_VERIFICATION,
    TOOL_RUN_VERIFICATION,
};
use baton_domain::exploration::{
    FindingStatus, MAX_MODEL_CALLS_LIMIT, MAX_STEPS_LIMIT, TOOL_DEFINE_EXPLORATION_CHARTER, TOOL_GET_EXPLORATION,
    TOOL_REPORT_EXPLORATION_FINDING, TOOL_REPRODUCE_EXPLORATION_FINDING, TOOL_TRIAGE_EXPLORATION_FINDING,
};
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
async fn tools_list_has_the_thirty_nine_tools_with_their_required_fields() {
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
            TOOL_CONTROLLER_ACTION,
            TOOL_SEND_MESSAGE,
            TOOL_READ_INBOX,
            TOOL_REGISTER_ARTIFACT,
            TOOL_RECORD_LESSON_CANDIDATE,
            TOOL_SEARCH_KNOWLEDGE,
            TOOL_BEGIN_EXTERNAL_OPERATION,
            TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
            TOOL_CONFIRM_HANDOFF,
            TOOL_PROPOSE_TASK,
            TOOL_UPDATE_TASK,
            TOOL_ADD_DEPENDENCY,
            TOOL_DISPATCH_TASK,
            TOOL_CANCEL_TASK,
            TOOL_PROPOSE_DECISION_REQUEST,
            TOOL_RECORD_UNDERSTANDING,
            TOOL_GET_PROJECT_UNDERSTANDING,
            TOOL_START_WORKFLOW,
            TOOL_RECORD_STAGE_RESULT,
            TOOL_GET_WORKFLOWS,
            TOOL_SUBMIT_REVIEW,
            TOOL_REPORT_DESIGN_DEVIATION,
            TOOL_CLASSIFY_FINDING,
            TOOL_RESOLVE_DESIGN_DEVIATION,
            TOOL_GET_REVIEWS,
            TOOL_RUN_VERIFICATION,
            TOOL_GET_VERIFICATION,
            TOOL_DEFINE_EXPLORATION_CHARTER,
            TOOL_REPORT_EXPLORATION_FINDING,
            TOOL_TRIAGE_EXPLORATION_FINDING,
            TOOL_REPRODUCE_EXPLORATION_FINDING,
            TOOL_GET_EXPLORATION,
            TOOL_ACCEPT_ARTIFACT,
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
    assert_eq!(schema(TOOL_CONFIRM_HANDOFF)["required"], json!(["completed", "incomplete", "unknown_side_effects"]));
    assert_eq!(schema(TOOL_BEGIN_EXTERNAL_OPERATION)["required"], json!(["idempotency_key", "operation", "target"]));
    let record = schema(TOOL_RECORD_EXTERNAL_OPERATION_RESULT);
    assert_eq!(record["required"], json!(["idempotency_key", "result"]));
    assert_eq!(record["properties"]["result"]["enum"], json!(["succeeded", "failed", "unknown"]));

    let action = schema(TOOL_CONTROLLER_ACTION);
    assert_eq!(action["required"], json!(["matter", "summary", "reason"]));
    assert!(action["properties"]["matter"]["enum"].as_array().unwrap().contains(&json!("modify_done_criteria")));

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

    // Controller 專屬工具：shim 不依角色過濾，所以描述必須寫明
    for name in [
        TOOL_PROPOSE_TASK,
        TOOL_UPDATE_TASK,
        TOOL_ADD_DEPENDENCY,
        TOOL_DISPATCH_TASK,
        TOOL_CANCEL_TASK,
        TOOL_PROPOSE_DECISION_REQUEST,
        TOOL_RECORD_UNDERSTANDING,
        TOOL_GET_PROJECT_UNDERSTANDING,
        TOOL_START_WORKFLOW,
        TOOL_RECORD_STAGE_RESULT,
        TOOL_GET_WORKFLOWS,
        TOOL_CLASSIFY_FINDING,
        TOOL_RESOLVE_DESIGN_DEVIATION,
        TOOL_GET_REVIEWS,
        TOOL_RUN_VERIFICATION,
        TOOL_GET_VERIFICATION,
        TOOL_DEFINE_EXPLORATION_CHARTER,
        TOOL_TRIAGE_EXPLORATION_FINDING,
        TOOL_REPRODUCE_EXPLORATION_FINDING,
        TOOL_GET_EXPLORATION,
        TOOL_ACCEPT_ARTIFACT,
    ] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert!(tool["description"].as_str().unwrap().contains("Controller 專屬"), "{name}");
    }
    // 票 54：Controller 也能用的共同工具，說明要寫明 Controller 的範圍與用法
    let description = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap()["description"].as_str().unwrap().to_string();
    for name in [TOOL_SEND_MESSAGE, TOOL_READ_INBOX, TOOL_RECORD_LESSON_CANDIDATE, TOOL_SEARCH_KNOWLEDGE] {
        assert!(description(name).contains("Controller"), "{name}");
    }
    assert!(description(TOOL_SEND_MESSAGE).contains("controller"), "收件人寫 controller 寄給 Controller");
    assert!(send["properties"]["to"]["description"].as_str().unwrap().contains("controller"));
    assert_eq!(lesson["properties"]["task_id"]["type"], "string");
    assert!(lesson.get("required") == Some(&json!(["title"])), "task_id 只有 Controller 必填，schema 不列為必填");
    let accept = schema(TOOL_ACCEPT_ARTIFACT);
    assert_eq!(accept["required"], json!(["task_id", "reason"]));
    for text in ["verification_gate_pending", "requires_decision"] {
        assert!(description(TOOL_ACCEPT_ARTIFACT).contains(text), "{text}");
    }
    let propose = schema(TOOL_PROPOSE_TASK);
    assert_eq!(propose["required"], json!(["title"]));
    assert_eq!(propose["properties"]["kind"]["enum"], json!(["development", "research", "document"]));
    assert_eq!(propose["properties"]["kind"]["default"], "research");
    assert_eq!(propose["properties"]["contract"]["type"], "object");
    for field in ["goal_id", "goal_title"] {
        assert_eq!(propose["properties"][field]["type"], "string", "{field}");
    }
    let update = schema(TOOL_UPDATE_TASK);
    assert_eq!(update["required"], json!(["task_id", "contract"]));
    assert_eq!(update["properties"]["note"]["type"], "string");
    assert_eq!(schema(TOOL_ADD_DEPENDENCY)["required"], json!(["task_id", "depends_on"]));
    let dispatch = schema(TOOL_DISPATCH_TASK);
    assert_eq!(dispatch["required"], json!(["task_id"]));
    assert_eq!(dispatch["properties"]["priority"]["type"], "integer");
    assert_eq!(schema(TOOL_CANCEL_TASK)["required"], json!(["task_id"]));
    let proposal = schema(TOOL_PROPOSE_DECISION_REQUEST);
    assert_eq!(proposal["required"], json!(["question", "reason", "options", "blocking"]));
    assert_eq!(proposal["properties"]["options"], decision["properties"]["options"]);
    assert_eq!(proposal["properties"]["blocking"]["type"], "array");
    assert_eq!(proposal["properties"]["blocking"]["minItems"], 1);
    assert_eq!(proposal["properties"]["blocking"]["maxItems"], BLOCKING_MAX_TASKS);
    let understanding = schema(TOOL_RECORD_UNDERSTANDING);
    assert_eq!(understanding["required"], json!(["section", "key"]));
    assert_eq!(understanding["properties"]["section"]["enum"], json!(["brief", "glossary", "map"]));
    assert_eq!(understanding["properties"]["status"]["enum"], json!(["confirmed", "hypothesis", "unknown", "retired"]));
    assert_eq!(understanding["properties"]["status"]["default"], "hypothesis");
    assert_eq!(understanding["properties"]["sources"]["type"], "array");
    assert_eq!(schema(TOOL_GET_PROJECT_UNDERSTANDING), json!({"type":"object","properties":{}}));
    let start = schema(TOOL_START_WORKFLOW);
    assert_eq!(start["required"], json!(["situation", "rationale"]));
    assert_eq!(
        start["properties"]["situation"]["enum"],
        json!(["ambiguous", "feature", "defect", "migration", "performance", "investigation", "exploratory_testing"])
    );
    for field in ["task_id", "goal_id", "rationale"] {
        assert_eq!(start["properties"][field]["type"], "string", "{field}");
    }
    assert_eq!(start["properties"]["rationale"]["maxLength"], WORKFLOW_TEXT_MAX_CHARS);
    let stage = schema(TOOL_RECORD_STAGE_RESULT);
    assert_eq!(stage["required"], json!(["workflow_id", "stage", "expected_revision", "outcome"]));
    assert_eq!(stage["properties"]["outcome"]["enum"], json!(["passed", "failed", "blocked"]));
    assert_eq!(stage["properties"]["expected_revision"]["type"], "integer");
    assert_eq!(stage["properties"]["expected_revision"]["minimum"], 1);
    assert_eq!(stage["properties"]["evidence"]["type"], "array");
    assert_eq!(stage["properties"]["evidence"]["maxItems"], WORKFLOW_EVIDENCE_MAX);
    assert_eq!(stage["properties"]["evidence"]["items"]["required"], json!(["check"]));
    for field in ["artifact_id", "decision_request_id", "task_id", "review_id", "evidence_bundle_id"] {
        assert_eq!(stage["properties"]["evidence"]["items"]["properties"][field]["type"], "string", "{field}");
    }
    assert_eq!(stage["properties"]["note"]["maxLength"], WORKFLOW_NOTE_MAX_CHARS);
    assert_eq!(schema(TOOL_GET_WORKFLOWS), json!({"type":"object","properties":{}}));

    // 雙軸審查：Worker 工具不是 Controller 專屬；作者不能審查自己的工作要寫在說明裡
    for name in [TOOL_SUBMIT_REVIEW, TOOL_REPORT_DESIGN_DEVIATION] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert!(!tool["description"].as_str().unwrap().contains("Controller 專屬"), "{name}");
    }
    let review_tool = tools.iter().find(|t| t["name"] == TOOL_SUBMIT_REVIEW).unwrap();
    let description = review_tool["description"].as_str().unwrap();
    for code in ["reviewer_is_author", "review_spec_outdated", "review_reuse_rejected"] {
        assert!(description.contains(code), "{code}");
    }
    let review = schema(TOOL_SUBMIT_REVIEW);
    assert_eq!(
        review["required"],
        json!(["subject_task_id", "base_revision", "candidate_revision", "spec_version", "standards_source", "standards", "spec"])
    );
    assert_eq!(review["properties"]["spec_version"]["type"], "integer");
    for axis in ["standards", "spec"] {
        let properties = &review["properties"][axis]["properties"];
        assert_eq!(review["properties"][axis]["type"], "object", "{axis}");
        assert_eq!(properties["paths"]["maxItems"], PATHS_MAX, "{axis}");
        assert_eq!(properties["findings"]["maxItems"], FINDINGS_MAX, "{axis}");
        assert_eq!(properties["findings"]["items"]["required"], json!(["title"]), "{axis}");
        for field in ["summary", "reuse_from"] {
            assert_eq!(properties[field]["type"], "string", "{axis}.{field}");
        }
    }
    let deviation = schema(TOOL_REPORT_DESIGN_DEVIATION);
    assert_eq!(deviation["required"], json!(["design_artifact_id", "design_version", "deviation", "reason", "proposed_design"]));
    assert_eq!(deviation["properties"]["design_version"]["type"], "integer");
    assert_eq!(deviation["properties"]["affected_contracts"]["type"], "array");
    assert_eq!(deviation["properties"]["affected_contracts"]["maxItems"], CONTRACTS_MAX);
    assert_eq!(deviation["properties"]["proposed_design"]["maxLength"], DESIGN_MAX_CHARS);
    let classify = schema(TOOL_CLASSIFY_FINDING);
    assert_eq!(classify["required"], json!(["finding_id", "classification", "reason"]));
    assert_eq!(classify["properties"]["classification"]["enum"], json!(["fix", "dismiss", "needs_decision"]));
    assert_eq!(classify["properties"]["evidence"]["type"], "array");
    let resolve = schema(TOOL_RESOLVE_DESIGN_DEVIATION);
    assert_eq!(resolve["required"], json!(["deviation_id", "resolution", "reason"]));
    assert_eq!(resolve["properties"]["resolution"]["enum"], json!(["accept", "reject"]));
    let reviews = schema(TOOL_GET_REVIEWS);
    assert!(reviews.get("required").is_none(), "task_id 可省略");
    assert_eq!(reviews["properties"]["task_id"]["type"], "string");
    let run = schema(TOOL_RUN_VERIFICATION);
    assert_eq!(run["required"], Value::Null);
    assert_eq!(run["properties"]["task_id"]["type"], "string");
    assert_eq!(run["properties"]["journeys"]["type"], "array");
    assert_eq!(run["properties"]["journeys"]["items"]["type"], "string");
    let run_description = tools.iter().find(|t| t["name"] == TOOL_RUN_VERIFICATION).unwrap()["description"].as_str().unwrap();
    for verdict in Verdict::ALL.map(Verdict::as_str) {
        assert!(run_description.contains(verdict), "{verdict}");
    }
    assert_eq!(schema(TOOL_GET_VERIFICATION), json!({"type":"object","properties":{}}));

    // 探索測試：章程欄位全部必填，上限取自 domain 常數
    let description = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap()["description"].as_str().unwrap().to_string();
    let charter = schema(TOOL_DEFINE_EXPLORATION_CHARTER);
    assert_eq!(
        charter["required"],
        json!(["task_id", "area", "role", "goal", "forbidden_side_effects", "max_steps", "max_model_calls"])
    );
    for field in ["task_id", "area", "role", "goal"] {
        assert_eq!(charter["properties"][field]["type"], "string", "{field}");
    }
    let side_effects = &charter["properties"]["forbidden_side_effects"];
    assert_eq!(side_effects["type"], "array");
    assert_eq!(side_effects["items"]["type"], "string");
    assert_eq!(side_effects["minItems"], 1);
    assert_eq!(side_effects["maxItems"], PROFILE_LIST_MAX);
    assert_eq!(charter["properties"]["max_steps"]["type"], "integer");
    assert_eq!(charter["properties"]["max_steps"]["minimum"], 1);
    assert_eq!(charter["properties"]["max_steps"]["maximum"], MAX_STEPS_LIMIT);
    assert_eq!(charter["properties"]["max_model_calls"]["type"], "integer");
    assert_eq!(charter["properties"]["max_model_calls"]["minimum"], 0);
    assert_eq!(charter["properties"]["max_model_calls"]["maximum"], MAX_MODEL_CALLS_LIMIT);

    // 回報疑點是 Worker 工具：不是 Controller 專屬，說明要寫明 worker_only 與「一律先是疑點」
    let report_description = description(TOOL_REPORT_EXPLORATION_FINDING);
    assert!(!report_description.contains("Controller 專屬"));
    for text in ["worker_only", "suspected"] {
        assert!(report_description.contains(text), "{text}");
    }
    let report = schema(TOOL_REPORT_EXPLORATION_FINDING);
    assert_eq!(report["required"], json!(["title", "steps", "expected", "actual"]));
    assert_eq!(report["properties"]["steps"]["type"], "array");
    assert_eq!(report["properties"]["steps"]["minItems"], 1);
    for field in ["title", "feature", "expected", "actual", "environment"] {
        assert_eq!(report["properties"][field]["type"], "string", "{field}");
        assert_eq!(report["properties"][field]["maxLength"], PROFILE_TEXT_MAX_CHARS, "{field}");
    }
    assert_eq!(report["properties"]["evidence"]["type"], "array");
    assert_eq!(report["properties"]["evidence"]["items"]["type"], "string");

    let triage = schema(TOOL_TRIAGE_EXPLORATION_FINDING);
    assert_eq!(triage["required"], json!(["finding_id", "action", "rationale"]));
    assert_eq!(triage["properties"]["action"]["enum"], json!(["exclude", "mark_unverified", "reopen"]));
    assert_eq!(
        triage["properties"]["reason"]["enum"],
        json!(["environment", "seed_data", "tool_limitation", "expected_design"])
    );
    let triage_description = description(TOOL_TRIAGE_EXPLORATION_FINDING);
    for text in ["exploration_defect_locked", "requires_decision", "expected_design", "reopen", "confirmed", "fixed", "regressed"] {
        assert!(triage_description.contains(text), "{text}");
    }

    let reproduce = schema(TOOL_REPRODUCE_EXPLORATION_FINDING);
    assert_eq!(reproduce["required"], json!(["finding_id"]));
    assert_eq!(reproduce["properties"]["journey"]["type"], "object");
    assert!(reproduce["properties"]["journey"]["description"].as_str().unwrap().contains("finding."));
    assert_eq!(reproduce["properties"]["task_id"]["type"], "string");
    let reproduce_description = description(TOOL_REPRODUCE_EXPLORATION_FINDING);
    for text in ["代表旅程", "product_failure", "confirmed", "unverified_risk", "fixed", "regressed", "診斷", "get_exploration"] {
        assert!(reproduce_description.contains(text), "{text}");
    }

    assert_eq!(schema(TOOL_GET_EXPLORATION), json!({"type":"object","properties":{}}));
    let overview_description = description(TOOL_GET_EXPLORATION);
    for status in FindingStatus::ALL.map(FindingStatus::as_str) {
        assert!(overview_description.contains(status), "{status}");
    }
}

#[tokio::test]
async fn controller_tools_forward_to_their_paths_with_credentials_and_unchanged_arguments() {
    let cases = [
        (
            TOOL_PROPOSE_TASK,
            json!({"title":"調查競品","goal_title":"市場分析","kind":"research","contract":{"objective":{"status":"filled","text":"列出三家競品"},"inputs":{"status":"unknown"},"required_capabilities":[]}}),
            r#"{"task_id":"t1","goal_id":"g1","status":"pending"}"#,
        ),
        (TOOL_UPDATE_TASK, json!({"task_id":"t1","contract":{"objective":{"status":"unknown"}},"note":"補充"}), r#"{"version":2}"#),
        (TOOL_ADD_DEPENDENCY, json!({"task_id":"t2","depends_on":"t1"}), r#"{"created":true}"#),
        (TOOL_DISPATCH_TASK, json!({"task_id":"t1","priority":5}), r#"{"status":"queued"}"#),
        (TOOL_CANCEL_TASK, json!({"task_id":"t2"}), r#"{"status":"cancelled"}"#),
        (TOOL_ACCEPT_ARTIFACT, json!({"task_id":"t1","reason":"報告符合完成條件"}), r#"{"status":"completed"}"#),
        (
            TOOL_PROPOSE_DECISION_REQUEST,
            json!({"question":"要哪一家雲端？","reason":"影響成本","options":[{"id":"a","label":"甲"},{"id":"b","label":"乙"}],"blocking":["t1"]}),
            r#"{"decision_request_id":"d1","status":"waiting_decision"}"#,
        ),
        (
            TOOL_RECORD_UNDERSTANDING,
            json!({"section":"glossary","key":"Run","text":"一次程序執行","status":"confirmed","sources":[{"path":"GLOSSARY.md","line":30}]}),
            r#"{"changed":true}"#,
        ),
        (TOOL_GET_PROJECT_UNDERSTANDING, json!({}), r#"{"items":[]}"#),
        (
            TOOL_START_WORKFLOW,
            json!({"situation":"defect","task_id":"task_1","rationale":"使用者回報登入失敗"}),
            r#"{"workflow_id":"wf_1","current_stage":"reproduction","revision":1}"#,
        ),
        (
            TOOL_RECORD_STAGE_RESULT,
            json!({"workflow_id":"wf_1","stage":"reproduction","expected_revision":1,"outcome":"passed","evidence":[{"check":"reproduction","artifact_id":"art_1"}],"note":"已重現"}),
            r#"{"outcome":"passed","current_stage":"diagnosis","revision":2}"#,
        ),
        (TOOL_GET_WORKFLOWS, json!({}), r#"{"playbooks":[],"workflows":[]}"#),
        (
            TOOL_CLASSIFY_FINDING,
            json!({"finding_id":"fnd_1","classification":"dismiss","reason":"規格沒有要求","evidence":["契約第 2 版"]}),
            r#"{"error":"requires_decision","decision_request_id":"d1"}"#,
        ),
        (
            TOOL_RESOLVE_DESIGN_DEVIATION,
            json!({"deviation_id":"dev_1","resolution":"reject","reason":"維持目前的介面"}),
            r#"{"deviation":{"id":"dev_1","status":"rejected"},"message":"已拒絕偏離"}"#,
        ),
        (TOOL_GET_REVIEWS, json!({"task_id":"task_1"}), r#"{"workspace_id":"w1","reviews":[],"deviations":[]}"#),
        (
            TOOL_RUN_VERIFICATION,
            json!({"task_id":"task_1","journeys":["login"]}),
            r#"{"id":"vreq_1","journeys":["login"],"status":"queued"}"#,
        ),
        (TOOL_GET_VERIFICATION, json!({}), r#"{"profile":null,"requests":[],"bundles":[]}"#),
        (
            TOOL_DEFINE_EXPLORATION_CHARTER,
            json!({"task_id":"task_9","area":"結帳","role":"一般會員","goal":"找出折扣計算錯誤","forbidden_side_effects":["不得送出真實付款"],"max_steps":40,"max_model_calls":5}),
            r#"{"charter":{"task_id":"task_9","version":1},"created":true}"#,
        ),
        (
            TOOL_TRIAGE_EXPLORATION_FINDING,
            json!({"finding_id":"xf_1","action":"exclude","reason":"expected_design","rationale":"規格寫明折扣不疊加"}),
            r#"{"error":"requires_decision","decision_request_id":"d2"}"#,
        ),
        (
            TOOL_REPRODUCE_EXPLORATION_FINDING,
            json!({
                "finding_id": "xf_1",
                "task_id": "task_10",
                "journey": {
                    "key": "case",
                    "title": "折扣碼套用兩次",
                    "steps": [{"key":"apply","operation":{"kind":"http","method":"POST","url":"http://127.0.0.1:8080/cart/discount"},"assertions":[{"kind":"status","equals":200}]}]
                }
            }),
            r#"{"request":{"id":"vreq_2","journeys":["smoke","finding.xf_1"],"status":"queued"},"message":"已排入重現"}"#,
        ),
        (TOOL_GET_EXPLORATION, json!({}), r#"{"workspace_id":"w1","charters":[],"findings":[]}"#),
    ];
    for (tool, arguments, response) in cases {
        let (url, log) = spawn_stub(200, response).await;

        let reply = call(&url, tool_call(14, tool, arguments.clone())).await;

        assert_eq!(reply["result"]["isError"], false, "{tool}");
        assert_eq!(reply["result"]["content"][0]["text"], response);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1, "{tool}");
        let request = &log[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, format!("{AGENT_TOOLS_PREFIX}/{tool}"));
        assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
        assert_eq!(request.headers["x-baton-client"], "baton-desktop");
        assert_eq!(serde_json::from_slice::<Value>(&request.body).unwrap(), arguments, "{tool}");
    }
}

#[tokio::test]
async fn a_worker_calling_a_controller_tool_gets_the_controller_only_error() {
    let (url, _) = spawn_stub(403, r#"{"error":"controller_only","message":"只有 Controller 可以使用這個工具"}"#).await;

    let reply = call(&url, tool_call(15, TOOL_DISPATCH_TASK, json!({"task_id":"t1"}))).await;

    assert_eq!(reply["result"]["isError"], true);
    assert!(reply.get("error").is_none());
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("controller_only") && text.contains("403"), "{text}");
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


#[tokio::test]
async fn external_operation_tools_forward_to_their_paths_unchanged() {
    let cases = [
        (
            TOOL_BEGIN_EXTERNAL_OPERATION,
            json!({"idempotency_key":"deploy-1","operation":"deploy","target":"production","request":{"version":"1.2.3"}}),
            r#"{"operation_id":"extop_1","status":"proceed","attempt":1,"existing":false,"message":"可以送出"}"#,
        ),
        (
            TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
            json!({"idempotency_key":"deploy-1","result":"unknown","reason":"逾時"}),
            r#"{"operation_id":"extop_1","status":"unknown","decision_request_id":"d1","message":"等待查證"}"#,
        ),
    ];
    for (tool, arguments, response) in cases {
        let (url, log) = spawn_stub(200, response).await;

        let reply = call(&url, tool_call(12, tool, arguments.clone())).await;

        assert_eq!(reply["result"]["isError"], false, "{tool}");
        assert_eq!(reply["result"]["content"][0]["text"], response);
        let log = log.lock().unwrap();
        assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{tool}"));
        assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), arguments);
    }
}

#[tokio::test]
async fn report_exploration_finding_is_forwarded_to_its_tool_path_unchanged() {
    let arguments = json!({
        "title": "折扣碼可以重複套用",
        "feature": "checkout.discount",
        "steps": ["加入商品", "輸入折扣碼 SAVE10 兩次"],
        "expected": "第二次被拒絕",
        "actual": "總價被折扣兩次",
        "environment": "build abc123、Chromium",
        "evidence": ["art_7", "vbun_3"]
    });
    let response = r#"{"id":"xf_1","status":"suspected"}"#;
    let (url, log) = spawn_stub(201, response).await;

    let reply = call(&url, tool_call(17, TOOL_REPORT_EXPLORATION_FINDING, arguments.clone())).await;

    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][0]["text"], response);
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].method, "POST");
    assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{TOOL_REPORT_EXPLORATION_FINDING}"));
    assert_eq!(log[0].headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), arguments);
}

#[tokio::test]
async fn exploration_runtime_errors_become_tool_errors_with_the_error_code() {
    let (url, _) = spawn_stub(
        409,
        r#"{"error":"exploration_defect_locked","message":"疑點 xf_1 已經過重現確認為缺陷（confirmed）"}"#,
    )
    .await;

    let reply = call(
        &url,
        tool_call(18, TOOL_TRIAGE_EXPLORATION_FINDING, json!({"finding_id":"xf_1","action":"mark_unverified","rationale":"無法重現"})),
    )
    .await;

    assert_eq!(reply["result"]["isError"], true);
    assert!(reply.get("error").is_none());
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("exploration_defect_locked") && text.contains("409"), "{text}");
}

#[tokio::test]
async fn review_worker_tools_forward_to_their_paths_unchanged() {
    let cases = [
        (
            TOOL_SUBMIT_REVIEW,
            json!({
                "subject_task_id": "task_1",
                "base_revision": "abc123",
                "candidate_revision": "def456",
                "spec_version": 2,
                "standards_source": "AGENTS.md@abc123",
                "standards": {"summary":"對照 AGENTS.md","paths":["src"],"findings":[{"title":"缺少錯誤處理","location":"src/export.rs:42"}]},
                "spec": {"reuse_from":"rev_0"}
            }),
            r#"{"id":"rev_1","passed":false}"#,
        ),
        (
            TOOL_REPORT_DESIGN_DEVIATION,
            json!({"design_artifact_id":"art_1","design_version":3,"deviation":"改用串流匯出","reason":"記憶體不足","affected_contracts":["export API"],"proposed_design":"# 設計
改用串流"}),
            r#"{"id":"dev_1","status":"open"}"#,
        ),
    ];
    for (tool, arguments, response) in cases {
        let (url, log) = spawn_stub(201, response).await;

        let reply = call(&url, tool_call(16, tool, arguments.clone())).await;

        assert_eq!(reply["result"]["isError"], false, "{tool}");
        assert_eq!(reply["result"]["content"][0]["text"], response);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1, "{tool}");
        assert_eq!(log[0].path, format!("{AGENT_TOOLS_PREFIX}/{tool}"));
        assert_eq!(log[0].headers["authorization"], format!("Bearer {TOKEN}"));
        assert_eq!(serde_json::from_slice::<Value>(&log[0].body).unwrap(), arguments, "{tool}");
    }
}
