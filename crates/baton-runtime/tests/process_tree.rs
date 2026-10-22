//! WindowsProcessControl 的整合測試：真實啟動程序樹，驗證 Job Object 能終止整棵樹。
#![cfg(windows)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use baton_ports::{command_hash, Liveness, ProcessControl, SpawnSpec, StopResult};
use baton_runtime::windows_process::WindowsProcessControl;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    JobObjectBasicProcessIdList, OpenJobObjectW, QueryInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_PROCESS_ID_LIST,
};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION};

const JOB_OBJECT_TERMINATE: u32 = 0x0008;
const JOB_OBJECT_QUERY: u32 = 0x0004;

const SHORT_GRACE: Duration = Duration::from_millis(100);

fn unique_id(tag: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("test-{tag}-{}-{nanos}", std::process::id())
}

fn cmd_spec(command_id: &str, args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: "cmd.exe".to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        working_dir: None,
        command_id: command_id.to_string(),
        env: Vec::new(),
    }
}

/// 長時間存活的三層程序樹：cmd → cmd → ping（ping 約 60 秒才結束）。
fn tree_spec(command_id: &str) -> SpawnSpec {
    cmd_spec(command_id, &["/c", "cmd.exe", "/c", "ping", "-n", "60", "127.0.0.1"])
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 以名稱開啟測試用 Job（與 ProcessControl 內部使用相同名稱）。
fn open_job(command_id: &str) -> Option<HANDLE> {
    let name = wide(&format!("Local\\Baton-Run-{command_id}"));
    // SAFETY：name 以 NUL 結尾。
    let handle = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr()) };
    (!handle.is_null()).then_some(handle)
}

/// 列出 Job 內存活的 PID；Job 不存在回傳空集合。
fn job_pids(command_id: &str) -> Vec<u32> {
    let Some(job) = open_job(command_id) else { return Vec::new() };
    let mut buffer = vec![0u64; 1 + 256];
    // SAFETY：job 有效，buffer 對齊且大小與傳入的位元組數一致。
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            buffer.as_mut_ptr() as *mut c_void,
            (buffer.len() * 8) as u32,
            std::ptr::null_mut(),
        )
    };
    let mut pids = Vec::new();
    if ok != 0 {
        let list = buffer.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST;
        // SAFETY：查詢成功，清單項目都在 buffer 內。
        unsafe {
            let first = std::ptr::addr_of!((*list).ProcessIdList) as *const usize;
            for i in 0..(*list).NumberOfProcessIdsInList as usize {
                pids.push(*first.add(i) as u32);
            }
        }
    }
    // SAFETY：job 是這裡開啟、尚未關閉的 handle。
    unsafe {
        CloseHandle(job);
    }
    pids
}

/// 測試結束（含失敗）時終止 Job，確保不留下孤兒程序。
struct JobGuard(String);

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Some(job) = open_job(&self.0) {
            // SAFETY：job 是這裡開啟、尚未關閉的 handle。
            unsafe {
                TerminateJobObject(job, 1);
                CloseHandle(job);
            }
        }
    }
}

/// 持有程序 handle：即使程序結束，PID 也不會被重用，可可靠等待結束。
struct Watched {
    pid: u32,
    handle: HANDLE,
}

