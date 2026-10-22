//! 正式的 ProcessControl：每個 Run 一個 Windows Job Object（spec D-10）。
//!
//! - Job 具名 `Local\Baton-Run-{command_id}`：Runtime 重啟後可用 `find` 以 OpenJobObject 找回。
//! - 不設 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`：關閉 Job handle（含 Runtime 崩潰）不會殺掉程序。
//! - 子孫程序預設繼承 Job 成員資格（本實作不允許 breakaway），所以 `stop` 能終止整棵程序樹。
//! - 程序的 stdin/stdout/stderr 依 `SpawnSpec` 導向檔案，未指定時導向 NUL。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use baton_ports::{
    command_hash, Liveness, PortError, PortResult, ProcessControl, ProcessIdentity, Receipt, SpawnSpec, StopResult,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER,
    ERROR_MORE_DATA, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_CREATION_DISPOSITION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
    OpenJobObjectW, QueryInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_PROCESS_ID_LIST,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetProcessTimes, OpenProcess, ResumeThread, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};

/// Job Object 存取權限（windows-sys 沒有提供這兩個常數）。
const JOB_OBJECT_TERMINATE: u32 = 0x0008;
const JOB_OBJECT_QUERY: u32 = 0x0004;
/// 等待 grace 期間的輪詢間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// 強制終止後，等待 Job 內程序全部消失的上限。
const KILL_WAIT: Duration = Duration::from_secs(10);
/// FILETIME（1601-01-01）到 Unix epoch 的毫秒差。
const FILETIME_TO_UNIX_MILLIS: i64 = 11_644_473_600_000;

/// 擁有所有權的 Win32 handle，Drop 時關閉。關閉 Job handle 不會殺程序（未設 KILL_ON_JOB_CLOSE）。
struct OwnedHandle(HANDLE);

// SAFETY：Win32 核心物件 handle 可以跨執行緒使用與關閉，沒有執行緒親和性。
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// 取得 handle 的所有權；呼叫失敗（NULL 或 INVALID_HANDLE_VALUE）回傳 `None`。
    fn from_raw(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(handle))
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY：self.0 是尚未關閉的有效 handle，且只在這裡關閉一次。
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// 一個受管 Run：Job handle 與根程序 handle。持有根程序 handle 期間 PID 不會被重用。
struct Entry {
    job: OwnedHandle,
    process: OwnedHandle,
}

/// 以 Windows Job Object 管理程序樹的 ProcessControl。
#[derive(Default)]
pub struct WindowsProcessControl {
    entries: Mutex<HashMap<ProcessIdentity, Arc<Entry>>>,
}

impl WindowsProcessControl {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry(&self, identity: &ProcessIdentity) -> Option<Arc<Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(identity).cloned()
    }

    fn register(&self, identity: ProcessIdentity, entry: Entry) {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).insert(identity, Arc::new(entry));
    }
}

fn last_error(context: &str) -> PortError {
    PortError::Failed(format!("{context}：{}", std::io::Error::last_os_error()))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn job_name(command_id: &str) -> PortResult<Vec<u16>> {
    if command_id.is_empty() || command_id.contains(['\\', '\0']) {
        return Err(PortError::Failed(format!("command_id 不能作為 Job 名稱：{command_id:?}")));
    }
    Ok(wide(&format!("Local\\Baton-Run-{command_id}")))
}

/// 依 MSVCRT 規則把一個引數加進命令列（含空白、引號時加引號並跳脫）。
fn append_arg(cmd: &mut Vec<u16>, arg: &str) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        cmd.extend(arg.encode_utf16());
        return;
    }
    cmd.push(b'"' as u16);
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                cmd.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
                cmd.push(b'"' as u16);
                backslashes = 0;
            }
            _ => {
                cmd.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
                backslashes = 0;
                let mut buf = [0u16; 2];
                cmd.extend(ch.encode_utf16(&mut buf).iter());
            }
        }
    }
    cmd.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    cmd.push(b'"' as u16);
}

fn command_line(spec: &SpawnSpec) -> Vec<u16> {
    let mut cmd = Vec::new();
    append_arg(&mut cmd, &spec.program);
    for arg in &spec.args {
        cmd.push(b' ' as u16);
        append_arg(&mut cmd, arg);
    }
    cmd.push(0);
    cmd
}

