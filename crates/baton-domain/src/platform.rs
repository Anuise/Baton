//! 平台辨識與支援狀態判定（spec DR-20261021-08：A＋A1＋E1＋L1）。
//!
//! 純邏輯：輸入是 PlatformInfo Port 回報的事實與目前時間，輸出是支援狀態與警告。
//! 非正式支援與已到期都只產生警告，永遠不阻擋使用（`blocks_dispatch` 恆為 false）。

use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// 無法取得的事實一律填這個值，不猜測。
pub const UNKNOWN: &str = "unknown";

/// 支援期限表的資料來源（Microsoft Windows 11 release information 的 Home／Pro 服務結束日）。
pub const SUPPORT_TABLE_SOURCE: &str = "https://learn.microsoft.com/windows/release-health/windows11-release-information";
/// 支援期限表最後一次查閱的日期。
pub const SUPPORT_TABLE_CHECKED_ON: &str = "2026-10-22";

/// 一個正式支援（驗證目標）的 Windows 11 功能版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEntry {
    pub feature_version: &'static str,
    pub build: u32,
    /// Home／Pro 的服務結束日（年、月、日）。
    pub home_pro_end_of_servicing: (i32, u32, u32),
    /// 日期是否為官方尚未公布、由發行節奏推估。
    pub estimated: bool,
}

/// 內建的支援期限表。新功能版本驗證後才加入（DR-20261021-08）。
pub const SUPPORT_TABLE: &[SupportEntry] = &[
    SupportEntry { feature_version: "25H2", build: 26200, home_pro_end_of_servicing: (2027, 10, 12), estimated: false },
    SupportEntry { feature_version: "26H2", build: 26300, home_pro_end_of_servicing: (2028, 10, 10), estimated: true },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    Windows11,
    Windows10,
    WindowsServer,
    Other,
}