impl Watched {
    fn exited_within(&self, timeout: Duration) -> bool {
        // SAFETY：handle 有效（SYNCHRONIZE）。
        unsafe { WaitForSingleObject(self.handle, timeout.as_millis() as u32) == WAIT_OBJECT_0 }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        // SAFETY：handle 由 OpenProcess 取得且只關閉一次。
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

fn watch_job_processes(command_id: &str) -> Vec<Watched> {
    job_pids(command_id)
        .into_iter()
        .filter_map(|pid| {
            // SAFETY：OpenProcess 失敗回傳 NULL（程序剛好結束），略過即可。
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
            (!handle.is_null()).then_some(Watched { pid, handle })
        })
        .collect()
}

/// 輪詢直到 Job 內存活程序數滿足條件。
async fn wait_for_job(command_id: &str, what: &str, condition: impl Fn(usize) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(job_pids(command_id).len()) {
        assert!(Instant::now() < deadline, "等不到條件：{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn assert_all_exited(processes: &[Watched]) {
    for p in processes {
        assert!(p.exited_within(Duration::from_secs(5)), "pid {} 在 stop 之後仍存活", p.pid);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_terminates_the_whole_process_tree() {
    let id = unique_id("tree");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();
    let spec = tree_spec(&id);

    let receipt = control.spawn(spec.clone()).await.unwrap();
    assert_eq!(receipt.identity.command_hash, command_hash(&spec));
    assert_eq!(control.inspect(&receipt.identity).await.unwrap(), Liveness::Alive);

    // 等到孫程序（ping）也已啟動，再記下整棵樹
    wait_for_job(&id, "程序樹至少 3 個程序", |n| n >= 3).await;
    let tree = watch_job_processes(&id);
    assert!(tree.len() >= 3, "程序樹只有 {} 個程序", tree.len());
    assert!(tree.iter().any(|p| p.pid == receipt.identity.pid), "根程序不在 Job 內");

    let result = control.stop(&receipt.identity, SHORT_GRACE).await.unwrap();
    assert_eq!(result, StopResult::ForceKilled);
    assert_all_exited(&tree);
    assert!(job_pids(&id).is_empty());
    assert_eq!(control.inspect(&receipt.identity).await.unwrap(), Liveness::Dead);

    // 冪等：再停一次仍回報已停止
    assert_eq!(control.stop(&receipt.identity, SHORT_GRACE).await.unwrap(), StopResult::StoppedGracefully);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_that_exits_by_itself_stops_gracefully() {
    let id = unique_id("exit");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();

    let receipt = control.spawn(cmd_spec(&id, &["/c", "exit", "0"])).await.unwrap();
    let result = control.stop(&receipt.identity, Duration::from_secs(10)).await.unwrap();
    assert_eq!(result, StopResult::StoppedGracefully);
    assert_eq!(control.inspect(&receipt.identity).await.unwrap(), Liveness::Dead);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_kills_descendants_left_behind_by_a_dead_root() {
    let id = unique_id("orphan");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();

    // cmd 啟動 ping 後立刻結束，ping 成為遺留的子孫
    let receipt = control.spawn(cmd_spec(&id, &["/c", "start", "/b", "ping", "-n", "60", "127.0.0.1"])).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while control.inspect(&receipt.identity).await.unwrap() != Liveness::Dead {
        assert!(Instant::now() < deadline, "根程序沒有結束");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_for_job(&id, "遺留的 ping 仍在 Job 內", |n| n >= 1).await;
    let leftovers = watch_job_processes(&id);
    assert!(!leftovers.is_empty());

    let result = control.stop(&receipt.identity, SHORT_GRACE).await.unwrap();
    assert_eq!(result, StopResult::ForceKilled);
    assert_all_exited(&leftovers);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_resumes_control_from_another_instance() {
    let id = unique_id("find");
    let _guard = JobGuard(id.clone());
    let spec = tree_spec(&id);

    let first = WindowsProcessControl::new();
    let receipt = first.spawn(spec.clone()).await.unwrap();
    wait_for_job(&id, "程序樹至少 3 個程序", |n| n >= 3).await;
    let tree = watch_job_processes(&id);

    // 第二個實例模擬 Runtime 重建：不知道任何 handle，只能靠 command_id 找回
    let second = WindowsProcessControl::new();
    let found = second.find(&spec).await.unwrap().expect("應找回執行中的程序");
    assert_eq!(found.identity, receipt.identity);
    assert_eq!(second.inspect(&found.identity).await.unwrap(), Liveness::Alive);

    assert_eq!(second.stop(&found.identity, SHORT_GRACE).await.unwrap(), StopResult::ForceKilled);
    assert_all_exited(&tree);
    assert_eq!(second.inspect(&found.identity).await.unwrap(), Liveness::Dead);
    assert_eq!(first.inspect(&receipt.identity).await.unwrap(), Liveness::Dead);

    // 全部結束後（第一個實例仍持有 Job handle）找不到
    assert_eq!(second.find(&spec).await.unwrap(), None);
    assert_eq!(WindowsProcessControl::new().find(&spec).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_returns_none_for_unknown_command_id() {
    let control = WindowsProcessControl::new();
    let spec = tree_spec(&unique_id("never-spawned"));
    assert_eq!(control.find(&spec).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_of_missing_program_fails_without_leftovers() {
    let id = unique_id("missing");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();
    let spec = SpawnSpec {
        program: "baton-no-such-program-xyz.exe".to_string(),
        args: vec![],
        working_dir: None,
        command_id: id.clone(),
        env: Vec::new(),
    };

    assert!(control.spawn(spec.clone()).await.is_err());
    assert!(open_job(&id).is_none(), "失敗的 spawn 不應留下 Job");
    assert_eq!(control.find(&spec).await.unwrap(), None);
    // 失敗之後同一個 command_id 仍可正常啟動
    let retry = cmd_spec(&id, &["/c", "exit", "0"]);
    assert!(control.spawn(retry).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_command_id_is_rejected() {
    let id = unique_id("dup");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();
    let spec = tree_spec(&id);

    let receipt = control.spawn(spec.clone()).await.unwrap();
    wait_for_job(&id, "程序樹至少 3 個程序", |n| n >= 3).await;
    let before = job_pids(&id).len();

    // 同一個實例與另一個實例（模擬 Runtime 重建後忘了 find）都必須被拒絕
    assert!(control.spawn(spec.clone()).await.is_err());
    assert!(WindowsProcessControl::new().spawn(spec.clone()).await.is_err());

    assert_eq!(job_pids(&id).len(), before, "重複 spawn 不應啟動新的程序");
    assert_eq!(control.inspect(&receipt.identity).await.unwrap(), Liveness::Alive);
    assert_eq!(control.stop(&receipt.identity, SHORT_GRACE).await.unwrap(), StopResult::ForceKilled);
}

#[tokio::test(flavor = "multi_thread")]
async fn extra_environment_reaches_the_child_and_does_not_change_the_command_hash() {
    let id = unique_id("env");
    let _guard = JobGuard(id.clone());
    let control = WindowsProcessControl::new();
    let marker = std::env::temp_dir().join(format!("baton-{id}.txt"));
    let script = format!("if defined BATON_TEST_ENV echo ok> {}", marker.display());
    let mut spec = cmd_spec(&id, &["/c", &script]);
    let plain_hash = command_hash(&spec);
    spec.env = vec![("BATON_TEST_ENV".to_string(), "secret-value".to_string())];

    let receipt = control.spawn(spec.clone()).await.unwrap();

    assert_eq!(receipt.identity.command_hash, plain_hash, "環境變數不納入指令雜湊");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "子程序沒有看到額外的環境變數");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = std::fs::remove_file(&marker);
    assert!(!format!("{spec:?}").contains("secret-value"), "Debug 輸出不可含環境變數的值");
}