/// 環境變數區塊：Runtime 自己的環境去掉 `spec.env_remove`，再加上 `spec.env`（同名以 `spec.env` 為準）。
/// 沒有額外變數也沒有要移除的變數時回傳 `None`，讓子程序直接繼承。
fn environment_block(spec: &SpawnSpec) -> Option<Vec<u16>> {
    if spec.env.is_empty() && spec.env_remove.is_empty() {
        return None;
    }
    let mut vars: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .filter(|(k, _)| !spec.env_remove.iter().any(|name| name.eq_ignore_ascii_case(k)))
        .filter(|(k, _)| !spec.env.iter().any(|(name, _)| name.eq_ignore_ascii_case(k)))
        .collect();
    vars.extend(spec.env.iter().cloned());
    // Windows 要求區塊依名稱排序（不分大小寫）
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(format!("{name}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    Some(block)
}

/// 以可繼承的 handle 開啟檔案（或 NUL）作為子程序的標準輸入輸出。
/// 共用模式允許其他程序在子程序寫入期間讀取、寫入或刪除。
fn open_std_handle(
    path: &str,
    access: u32,
    disposition: FILE_CREATION_DISPOSITION,
    security: &SECURITY_ATTRIBUTES,
) -> PortResult<OwnedHandle> {
    // SAFETY：路徑為 NUL 結尾的字串，security 指向有效結構。
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            security,
            disposition,
            0,
            std::ptr::null_mut(),
        )
    };
    OwnedHandle::from_raw(raw).ok_or_else(|| last_error(&format!("開啟 {path} 失敗")))
}

/// 程序建立時間（Unix epoch 毫秒）。
fn created_at_millis(process: HANDLE) -> PortResult<i64> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY：process 是有效 handle（需 PROCESS_QUERY_LIMITED_INFORMATION），四個輸出都指向本地變數。
    let ok = unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(last_error("GetProcessTimes 失敗"));
    }
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    Ok((ticks / 10_000) as i64 - FILETIME_TO_UNIX_MILLIS)
}

/// 程序是否已結束（以 handle 的訊號狀態判斷，不受結束碼剛好等於 STILL_ACTIVE 影響）。
fn has_exited(process: HANDLE) -> PortResult<bool> {
    // SAFETY：process 是有效 handle（需 SYNCHRONIZE），逾時為 0 不會阻塞。
    match unsafe { WaitForSingleObject(process, 0) } {
        WAIT_TIMEOUT => Ok(false),
        WAIT_OBJECT_0 => Ok(true),
        _ => Err(last_error("WaitForSingleObject 失敗")),
    }
}

/// Job 內仍存活的程序數。
fn active_processes(job: HANDLE) -> PortResult<u32> {
    // SAFETY：全零是這個純數值結構的有效初始值。
    let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY：job 是有效 handle（需 JOB_OBJECT_QUERY），緩衝區大小與結構相符。
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicAccountingInformation,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("QueryInformationJobObject（accounting）失敗"));
    }
    Ok(info.ActiveProcesses)
}

/// 列出 Job 內仍存活程序的 PID。
fn job_pids(job: HANDLE) -> PortResult<Vec<u32>> {
    let header = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() - std::mem::size_of::<usize>();
    let mut capacity = 64usize;
    loop {
        let bytes = header + capacity * std::mem::size_of::<usize>();
        // 以 u64 配置確保 8 位元組對齊（結構內含 usize）
        let mut buffer = vec![0u64; bytes.div_ceil(8)];
        // SAFETY：buffer 夠大且對齊，job 是有效 handle（需 JOB_OBJECT_QUERY）。
        let ok = unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr() as *mut c_void,
                bytes as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // SAFETY：緊接在失敗呼叫之後讀取。
            if unsafe { GetLastError() } == ERROR_MORE_DATA {
                capacity *= 4;
                continue;
            }
            return Err(last_error("QueryInformationJobObject（process list）失敗"));
        }
        let list = buffer.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST;
        // SAFETY：呼叫成功，標頭與其後 NumberOfProcessIdsInList 個項目都在 buffer 內。
        unsafe {
            let count = (*list).NumberOfProcessIdsInList as usize;
            let first = std::ptr::addr_of!((*list).ProcessIdList) as *const usize;
            return Ok((0..count).map(|i| *first.add(i) as u32).collect());
        }
    }
}

fn open_process(pid: u32) -> Option<OwnedHandle> {
    // SAFETY：OpenProcess 不需要任何前置條件，失敗回傳 NULL。
    OwnedHandle::from_raw(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) })
}

