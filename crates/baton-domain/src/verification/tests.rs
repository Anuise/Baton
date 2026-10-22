use super::*;
use crate::artifact::ArtifactVersion;
use crate::run::Run;
use crate::task::{ContractContent, ContractField, TaskContract};
use crate::workspace::{self, Workspace, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);
const T1: Timestamp = Timestamp::from_millis(2_000_000);
/// SHA-256("abc")
const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn command(program: &str, expected_exit_code: i32) -> VerificationCheck {
    VerificationCheck::Command { program: program.into(), args: vec!["test".into()], expected_exit_code }
}

fn artifact_check(name: &str, sha256: Option<&str>) -> VerificationCheck {
    VerificationCheck::Artifact { name: name.into(), sha256: sha256.map(Into::into) }
}

fn file_check(path: &str, sha256: Option<&str>) -> VerificationCheck {
    VerificationCheck::File { path: path.into(), sha256: sha256.map(Into::into) }
}

fn passed(check: VerificationCheck) -> CheckResult {
    CheckResult { check, status: CheckStatus::Passed, detail: "ok".into() }
}

fn failed(check: VerificationCheck) -> CheckResult {
    CheckResult { check, status: CheckStatus::Failed, detail: "結束碼 1（預期 0）".into() }
}

/// 一個驗證中的 Task：Run（世代 1）已成功結束，契約宣告了 `checks`。
struct World {
    workspace: Workspace,
    tasks: Vec<Task>,
    contracts: Vec<TaskContract>,
    runs: Vec<Run>,
    artifacts: Vec<Artifact>,
    workdirs: Vec<Workdir>,
    integrations: Vec<IntegrationEntry>,
}

impl World {
    fn verifying(checks: Vec<VerificationCheck>) -> Self {
        let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
        let workspace = workspace::decide(&[], create, T0).unwrap().workspace;
        let task = Task {
            id: "t1".into(),
            workspace_id: "w1".into(),
            goal_id: "g1".into(),
            title: "整理".into(),
            kind: TaskKind::Research,
            status: TaskStatus::Verifying,
            version: 5,
            contract_version: 1,
            created_at: T0,
            updated_at: T0,
        };
        let content = ContractContent {
            objective: Some(ContractField::Filled { text: "x".into(), references: vec![] }),
            verification_checks: checks,
            ..ContractContent::default()
        };
        let contract = TaskContract { task_id: "t1".into(), version: 1, content, note: None, created_at: T0 };
        let mut world =
            Self {
            workspace,
            tasks: vec![task],
            contracts: vec![contract],
            runs: vec![],
            artifacts: vec![],
            workdirs: vec![],
            integrations: vec![],
        };
        world.add_run("r1", 1, RunStatus::Succeeded);
        world
    }

    fn add_run(&mut self, id: &str, generation: i64, status: RunStatus) {
        self.runs.push(Run {
            id: id.into(),
            workspace_id: "w1".into(),
            task_id: "t1".into(),
            profile_id: "p1".into(),
            status,
            version: 3,
            generation,
            contract_version: 1,
            authorization_revision: None,
            command_id: None,
            process: None,
            cancel_requested_at: None,
            end_reason: None,
            created_at: T0,
            updated_at: T0,
            started_at: None,
            ended_at: None,
            native_session_id: None,
        });
    }

    fn add_artifact(&mut self, id: &str, name: &str, versions: &[&str]) {
        self.artifacts.push(Artifact {
            id: id.into(),
            workspace_id: "w1".into(),
            task_id: "t1".into(),
            name: name.into(),
            created_at: T0,
            versions: versions
                .iter()
                .enumerate()
                .map(|(i, sha)| ArtifactVersion {
                    version: i as i64 + 1,
                    description: String::new(),
                    size_bytes: 3,
                    sha256: (*sha).into(),
                    storage_path: format!("w1/blob_{id}_{i}"),
                    produced_by_run_id: "r1".into(),
                    produced_by_generation: 1,
                    created_at: T0,
                })
                .collect(),
        });
    }

