//! 正式執行用的 RuntimeHost：以程序判斷 Runtime 是否還在，以脫離監護的方式啟動 `baton-runtime.exe`。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use async_trait::async_trait;
use baton_domain::guardian::GUARDIAN_ATTEMPT_ENV;
use baton_domain::login::{START_SOURCE_ENV, START_SOURCE_LOGIN};

use crate::files;
use crate::RuntimeHost;

pub struct ProcessHost {
    runtime_exe: PathBuf,
    data_dir: PathBuf,
    /// 交給 Runtime 的設定（資料目錄與埠），與 App 啟動 Runtime 時相同。
    envs: Vec<(String, String)>,
    /// 監護自己啟動的 Runtime。
    child: Option<Child>,
}

impl ProcessHost {
    pub fn new(runtime_exe: PathBuf, data_dir: PathBuf, envs: Vec<(String, String)>) -> Self {
        Self { runtime_exe, data_dir, envs, child: None }
    }
}

#[async_trait]
impl RuntimeHost for ProcessHost {
    async fn runtime_running(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                return true;
            }
            self.child = None;
        }
        // App（或另一個入口）啟動的 Runtime：以 Runtime 寫下的程序身分確認；
        // 自己啟動的那個因為單實例而結束時，也由這裡找到正在執行的那一個
        files::instance_alive(&files::runtime_instance_path(&self.data_dir))
    }

    async fn start_runtime(&mut self, attempt_id: &str) -> Result<(), String> {
        self.spawn((GUARDIAN_ATTEMPT_ENV, attempt_id))
    }

    async fn start_runtime_at_login(&mut self) -> Result<(), String> {
        self.spawn((START_SOURCE_ENV, START_SOURCE_LOGIN))
    }
}

impl ProcessHost {
    /// 以脫離監護的方式啟動 Runtime，並交付一個標明啟動方式的環境變數。
    fn spawn(&mut self, (name, value): (&str, &str)) -> Result<(), String> {
        let mut command = Command::new(&self.runtime_exe);
        command
            .envs(self.envs.iter().map(|(k, v)| (k, v)))
            .env(name, value)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // 與 App 啟動 Runtime 相同：新的程序群組、沒有主控台視窗；監護結束時 Runtime 不受影響
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        let child = command.spawn().map_err(|e| format!("無法啟動 {}：{e}", self.runtime_exe.display()))?;
        self.child = Some(child);
        Ok(())
    }
}
