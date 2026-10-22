//! 以真實 `git` CLI 實作 Git Workspace 的檔案系統操作（票 20）。
//!
//! 全部是同步（阻塞）函式，正式版 `WorkspaceFs` 以 `spawn_blocking` 包裝；語意見 [`crate::WorkspaceFs`] 的文件註解。
//! 一律以 `git -C <dir>` 執行、不經過 shell。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{AdvanceOutcome, GitState, MergeOutcome, PortError, PortResult};

/// 錯誤訊息中 git 輸出摘要的長度上限（字元）。
const SUMMARY_LIMIT: usize = 800;

/// `path` 是不是 Git repo 的頂層工作目錄，以及目前簽出的分支。找不到 git 執行檔是錯誤，不是 `NotRepository`。
pub fn git_inspect(path: &Path) -> PortResult<GitState> {
    if !path.is_dir() {
        return Ok(GitState::NotRepository);
    }
    let top = output(git(path).args(["rev-parse", "--show-toplevel"]))?;
    if !top.success || !same_path(Path::new(top.stdout.trim()), path) {
        return Ok(GitState::NotRepository);
    }
    let head = output(git(path).args(["symbolic-ref", "--quiet", "--short", "HEAD"]))?;
    Ok(GitState::Repository { branch: head.success.then(|| head.stdout.trim().to_string()) })
}

/// 在 `target` 建立簽出 `branch` 的 worktree；`branch` 不存在時從 `repo` 的 HEAD 建立。冪等。
pub fn git_create_worktree(repo: &Path, branch: &str, target: &Path) -> PortResult<()> {
    prune_worktrees(repo)?;
    let branch_ref = format!("refs/heads/{branch}");
    if let Some(existing) = worktrees(repo)?.into_iter().find(|w| same_path(&w.path, target)) {
        return match existing.branch.as_deref() {
            Some(checked_out) if checked_out == branch_ref => Ok(()),
            other => Err(PortError::Failed(format!(
                "{} 已經是簽出 {} 的 worktree，不是 {branch}",
                target.display(),
                other.and_then(|r| r.strip_prefix("refs/heads/")).unwrap_or("detached HEAD")
            ))),
        };
    }
    create_parent(target)?;
    if resolve(repo, &branch_ref)?.is_some() {
        checked(git(repo).args(["worktree", "add", "--quiet"]).arg(target).arg(branch))?;
    } else {
        checked(git(repo).args(["worktree", "add", "--quiet", "-b", branch]).arg(target).arg("HEAD"))?;
    }
    Ok(())
}

/// 在整合目錄以 detached HEAD 簽出 `target_branch` 目前的 commit 後以 `--no-ff` 合併 `branch`；目標分支不變。
pub fn git_merge_for_integration(
    repo: &Path,
    target_branch: &str,
    branch: &str,
    integration_dir: &Path,
) -> PortResult<MergeOutcome> {
    prune_worktrees(repo)?;
    let target_tip = branch_tip(repo, target_branch)?;
    branch_tip(repo, branch)?;
    prepare_integration_dir(repo, &target_tip, integration_dir)?;

    let message = format!("Baton：整合 {branch} 到 {target_branch}");
    let mut merge = git(integration_dir);
    identity_fallback(&mut merge, integration_dir)?;
    merge.args(["merge", "--no-ff", "--no-edit", "--quiet", "-m", &message, &format!("refs/heads/{branch}")]);
    let result = output(&mut merge)?;
    if result.success {
        let commit = checked(git(integration_dir).args(["rev-parse", "HEAD"]))?;
        return Ok(MergeOutcome::Merged { commit, target_tip });
    }

    let conflicted = checked(git(integration_dir).args(["diff", "--name-only", "--diff-filter=U"]))?;
    // 不論是衝突還是其他失敗（例如 hook 拒絕），都放棄合併，整合目錄不殘留合併狀態
    reset_integration_dir(integration_dir, &target_tip)?;
    if conflicted.is_empty() {
        return Err(failure(&merge, &result));
    }
    let files: Vec<&str> = conflicted.lines().collect();
    Ok(MergeOutcome::Conflict {
        detail: format!("合併 {branch} 到 {target_branch} 時發生衝突，衝突的檔案：{}", files.join("、")),
    })
}

