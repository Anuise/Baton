//! PlatformInfo Port 的正式實作：讀取 Windows 登錄與 `IsWow64Process2`。
//!
//! 判斷依據刻意避開兩個陷阱：
//! - 登錄的 `ProductName` 在 Windows 11 上仍可能寫著「Windows 10」，所以用 build 編號（≥ 22000）判定 Windows 11。
//! - 程序是 x64 不代表機器是 x64（ARM 上的 x64 模擬），所以架構取 `IsWow64Process2` 回報的原生架構，
//!   與本程序的編譯架構分開記錄。

use baton_domain::platform::{OsFamily, PlatformSnapshot, UNKNOWN};
use baton_ports::PlatformInfo;

/// Windows 11 的第一個 build 編號。
const WINDOWS_11_FIRST_BUILD: u32 = 22000;

/// 從系統讀到的原始事實；讀不到的欄位是 `None`。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RawFacts {
    /// 登錄 `InstallationType`：`Client`、`Server`、`Server Core`…
    pub installation_type: Option<String>,
    /// 登錄 `CurrentBuild`。
    pub current_build: Option<String>,
    /// 登錄 `CurrentMajorVersionNumber`。
    pub major_version: Option<u32>,
    /// 登錄 `DisplayVersion`，例如 `25H2`。
    pub display_version: Option<String>,
    /// 登錄 `EditionID`，例如 `Professional`。
    pub edition_id: Option<String>,
    /// `IsWow64Process2` 的 nativeMachine（IMAGE_FILE_MACHINE_*）。
    pub native_machine: Option<u16>,
}

/// 把原始事實整理成 Port 回報的快照；`process_arch` 是本程序自己的架構。
pub fn snapshot_from_raw(raw: &RawFacts, process_arch: &str) -> PlatformSnapshot {
    let build = raw.current_build.as_deref().and_then(|b| b.trim().parse::<u32>().ok());
    let is_server = raw.installation_type.as_deref().is_some_and(|t| t.starts_with("Server"));
    let os_family = match build {
        _ if is_server => OsFamily::WindowsServer,
        Some(b) if b >= WINDOWS_11_FIRST_BUILD => OsFamily::Windows11,
        Some(_) if raw.major_version == Some(10) => OsFamily::Windows10,
        _ => OsFamily::Other,
    };
    PlatformSnapshot {
        os_family,
        os_build: build.map_or_else(|| UNKNOWN.to_string(), |b| b.to_string()),
        feature_version: non_empty(raw.display_version.as_deref()),
        edition: normalize_edition(raw.edition_id.as_deref()),
        native_arch: raw.native_machine.map_or(UNKNOWN, machine_to_arch).to_string(),
        process_arch: process_arch.to_string(),
    }
}

fn non_empty(value: Option<&str>) -> String {
    value.map(str::trim).filter(|v| !v.is_empty()).unwrap_or(UNKNOWN).to_string()
}

/// `EditionID` 正規化：Home 系列合併為 `Home`，Pro（含 N 版）為 `Pro`，其餘保留原值。
fn normalize_edition(edition_id: Option<&str>) -> String {
    match edition_id.map(str::trim) {
        Some("Core" | "CoreN" | "CoreSingleLanguage" | "CoreCountrySpecific") => "Home".to_string(),
        Some("Professional" | "ProfessionalN") => "Pro".to_string(),
        other => non_empty(other),
    }
}

fn machine_to_arch(machine: u16) -> &'static str {
    match machine {
        0x8664 => "x64",
        0xAA64 => "arm64",
        0x014c => "x86",
        0x01c0 | 0x01c4 => "arm",
        _ => UNKNOWN,
    }
}

/// 本程序的編譯架構，用語與原生架構一致。
fn process_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => UNKNOWN,
    }
}

/// 讀取本機的真實平台資訊。
pub struct WindowsPlatformInfo;

impl PlatformInfo for WindowsPlatformInfo {
    fn snapshot(&self) -> PlatformSnapshot {
        snapshot_from_raw(&read_raw_facts(), process_arch())
    }
}

#[cfg(windows)]
fn read_raw_facts() -> RawFacts {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};

    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let mut process_machine = 0u16;
    let mut native_machine = 0u16;
    // SAFETY: 兩個輸出參數都指向有效的區域變數；GetCurrentProcess 回傳不需關閉的偽控制代碼。
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process_machine, &mut native_machine) };
    RawFacts {
        installation_type: registry::read_string(KEY, "InstallationType"),
        current_build: registry::read_string(KEY, "CurrentBuild"),
        major_version: registry::read_dword(KEY, "CurrentMajorVersionNumber"),
        display_version: registry::read_string(KEY, "DisplayVersion"),
        edition_id: registry::read_string(KEY, "EditionID"),
        native_machine: (ok != 0 && native_machine != 0).then_some(native_machine),
    }
}

