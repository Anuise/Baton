use serde_json::json;

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);

fn file(path: &str, content: &str) -> SourceFile {
    SourceFile { path: path.into(), bytes: Some(content.as_bytes().to_vec()) }
}

fn absent(path: &str) -> SourceFile {
    SourceFile { path: path.into(), bytes: None }
}

fn sources(settings: Option<&str>, instructions: Option<&str>, state: Option<&str>) -> SourceFiles {
    SourceFiles {
        settings: settings.map_or_else(|| absent("settings.json"), |c| file("settings.json", c)),
        instructions: instructions.map_or_else(|| absent("CLAUDE.md"), |c| file("CLAUDE.md", c)),
        user_state: state.map_or_else(|| absent(".claude.json"), |c| file(".claude.json", c)),
    }
}

fn import_ok(settings: Option<&str>, instructions: Option<&str>, state: Option<&str>) -> ImportCandidate {
    import(&sources(settings, instructions, state)).expect("可匯入")
}

#[test]
fn imports_preferences_instructions_hooks_mcp_and_plugins_with_their_sources() {
    let candidate = import_ok(
        Some(
            r#"{"model":"opus","theme":"dark",
                "hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]},
                "enabledPlugins":{"fmt@market":true},
                "extraKnownMarketplaces":{"market":{"source":{"source":"github","repo":"a/b"}}}}"#,
        ),
        Some("# 指示\n請用繁體中文"),
        Some(r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","docs-mcp"]}},"userID":"abc"}"#),
    );

    let bundle = &candidate.bundle;
    assert_eq!(bundle.preferences["model"], json!("opus"));
    assert_eq!(bundle.preferences["theme"], json!("dark"));
    assert!(!bundle.preferences.contains_key("hooks"), "hooks 不算偏好");
    assert_eq!(bundle.instructions.as_deref(), Some("# 指示\n請用繁體中文"));
    assert!(bundle.hooks.contains_key("PreToolUse"));
    assert_eq!(bundle.mcp_servers["docs"]["command"], json!("npx"));
    assert_eq!(bundle.plugins["fmt@market"], json!(true));
    assert!(bundle.marketplaces.contains_key("market"));
    assert!(candidate.exclusions.is_empty());
    // 來源：三個檔案各一筆，附路徑與是否找到
    let kinds: Vec<_> = candidate.sources.iter().map(|s| (s.kind, s.found)).collect();
    assert_eq!(
        kinds,
        [(SourceKind::Settings, true), (SourceKind::Instructions, true), (SourceKind::UserState, true)]
    );
    assert!(candidate.sources.iter().all(|s| s.sha256.as_deref().is_some_and(|h| h.len() == 64)));
}

#[test]
fn user_state_file_contributes_only_mcp_servers_never_its_login_information() {
    let candidate = import_ok(
        None,
        None,
        Some(r#"{"oauthAccount":{"emailAddress":"a@b.c","accountUuid":"u-1"},"userID":"abc","mcpServers":{}}"#),
    );

    let serialized = serde_json::to_string(&candidate).unwrap();
    assert!(!serialized.contains("a@b.c") && !serialized.contains("accountUuid") && !serialized.contains("userID"));
    assert_eq!(candidate.bundle, SettingsBundle::default());
}

#[test]
fn missing_sources_import_as_empty_without_error() {
    let candidate = import_ok(None, None, None);

    assert_eq!(candidate.bundle, SettingsBundle::default());
    assert!(candidate.sources.iter().all(|s| !s.found && s.sha256.is_none()));
}

#[test]
fn login_related_settings_are_excluded_and_recorded_without_values() {
    let candidate = import_ok(
        Some(
            r#"{"model":"opus","apiKeyHelper":"/bin/get-key.sh","awsAuthRefresh":"aws sso login",
                "env":{"ANTHROPIC_API_KEY":"x","ANTHROPIC_BASE_URL":"https://proxy","CLAUDE_CODE_OAUTH_TOKEN":"y","EDITOR":"vim"}}"#,
        ),
        None,
        None,
    );

    assert_eq!(candidate.bundle.preferences["model"], json!("opus"));
    assert!(!candidate.bundle.preferences.contains_key("apiKeyHelper"));
    assert!(!candidate.bundle.preferences.contains_key("awsAuthRefresh"));
    assert_eq!(candidate.bundle.preferences["env"], json!({"EDITOR":"vim"}));
    let excluded: Vec<_> = candidate.exclusions.iter().map(|e| (e.item.as_str(), e.reason)).collect();
    for item in ["apiKeyHelper", "awsAuthRefresh", "env.ANTHROPIC_API_KEY", "env.ANTHROPIC_BASE_URL", "env.CLAUDE_CODE_OAUTH_TOKEN"] {
        assert!(excluded.contains(&(item, ExclusionReason::LoginRelated)), "{item} 應被排除：{excluded:?}");
    }
}

