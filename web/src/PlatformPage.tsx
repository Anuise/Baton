import { useEffect, useState } from "react";

import { CopyButton } from "./LongText";
import type { PlatformResponse, RuntimeClient } from "./runtimeClient";
import { docsHost } from "./textFormat";

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
    <section className="platform-page" aria-labelledby="platform-title">
      <h2 id="platform-title">平台狀態</h2>
      {page.state === "loading" && <p>平台檢查中…</p>}
      {page.state === "failed" && <p>無法取得平台狀態：{page.reason}</p>}
      {page.state === "ready" && <PlatformDetails platform={page.platform} />}
    </section>
  );
}

function PlatformDetails({ platform }: { platform: PlatformResponse }) {
  const { assessment } = platform;
  return (
    <>
      <p className={`platform-status platform-${assessment.status}`}>
        <span className={`badge ${assessment.status === "supported" ? "badge-success" : "badge-warning"}`}>
          {platformStatusLabel(assessment.status)}
        </span>
      </p>
      {assessment.issues.map((issue) => (
        <p key={issue.code} role="alert" className="platform-warning">
          {issue.message}
        </p>
      ))}
      {assessment.status !== "supported" && <p>平台警告不會阻擋使用，也不會阻擋派工、交接或恢復。</p>}
      <dl className="platform-facts settings-facts">
        <dt>OS build</dt>
        <dd>{platform.platform.os_build}</dd>
        <dt>功能版本</dt>
        <dd>{platform.platform.feature_version}</dd>
        <dt>版本別</dt>
        <dd>{platform.platform.edition}</dd>
        <dt>原生架構</dt>
        <dd data-field="native-arch">{platform.platform.native_arch}</dd>
        <dt>程序架構</dt>
        <dd>{platform.platform.process_arch}</dd>
        <dt>App 版本</dt>
        <dd>{platform.app_version}</dd>
        {assessment.support_ends_on && (
          <>
            {/* 標籤欄放不下時在空白處換行，不在「服務結束日」中間拆開 */}
            <dt>
              Microsoft <span className="nowrap">服務結束日</span>
            </dt>
            <dd>{assessment.support_ends_on}</dd>
          </>
        )}
      </dl>
      <p className="platform-source section-hint">
        支援期限資料來源：
        <span className="mono" title={platform.support_table_source}>
          {docsHost(platform.support_table_source)}
        </span>
        <CopyButton text={platform.support_table_source} label="複製網址" />
        <span className="platform-checked-on">（查閱日期 {platform.support_table_checked_on}）</span>
      </p>
    </>
  );
}