/// PlatformInfo Port 回報的事實。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSnapshot {
    pub os_family: OsFamily,
    /// OS build 編號，例如 `26300`。
    pub os_build: String,
    /// 功能版本，例如 `25H2`。
    pub feature_version: String,
    /// 版本別，已正規化為 `Home`、`Pro` 或原始的 EditionID。
    pub edition: String,
    /// 作業系統的原生架構（不受程序模擬影響），例如 `x64`、`arm64`。
    pub native_arch: String,
    /// 本程序的架構；與原生架構不同代表正在模擬。
    pub process_arch: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformIssueCode {
    /// Windows 10、Server 或其他作業系統。
    UnsupportedOs,
    /// Home／Pro 以外的版本別。
    UnsupportedEdition,
    /// 原生架構不是 x64（含 ARM 上的 x64 模擬）。
    UnsupportedArchitecture,
    /// Windows 11 x64 Home／Pro，但功能版本不在驗證目標內。
    UnverifiedVersion,
    /// 功能版本已過 Microsoft 的服務結束日（L1）。
    SupportEnded,
    /// 讀不到足以判斷的事實。
    FactsUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformIssue {
    pub code: PlatformIssueCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAssessment {
    pub status: SupportStatus,
    pub issues: Vec<PlatformIssue>,
    /// 符合支援期限表時，該功能版本的服務結束日（`YYYY-MM-DD`）。
    pub support_ends_on: Option<String>,
    /// 平台支援狀態是否阻擋派工、交接或恢復；L1 之下永遠是 false。
    pub blocks_dispatch: bool,
}

/// 依平台事實與目前時間判定支援狀態。
pub fn assess(snapshot: &PlatformSnapshot, now: Timestamp) -> PlatformAssessment {
    let issue = |code, message: &str| PlatformIssue { code, message: message.to_string() };
    let result = |status, issues, support_ends_on| PlatformAssessment {
        status,
        issues,
        support_ends_on,
        blocks_dispatch: false,
    };

    if snapshot.os_build == UNKNOWN || snapshot.native_arch == UNKNOWN {
        return result(
            SupportStatus::Unknown,
            vec![issue(PlatformIssueCode::FactsUnavailable, "無法讀取 OS build 或原生架構，無法判斷支援狀態")],
            None,
        );
    }

    let mut issues = Vec::new();
    match snapshot.os_family {
        OsFamily::Windows11 => {}
        OsFamily::Windows10 => issues.push(issue(PlatformIssueCode::UnsupportedOs, "Windows 10 不在正式支援範圍")),
        OsFamily::WindowsServer => {
            issues.push(issue(PlatformIssueCode::UnsupportedOs, "Windows Server 不在正式支援範圍"))
        }
        OsFamily::Other => issues.push(issue(PlatformIssueCode::UnsupportedOs, "此作業系統不在正式支援範圍")),
    }
    if !matches!(snapshot.edition.to_ascii_lowercase().as_str(), "home" | "pro") {
        issues.push(PlatformIssue {
            code: PlatformIssueCode::UnsupportedEdition,
            message: format!("版本別 {} 不在正式支援範圍（僅 Home／Pro）", snapshot.edition),
        });
    }
    if snapshot.native_arch != "x64" {
        let message = if snapshot.process_arch != snapshot.native_arch {
            format!("原生架構為 {}，目前是 {} 模擬執行，不在正式支援範圍", snapshot.native_arch, snapshot.process_arch)
        } else {
            format!("原生架構 {} 不在正式支援範圍（僅 x64）", snapshot.native_arch)
        };
        issues.push(PlatformIssue { code: PlatformIssueCode::UnsupportedArchitecture, message });
    }

    let mut support_ends_on = None;
    if issues.is_empty() {
        let entry = snapshot.os_build.parse::<u32>().ok().and_then(|build| SUPPORT_TABLE.iter().find(|e| e.build == build));
        match entry {
            None => issues.push(PlatformIssue {
                code: PlatformIssueCode::UnverifiedVersion,
                message: format!("功能版本 {}（build {}）尚未列入驗證目標", snapshot.feature_version, snapshot.os_build),
            }),
            Some(entry) => {
                let (year, month, day) = entry.home_pro_end_of_servicing;
                support_ends_on = Some(format!("{year:04}-{month:02}-{day:02}"));
                if civil_date(now) >= entry.home_pro_end_of_servicing {
                    issues.push(PlatformIssue {
                        code: PlatformIssueCode::SupportEnded,
                        message: format!(
                            "Windows 11 {} 已於 {year:04}-{month:02}-{day:02} 結束 Microsoft 服務；仍可繼續使用，但不再取得 OS 安全更新（L1 警告，不阻擋派工）",
                            entry.feature_version
                        ),
                    });
                }
            }
        }
    }

    let status = if issues.is_empty() { SupportStatus::Supported } else { SupportStatus::Unsupported };
    result(status, issues, support_ends_on)
}

/// UTC 日期（年、月、日）。
fn civil_date(at: Timestamp) -> (i32, u32, u32) {
    // Howard Hinnant 的 civil_from_days 演算法
    let days = at.as_millis().div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = (year_of_era + era * 400) as i32 + i32::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-22 00:00:00 UTC
    const TODAY: Timestamp = Timestamp::from_millis(1_792_627_200_000);

    fn windows11(build: &str, feature: &str, edition: &str) -> PlatformSnapshot {
        PlatformSnapshot {
            os_family: OsFamily::Windows11,
            os_build: build.into(),
            feature_version: feature.into(),
            edition: edition.into(),
            native_arch: "x64".into(),
            process_arch: "x64".into(),
        }
    }

    fn codes(assessment: &PlatformAssessment) -> Vec<PlatformIssueCode> {
        assessment.issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn civil_date_converts_utc_milliseconds() {
        assert_eq!(civil_date(Timestamp::from_millis(0)), (1970, 1, 1));
        assert_eq!(civil_date(TODAY), (2026, 10, 22));
        assert_eq!(civil_date(TODAY.plus_millis(86_400_000 - 1)), (2026, 10, 22));
        assert_eq!(civil_date(TODAY.plus_millis(86_400_000)), (2026, 10, 23));
        // 閏日
        assert_eq!(civil_date(Timestamp::from_millis(1_709_164_800_000)), (2024, 2, 29));
    }

    #[test]
    fn the_four_supported_combinations_are_supported() {
        for (build, feature) in [("26200", "25H2"), ("26300", "26H2")] {
            for edition in ["Home", "Pro"] {
                let assessment = assess(&windows11(build, feature, edition), TODAY);
                assert_eq!(assessment.status, SupportStatus::Supported, "{feature} {edition}");
                assert!(assessment.issues.is_empty());
                assert!(!assessment.blocks_dispatch);
            }
        }
    }

    #[test]
    fn windows_10_is_unsupported() {
        let snapshot = PlatformSnapshot { os_family: OsFamily::Windows10, ..windows11("19045", "22H2", "Pro") };
        let assessment = assess(&snapshot, TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::UnsupportedOs]);
        assert!(!assessment.blocks_dispatch);
    }

    #[test]
    fn windows_server_is_unsupported() {
        let snapshot = PlatformSnapshot { os_family: OsFamily::WindowsServer, ..windows11("26100", "24H2", "ServerStandard") };
        let assessment = assess(&snapshot, TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert!(codes(&assessment).contains(&PlatformIssueCode::UnsupportedOs));
    }

    #[test]
    fn native_arm64_is_unsupported() {
        let snapshot = PlatformSnapshot { native_arch: "arm64".into(), process_arch: "arm64".into(), ..windows11("26200", "25H2", "Pro") };
        let assessment = assess(&snapshot, TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::UnsupportedArchitecture]);
    }

    #[test]
    fn x64_emulation_on_arm_is_not_mistaken_for_supported() {
        // 程序是 x64，但原生架構是 arm64
        let snapshot = PlatformSnapshot { native_arch: "arm64".into(), process_arch: "x64".into(), ..windows11("26200", "25H2", "Pro") };
        let assessment = assess(&snapshot, TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::UnsupportedArchitecture]);
        assert!(assessment.issues[0].message.contains("模擬"));
    }

    #[test]
    fn editions_other_than_home_and_pro_are_unsupported() {
        let assessment = assess(&windows11("26200", "25H2", "Enterprise"), TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::UnsupportedEdition]);
    }

    #[test]
    fn a_windows_11_version_outside_the_table_is_unverified() {
        let assessment = assess(&windows11("26100", "24H2", "Pro"), TODAY);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::UnverifiedVersion]);
        assert_eq!(assessment.support_ends_on, None);
    }

    #[test]
    fn a_version_past_its_end_of_servicing_gets_an_l1_warning_without_blocking() {
        let after_end = Timestamp::from_millis(1_823_299_200_000); // 2027-10-12 00:00:00 UTC
        let assessment = assess(&windows11("26200", "25H2", "Pro"), after_end);
        assert_eq!(assessment.status, SupportStatus::Unsupported);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::SupportEnded]);
        assert_eq!(assessment.support_ends_on.as_deref(), Some("2027-10-12"));
        assert!(!assessment.blocks_dispatch);
        // 到期前一天仍是正式支援，但帶出結束日
        let day_before = assess(&windows11("26200", "25H2", "Pro"), after_end.plus_millis(-1));
        assert_eq!(day_before.status, SupportStatus::Supported);
        assert_eq!(day_before.support_ends_on.as_deref(), Some("2027-10-12"));
    }

    #[test]
    fn unreadable_facts_are_unknown_rather_than_supported() {
        let snapshot = PlatformSnapshot { os_build: UNKNOWN.into(), native_arch: UNKNOWN.into(), ..windows11("", "", "Pro") };
        let assessment = assess(&snapshot, TODAY);
        assert_eq!(assessment.status, SupportStatus::Unknown);
        assert_eq!(codes(&assessment), vec![PlatformIssueCode::FactsUnavailable]);
        assert!(!assessment.blocks_dispatch);
    }

    #[test]
    fn support_table_states_its_source_and_lookup_date() {
        assert!(SUPPORT_TABLE_SOURCE.starts_with("https://"));
        assert_eq!(SUPPORT_TABLE_CHECKED_ON.len(), 10);
    }
}