    fn task_ctx(&self) -> TaskContext<'_> {
        TaskContext {
            workspace_id: "w1",
            workspace: Some(&self.workspace),
            authorization: None,
            goals: &[],
            tasks: &self.tasks,
            dependencies: &[],
            contracts: &self.contracts,
        }
    }

    fn basis(&self) -> AcceptanceBasis {
        current_basis(&self.tasks[0], &self.runs, &self.artifacts, &self.integrations).expect("驗證中的 Task 應該有接受基礎")
    }

    fn decide(&self, command: AcceptanceCommand) -> Result<AcceptanceChange, AcceptanceError> {
        let task_ctx = self.task_ctx();
        let ctx = AcceptanceContext {
            task: &task_ctx,
            runs: &self.runs,
            artifacts: &self.artifacts,
            workdirs: &self.workdirs,
            integrations: &self.integrations,
        };
        super::decide(&ctx, command, "ver_1".into(), T1)
    }

    fn record(&self, results: Vec<CheckResult>) -> Result<AcceptanceChange, AcceptanceError> {
        self.decide(AcceptanceCommand::RecordChecks { task_id: "t1".into(), basis: self.basis(), results })
    }

    fn accept(&self, basis: AcceptanceBasis) -> Result<AcceptanceChange, AcceptanceError> {
        self.decide(AcceptanceCommand::AcceptManually {
            task_id: "t1".into(),
            basis,
            accepted_by: "desk_fingerprint".into(),
            note: Some("  看過了  ".into()),
        })
    }
}

// ---------------------------------------------------------------- 契約中的檢查

#[test]
fn checks_are_normalized_paths_use_slashes_and_hashes_are_lowercase() {
    let upper = SHA_ABC.to_ascii_uppercase();
    let checks = normalize_checks(vec![
        VerificationCheck::Command { program: "  cargo ".into(), args: vec!["test".into()], expected_exit_code: 0 },
        file_check(" out\\report.md ", Some(&upper)),
        artifact_check("  report.md ", None),
    ])
    .unwrap();

    assert_eq!(
        checks,
        vec![command("cargo", 0), file_check("out/report.md", Some(SHA_ABC)), artifact_check("report.md", None)]
    );
}

#[test]
fn checks_reject_paths_outside_the_working_directory_bad_hashes_and_bad_names() {
    for path in ["", "..\\secret", "a/../b", "C:\\x", "/etc/passwd", "\\\\server\\share", "a//b", "./a", "a:stream"] {
        assert!(normalize_checks(vec![file_check(path, None)]).is_err(), "{path:?} 應該被拒絕");
    }
    assert!(normalize_checks(vec![file_check("a.txt", Some("abc"))]).is_err());
    assert!(normalize_checks(vec![artifact_check("a/b", None)]).is_err());
    assert!(normalize_checks(vec![command("   ", 0)]).is_err());
    assert!(normalize_checks(vec![command("x", 0); MAX_CHECKS + 1]).is_err());
}

#[test]
fn contract_with_invalid_checks_is_rejected_when_saved() {
    let content = ContractContent { verification_checks: vec![file_check("..\\x", None)], ..ContractContent::default() };
    assert!(matches!(content.normalized(), Err(crate::task::TaskError::ContractInvalid(_))));
}

#[test]
fn contract_json_without_checks_still_parses() {
    let content: ContractContent = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(content.verification_checks.is_empty());
    let check: VerificationCheck = serde_json::from_value(serde_json::json!({ "kind": "command", "program": "cargo" })).unwrap();
    assert_eq!(check, VerificationCheck::Command { program: "cargo".into(), args: vec![], expected_exit_code: 0 });
}

// ---------------------------------------------------------------- 判定

#[test]
fn command_check_passes_only_on_the_expected_exit_code() {
    let check = command("cargo", 0);
    let ok = judge(&check, Observation::Command(CommandObservation::Exited { code: 0, output: "全部通過".into() }));
    let bad = judge(&check, Observation::Command(CommandObservation::Exited { code: 101, output: "1 failed".into() }));
    let timeout = judge(&check, Observation::Command(CommandObservation::TimedOut { output: String::new() }));
    let missing = judge(&check, Observation::Command(CommandObservation::NotStarted { reason: "找不到程式".into() }));

    assert_eq!(ok.status, CheckStatus::Passed);
    assert_eq!(bad.status, CheckStatus::Failed);
    assert!(bad.detail.contains("結束碼 101") && bad.detail.contains("1 failed"), "{}", bad.detail);
    assert_eq!((timeout.status, missing.status), (CheckStatus::Failed, CheckStatus::Failed));
    assert!(missing.detail.contains("找不到程式"));
    assert_eq!(judge(&command("x", 3), Observation::Command(CommandObservation::Exited { code: 3, output: String::new() })).status, CheckStatus::Passed);
}