/// 把 `target_branch` 從 `expected_tip` fast-forward 到 `commit`；簽出該分支的工作目錄一併更新。
pub fn git_advance_branch(
    repo: &Path,
    target_branch: &str,
    expected_tip: &str,
    commit: &str,
) -> PortResult<AdvanceOutcome> {
    let target_ref = format!("refs/heads/{target_branch}");
    let actual = branch_tip(repo, target_branch)?;
    let commit =
        resolve(repo, commit)?.ok_or_else(|| PortError::Failed(format!("找不到要前移到的 commit {commit}")))?;
    if actual == commit {
        return Ok(AdvanceOutcome::Advanced);
    }
    let expected = resolve(repo, expected_tip)?
        .ok_or_else(|| PortError::Failed(format!("找不到合併當時的 commit {expected_tip}")))?;
    if actual != expected {
        return Ok(AdvanceOutcome::TargetMoved { actual });
    }
    if !output(git(repo).args(["merge-base", "--is-ancestor", &expected, &commit]))?.success {
        return Err(PortError::Failed(format!("{commit} 不是 {target_branch}（{expected}）的後代，無法 fast-forward")));
    }

    prune_worktrees(repo)?;
    let checked_out = worktrees(repo)?.into_iter().find(|w| w.branch.as_deref() == Some(target_ref.as_str()));
    let (mut command, dir) = match &checked_out {
        Some(worktree) => {
            let mut command = git(&worktree.path);
            command.args(["merge", "--ff-only", "--quiet", &commit]);
            (command, Some(&worktree.path))
        }
        None => {
            let mut command = git(repo);
            command.args(["update-ref", &target_ref, &commit, &expected]);
            (command, None)
        }
    };
    let result = output(&mut command)?;
    if result.success {
        return Ok(AdvanceOutcome::Advanced);
    }
    // 失敗時先確認是不是目標分支在這期間被移動了
    let now = branch_tip(repo, target_branch)?;
    if now != expected {
        return Ok(AdvanceOutcome::TargetMoved { actual: now });
    }
    match dir {
        Some(dir) => Ok(AdvanceOutcome::Blocked {
            detail: format!(
                "無法在 {} 把 {target_branch} 前移到 {commit}：{}",
                dir.display(),
                summary(&result.stderr, &result.stdout)
            ),
        }),
        None => Err(failure(&command, &result)),
    }
}

/// 準備整合目錄：不存在就建立 detached worktree；已是本 repo 的 detached worktree 就整個重設。
/// 存在但不是本 repo 的 worktree 時回錯誤而不刪除（避免路徑設錯時誤刪使用者的檔案）。
fn prepare_integration_dir(repo: &Path, target_tip: &str, dir: &Path) -> PortResult<()> {
    let list = worktrees(repo)?;
    if let Some(index) = list.iter().position(|w| same_path(&w.path, dir)) {
        // 防呆：整合目錄絕不能是主 checkout 或簽出分支的 worktree，否則重設會毀掉使用者或 Task 的工作
        if index == 0 || list[index].branch.is_some() {
            return Err(PortError::Failed(format!(
                "整合目錄 {} 是主 checkout 或簽出分支的 worktree，拒絕重設",
                dir.display()
            )));
        }
        return reset_integration_dir(dir, target_tip);
    }
    let non_empty = std::fs::read_dir(dir).map(|mut entries| entries.next().is_some()).unwrap_or(false);
    if non_empty {
        return Err(PortError::Failed(format!(
            "整合目錄 {} 已存在但不是 {} 的 worktree；為避免誤刪，請手動移除後重試",
            dir.display(),
            repo.display()
        )));
    }
    create_parent(dir)?;
    checked(git(repo).args(["worktree", "add", "--quiet", "--detach"]).arg(dir).arg(target_tip))?;
    Ok(())
}

/// 放棄任何進行中的合併，以 detached HEAD 簽出 `target_tip` 並清掉所有未追蹤／忽略的檔案。
fn reset_integration_dir(dir: &Path, target_tip: &str) -> PortResult<()> {
    // `reset --hard` 會一併清掉 MERGE_HEAD 等合併狀態
    checked(git(dir).args(["reset", "--hard", "--quiet"]))?;
    checked(git(dir).args(["checkout", "--quiet", "--detach", "--force", target_tip]))?;
    checked(git(dir).args(["clean", "-ffdx", "--quiet"]))?;
    Ok(())
}

