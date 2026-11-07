import { useEffect, useState } from "react";

import { CopyButton } from "./LongText";
import type { PlatformResponse, RuntimeClient } from "./runtimeClient";
import { docsHost, splitFirstSentence } from "./textFormat";

type Assessment = PlatformResponse["assessment"];

/** 平台支援狀態的顯示文字（狀態列與平台狀態頁共用）。 */
export function platformStatusLabel(status: Assessment["status"]): string {
  switch (status) {
    case "supported":
      return "正式支援";
    case "unsupported":
      return "非正式支援";
    default:
      return "未知";
  }
}

type PageState = { state: "loading" } | { state: "ready"; platform: PlatformResponse } | { state: "failed"; reason: string };

interface Props {
  client: RuntimeClient;
}

/** 平台狀態頁：顯示 OS 診斷資訊與支援判定；非正式支援只警告，不阻擋使用（DR-20261021-08）。 */
export function PlatformPage({ client }: Props) {
  const [page, setPage] = useState<PageState>({ state: "loading" });

  useEffect(() => {
    let cancelled = false;
    client
      .getPlatform()
      .then((platform) => !cancelled && setPage({ state: "ready", platform }))
      .catch((error) => !cancelled && setPage({ state: "failed", reason: error instanceof Error ? error.message : String(error) }));
    return () => {
      cancelled = true;
    };
  }, [client]);

  return (
    <section className="section platform-page" aria-labelledby="platform-title">
      <div className="section-heading">
        <h3 id="platform-title">平台狀態</h3>
        {page.state === "ready" && (
          <span
            className={`badge platform-status platform-${page.platform.assessment.status} ${
              page.platform.assessment.status === "supported" ? "badge-success" : "badge-warning"
            }`}
          >
            {platformStatusLabel(page.platform.assessment.status)}
          </span>
        )}
        {/* 與並排的崩潰恢復一樣有一行說明，兩個面板上緣對齊 */}
        <p>Windows 版本與支援判定；非正式支援只會警告，不會阻擋使用、派工、交接或恢復。</p>
      </div>
      {page.state === "loading" && <p className="empty-note">平台檢查中…</p>}
      {page.state === "failed" && <p className="callout callout-danger">無法取得平台狀態：{page.reason}</p>}
      {page.state === "ready" && <PlatformDetails platform={page.platform} />}
    </section>
  );
}

/** 一則平台警告：長警示（只有色條與狀態詞上色）；第一句完整顯示，其餘收進「詳細資訊」。 */
function PlatformWarning({ message }: { message: string }) {
  const { summary, rest } = splitFirstSentence(message);
  return (
    <div role="alert" className="notice notice-warning platform-warning">
      <p className="notice-title">平台警告</p>
      <p>{summary}</p>
      {rest !== "" && (
        <details>
          <summary>詳細資訊</summary>
          <p>{rest}</p>
        </details>
      )}
    </div>
  );
}

function PlatformDetails({ platform }: { platform: PlatformResponse }) {
  const { assessment } = platform;
  return (
    <>
      {assessment.issues.map((issue) => (
        <PlatformWarning key={issue.code} message={issue.message} />
      ))}
      <div className="panel">
        <dl className="facts platform-facts">
          <dt>OS build</dt>
          <dd className="tabular">{platform.platform.os_build}</dd>
          <dt>功能版本</dt>
          <dd>{platform.platform.feature_version}</dd>
          <dt>版本別</dt>
          <dd>{platform.platform.edition}</dd>
          <dt>原生架構</dt>
          <dd data-field="native-arch">{platform.platform.native_arch}</dd>
          <dt>程序架構</dt>
          <dd>{platform.platform.process_arch}</dd>
          <dt>App 版本</dt>
          <dd className="mono">{platform.app_version}</dd>
          {assessment.support_ends_on && (
            <>
              {/* 標籤欄放不下時在空白處換行，不在「服務結束日」中間拆開 */}
              <dt>
                Microsoft <span className="nowrap">服務結束日</span>
              </dt>
              <dd className="tabular">{assessment.support_ends_on}</dd>
            </>
          )}
        </dl>
        <p className="meta platform-source">
          <span className="copy-text">
            {/* 標籤不縮小，寬度不夠時只截斷網址 */}
            <span className="no-shrink">支援期限資料來源：</span>
            <span className="mono truncate" title={platform.support_table_source}>
              {docsHost(platform.support_table_source)}
            </span>
            <CopyButton text={platform.support_table_source} label="複製網址" />
          </span>
          <span className="tabular">查閱日期 {platform.support_table_checked_on}</span>
        </p>
      </div>
    </>
  );
}