#[test]
fn file_check_needs_the_file_and_the_hash_when_declared() {
    let any = file_check("a.txt", None);
    let pinned = file_check("a.txt", Some(SHA_ABC));

    assert_eq!(judge(&any, Observation::File(Ok(Some(b"xyz".to_vec())))).status, CheckStatus::Passed);
    assert_eq!(judge(&any, Observation::File(Ok(None))).status, CheckStatus::Failed);
    assert_eq!(judge(&any, Observation::File(Err("拒絕存取".into()))).status, CheckStatus::Failed);
    assert_eq!(judge(&pinned, Observation::File(Ok(Some(b"abc".to_vec())))).status, CheckStatus::Passed);
    let wrong = judge(&pinned, Observation::File(Ok(Some(b"abd".to_vec()))));
    assert_eq!(wrong.status, CheckStatus::Failed);
    assert!(wrong.detail.contains("SHA-256 不符"), "{}", wrong.detail);
}

#[test]
fn artifact_check_verifies_presence_manifest_integrity_and_declared_hash() {
    let entry = ManifestEntry { artifact_id: "a1".into(), name: "r.md".into(), version: 2, sha256: SHA_ABC.into() };
    let any = artifact_check("r.md", None);
    let observe = |latest: Option<ManifestEntry>, content: Option<&[u8]>| Observation::Artifact {
        latest,
        content: Ok(content.map(<[u8]>::to_vec)),
    };

    let ok = judge(&any, observe(Some(entry.clone()), Some(b"abc")));
    assert_eq!(ok.status, CheckStatus::Passed);
    assert!(ok.detail.contains("版本 2"));
    assert_eq!(judge(&any, observe(None, None)).status, CheckStatus::Failed);
    assert_eq!(judge(&any, observe(Some(entry.clone()), None)).status, CheckStatus::Failed);
    let tampered = judge(&any, observe(Some(entry.clone()), Some(b"abd")));
    assert!(tampered.status == CheckStatus::Failed && tampered.detail.contains("manifest 不符"), "{}", tampered.detail);
    let pinned = artifact_check("r.md", Some(&"0".repeat(64)));
    assert_eq!(judge(&pinned, observe(Some(entry), Some(b"abc"))).status, CheckStatus::Failed);
}

#[test]
fn long_evidence_keeps_the_end_of_the_output() {
    let output = format!("{}最後一行：失敗原因", "x".repeat(DETAIL_MAX_CHARS * 2));
    let result = judge(&command("cargo", 0), Observation::Command(CommandObservation::Exited { code: 1, output }));
    assert!(result.detail.chars().count() <= DETAIL_MAX_CHARS + 1);
    assert!(result.detail.ends_with("最後一行：失敗原因"));
}

// ---------------------------------------------------------------- 何時需要檢查

#[test]
fn worker_success_alone_leaves_a_task_without_checks_waiting_for_manual_acceptance() {
    let world = World::verifying(vec![]);
    let pending = pending_check(&world.task_ctx(), &world.runs, &world.artifacts, &[], &world.tasks[0]);
    assert_eq!(pending, None, "沒有宣告檢查時 Runtime 不會自動完成它");
    assert!(world.basis().run_id == "r1");
}

#[test]
fn declared_checks_make_the_task_pending_with_the_current_basis() {
    let mut world = World::verifying(vec![artifact_check("r.md", None)]);
    world.add_artifact("a1", "r.md", &[SHA_ABC]);

    let pending = pending_check(&world.task_ctx(), &world.runs, &world.artifacts, &[], &world.tasks[0]).unwrap();

    assert_eq!(pending.checks, vec![artifact_check("r.md", None)]);
    assert_eq!(
        pending.basis,
        AcceptanceBasis {
            run_id: "r1".into(),
            generation: 1,
            contract_version: 1,
            artifacts: vec![ManifestRef { artifact_id: "a1".into(), version: 1 }],
        }
    );
}

