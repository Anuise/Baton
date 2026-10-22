import { Fragment } from "react";

import {
  CONTRACT_FIELDS,
  type Capability,
  type ContractField,
  type ContractFieldName,
  type ContractInput,
  type ContractReference,
  type TaskContract,
  type VerificationCheck,
} from "./runtimeClient";

export const FIELD_LABELS: Record<ContractFieldName, string> = {
  objective: "目標",
  inputs: "輸入與依據",
  scope: "範圍與限制",
  deliverables: "預期成果",
  done_criteria: "完成與驗證條件",
  blocker_handling: "阻礙處理",
};

type Mode = "missing" | "filled" | "not_applicable" | "unknown";

const MODE_LABELS: Record<Mode, string> = {
  missing: "未提供",
  filled: "填寫",
  not_applicable: "不適用",
  unknown: "未知",
};

/** 這兩項是派工的底線，不能標為不適用。 */
const NOT_APPLICABLE_FORBIDDEN = new Set<ContractFieldName>(["objective", "done_criteria"]);

const REFERENCE_KINDS = ["task_contract", "goal", "authorization"] as const;

interface FieldDraft {
  mode: Mode;
  text: string;
  /** 每行一個引用：`kind:target@version`。 */
  references: string;
  reason: string;
  note: string;
}

export interface ContractDraft {
  fields: Record<ContractFieldName, FieldDraft>;
  capabilities: Capability[];
  /** 每行一項確定性檢查，格式見 `CHECK_FORMATS`。 */
  checks: string;
}

const emptyField = (): FieldDraft => ({ mode: "missing", text: "", references: "", reason: "", note: "" });

export function emptyDraft(): ContractDraft {
  const fields = {} as Record<ContractFieldName, FieldDraft>;
  for (const name of CONTRACT_FIELDS) fields[name] = emptyField();
  return { fields, capabilities: [], checks: "" };
}

const referenceText = ({ kind, target, version }: ContractReference) => `${kind}:${target}@${version}`;

function fieldDraftOf(field: ContractField | null): FieldDraft {
  if (field === null) return emptyField();
  switch (field.status) {
    case "filled":
      return { ...emptyField(), mode: "filled", text: field.text, references: (field.references ?? []).map(referenceText).join("\n") };
    case "not_applicable":
      return { ...emptyField(), mode: "not_applicable", reason: field.reason };
    case "unknown":
      return { ...emptyField(), mode: "unknown", note: field.note ?? "" };
  }
}

/** 編輯既有 Task 時，載入它目前的契約。 */
export function draftFromContract(contract: TaskContract): ContractDraft {
  const draft = emptyDraft();
  for (const name of CONTRACT_FIELDS) draft.fields[name] = fieldDraftOf(contract[name]);
  draft.capabilities = [...contract.required_capabilities];
  draft.checks = contract.verification_checks.map(checkText).join("\n");
  return draft;
}

const CHECK_FORMATS = "command: 程式 引數…、command[結束碼]: 程式 引數…、file: 相對路徑 [sha256]、artifact: 成果名稱 [sha256]";

/** 檢查的文字格式；與 `parseCheck` 互為反向。 */
function checkText(check: VerificationCheck): string {
  switch (check.kind) {
    case "command": {
      const prefix = check.expected_exit_code === 0 ? "command" : `command[${check.expected_exit_code}]`;
      return `${prefix}: ${[check.program, ...check.args].join(" ")}`;
    }
    case "file":
      return `file: ${check.path}${check.sha256 ? ` ${check.sha256}` : ""}`;
    case "artifact":
      return `artifact: ${check.name}${check.sha256 ? ` ${check.sha256}` : ""}`;
  }
}

/** 解析一行確定性檢查；以空白分隔（不支援引號），格式不符時回傳錯誤說明。 */
function parseCheck(line: string): VerificationCheck | { error: string } {
  const match = /^(command|file|artifact)(?:\[(-?\d+)\])?:(.*)$/.exec(line);
  if (!match) return { error: `格式錯誤「${line}」，應為 ${CHECK_FORMATS}` };
  const [, kind, exitCode, rest] = match;
  const tokens = rest.split(/\s+/).filter((token) => token !== "");
  if (kind === "command") {
    if (tokens.length === 0) return { error: `「${line}」缺少要執行的程式` };
    const [program, ...args] = tokens;
    return { kind: "command", program, args, expected_exit_code: exitCode === undefined ? 0 : Number(exitCode) };
  }
  const targetLabel = kind === "file" ? "相對路徑" : "成果名稱";
  if (exitCode !== undefined) return { error: `「${line}」只有 command 可以指定結束碼` };
  if (tokens.length === 0 || tokens.length > 2) {
    return { error: `「${line}」應為 ${kind}: ${targetLabel} [sha256]（${targetLabel}不可含空白）` };
  }
  const [target, hash] = tokens;
  // PowerShell 的 Get-FileHash 輸出大寫，這裡一律轉成 Runtime 要求的小寫
  if (hash !== undefined && !/^[0-9a-fA-F]{64}$/.test(hash)) return { error: `「${line}」的 sha256 應為 64 位十六進位` };
  const sha256 = hash === undefined ? null : hash.toLowerCase();
  return kind === "file" ? { kind: "file", path: target, sha256 } : { kind: "artifact", name: target, sha256 };
}