#[test]
fn secret_named_string_entries_are_dropped_but_the_item_survives() {
    let candidate = import_ok(
        None,
        None,
        Some(
            r#"{"mcpServers":{"gh":{"command":"gh-mcp","env":{"GITHUB_TOKEN":"ghp_abcdefghijklmnopqrstuvwxyz0123","REGION":"eu"},
                "headers":{"Authorization":"Bearer abcdefghijklmnopqrstuvwxyz","X-Trace":"1"}}}}"#,
        ),
    );

    let gh = &candidate.bundle.mcp_servers["gh"];
    assert_eq!(gh["env"], json!({"REGION":"eu"}));
    assert_eq!(gh["headers"], json!({"X-Trace":"1"}));
    let excluded: Vec<_> = candidate.exclusions.iter().map(|e| (e.item.as_str(), e.reason)).collect();
    assert!(excluded.contains(&("mcpServers.gh.env.GITHUB_TOKEN", ExclusionReason::SecretKey)));
    assert!(excluded.contains(&("mcpServers.gh.headers.Authorization", ExclusionReason::SecretKey)));
}

#[test]
fn environment_variable_references_are_not_secrets() {
    let candidate = import_ok(
        None,
        None,
        Some(r#"{"mcpServers":{"gh":{"command":"gh-mcp","headers":{"Authorization":"Bearer ${GITHUB_TOKEN}"},"env":{"TOKEN":"${MY_TOKEN}"}}}}"#),
    );

    assert!(candidate.exclusions.is_empty(), "{:?}", candidate.exclusions);
    assert_eq!(candidate.bundle.mcp_servers["gh"]["headers"]["Authorization"], json!("Bearer ${GITHUB_TOKEN}"));
}

#[test]
fn an_item_containing_a_secret_looking_value_is_excluded_as_a_whole() {
    let candidate = import_ok(
        Some(
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"curl -H 'Authorization: Bearer abcdefghijklmnopqrstuv' x"}]}],
                         "Stop":[{"hooks":[{"type":"command","command":"echo done"}]}]}}"#,
        ),
        None,
        Some(r#"{"mcpServers":{"leaky":{"command":"x","args":["--key=sk-ant-api03-abcdefghijklmnop"]},"ok":{"command":"y"}}}"#),
    );

    assert!(!candidate.bundle.hooks.contains_key("PreToolUse"));
    assert!(candidate.bundle.hooks.contains_key("Stop"));
    assert!(!candidate.bundle.mcp_servers.contains_key("leaky"));
    assert!(candidate.bundle.mcp_servers.contains_key("ok"));
    let excluded: Vec<_> = candidate.exclusions.iter().map(|e| (e.item.as_str(), e.reason)).collect();
    assert!(excluded.contains(&("hooks.PreToolUse", ExclusionReason::SecretValue)));
    assert!(excluded.contains(&("mcpServers.leaky", ExclusionReason::SecretValue)));
}

#[test]
fn instructions_containing_a_secret_are_excluded_whole() {
    let candidate = import_ok(None, Some("備註\nmy key is sk-ant-api03-abcdefghijklmnop\n"), None);

    assert_eq!(candidate.bundle.instructions, None);
    assert_eq!(candidate.exclusions.len(), 1);
    assert_eq!(candidate.exclusions[0].item, "CLAUDE.md");
    assert_eq!(candidate.exclusions[0].reason, ExclusionReason::SecretValue);
}