#[test]
fn a_task_whose_latest_run_did_not_succeed_has_nothing_to_verify() {
    let mut world = World::verifying(vec![command("cargo", 0)]);
    world.add_run("r2", 2, RunStatus::Running);
    assert_eq!(current_basis(&world.tasks[0], &world.runs, &world.artifacts, &[]), None);
}

// ---------------------------------------------------------------- 接受交易

#[test]
fn all_checks_passing_completes_the_task_and_records_the_evidence() {
    let checks = vec![command("cargo", 0), artifact_check("r.md", None)];
    let mut world = World::verifying(checks.clone());
    world.add_artifact("a1", "r.md", &[SHA_ABC]);

    let change = world.record(checks.into_iter().map(passed).collect()).unwrap();

    assert_eq!(change.task.status, TaskStatus::Completed);
    let record = &change.record;
    assert_eq!((record.method, record.outcome), (VerificationMethod::Checks, VerificationOutcome::Passed));
    assert_eq!((record.run_id.as_str(), record.generation, record.contract_version), ("r1", 1, 1));
    assert_eq!(record.artifacts.len(), 1);
    assert_eq!(record.checks.len(), 2);
    let types: Vec<_> = change.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EVENT_TASK_VERIFICATION_RECORDED, crate::task::EVENT_STATUS_CHANGED]);
    assert_eq!(change.events[1].payload["to"], "completed");
    assert_eq!((change.task.version, change.events[1].aggregate_version), (7, 7));
}

#[test]
fn a_failed_check_sends_the_task_back_to_ready_and_keeps_the_evidence() {
    let checks = vec![command("cargo", 0), command("lint", 0)];
    let world = World::verifying(checks.clone());

    let change = world.record(vec![passed(checks[0].clone()), failed(checks[1].clone())]).unwrap();

    assert_eq!(change.task.status, TaskStatus::Ready);
    assert_eq!(change.record.outcome, VerificationOutcome::Failed);
    assert_eq!(change.record.checks[1].status, CheckStatus::Failed);
    assert!(change.record.note.as_deref().unwrap().contains("1 項未通過"));
    assert_eq!(change.events[0].payload["failed_checks"][0], checks[1].describe());
    assert_eq!(change.events[1].payload["reason"], "verification_failed");
}

#[test]
fn results_must_answer_exactly_the_declared_checks() {
    let checks = vec![command("cargo", 0), command("lint", 0)];
    let world = World::verifying(checks.clone());

    assert_eq!(world.record(vec![]), Err(AcceptanceError::ResultsMismatch));
    assert_eq!(world.record(vec![passed(checks[0].clone())]), Err(AcceptanceError::ResultsMismatch));
    assert_eq!(
        world.record(vec![passed(checks[1].clone()), passed(checks[0].clone())]),
        Err(AcceptanceError::ResultsMismatch)
    );
    assert_eq!(World::verifying(vec![]).record(vec![]), Err(AcceptanceError::ResultsMismatch), "沒有宣告檢查就只能手動接受");
}

#[test]
fn reports_based_on_a_stale_run_generation_cannot_advance_the_task() {
    let mut world = World::verifying(vec![command("cargo", 0)]);
    let stale = world.basis();
    // 世代 1 之後又有世代 2 成功結束：依世代 1 的觀察不能再接受
    world.add_run("r2", 2, RunStatus::Succeeded);

    for result in [
        world.decide(AcceptanceCommand::RecordChecks {
            task_id: "t1".into(),
            basis: stale.clone(),
            results: vec![passed(command("cargo", 0))],
        }),
        world.accept(stale.clone()),
        world.accept(AcceptanceBasis { generation: 2, ..stale.clone() }),
    ] {
        assert!(matches!(result, Err(AcceptanceError::RunStale { ref current, .. }) if current.as_deref() == Some("r2")), "{result:?}");
    }
    assert!(world.accept(world.basis()).is_ok());
}

