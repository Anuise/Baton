import { useEffect, useState } from "react";

import type {
  BackupClient,
  BackupStatus,
  BackupTrigger,
  DrillCheckKind,
  DrillReport,
  RetentionReason,
} from "./runtimeClient";
import { formatNumber, splitFirstSentence } from "./textFormat";
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
  pre_update: "更新前",
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
    <section className="section backup-settings" aria-labelledby="backup-title">
      <div className="section-heading">
        <h3 id="backup-title">備份與還原演練</h3>
        <p>每日自動備份，保留 7 份每日與 4 份每週備份；每週自動進行一次還原演練。備份不含登入秘密。</p>
      </div>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <p role="alert" className="callout callout-danger">
          無法取得備份狀態：{loaded.reason}
        </p>
      )}
      {loaded.state === "ready" && (
        <>
          {loaded.status.latest_drill ? (
            <Drill drill={loaded.status.latest_drill} timeZone={timeZone} />
          ) : (
            <div className="panel">
              <h4>最新一次還原演練</h4>
              <p className="empty-note">尚未進行還原演練（每週自動一次）</p>
            </div>
          )}
          <div className="panel backup-list">
            {/* 計數放在標題外（MASTER 7.1），標題的可存取名稱只有「備份清單」 */}
            <div className="backup-list-head">
              <h4>備份清單</h4>
              <span className="count">{formatNumber(loaded.status.backups.length)}</span>
            </div>
            {loaded.status.backups.length === 0 ? (
              <p className="empty-note">尚無備份</p>
            ) : (
              <div className="table-wrap">
                <table className="data-table" aria-label="備份清單">
                  <thead>
                    <tr>
                      <th scope="col">建立時間</th>
                      <th scope="col">觸發原因</th>
                      <th scope="col" className="num">
                        成果檔案
                      </th>
                      <th scope="col">保留原因</th>
                    </tr>
                  </thead>
                  <tbody>
                    {loaded.status.backups.map((backup) => (
                      <tr key={backup.id}>
                        <td className="tabular">{formatTimestamp(backup.created_at, timeZone)}</td>
                        <td>{TRIGGER_LABELS[backup.trigger]}</td>
                        <td className="num">{formatNumber(backup.artifact_count)} 個</td>
                        <td>{backup.retained_as.map((reason) => RETENTION_LABELS[reason]).join("、")}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </>
      )}
    </section>
  );
}

function Drill({ drill, timeZone }: { drill: DrillReport; timeZone?: string }) {
  return (
    <div className="panel backup-drill">
      <div className="backup-drill-head">
        <h4>最新一次還原演練</h4>
        {drill.passed ? (
          <span className="badge badge-success">還原演練通過</span>
        ) : (
          <span className="badge badge-danger" role="alert">
            還原演練未通過
          </span>
        )}
      </div>
      <dl className="facts">
        <dt>演練時間</dt>
        <dd className="tabular">{formatTimestamp(drill.performed_at, timeZone)}</dd>
        <dt>被演練的備份</dt>
        <dd className="tabular">
          {drill.backup_created_at === null ? "沒有可演練的備份" : formatTimestamp(drill.backup_created_at, timeZone)}
        </dd>
        <dt>還原時標為中斷的 Run</dt>
        <dd>{formatNumber(drill.interrupted_runs)} 個（還原後不會自動重做）</dd>
      </dl>
      <ul className="backup-checks" aria-label="核對項目">
        {/* 結果是徽章，說明維持一般文字。長說明只留摘要，其餘收進「技術細節」 */}
        {drill.checks.map((check) => {
          const { summary, rest } = briefDetail(check.detail);
          return (
            <li key={check.kind} className={check.passed ? "backup-check" : "backup-check backup-check-failed"}>
              <span className="backup-check-name">{CHECK_LABELS[check.kind]}</span>
              <span className={`badge ${check.passed ? "badge-success" : "badge-danger"}`}>{check.passed ? "通過" : "未通過"}</span>
              {summary !== "" && <span className="backup-check-detail">{summary}</span>}
              {rest !== "" && (
                <details className="backup-check-more">
                  <summary>技術細節</summary>
                  <p className="pre-wrap">{rest}</p>
                </details>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** 核對說明的摘要上限（字元）：約一行，超過就不放在徽章旁。 */
const BRIEF_MAX = 60;

/**
 * 核對說明拆成摘要（徽章旁完整顯示）與其餘（收進「技術細節」）。
 * 先取第一句；第一句太長時改取第一個「：」「；」之前的子句；仍太長就整段收起，結果只看徽章。
 */
function briefDetail(detail: string): { summary: string; rest: string } {
  const sentence = splitFirstSentence(detail);
  if (sentence.summary.length <= BRIEF_MAX) return sentence;
  const clause = /[：；]/.exec(detail);
  if (clause !== null && clause.index > 0 && clause.index <= BRIEF_MAX) {
    return { summary: detail.slice(0, clause.index).trim(), rest: detail.slice(clause.index + 1).trim() };
  }
  return { summary: "", rest: detail.trim() };
}