#[test]
fn exclusion_records_never_carry_the_secret_value() {
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123";
    let candidate = import_ok(
        Some(&format!(r#"{{"apiKey":"{secret}","env":{{"MY_SECRET":"{secret}"}}}}"#)),
        Some(&format!("token {secret}")),
        None,
    );

    assert!(!serde_json::to_string(&candidate).unwrap().contains(secret));
    assert!(candidate.exclusions.len() >= 3);
}

#[test]
fn secret_value_detection_covers_common_token_shapes_and_ignores_plain_text() {
    for secret in [
        "sk-ant-api03-abcdefghijklmnop",
        "sk-proj-abcdefghijklmnopqrstuvwx",
        "ghp_abcdefghijklmnopqrstuvwxyz0123",
        "github_pat_11ABCDEFG0abcdefghijklmnop",
        "xoxb-1234567890-abcdef",
        "AKIAABCDEFGHIJKLMNOP",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk",
        "Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
        "--password=hunter2hunter2",
        "postgres://user:pa55word@db.local/app",
        "-----BEGIN RSA PRIVATE KEY-----",
    ] {
        assert!(secrets::contains_secret_value(secret), "{secret} 應被偵測");
    }
    for plain in [
        "echo hello",
        "npx -y @scope/server",
        "https://example.com/path?x=1",
        "請在提交前執行測試，token 預算要節省",
        "sk-short",
        "max_tokens=4000",
    ] {
        assert!(!secrets::contains_secret_value(plain), "{plain} 不應被偵測");
    }
}

#[test]
fn secret_key_detection_ignores_token_budgets_and_non_strings() {
    assert!(secrets::is_secret_key("GITHUB_TOKEN"));
    assert!(secrets::is_secret_key("x-api-key"));
    assert!(secrets::is_secret_key("Authorization"));
    assert!(secrets::is_secret_key("client_secret"));
    assert!(!secrets::is_secret_key("CLAUDE_CODE_MAX_OUTPUT_TOKENS"));
    assert!(!secrets::is_secret_key("model"));

    let candidate = import_ok(Some(r#"{"tokenLimit":5000,"autoUpdates":true}"#), None, None);
    assert_eq!(candidate.bundle.preferences["tokenLimit"], json!(5000));
}

#[test]
fn malformed_sources_are_reported_not_guessed() {
    assert!(matches!(
        import(&sources(Some("{ not json"), None, None)),
        Err(ImportError::InvalidJson { file: SourceKind::Settings, .. })
    ));
    assert!(matches!(
        import(&sources(Some("[1,2]"), None, None)),
        Err(ImportError::NotAnObject { file: SourceKind::Settings })
    ));
    assert!(matches!(
        import(&sources(Some(r#"{"hooks":"oops"}"#), None, None)),
        Err(ImportError::UnexpectedShape { file: SourceKind::Settings, .. })
    ));
    let bad_text = SourceFiles {
        instructions: SourceFile { path: "CLAUDE.md".into(), bytes: Some(vec![0xff, 0xfe, 0x00]) },
        ..sources(None, None, None)
    };
    assert!(matches!(import(&bad_text), Err(ImportError::NotUtf8 { file: SourceKind::Instructions })));
    // 空白檔案視為沒有內容
    assert_eq!(import_ok(Some("  \n"), None, None).bundle, SettingsBundle::default());
}

#[test]
fn content_hash_is_stable_and_ignores_source_key_order_and_formatting() {
    let a = import_ok(Some(r#"{"model":"opus","theme":"dark"}"#), Some("x"), None);
    let b = import_ok(Some("{\n  \"theme\": \"dark\",\n  \"model\": \"opus\"\n}"), Some("x"), None);
    let c = import_ok(Some(r#"{"model":"sonnet","theme":"dark"}"#), Some("x"), None);

    assert_eq!(a.content_hash, b.content_hash);
    assert_ne!(a.content_hash, c.content_hash);
    assert_eq!(a.content_hash, a.bundle.content_hash());
    assert_eq!(a.content_hash.len(), 64);
}

#[test]
fn diff_lists_added_removed_and_changed_items_per_category() {
    let before = import_ok(
        Some(r#"{"model":"opus","theme":"dark","enabledPlugins":{"a@m":true}}"#),
        Some("舊指示"),
        Some(r#"{"mcpServers":{"docs":{"command":"x"}}}"#),
    );
    let after = import_ok(
        Some(r#"{"model":"sonnet","verbose":true,"enabledPlugins":{"a@m":true,"b@m":true}}"#),
        Some("新指示"),
        Some(r#"{"mcpServers":{}}"#),
    );

    let entries = diff(Some(&before.bundle), &after.bundle);
    let view: Vec<_> = entries.iter().map(|e| (e.category, e.key.as_str(), e.change)).collect();

    assert!(view.contains(&(Category::Preferences, "model", Change::Changed)));
    assert!(view.contains(&(Category::Preferences, "theme", Change::Removed)));
    assert!(view.contains(&(Category::Preferences, "verbose", Change::Added)));
    assert!(view.contains(&(Category::Plugins, "b@m", Change::Added)));
    assert!(view.contains(&(Category::Mcp, "docs", Change::Removed)));
    assert!(view.contains(&(Category::Instructions, "CLAUDE.md", Change::Changed)));
    assert!(!view.iter().any(|(_, k, _)| *k == "a@m"), "沒變的項目不列出");
    let model = entries.iter().find(|e| e.key == "model").unwrap();
    assert_eq!((model.before.clone(), model.after.clone()), (Some(json!("opus")), Some(json!("sonnet"))));
}

#[test]
fn diff_against_nothing_lists_everything_as_added_and_identical_bundles_have_no_diff() {
    let candidate = import_ok(Some(r#"{"model":"opus"}"#), Some("x"), None);

    let first = diff(None, &candidate.bundle);
    assert!(first.iter().all(|e| e.change == Change::Added) && first.len() == 2);
    assert!(diff(Some(&candidate.bundle), &candidate.bundle).is_empty());
}

#[test]
fn adoption_numbers_versions_consecutively_and_emits_an_event() {
    let candidate = import_ok(Some(r#"{"model":"opus"}"#), None, None);

    let first = adopt(None, candidate.clone(), T1);
    let second = adopt(Some(first.version.version), candidate.clone(), T1.plus_millis(5));

    assert_eq!((first.version.version, second.version.version), (1, 2));
    assert_eq!(first.version.content_hash, candidate.content_hash);
    assert_eq!(second.version.adopted_at, T1.plus_millis(5));
    assert_eq!(first.event.event_type, EVENT_ADOPTED);
    assert_eq!(
        (first.event.aggregate_type, first.event.aggregate_id.as_str(), first.event.aggregate_version),
        (AGGREGATE_TYPE, AGGREGATE_ID, 1)
    );
    assert_eq!(second.event.aggregate_version, 2);
}

#[test]
fn snapshot_files_round_trip_to_the_same_bundle_and_hash() {
    let candidate = import_ok(
        Some(r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo"}]}]},"enabledPlugins":{"a@m":true},"extraKnownMarketplaces":{"m":{"source":{"source":"github","repo":"a/b"}}}}"#),
        Some("# 指示"),
        Some(r#"{"mcpServers":{"docs":{"command":"x"}}}"#),
    );
    let version = adopt(None, candidate, T1).version;

    let files = snapshot_files(&version);
    let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, [SETTINGS_FILE, MCP_FILE, INSTRUCTIONS_FILE, MANIFEST_FILE]);

    let content: Vec<_> = files.iter().map(|f| (f.name.clone(), f.bytes.clone())).collect();
    let rebuilt = bundle_from_snapshot_files(&content).unwrap();
    assert_eq!(rebuilt, version.bundle);
    assert_eq!(rebuilt.content_hash(), version.content_hash);
    let manifest: serde_json::Value = serde_json::from_slice(&files.last().unwrap().bytes).unwrap();
    assert_eq!(manifest["content_hash"], json!(version.content_hash));
    assert_eq!(manifest["version"], json!(1));
    assert_eq!(manifest["files"][SETTINGS_FILE], json!(sha256_hex(&files[0].bytes)));
}

#[test]
fn snapshot_of_an_empty_bundle_has_only_a_manifest_and_rendering_is_deterministic() {
    let version = adopt(None, import_ok(None, None, None), T1).version;

    let files = snapshot_files(&version);
    assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), [MANIFEST_FILE]);
    assert_eq!(snapshot_files(&version), files);
    assert_eq!(bundle_from_snapshot_files(&[]).unwrap(), SettingsBundle::default());
}

#[test]
fn secrets_glued_to_cjk_text_or_flags_are_still_detected() {
    for secret in [
        "備忘：ghp_abcdefghijklmnopqrstuvwxyz0123",
        "金鑰sk-ant-api03-abcdefghijklmnop",
        "--key=sk-ant-api03-abcdefghijklmnop",
        "備忘password=hunter2hunter2",
    ] {
        assert!(secrets::contains_secret_value(secret), "{secret} 應被偵測");
    }
    assert!(!secrets::contains_secret_value("備忘：下一步 token=1"), "短值不算");
}

#[test]
fn secrets_used_as_item_names_are_masked_in_exclusion_records() {
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123";
    let candidate = import_ok(
        Some(&format!(
            r#"{{"{secret}":true,"model":"opus",
                "hooks":{{"{secret}":[{{"hooks":[{{"type":"command","command":"echo hi"}}]}}]}},
                "enabledPlugins":{{"{secret}":true}}}}"#
        )),
        None,
        Some(&format!(r#"{{"mcpServers":{{"{secret}":{{"command":"npx"}}}}}}"#)),
    );

    assert!(!serde_json::to_string(&candidate).unwrap().contains(secret));
    let items: Vec<_> = candidate.exclusions.iter().map(|e| (e.source, e.item.as_str(), e.reason)).collect();
    for expected in [
        (SourceKind::Settings, MASKED, ExclusionReason::SecretValue),
        (SourceKind::Settings, "hooks.<已遮蔽>", ExclusionReason::SecretValue),
        (SourceKind::Settings, "enabledPlugins.<已遮蔽>", ExclusionReason::SecretValue),
        (SourceKind::UserState, "mcpServers.<已遮蔽>", ExclusionReason::SecretValue),
    ] {
        assert!(items.contains(&expected), "{expected:?} 應在排除清單：{items:?}");
    }
    assert_eq!(candidate.bundle.preferences["model"], json!("opus"));
}

#[test]
fn secret_shaped_login_env_names_are_masked_in_exclusion_records() {
    let secret = "sk-ant-api03-abcdefghijklmnop";
    let candidate = import_ok(Some(&format!(r#"{{"env":{{"ANTHROPIC_API_KEY={secret}":"x","EDITOR":"vim"}}}}"#)), None, None);

    assert!(!serde_json::to_string(&candidate).unwrap().contains(secret));
    assert!(candidate.exclusions.iter().any(|e| e.item == "env.<已遮蔽>"));
}

#[test]
fn secret_named_headers_with_long_token_values_are_detected() {
    for secret in [
        "curl -H 'x-api-key: abcdefghijklmnop1234567890'",
        "curl -H 'X-Api-Key:abcdefghijklmnop1234567890'",
        "Authorization: abcdefghijklmnop1234567890",
    ] {
        assert!(secrets::contains_secret_value(secret), "{secret} 應被偵測");
    }
    for plain in [
        "x-api-key: ${API_KEY}",
        "Authorization: Bearer ${TOKEN}",
        "password: 請向管理員索取",
        "token: short123",
        "https://example.com:8443/abcdefghijklmnop1234567890",
    ] {
        assert!(!secrets::contains_secret_value(plain), "{plain} 不應被偵測");
    }
}