/// 找出 Job 內建立時間最早者作為根程序，回傳它的身分資訊與 handle。
fn root_of_job(job: HANDLE) -> PortResult<(u32, i64, OwnedHandle)> {
    let mut oldest: Option<(u32, i64, OwnedHandle)> = None;
    for pid in job_pids(job)? {
        // 程序可能在列舉與開啟之間結束，或無權開啟：略過
        let Some(handle) = open_process(pid) else { continue };
        let Ok(created) = created_at_millis(handle.0) else { continue };
        if oldest.as_ref().is_none_or(|(_, best, _)| created < *best) {
            oldest = Some((pid, created, handle));
        }
    }
    oldest.ok_or_else(|| PortError::Failed("Job 內找不到可開啟的程序".to_string()))
}

/// 同步啟動：建立具名 Job → 以暫停狀態建立程序 → 加入 Job → 恢復執行。
fn spawn_blocking_impl(spec: &SpawnSpec) -> PortResult<(ProcessIdentity, Entry)> {
    let name = job_name(&spec.command_id)?;
    // 先建立 Job 再建立程序：同名 Job 已存在就直接拒絕，不會啟動第二個程序。
    // SAFETY：name 以 NUL 結尾；SetLastError(0) 讓後面的 ERROR_ALREADY_EXISTS 判斷不受舊值影響。
    let job = unsafe {
        SetLastError(0);
        CreateJobObjectW(std::ptr::null(), name.as_ptr())
    };
    // SAFETY：緊接在 CreateJobObjectW 之後讀取。
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let Some(job) = OwnedHandle::from_raw(job) else {
        return Err(last_error("CreateJobObjectW 失敗"));
    };
    if already_exists {
        return Err(PortError::Failed(format!("command_id {} 已有對應的 Job，拒絕重複啟動", spec.command_id)));
    }

    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    // 檔案在確認 Job 不重複之後才開啟：重複的 spawn 不會覆寫執行中程序的輸出檔。
    // 失敗時直接回傳，Job 尚無程序，handle 關閉後即消失。
    let stdin = open_std_handle(spec.stdin_file.as_deref().unwrap_or("NUL"), GENERIC_READ, OPEN_EXISTING, &security)?;
    let open_output = |path: Option<&str>| match path {
        Some(path) => open_std_handle(path, GENERIC_WRITE, CREATE_ALWAYS, &security),
        None => open_std_handle("NUL", GENERIC_WRITE, OPEN_EXISTING, &security),
    };
    let stdout = open_output(spec.stdout_file.as_deref())?;
    // stdout 與 stderr 指向同一個檔案時共用 handle：兩個 handle 各有檔案位置，會互相覆寫。
    let stderr = match (&spec.stdout_file, &spec.stderr_file) {
        (Some(out), Some(err)) if out.eq_ignore_ascii_case(err) => None,
        _ => Some(open_output(spec.stderr_file.as_deref())?),
    };

    // SAFETY：全零是 STARTUPINFOW 的有效初始值，之後設定必要欄位。
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = stdin.0;
    startup.hStdOutput = stdout.0;
    startup.hStdError = stderr.as_ref().unwrap_or(&stdout).0;
    // SAFETY：全零是 PROCESS_INFORMATION 的有效初始值，由 CreateProcessW 填寫。
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let mut cmd = command_line(spec);
    let dir = spec.working_dir.as_deref().map(wide);
    let environment = environment_block(spec);
    // SAFETY：cmd 是可寫、NUL 結尾的緩衝區；environment 是以雙 NUL 結尾的 Unicode 環境區塊或 NULL；
    // dir 為 NUL 結尾或 NULL；startup 與 info 皆為有效結構。
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ref().map_or(std::ptr::null(), |e| e.as_ptr() as *const c_void),
            dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(last_error(&format!("無法啟動 {}", spec.program)));
    }
    let process = OwnedHandle(info.hProcess);
    let thread = OwnedHandle(info.hThread);

    // 此後任何一步失敗都要殺掉剛建立（仍暫停）的程序，不留孤兒
    let abort = |error: PortError| -> PortError {
        // SAFETY：process 是有效 handle（CreateProcessW 回傳 PROCESS_ALL_ACCESS）。
        unsafe {
            TerminateProcess(process.0, 1);
        }
        error
    };
    // SAFETY：兩個 handle 皆有效，程序尚未執行任何碼。
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        return Err(abort(last_error("AssignProcessToJobObject 失敗")));
    }
    let created = created_at_millis(process.0).map_err(abort)?;
    // SAFETY：thread 是有效的主執行緒 handle，目前為暫停狀態。
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err(abort(last_error("ResumeThread 失敗")));
    }

    let identity = ProcessIdentity { pid: info.dwProcessId, created_at_millis: created, command_hash: command_hash(spec) };
    Ok((identity, Entry { job, process }))
}