function parseReference(line: string): ContractReference | null {
  const match = /^(task_contract|goal|authorization):(.+)@(\d+)$/.exec(line);
  if (!match) return null;
  const version = Number(match[3]);
  const target = match[2].trim();
  return version >= 1 && target !== "" ? { kind: match[1] as ContractReference["kind"], target, version } : null;
}

const nonEmptyLines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");

/** 把畫面草稿轉成請求；有任何錯誤就不送出，錯誤逐項列出。 */
export function buildContract(draft: ContractDraft): { contract: ContractInput; errors: string[] } {
  const contract: ContractInput = {};
  const errors: string[] = [];
  for (const name of CONTRACT_FIELDS) {
    const label = FIELD_LABELS[name];
    const field = draft.fields[name];
    if (field.mode === "filled") {
      if (field.text.trim() === "") errors.push(`${label}：填寫模式需要內容`);
      const references: ContractReference[] = [];
      for (const line of nonEmptyLines(field.references)) {
        const reference = parseReference(line);
        if (reference) references.push(reference);
        else errors.push(`${label}：引用格式錯誤「${line}」，應為 kind:target@version（kind 為 ${REFERENCE_KINDS.join("／")}）`);
      }
      contract[name] = { status: "filled", text: field.text.trim(), ...(references.length > 0 && { references }) };
    } else if (field.mode === "not_applicable") {
      if (field.reason.trim() === "") errors.push(`${label}：不適用必須填寫理由`);
      contract[name] = { status: "not_applicable", reason: field.reason.trim() };
    } else if (field.mode === "unknown") {
      contract[name] = { status: "unknown", ...(field.note.trim() !== "" && { note: field.note.trim() }) };
    }
  }
  contract.required_capabilities = draft.capabilities;
  const checks: VerificationCheck[] = [];
  for (const line of nonEmptyLines(draft.checks)) {
    const parsed = parseCheck(line);
    if ("error" in parsed) errors.push(`確定性檢查：${parsed.error}`);
    else checks.push(parsed);
  }
  // 省略即為空陣列：沒有檢查時不送這個欄位
  if (checks.length > 0) contract.verification_checks = checks;
  return { contract, errors };
}

interface Props {
  draft: ContractDraft;
  onChange: (draft: ContractDraft) => void;
  capabilities: { capability: Capability; label: string }[];
}

/** 六項最小派工契約、確定性檢查與所需能力的編輯器；建立與編輯 Task 共用。 */
export function ContractEditor({ draft, onChange, capabilities }: Props) {
  const change = (name: ContractFieldName, patch: Partial<FieldDraft>) =>
    onChange({ ...draft, fields: { ...draft.fields, [name]: { ...draft.fields[name], ...patch } } });

  const toggle = (capability: Capability, enabled: boolean) =>
    onChange({
      ...draft,
      capabilities: enabled ? [...draft.capabilities, capability] : draft.capabilities.filter((c) => c !== capability),
    });

  return (
    <div className="contract-editor">
      {CONTRACT_FIELDS.map((name) => {
        const label = FIELD_LABELS[name];
        const field = draft.fields[name];
        const modes = (Object.keys(MODE_LABELS) as Mode[]).filter(
          (mode) => mode !== "not_applicable" || !NOT_APPLICABLE_FORBIDDEN.has(name),
        );
        return (
          <Fragment key={name}>
            <fieldset>
              <legend>{label}</legend>
              <label>
                {label}：模式
                <select value={field.mode} onChange={(e) => change(name, { mode: e.target.value as Mode })}>
                  {modes.map((mode) => (
                    <option key={mode} value={mode}>
                      {MODE_LABELS[mode]}
                    </option>
                  ))}
                </select>
              </label>
              {field.mode === "filled" && (
                <>
                  <label>
                    {label}：內容
                    <textarea value={field.text} onChange={(e) => change(name, { text: e.target.value })} />
                  </label>
                  <label>
                    {label}：引用（每行 kind:target@version）
                    <textarea value={field.references} onChange={(e) => change(name, { references: e.target.value })} />
                  </label>
                </>
              )}
              {field.mode === "not_applicable" && (
                <label>
                  {label}：不適用的理由
                  <input value={field.reason} onChange={(e) => change(name, { reason: e.target.value })} />
                </label>
              )}
              {field.mode === "unknown" && (
                <label>
                  {label}：未知的說明（選填）
                  <input value={field.note} onChange={(e) => change(name, { note: e.target.value })} />
                </label>
              )}
            </fieldset>
            {name === "done_criteria" && (
              <fieldset>
                <legend>確定性檢查</legend>
                <p>
                  Worker 成功結束後，Runtime 會執行這些檢查，全部通過才自動完成；也可以在驗證中手動接受成果。每行一項：
                  {CHECK_FORMATS}（以空白分隔，不支援引號）。
                </p>
                <label>
                  確定性檢查（每行一項）
                  <textarea value={draft.checks} onChange={(e) => onChange({ ...draft, checks: e.target.value })} />
                </label>
              </fieldset>
            )}
          </Fragment>
        );
      })}
      <fieldset>
        <legend>所需能力</legend>
        {capabilities.map(({ capability, label }) => (
          <label key={capability}>
            <input
              type="checkbox"
              checked={draft.capabilities.includes(capability)}
              onChange={(e) => toggle(capability, e.target.checked)}
            />
            {label}
          </label>
        ))}
      </fieldset>
    </div>
  );
}