/// 非 Windows 環境（開發用）讀不到任何事實，快照會是 unknown 而不是假裝支援。
#[cfg(not(windows))]
fn read_raw_facts() -> RawFacts {
    RawFacts::default()
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read_string(subkey: &str, name: &str) -> Option<String> {
        let (subkey, name) = (wide(subkey), wide(name));
        let mut bytes = 0u32;
        // SAFETY: 字串皆以 NUL 結尾；第一次呼叫只查詢所需位元組數。
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        if status != 0 || bytes == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        // SAFETY: 緩衝區大小至少為 `bytes` 位元組。
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    pub fn read_dword(subkey: &str, name: &str) -> Option<u32> {
        let (subkey, name) = (wide(subkey), wide(name));
        let mut value = 0u32;
        let mut bytes = std::mem::size_of::<u32>() as u32;
        // SAFETY: 輸出緩衝區是一個 u32，大小與 `bytes` 一致。
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut bytes,
            )
        };
        (status == 0).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMD64: u16 = 0x8664;
    const ARM64: u16 = 0xAA64;

    fn pro_26h2() -> RawFacts {
        RawFacts {
            installation_type: Some("Client".into()),
            current_build: Some("26300".into()),
            major_version: Some(10),
            display_version: Some("26H2".into()),
            edition_id: Some("Professional".into()),
            native_machine: Some(AMD64),
        }
    }

    #[test]
    fn windows_11_is_recognised_by_build_even_though_major_version_is_10() {
        let snapshot = snapshot_from_raw(&pro_26h2(), "x64");
        assert_eq!(snapshot.os_family, OsFamily::Windows11);
        assert_eq!(snapshot.os_build, "26300");
        assert_eq!(snapshot.feature_version, "26H2");
        assert_eq!(snapshot.edition, "Pro");
        assert_eq!((snapshot.native_arch.as_str(), snapshot.process_arch.as_str()), ("x64", "x64"));
    }

    #[test]
    fn x64_process_on_arm64_windows_reports_arm64_as_native_architecture() {
        let raw = RawFacts { native_machine: Some(ARM64), ..pro_26h2() };
        let snapshot = snapshot_from_raw(&raw, "x64");
        assert_eq!(snapshot.native_arch, "arm64");
        assert_eq!(snapshot.process_arch, "x64");
    }

    #[test]
    fn windows_10_below_build_22000_is_windows_10() {
        let raw = RawFacts { current_build: Some("19045".into()), display_version: Some("22H2".into()), ..pro_26h2() };
        assert_eq!(snapshot_from_raw(&raw, "x64").os_family, OsFamily::Windows10);
    }

    #[test]
    fn server_installation_type_wins_over_build() {
        let raw = RawFacts { installation_type: Some("Server".into()), edition_id: Some("ServerStandard".into()), ..pro_26h2() };
        let snapshot = snapshot_from_raw(&raw, "x64");
        assert_eq!(snapshot.os_family, OsFamily::WindowsServer);
        assert_eq!(snapshot.edition, "ServerStandard");
    }

    #[test]
    fn home_editions_are_normalised_and_others_kept() {
        for (id, expected) in [("Core", "Home"), ("CoreSingleLanguage", "Home"), ("ProfessionalN", "Pro"), ("Enterprise", "Enterprise")] {
            let raw = RawFacts { edition_id: Some(id.into()), ..pro_26h2() };
            assert_eq!(snapshot_from_raw(&raw, "x64").edition, expected, "{id}");
        }
    }

    #[test]
    fn missing_facts_become_unknown_instead_of_guesses() {
        let snapshot = snapshot_from_raw(&RawFacts::default(), "x64");
        assert_eq!(snapshot.os_family, OsFamily::Other);
        assert_eq!(snapshot.os_build, UNKNOWN);
        assert_eq!(snapshot.native_arch, UNKNOWN);
        assert_eq!(snapshot.edition, UNKNOWN);
        assert_eq!(snapshot.feature_version, UNKNOWN);
    }

    #[test]
    fn machine_codes_map_to_architecture_names() {
        assert_eq!(machine_to_arch(0x014c), "x86");
        assert_eq!(machine_to_arch(0x01c4), "arm");
        assert_eq!(machine_to_arch(0x1234), UNKNOWN);
    }

    /// 在本機實測：不依賴特定版本，只驗證讀到的事實內部一致。
    /// 只在 Windows 10／11 桌面版執行；Windows Server（例如 CI 的 windows-latest runner）會略過。
    #[cfg(windows)]
    #[test]
    fn real_implementation_reads_this_machine() {
        let snapshot = WindowsPlatformInfo.snapshot();
        if snapshot.os_family == OsFamily::WindowsServer {
            eprintln!("略過：偵測到 Windows Server，這個測試只在 Windows 10／11 桌面版執行");
            return;
        }
        let build: u32 = snapshot.os_build.parse().expect("應讀到數字的 OS build");
        let expected = if build >= WINDOWS_11_FIRST_BUILD { OsFamily::Windows11 } else { OsFamily::Windows10 };
        assert_eq!(snapshot.os_family, expected);
        assert_ne!(snapshot.native_arch, UNKNOWN);
        assert_eq!(snapshot.process_arch, process_arch());
        // 原生架構與程序架構不同才代表模擬；在 x64 機器上兩者必須相同
        if snapshot.native_arch == "x64" {
            assert_eq!(snapshot.process_arch, "x64");
        }
    }
}