/// 以 `spec.command_id` 找回具名 Job 與其根程序；沒有或已無程序回傳 `None`。
fn find_blocking_impl(spec: &SpawnSpec) -> PortResult<Option<(ProcessIdentity, Entry)>> {
    let name = job_name(&spec.command_id)?;
    // SAFETY：name 以 NUL 結尾。
    let raw = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | SYNCHRONIZE, 0, name.as_ptr()) };
    let Some(job) = OwnedHandle::from_raw(raw) else {
        // SAFETY：緊接在失敗呼叫之後讀取。
        return match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => Ok(None),
            _ => Err(last_error("OpenJobObjectW 失敗")),
        };
    };
    if active_processes(job.0)? == 0 {
        return Ok(None);
    }
    let (pid, created, process) = root_of_job(job.0)?;
    let identity = ProcessIdentity { pid, created_at_millis: created, command_hash: command_hash(spec) };
    Ok(Some((identity, Entry { job, process })))
}

fn join_error(error: tokio::task::JoinError) -> PortError {
    PortError::Failed(format!("背景工作失敗：{error}"))
}

#[async_trait]
impl ProcessControl for WindowsProcessControl {
    async fn spawn(&self, spec: SpawnSpec) -> PortResult<Receipt> {
        let (identity, entry) =
            tokio::task::spawn_blocking(move || spawn_blocking_impl(&spec)).await.map_err(join_error)??;
        self.register(identity.clone(), entry);
        Ok(Receipt { identity })
    }

    async fn find(&self, spec: &SpawnSpec) -> PortResult<Option<Receipt>> {
        let spec = spec.clone();
        let found = tokio::task::spawn_blocking(move || find_blocking_impl(&spec)).await.map_err(join_error)??;
        Ok(found.map(|(identity, entry)| {
            self.register(identity.clone(), entry);
            Receipt { identity }
        }))
    }

    async fn inspect(&self, identity: &ProcessIdentity) -> PortResult<Liveness> {
        if let Some(entry) = self.entry(identity) {
            return Ok(root_liveness(entry.process.0, identity.created_at_millis));
        }
        // 沒有對應的 handle（例如 Runtime 重建後尚未 find）：只能用 PID 開啟，無法判定就回 Unknown
        match open_process(identity.pid) {
            Some(process) => Ok(root_liveness(process.0, identity.created_at_millis)),
            // SAFETY：緊接在失敗呼叫之後讀取。
            None if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER => Ok(Liveness::Dead),
            None => Ok(Liveness::Unknown),
        }
    }

    async fn stop(&self, identity: &ProcessIdentity, grace: Duration) -> PortResult<StopResult> {
        let Some(entry) = self.entry(identity) else {
            // 沒有 Job handle：根程序確定已不存在才視為已停止，否則無法終止
            return match self.inspect(identity).await? {
                Liveness::Dead => Ok(StopResult::StoppedGracefully),
                other => Err(PortError::Failed(format!("找不到 pid {} 的 Job（目前狀態 {other:?}），請先 find", identity.pid))),
            };
        };

        // 受控停止：等待 grace 內根程序與 Job 內所有程序自行結束
        let deadline = Instant::now() + grace;
        loop {
            if active_processes(entry.job.0)? == 0 {
                return Ok(StopResult::StoppedGracefully);
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
        }

        // 逾時：終止整個 Job（含根程序已死後遺留的子孫）
        // SAFETY：job 是有效 handle（需 JOB_OBJECT_TERMINATE）。
        if unsafe { TerminateJobObject(entry.job.0, 1) } == 0 {
            return Err(last_error("TerminateJobObject 失敗"));
        }
        let deadline = Instant::now() + KILL_WAIT;
        loop {
            if active_processes(entry.job.0)? == 0 {
                return Ok(StopResult::ForceKilled);
            }
            if Instant::now() >= deadline {
                return Ok(StopResult::StillAlive);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// 以根程序判斷存活：建立時間不符（PID 被重用）或已結束為 Dead；任何無法判定為 Unknown。
fn root_liveness(process: HANDLE, expected_created_millis: i64) -> Liveness {
    match created_at_millis(process) {
        Ok(created) if created != expected_created_millis => Liveness::Dead,
        Ok(_) => match has_exited(process) {
            Ok(true) => Liveness::Dead,
            Ok(false) => Liveness::Alive,
            Err(_) => Liveness::Unknown,
        },
        Err(_) => Liveness::Unknown,
    }
}
