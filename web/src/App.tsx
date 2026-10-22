import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { BackupSettings } from "./BackupSettings";
import { ConversationPage } from "./ConversationPage";
import { ModelOverridesPanel } from "./ModelOverridesPanel";
import { DecisionCenter } from "./DecisionCenter";
import { ExitProgress } from "./ExitProgress";
import { KnowledgePage } from "./KnowledgePage";
import { LoginStartupSettings } from "./LoginStartupSettings";
import { PlatformPage } from "./PlatformPage";
import { ProjectUnderstandingPage } from "./ProjectUnderstandingPage";
import { RecoveryPanel } from "./RecoveryPanel";
import { SchedulingSettings } from "./SchedulingSettings";
import { SetupWizard } from "./SetupWizard";
import { SharedSettingsPage } from "./SharedSettingsPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type PendingDecisions, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { TimelinePage } from "./TimelinePage";
import { UpdateNotice } from "./UpdateNotice";
import { UpdateSettings } from "./UpdateSettings";
import { VerificationPage } from "./VerificationPage";
import { WorkflowPage } from "./WorkflowPage";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 使用者選取某個 Workspace 後，主畫面顯示它的授權與模型設定、對話、工作面板、裁定中心、成果與驗證、知識、時間線、專案理解、流程或驗證。 */
type Opened = {
  view:
    | "authorization"
    | "conversation"
    | "tasks"
    | "decisions"
    | "artifacts"
    | "knowledge"
    | "timeline"
    | "understanding"
    | "workflows"
    | "verification";
  workspace: Workspace;
};

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);
  const [opened, setOpened] = useState<Opened | null>(null);
  // 待裁定數由狀態列輪詢；回答裁定後要求它立即重新讀取
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  const [pendingVersion, setPendingVersion] = useState(0);
  const pendingByWorkspace = Object.fromEntries((pending?.by_workspace ?? []).map((w) => [w.workspace_id, w.pending_count]));

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} onPending={setPending} refreshKey={pendingVersion} /> : <header className="status-bar" />}
      <div className="body">
        {client && (
          <WorkspaceSidebar
            client={client}
            onSelect={(workspace) => setOpened({ view: "authorization", workspace })}
            onOpenConversation={(workspace) => setOpened({ view: "conversation", workspace })}
            onOpenTasks={(workspace) => setOpened({ view: "tasks", workspace })}
            onOpenDecisions={(workspace) => {
              setOpened({ view: "decisions", workspace });
              setPendingVersion((version) => version + 1);
            }}
            pendingDecisions={pendingByWorkspace}
            onOpenArtifacts={(workspace) => setOpened({ view: "artifacts", workspace })}
            onOpenKnowledge={(workspace) => setOpened({ view: "knowledge", workspace })}
            onOpenTimeline={(workspace) => setOpened({ view: "timeline", workspace })}
            onOpenUnderstanding={(workspace) => setOpened({ view: "understanding", workspace })}
            onOpenWorkflows={(workspace) => setOpened({ view: "workflows", workspace })}
            onOpenVerification={(workspace) => setOpened({ view: "verification", workspace })}
          />
        )}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {/* 有新版本時只通知；更新維護中以覆蓋層顯示進度（spec D-19） */}
          {client && <UpdateNotice client={client} runtimeStartedAt={health?.started_at ?? null} />}
          {client && <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <SchedulingSettings client={client} />}
          {client && <LoginStartupSettings client={client} />}
          {client && <UpdateSettings client={client} />}
          {client && <SharedSettingsPage client={client} />}
          {client && <BackupSettings client={client} />}
          {client && <RecoveryPanel client={client} />}
          {client && opened?.view === "authorization" && <AuthorizationPanel client={client} workspace={opened.workspace} />}
          {client && opened?.view === "authorization" && <ModelOverridesPanel client={client} workspace={opened.workspace} />}
          {client && opened?.view === "conversation" && <ConversationPage client={client} workspace={opened.workspace} />}
          {client && opened?.view === "tasks" && <TaskBoard client={client} workspace={opened.workspace} />}
          {client && opened?.view === "decisions" && (
            <DecisionCenter
              client={client}
              workspace={opened.workspace}
              onChange={() => setPendingVersion((version) => version + 1)}
            />
          )}
          {client && opened?.view === "artifacts" && <ArtifactsPage client={client} workspace={opened.workspace} />}
          {client && opened?.view === "knowledge" && <KnowledgePage client={client} workspace={opened.workspace} />}
          {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 的 Task 過濾 */}
          {client && opened?.view === "timeline" && (
            <TimelinePage key={opened.workspace.id} client={client} workspace={opened.workspace} />
          )}
          {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 的協調提示與錯誤 */}
          {client && opened?.view === "understanding" && (
            <ProjectUnderstandingPage key={opened.workspace.id} client={client} workspace={opened.workspace} />
          )}
          {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 的跳過原因與錯誤 */}
          {client && opened?.view === "workflows" && (
            <WorkflowPage key={opened.workspace.id} client={client} workspace={opened.workspace} />
          )}
          {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 正在編輯的 Profile 與訊息 */}
          {client && opened?.view === "verification" && (
            <VerificationPage key={opened.workspace.id} client={client} workspace={opened.workspace} />
          )}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
      {/* 系統匣「完全退出」的停止進度，以覆蓋層顯示 */}
      {client && <ExitProgress client={client} />}
    </div>
  );
}
