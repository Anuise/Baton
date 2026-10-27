# 02: 建立 GLOSSARY.md 與既有決策的 ADR

Status: completed
Blocked by: None

**What to build:** 依 docs/agents/domain.md，在 repo 根目錄建立 GLOSSARY.md，收錄 spec 與 issue #3 的領域詞彙（Workspace、Goal、Task、Run、Session、Artifact、Decision Request、Decision、Lesson、Guard、Controller、Worker、Runtime、Workflow、Stage、Gate、Playbook、Verification Profile、Evidence Bundle、handoff、checkpoint、draining 等）及關係；在 docs/adr/ 為 DR-20261021-01～08、DR-20261022-01～03 及 spec 的 D-04（測試 seam）、D-05（MCP 工具介面）各建一份 ADR。只寫文件，不寫程式。

**Blocked by:** None（可立即開始）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] GLOSSARY.md 每個詞有定義、關係與「避免使用的同義詞」，不混入執行進度
- [x] 每份 ADR 含背景、決策、替代方案、後果，並連回 issue #3 或 spec 對應段落
- [x] spec 中標示的 Claude 決策若寫成 ADR，明確標示為「AI 暫定，待使用者確認」

## Comments
