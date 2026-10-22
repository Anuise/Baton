import { useEffect, useState } from "react";

import type {
  BackupClient,
  BackupStatus,
  BackupSummary,
  BackupTrigger,
  DrillCheckKind,
  DrillReport,
  RetentionReason,
} from "./runtimeClient";
import { formatTimestamp } from "./time";

type StatusState = { state: "loading" } | { state: "ready"; status: BackupStatus } | { state: "failed"; reason: string };

interface Props {
  client: BackupClient;
  /** 只供測試指定時區；平常使用系統時區（spec D-33）。 */
  timeZone?: string;
}

const TRIGGER_LABELS: Record<BackupTrigger, string> = {
  daily: "每日",
  pre_migration: "migration 前",
  milestone: "重要裁定／Stage 完成",
};

const RETENTION_LABELS: Record<RetentionReason, string> = {
  daily: "每日",
  weekly: "每週",
  latest_pre_migration: "最新的 migration 前備份",
};

const CHECK_LABELS: Record<DrillCheckKind, string> = {
  restore: "還原",
  database_integrity: "資料庫完整性",
  foreign_keys: "外鍵關聯",
  artifact_hashes: "成果檔案雜湊",
  artifact_relations: "成果與資料庫的對應",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 備份與還原演練：唯讀顯示 Runtime 回報的備份清單與最新一次還原演練結果。 */
export function BackupSettings({ client, timeZone }: Props) {
  const [loaded, setLoaded] = useState<StatusState>({ state: "loading" });

  useEffect(() => {
    void (async () => {
      try {
        setLoaded({ state: "ready", status: await client.backupStatus() });
      } catch (error) {
        setLoaded({ state: "failed", reason: reasonOf(error) });
      }
    })();
  }, [client]);

  return (
    <section className="backup-settings" aria-labelledby="backup-title">
      <h2 id="backup-title">備份與還原演練</h2>
      <p className="backup-policy">
        每日自動備份，保留 7 份每日與 4 份每週備份；每週自動進行一次還原演練。備份不含登入秘密。
      </p>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得備份狀態：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <h3>最新一次還原演練</h3>
          {loaded.status.latest_drill ? (
            <Drill drill={loaded.status.latest_drill} timeZone={timeZone} />
          ) : (
            <p>尚未進行還原演練（每週自動一次）</p>
          )}
          <h3>備份清單</h3>
          {loaded.status.backups.length === 0 ? (
            <p>尚無備份</p>
          ) : (
            <ul className="backup-list" aria-label="備份清單">
              {loaded.status.backups.map((backup) => (
                <li key={backup.id}>{describeBackup(backup, timeZone)}</li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

function Drill({ drill, timeZone }: { drill: DrillReport; timeZone?: string }) {
  return (
    <div className="backup-drill">
      {drill.passed ? (
        <p className="backup-drill-passed">還原演練通過</p>
      ) : (
        <p className="backup-drill-failed" role="alert">
          還原演練未通過
        </p>
      )}
      <p>演練時間：{formatTimestamp(drill.performed_at, timeZone)}</p>
      <p>
        被演練的備份：
        {drill.backup_created_at === null ? "沒有可演練的備份" : formatTimestamp(drill.backup_created_at, timeZone)}
      </p>
      <ul className="backup-checks" aria-label="核對項目">
        {drill.checks.map((check) => (
          <li key={check.kind} className={check.passed ? undefined : "backup-check-failed"}>
            {CHECK_LABELS[check.kind]}：{check.passed ? "通過" : "未通過"}（{check.detail}）
          </li>
        ))}
      </ul>
      <p>還原時標為中斷的 Run：{drill.interrupted_runs} 個（還原後不會自動重做）</p>
    </div>
  );
}

/** 一份備份的一行說明；沒有保留原因時省略該段。 */
function describeBackup(backup: BackupSummary, timeZone?: string): string {
  const parts = [
    formatTimestamp(backup.created_at, timeZone),
    TRIGGER_LABELS[backup.trigger],
    `成果檔案 ${backup.artifact_count} 個`,
  ];
  if (backup.retained_as.length > 0) {
    parts.push(`保留原因：${backup.retained_as.map((reason) => RETENTION_LABELS[reason]).join("、")}`);
  }
  return parts.join("・");
}