/// 找不到使用者身分設定時才補上 Baton 的作者資訊，讓合併 commit 仍能建立。
fn identity_fallback(command: &mut Command, dir: &Path) -> PortResult<()> {
    for (key, value) in [("user.name", "Baton"), ("user.email", "baton@localhost")] {
        if !output(git(dir).args(["config", "--get", key]))?.success {
            command.arg("-c").arg(format!("{key}={value}"));
        }
    }
    Ok(())
}

fn prune_worktrees(repo: &Path) -> PortResult<()> {
    // 清掉目錄已不存在的 worktree 登記，否則 `worktree add` 會拒絕重建同一路徑
    checked(git(repo).args(["worktree", "prune"]))?;
    Ok(())
}

struct Worktree {
    path: PathBuf,
    /// 簽出的分支完整 ref（`refs/heads/...`）；detached HEAD 時是 `None`。
    branch: Option<String>,
}

/// `git worktree list --porcelain`；第一筆是主 checkout。
fn worktrees(repo: &Path) -> PortResult<Vec<Worktree>> {
    let listed = checked(git(repo).args(["worktree", "list", "--porcelain"]))?;
    let mut found: Vec<Worktree> = Vec::new();
    for line in listed.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found.push(Worktree { path: PathBuf::from(path), branch: None });
        } else if let (Some(branch), Some(last)) = (line.strip_prefix("branch "), found.last_mut()) {
            last.branch = Some(branch.to_string());
        }
    }
    Ok(found)
}

/// 分支目前的完整 sha；分支不存在是錯誤。
fn branch_tip(repo: &Path, branch: &str) -> PortResult<String> {
    resolve(repo, &format!("refs/heads/{branch}"))?
        .ok_or_else(|| PortError::Failed(format!("分支 {branch} 不存在於 {}", repo.display())))
}

/// 把 revision 解析成完整的 commit sha；不存在回 `None`。
fn resolve(repo: &Path, rev: &str) -> PortResult<Option<String>> {
    let result = output(git(repo).args(["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]))?;
    Ok(result.success.then(|| result.stdout.trim().to_string()))
}

fn create_parent(path: &Path) -> PortResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PortError::Failed(format!("無法建立目錄 {}：{e}", parent.display())))?;
    }
    Ok(())
}

/// 兩個既存路徑是否指向同一個目錄；git 回傳 `C:/x/y` 形式，Windows 上另忽略大小寫。
fn same_path(a: &Path, b: &Path) -> bool {
    fn key(path: &Path) -> Option<String> {
        let canonical = std::fs::canonicalize(path).ok()?.to_string_lossy().into_owned();
        Some(if cfg!(windows) { canonical.to_lowercase() } else { canonical })
    }
    matches!((key(a), key(b)), (Some(a), Some(b)) if a == b)
}

/// 在 `dir` 執行 git 的指令（尚未加子命令）。
fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        // 外部環境若帶著這些變數，`-C` 指定的目錄會被忽略
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不要跳出主控台視窗
        command.creation_flags(0x0800_0000);
    }
    command
}

struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn output(command: &mut Command) -> PortResult<GitOutput> {
    let result = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PortError::Failed(format!("找不到 git 執行檔，請安裝 Git for Windows 並確認已加入 PATH：{e}"))
        } else {
            PortError::Failed(format!("無法執行 git：{e}"))
        }
    })?;
    Ok(GitOutput {
        success: result.status.success(),
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
    })
}

/// 執行並要求成功，回傳去掉頭尾空白的 stdout。
fn checked(command: &mut Command) -> PortResult<String> {
    let result = output(command)?;
    if result.success {
        Ok(result.stdout.trim().to_string())
    } else {
        Err(failure(command, &result))
    }
}

fn failure(command: &Command, result: &GitOutput) -> PortError {
    let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    PortError::Failed(format!("git {} 失敗：{}", args.join(" "), summary(&result.stderr, &result.stdout)))
}

