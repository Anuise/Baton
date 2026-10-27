use super::*;

/// 去掉標題行之後的內文（標題行帶版本號）。
fn body(text: &str) -> &str {
    text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
}

#[test]
fn the_playbook_title_carries_its_version() {
    let title = CONTROLLER_PLAYBOOK.lines().next().unwrap();
    assert_eq!(title, format!("# Baton Controller 操作手冊（版本 {PLAYBOOK_VERSION}）"));
    assert!(!PLAYBOOK_VERSION.is_empty() && PLAYBOOK_VERSION.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
}

/// 帳號名稱、額度數字、並行上限與 worktree 路徑都由 Runtime 負責，不寫進操作手冊（spec「Controller 操作手冊」）。
#[test]
fn the_playbook_has_no_account_names_quota_numbers_concurrency_limits_or_worktree_paths() {
    let text = CONTROLLER_PLAYBOOK;
    for forbidden in [
        // 帳號名稱與帳號目錄
        "Work-", ".claude-", "~/", "~\\",
        // 額度門檻
        "%", "％", "5h", "7d", "≥", ">=", "剩餘",
        // 並行上限
        "< ", "並行數上限為", "進行中 run",
        // worktree 路徑
        "-wt", "../", "..\\", ":\\", ":/",
    ] {
        assert!(!text.contains(forbidden), "操作手冊不應含有 {forbidden:?}");
    }
    // 內文除了編號清單的序號與等待指令的結束碼之外，唯一允許的數字是「最多修正 3 次」
    for line in body(text).lines() {
        let line = line.trim_start();
        let numbered = line.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        let mut rest = numbered.map_or(line, |(_, rest)| rest);
        while let Some(at) = rest.find(|c: char| c.is_ascii_digit()) {
            let digits: String = rest[at..].chars().take_while(char::is_ascii_digit).collect();
            let after = rest[at + digits.len()..].trim_start();
            let exit_code = rest[..at].ends_with("結束碼 ") && digits.len() == 1;
            assert!(exit_code || (digits == "3" && after.starts_with('次')), "操作手冊內文出現數字 {digits:?}：{line}");
            rest = &rest[at + digits.len()..];
        }
    }
}

#[test]
fn the_playbook_covers_the_platform_way_of_working() {
    for topic in [
        "frontier",
        "dispatch_task",
        "\"$BATON_MCP_SHIM\" wait",
        "背景",
        "結束碼 0",
        "結束碼 3",
        "結束碼 4",
        "唯讀驗證",
        "最多修正 3 次",
        "受阻",
        "先自己決定",
        "決策權限表",
        "propose_decision_request",
        AI_DECISIONS_RELATIVE_PATH,
        "- 審閱：未審",
        "不能放寬",
        "CLAUDE.md",
    ] {
        assert!(CONTROLLER_PLAYBOOK.contains(topic), "操作手冊應提到 {topic:?}");
    }
}

#[test]
fn the_worker_rules_require_tdd_the_task_branch_at_most_three_subagents_and_append_only_decisions() {
    for rule in ["TDD", "worktree 分支", "最多 3 個 subagent", "AI 決策紀錄", "只能在檔尾追加", "- 審閱：未審"] {
        assert!(WORKER_RULES.contains(rule), "Worker 通則應包含 {rule:?}");
    }
}

/// 範本的說明與條目格式和 `.scratch/platform-rebuild-v1/ai-decisions.md` 的檔頭相同。
#[test]
fn the_ai_decisions_template_has_the_same_header_format_as_the_scratch_files() {
    let entry_format = "## 條目格式\n\n```markdown\n### AI-NN：<主題>\n- 重要度：高｜中｜低\n- 時間／階段：YYYY-MM-DD，<拆票｜票 NN 實作｜驗證…>\n- 決策：…\n- 理由：…\n- 替代方案：…\n- 影響：…\n- 審閱：未審\n```\n\n---\n";
    assert!(AI_DECISIONS_TEMPLATE.starts_with("# AI 決策紀錄\n"), "{AI_DECISIONS_TEMPLATE}");
    assert!(AI_DECISIONS_TEMPLATE.ends_with(entry_format), "{AI_DECISIONS_TEMPLATE}");
    for line in [
        "- 本檔記錄",
        "編號為 AI-NN，只能往後追加。",
        "- **重要度**：",
        "  - **高**：做錯了會讓帳號額度、安全、資料或大量工作白費，請優先查看。",
        "  - **中**：影響工作方式或範圍，事後修改有一定成本。",
        "  - **低**：預設值或慣例，改起來便宜。",
        "- 每個條目都有一行 `- 審閱：未審｜認同 YYYY-MM-DD｜已修正`。新條目一律寫 `- 審閱：未審`；找未審條目用 `grep -n \"^- 審閱：未審\"`。",
        "- 使用者表示同意時，把審閱行改成 `- 審閱：認同 YYYY-MM-DD`。",
        "- 使用者修正時，把審閱行改成 `- 審閱：已修正`，並照舊在條目下方加一行 `使用者修正：…`；AI 看到後依修正調整，並在需要時另開新條目說明影響。",
    ] {
        assert!(AI_DECISIONS_TEMPLATE.contains(line), "範本缺少 {line:?}");
    }
}

#[test]
fn the_ai_decisions_file_lives_under_the_workspaces_baton_directory() {
    assert_eq!(AI_DECISIONS_RELATIVE_PATH, ".baton/ai-decisions.md");
    let path = ai_decisions_path(std::path::Path::new(r"C:\work\app"));
    assert_eq!(path, std::path::Path::new(r"C:\work\app").join(".baton").join("ai-decisions.md"));
}

#[test]
fn the_system_prompt_is_the_base_followed_by_the_shared_instructions_when_there_are_any() {
    assert_eq!(system_prompt("規則\n", None), "規則\n");
    assert_eq!(system_prompt("規則\n", Some("  \n")), "規則\n", "空白的共同指示不附加");
    assert_eq!(system_prompt("規則\n", Some("使用者指示\n")), "規則\n\n---\n\n使用者指示\n");
    assert!(system_prompt(CONTROLLER_PLAYBOOK, Some("x")).starts_with(CONTROLLER_PLAYBOOK));
}