#[test]
fn only_verifying_tasks_can_be_accepted() {
    let mut world = World::verifying(vec![]);
    let basis = world.basis();
    for status in [TaskStatus::Running, TaskStatus::Ready, TaskStatus::Completed, TaskStatus::Cancelled] {
        world.tasks[0].status = status;
        assert_eq!(world.accept(basis.clone()), Err(AcceptanceError::TaskNotVerifying(status)));
    }
}

#[test]
fn acceptance_rejects_a_changed_contract_version_or_manifest() {
    let mut world = World::verifying(vec![]);
    world.add_artifact("a1", "r.md", &[SHA_ABC]);
    let basis = world.basis();

    assert!(matches!(
        world.accept(AcceptanceBasis { contract_version: 2, ..basis.clone() }),
        Err(AcceptanceError::ContractStale { .. })
    ));
    assert_eq!(world.accept(AcceptanceBasis { artifacts: vec![], ..basis.clone() }), Err(AcceptanceError::ManifestStale));

    // 檢視之後成果多了一個版本（manifest 改變）
    let first = world.artifacts[0].versions[0].clone();
    world.artifacts[0].versions.push(ArtifactVersion { version: 2, ..first });
    assert_eq!(world.accept(basis), Err(AcceptanceError::ManifestStale));
}

#[test]
fn work_done_under_an_older_contract_fails_verification_instead_of_being_accepted() {
    let mut world = World::verifying(vec![command("cargo", 0)]);
    world.tasks[0].contract_version = 2;
    world.contracts[0].version = 2;

    let manual = world.accept(world.basis());
    assert!(matches!(manual, Err(AcceptanceError::ContractStale { run: 1, current: 2, .. })), "{manual:?}");

    let pending = pending_check(&world.task_ctx(), &world.runs, &world.artifacts, &[], &world.tasks[0]).unwrap();
    assert!(pending.checks.is_empty(), "契約不符時不執行檢查");
    let change = world.record(vec![]).unwrap();
    assert_eq!((change.task.status, change.record.outcome), (TaskStatus::Ready, VerificationOutcome::Failed));
    assert!(change.record.note.as_deref().unwrap().contains("版本 1 變更為 2"));
}

#[test]
fn manual_acceptance_records_who_accepted_and_completes_the_task() {
    let world = World::verifying(vec![command("cargo", 0)]);

    let change = world.accept(world.basis()).unwrap();

    assert_eq!(change.task.status, TaskStatus::Completed);
    assert_eq!((change.record.method, change.record.outcome), (VerificationMethod::Manual, VerificationOutcome::Passed));
    assert_eq!(change.record.accepted_by.as_deref(), Some("desk_fingerprint"));
    assert_eq!(change.record.note.as_deref(), Some("看過了"));
    assert_eq!(change.events[0].payload["accepted_by"], "desk_fingerprint");
    assert_eq!(change.events[0].payload["method"], "manual");
}

#[test]
fn a_verifying_task_in_an_archived_workspace_is_not_pending_for_checks() {
    let mut world = World::verifying(vec![command("cargo", 0)]);
    world.workspace.status = WorkspaceStatus::Archived;

    let pending = pending_check(&world.task_ctx(), &world.runs, &world.artifacts, &[], &world.tasks[0]);
    assert_eq!(pending, None, "封存後提交一定被拒絕，執行檢查只會白跑命令");
    assert!(!view(&world.task_ctx(), &world.runs, &world.artifacts, &[], &[], &world.tasks[0]).automatic);
}

#[test]
fn manual_acceptance_note_is_limited_and_archived_workspaces_refuse() {
    let mut world = World::verifying(vec![]);
    let long = AcceptanceCommand::AcceptManually {
        task_id: "t1".into(),
        basis: world.basis(),
        accepted_by: "d".into(),
        note: Some("字".repeat(NOTE_MAX_CHARS + 1)),
    };
    assert_eq!(world.decide(long), Err(AcceptanceError::NoteTooLong));

    world.workspace.status = WorkspaceStatus::Archived;
    assert!(matches!(world.accept(world.basis()), Err(AcceptanceError::WorkspaceArchived(_))));
}

