import {
  CONTRACT_FIELDS,
  type Capability,
  type ContractField,
  type ContractFieldName,
  type ContractInput,
  type ContractReference,
  type TaskContract,
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
}

const emptyField = (): FieldDraft => ({ mode: "missing", text: "", references: "", reason: "", note: "" });

export function emptyDraft(): ContractDraft {
  const fields = {} as Record<ContractFieldName, FieldDraft>;
  for (const name of CONTRACT_FIELDS) fields[name] = emptyField();
  return { fields, capabilities: [] };
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
  return draft;
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
  return { contract, errors };
}

interface Props {
  draft: ContractDraft;
  onChange: (draft: ContractDraft) => void;
  capabilities: { capability: Capability; label: string }[];
}

/** 六項最小派工契約與所需能力的編輯器；建立與編輯 Task 共用。 */
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
          <fieldset key={name}>
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