/// git 輸出的摘要：優先用 stderr，空的話用 stdout，過長時截斷。
fn summary(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
    if text.chars().count() > SUMMARY_LIMIT {
        format!("{}…", text.chars().take(SUMMARY_LIMIT).collect::<String>())
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    /// 在 `dir` 執行 git 並要求成功；找不到 git 時明確失敗（Git for Windows 是產品前提，不靜默跳過）。
    fn sh(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("找不到 git，無法執行測試（Git for Windows 是產品前提）：{e}"));
        assert!(
            output.status.success(),
            "git {args:?} 在 {} 失敗：{}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit_file(dir: &Path, name: &str, content: &str, message: &str) -> String {
        std::fs::write(dir.join(name), content).unwrap();
        sh(dir, &["add", name]);
        sh(dir, &["commit", "-q", "-m", message]);
        sh(dir, &["rev-parse", "HEAD"])
    }

    /// 建立含一個初始 commit 的 repo（main 分支），並放好 `.baton/.gitignore`（內容 `*`）。
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        sh(&repo, &["config", "user.name", "Tester"]);
        sh(&repo, &["config", "user.email", "tester@example.com"]);
        sh(&repo, &["config", "commit.gpgsign", "false"]);
        sh(&repo, &["config", "core.autocrlf", "false"]);
        commit_file(&repo, "README.md", "hello\n", "init");
        std::fs::create_dir_all(repo.join(".baton")).unwrap();
        std::fs::write(repo.join(".baton").join(".gitignore"), "*").unwrap();
        (temp, repo)
    }

    fn worktree_dir(repo: &Path, id: &str) -> PathBuf {
        repo.join(".baton").join("worktrees").join(id)
    }

    fn integration_dir(repo: &Path) -> PathBuf {
        repo.join(".baton").join("integration")
    }

    fn tip(repo: &Path, branch: &str) -> String {
        sh(repo, &["rev-parse", &format!("refs/heads/{branch}")])
    }

    fn merged(outcome: MergeOutcome) -> (String, String) {
        match outcome {
            MergeOutcome::Merged { commit, target_tip } => (commit, target_tip),
            other => panic!("預期合併成功，實際是 {other:?}"),
        }
    }

    /// 建立 Task 的 worktree 並在其中 commit 一個檔案。
    fn task_with_commit(repo: &Path, id: &str, file: &str, content: &str) -> (String, PathBuf) {
        let branch = format!("baton/{id}");
        let dir = worktree_dir(repo, id);
        git_create_worktree(repo, &branch, &dir).unwrap();
        commit_file(&dir, file, content, &format!("{id} 的變更"));
        (branch, dir)
    }

    #[test]
    fn inspect_distinguishes_non_repo_top_level_subdirectory_and_detached_head() {
        let (temp, repo) = init_repo();
        assert_eq!(git_inspect(&temp.path().join("missing")).unwrap(), GitState::NotRepository);
        let plain = temp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(git_inspect(&plain).unwrap(), GitState::NotRepository);

        assert_eq!(git_inspect(&repo).unwrap(), GitState::Repository { branch: Some("main".into()) });
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(git_inspect(&sub).unwrap(), GitState::NotRepository);

        sh(&repo, &["checkout", "-q", "--detach", "HEAD"]);
        assert_eq!(git_inspect(&repo).unwrap(), GitState::Repository { branch: None });
    }

    #[test]
    fn inspect_accepts_a_path_spelled_with_different_separators_and_case() {
        let (_temp, repo) = init_repo();
        let spelled = repo.to_string_lossy().replace('\\', "/");
        #[cfg(windows)]
        let spelled = spelled.to_uppercase();
        assert_eq!(git_inspect(Path::new(&spelled)).unwrap(), GitState::Repository { branch: Some("main".into()) });
    }

    #[test]
    fn worktrees_are_separate_idempotent_and_keep_the_main_checkout_clean() {
        let (_temp, repo) = init_repo();
        let main_before = tip(&repo, "main");
        let (branch_a, dir_a) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let (branch_b, dir_b) = task_with_commit(&repo, "task-b", "b.txt", "b\n");

        assert_ne!(dir_a, dir_b);
        assert_eq!(git_inspect(&dir_a).unwrap(), GitState::Repository { branch: Some(branch_a.clone()) });
        assert_eq!(git_inspect(&dir_b).unwrap(), GitState::Repository { branch: Some(branch_b.clone()) });
        // 各自的 commit 互不影響，main 也沒動
        assert!(dir_a.join("a.txt").exists() && !dir_a.join("b.txt").exists());
        assert!(dir_b.join("b.txt").exists() && !dir_b.join("a.txt").exists());
        assert_eq!(tip(&repo, "main"), main_before);
        assert_ne!(tip(&repo, &branch_a), tip(&repo, &branch_b));

        // 重做（例如建立後、登記前崩潰）直接成功，分支內容不變
        let a_tip = tip(&repo, &branch_a);
        git_create_worktree(&repo, &branch_a, &dir_a).unwrap();
        assert_eq!(tip(&repo, &branch_a), a_tip);

        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");
    }

    #[test]
    fn create_worktree_reuses_an_existing_branch_without_worktree() {
        let (_temp, repo) = init_repo();
        sh(&repo, &["branch", "baton/old"]);
        let dir = worktree_dir(&repo, "old");
        git_create_worktree(&repo, "baton/old", &dir).unwrap();
        assert_eq!(git_inspect(&dir).unwrap(), GitState::Repository { branch: Some("baton/old".into()) });
    }

    #[test]
    fn create_worktree_rejects_a_target_holding_another_branch() {
        let (_temp, repo) = init_repo();
        let dir = worktree_dir(&repo, "x");
        git_create_worktree(&repo, "baton/x", &dir).unwrap();
        assert!(git_create_worktree(&repo, "baton/y", &dir).is_err());
    }

    #[test]
    fn merge_for_integration_merges_in_its_own_directory_without_touching_main() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let main_before = tip(&repo, "main");
        let integration = integration_dir(&repo);

        let (commit, target_tip) = merged(git_merge_for_integration(&repo, "main", &branch, &integration).unwrap());
        assert_eq!(target_tip, main_before);
        assert_eq!(commit.len(), 40);
        assert_eq!(sh(&repo, &["show", &format!("{commit}:a.txt")]), "a");
        assert_eq!(sh(&repo, &["rev-parse", &format!("{commit}^1")]), main_before);
        assert_eq!(sh(&repo, &["rev-parse", &format!("{commit}^2")]), tip(&repo, &branch));
        assert_eq!(sh(&repo, &["log", "-1", "--format=%s", &commit]), format!("Baton：整合 {branch} 到 main"));
        assert_eq!(tip(&repo, "main"), main_before);
        assert!(!repo.join("a.txt").exists());

        // 整合目錄已存在（殘留上一次的結果與雜物）時重設後再合併
        std::fs::write(integration.join("junk.txt"), "junk").unwrap();
        let (again, target_tip) = merged(git_merge_for_integration(&repo, "main", &branch, &integration).unwrap());
        assert_eq!(target_tip, main_before);
        assert_eq!(sh(&repo, &["show", &format!("{again}:a.txt")]), "a");
        assert!(!integration.join("junk.txt").exists());
        assert_eq!(tip(&repo, "main"), main_before);
        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");
    }

    #[test]
    fn merge_conflict_reports_files_and_leaves_no_merge_state() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "README.md", "task\n");
        commit_file(&repo, "README.md", "main\n", "main 的變更");
        let main_before = tip(&repo, "main");
        let integration = integration_dir(&repo);

        match git_merge_for_integration(&repo, "main", &branch, &integration).unwrap() {
            MergeOutcome::Conflict { detail } => assert!(detail.contains("README.md"), "{detail}"),
            other => panic!("預期衝突，實際是 {other:?}"),
        }
        assert_eq!(tip(&repo, "main"), main_before);
        assert_eq!(sh(&integration, &["status", "--porcelain"]), "");
        let merge_head = Command::new("git")
            .arg("-C")
            .arg(&integration)
            .args(["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
            .output()
            .unwrap();
        assert!(!merge_head.status.success(), "整合目錄不應殘留合併狀態");

        // 衝突後整合目錄仍可再用
        let (other, _) = task_with_commit(&repo, "task-b", "b.txt", "b\n");
        merged(git_merge_for_integration(&repo, "main", &other, &integration).unwrap());
    }

    #[test]
    fn merge_for_integration_rejects_an_unregistered_existing_directory() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let integration = integration_dir(&repo);
        std::fs::create_dir_all(&integration).unwrap();
        std::fs::write(integration.join("keep.txt"), "keep").unwrap();

        assert!(git_merge_for_integration(&repo, "main", &branch, &integration).is_err());
        assert!(integration.join("keep.txt").exists());
    }

    #[test]
    fn advance_fast_forwards_the_checked_out_main_and_is_idempotent() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let (commit, target_tip) =
            merged(git_merge_for_integration(&repo, "main", &branch, &integration_dir(&repo)).unwrap());

        assert_eq!(git_advance_branch(&repo, "main", &target_tip, &commit).unwrap(), AdvanceOutcome::Advanced);
        assert_eq!(tip(&repo, "main"), commit);
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a\n");
        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");

        assert_eq!(git_advance_branch(&repo, "main", &target_tip, &commit).unwrap(), AdvanceOutcome::Advanced);
        assert_eq!(tip(&repo, "main"), commit);
    }

    #[test]
    fn advance_reports_target_moved_without_changing_anything() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let (commit, target_tip) =
            merged(git_merge_for_integration(&repo, "main", &branch, &integration_dir(&repo)).unwrap());
        let moved = commit_file(&repo, "other.txt", "x\n", "整合期間的其他更新");

        assert_eq!(
            git_advance_branch(&repo, "main", &target_tip, &commit).unwrap(),
            AdvanceOutcome::TargetMoved { actual: moved.clone() }
        );
        assert_eq!(tip(&repo, "main"), moved);
    }

    #[test]
    fn advance_is_blocked_by_uncommitted_changes_in_the_checkout() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "README.md", "task\n");
        let (commit, target_tip) =
            merged(git_merge_for_integration(&repo, "main", &branch, &integration_dir(&repo)).unwrap());
        std::fs::write(repo.join("README.md"), "local edit\n").unwrap();

        match git_advance_branch(&repo, "main", &target_tip, &commit).unwrap() {
            AdvanceOutcome::Blocked { detail } => assert!(!detail.is_empty()),
            other => panic!("預期被擋下，實際是 {other:?}"),
        }
        assert_eq!(tip(&repo, "main"), target_tip);
        assert_eq!(std::fs::read_to_string(repo.join("README.md")).unwrap(), "local edit\n");
    }

    #[test]
    fn advance_updates_the_ref_when_no_worktree_has_the_branch_checked_out() {
        let (_temp, repo) = init_repo();
        let (branch, _dir) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let (commit, target_tip) =
            merged(git_merge_for_integration(&repo, "main", &branch, &integration_dir(&repo)).unwrap());
        sh(&repo, &["checkout", "-q", "--detach", "HEAD"]);

        assert_eq!(git_advance_branch(&repo, "main", &target_tip, &commit).unwrap(), AdvanceOutcome::Advanced);
        assert_eq!(tip(&repo, "main"), commit);
    }

    #[test]
    fn two_branches_integrate_one_after_another() {
        let (_temp, repo) = init_repo();
        let (branch_a, _) = task_with_commit(&repo, "task-a", "a.txt", "a\n");
        let (branch_b, _) = task_with_commit(&repo, "task-b", "b.txt", "b\n");
        let integration = integration_dir(&repo);

        let (commit_a, tip_a) = merged(git_merge_for_integration(&repo, "main", &branch_a, &integration).unwrap());
        assert_eq!(git_advance_branch(&repo, "main", &tip_a, &commit_a).unwrap(), AdvanceOutcome::Advanced);

        let (commit_b, tip_b) = merged(git_merge_for_integration(&repo, "main", &branch_b, &integration).unwrap());
        assert_eq!(tip_b, commit_a);
        assert_eq!(git_advance_branch(&repo, "main", &tip_b, &commit_b).unwrap(), AdvanceOutcome::Advanced);

        assert_eq!(tip(&repo, "main"), commit_b);
        assert!(repo.join("a.txt").exists() && repo.join("b.txt").exists());
        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");
    }
}
