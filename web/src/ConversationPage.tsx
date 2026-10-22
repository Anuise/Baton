import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import type { ConversationClient, ConversationTurn, ConversationTurnStatus, ConversationView, Workspace } from "./runtimeClient";

/** 定期重新讀取對話的間隔：Controller 的回覆與輸入狀態由 Runtime 推進。 */
export const CONVERSATION_REFRESH_INTERVAL_MS = 3000;

interface Props {
  client: ConversationClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 重新讀取的間隔（毫秒）；只有測試會指定。 */
  pollMs?: number;
}

const KIND_LABELS: Record<ConversationTurn["kind"], string> = {
  user_message: "你",
  controller_reply: "Controller",
  work_event: "工作事件",
};

const STATUS_LABELS: Record<ConversationTurnStatus, string> = {
  pending: "已收到／待處理",
  processing: "處理中",
  processed: "已處理",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Workspace 的「對話」：與 Controller 的長期對話（票 16）。
 * 同一對話同時最多一輪 Controller；忙碌時新訊息先保存為待處理，本輪結束後依序處理。
 */
export function ConversationPage({ client, workspace, timeZone, pollMs = CONVERSATION_REFRESH_INTERVAL_MS }: Props) {
  const [view, setView] = useState<ConversationView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  // 只採用最後發出的讀取結果，避免較早的輪詢回應蓋掉剛送出的訊息
  const latest = useRef(0);

  const load = useCallback(async () => {
    const request = ++latest.current;
    try {
      const loaded = await client.getConversation(workspace.id);
      if (request !== latest.current) return;
      setView(loaded);
      setLoadError(null);
    } catch (e) {
      if (request === latest.current) setLoadError(reasonOf(e));
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setView(null);
    setLoadError(null);
    void load();
    const timer = setInterval(() => void load(), pollMs);
    return () => clearInterval(timer);
  }, [load, pollMs]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (draft.trim() === "") {
      setSendError("請輸入訊息");
      return;
    }
    setSending(true);
    setSendError(null);
    try {
      const { turn } = await client.sendConversationMessage(workspace.id, draft);
      setDraft("");
      // 先把新訊息顯示為待處理，再重新讀取；進行中的輪詢結果作廢
      latest.current += 1;
      setView((current) =>
        current === null ? current : { ...current, conversation_id: turn.conversation_id, turns: [...current.turns, turn] },
      );
      await load();
    } catch (e) {
      // 保留輸入，讓使用者修正後重送
      setSendError(reasonOf(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <section className="conversation-page" aria-label="對話">
      <h2>對話：{workspace.name}</h2>
      <p className="conversation-hint">暫停與取消請在「工作面板」操作，會直接生效，不會排在對話佇列之後。</p>
      {view === null && loadError === null && <p>載入中…</p>}
      {loadError && <p role="alert">無法取得對話：{loadError}</p>}
      {view && view.busy && (
        <p className="conversation-busy" role="status">
          Controller 正在處理上一輪，新訊息會在本輪結束後依序處理
        </p>
      )}
      {view && !view.busy && view.next_wake_at !== null && (
        <p className="conversation-wake" role="status">
          待處理的輸入預計於 {formatTimestamp(view.next_wake_at, timeZone)} 交給 Controller
        </p>
      )}
      {view && view.turns.length === 0 && <p>還沒有對話，輸入訊息開始與 Controller 對話</p>}
      {view && view.turns.length > 0 && (
        <ol className="conversation-turns">
          {view.turns.map((turn) => (
            <TurnItem key={turn.id} turn={turn} timeZone={timeZone} />
          ))}
        </ol>
      )}
      <form className="conversation-form" onSubmit={(event) => void submit(event)}>
        <label>
          訊息
          <textarea value={draft} onChange={(e) => setDraft(e.target.value)} />
        </label>
        <button type="submit" disabled={sending}>
          送出
        </button>
        {sendError && <p role="alert">{sendError}</p>}
      </form>
    </section>
  );
}

/** 一筆輸入或回覆；工作事件以系統通知的樣式顯示，使用者訊息附上處理狀態。 */
function TurnItem({ turn, timeZone }: { turn: ConversationTurn; timeZone?: string }) {
  const label = KIND_LABELS[turn.kind];
  return (
    <li className={`conversation-turn conversation-${turn.kind}`} aria-label={`${label} #${turn.seq}`}>
      <p className="conversation-meta">
        <strong>{label}</strong>
        <span>{formatTimestamp(turn.created_at, timeZone)}</span>
        {turn.kind === "work_event" && turn.source !== null && <span>來源：{turn.source}</span>}
        {turn.kind === "user_message" && (
          <span className={`conversation-status conversation-status-${turn.status}`}>{STATUS_LABELS[turn.status]}</span>
        )}
        {turn.kind === "user_message" && turn.attempts > 1 && (
          <span className="conversation-status">重新交付（第 {turn.attempts} 次）</span>
        )}
      </p>
      <p className="conversation-content">{turn.content}</p>
    </li>
  );
}