#[test]
fn view_shows_basis_checks_manifest_and_whether_runtime_will_check_automatically() {
    let mut world = World::verifying(vec![]);
    world.add_artifact("a1", "r.md", &[SHA_ABC, SHA_ABC]);
    let records = vec![];

    let manual = view(&world.task_ctx(), &world.runs, &world.artifacts, &records, &[], &world.tasks[0]);
    assert!(!manual.automatic);
    assert_eq!(manual.artifacts[0].version, 2);
    assert_eq!(manual.basis.as_ref().unwrap().artifacts, vec![ManifestRef { artifact_id: "a1".into(), version: 2 }]);

    world.contracts[0].content.verification_checks = vec![command("cargo", 0)];
    let automatic = view(&world.task_ctx(), &world.runs, &world.artifacts, &records, &[], &world.tasks[0]);
    assert!(automatic.automatic);
    assert_eq!(automatic.checks.len(), 1);
}

// ---------------------------------------------------------------- 開發 Task 與整合佇列（票 20）

impl World {
    /// 把 Task 改成開發 Task，並登記它的 worktree（平台分支 baton/t1，整合目標 main）。
    fn development(mut self) -> Self {
        self.tasks[0].kind = TaskKind::Development;
        let plan = crate::workdir::plan(&self.workspace, &self.tasks[0]);
        let (workdir, _) = crate::workdir::record_prepared(None, &plan, Some("main".into()), T0);
        self.workdirs.push(workdir);
        self
    }
}

#[test]
fn a_passing_development_task_is_queued_for_integration_instead_of_completing() {
    let world = World::verifying(vec![command("cargo", 0)]).development();

    let change = world.record(vec![passed(command("cargo", 0))]).unwrap();

    assert_eq!(change.record.outcome, VerificationOutcome::Passed);
    assert_eq!(change.task.status, TaskStatus::Verifying, "通過驗證但還沒整合：不能宣告完成");
    let entry = change.integration.expect("應該排入整合佇列");
    assert_eq!((entry.task_id.as_str(), entry.run_id.as_str(), entry.generation), ("t1", "r1", 1));
    assert_eq!((entry.branch.as_str(), entry.target_branch.as_str()), ("baton/t1", "main"));
    assert_eq!((entry.status, entry.id.as_str()), (crate::integration::IntegrationStatus::Queued, "int_ver_1"));
    let types: Vec<_> = change.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EVENT_TASK_VERIFICATION_RECORDED, crate::integration::EVENT_QUEUED]);
}

#[test]
fn manual_acceptance_of_a_development_task_also_goes_through_integration() {
    let world = World::verifying(vec![]).development();
    let change = world.accept(world.basis()).unwrap();
    assert_eq!(change.task.status, TaskStatus::Verifying);
    assert!(change.integration.is_some());
}

#[test]
fn a_failing_development_task_goes_back_to_ready_without_integration() {
    let world = World::verifying(vec![command("cargo", 0)]).development();
    let change = world.record(vec![failed(command("cargo", 0))]).unwrap();
    assert_eq!((change.task.status, change.integration), (TaskStatus::Ready, None));
}

#[test]
fn a_run_already_queued_for_integration_is_neither_rechecked_nor_accepted_again() {
    let mut world = World::verifying(vec![command("cargo", 0)]).development();
    let basis = world.basis();
    let entry = world.record(vec![passed(command("cargo", 0))]).unwrap().integration.unwrap();
    world.integrations.push(entry);

    assert_eq!(pending_check(&world.task_ctx(), &world.runs, &world.artifacts, &world.integrations, &world.tasks[0]), None);
    assert_eq!(current_basis(&world.tasks[0], &world.runs, &world.artifacts, &world.integrations), None);
    assert_eq!(world.accept(basis), Err(AcceptanceError::AlreadyQueuedForIntegration));
    let shown = view(&world.task_ctx(), &world.runs, &world.artifacts, &[], &world.integrations, &world.tasks[0]);
    assert_eq!(shown.integration.map(|e| e.id), Some("int_ver_1".to_string()));
}

#[test]
fn a_development_task_without_a_registered_worktree_cannot_be_accepted() {
    let mut world = World::verifying(vec![]);
    world.tasks[0].kind = TaskKind::Development;
    assert_eq!(world.accept(world.basis()), Err(AcceptanceError::WorktreeMissing));
}
