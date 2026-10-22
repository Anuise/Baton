# 42: Project Brief、Glossary 與 Project Map

Status: completed
Blocked by: 17, 37

**What to build:** Workspace 首次接入專案時，Controller 透過工具建立並維護附來源的 Project Brief、Glossary 與 Project Map；每項描述附來源位置、版本與查驗時間，未確認關係標為假說。repo 內的 GLOSSARY／ADR 作為帶版本的輸出投影，偵測外部編輯時重新協調。App 新增專案理解視圖。

**Blocked by:** 17 Controller 拆工與派工工具、漸進拆工；37 知識、教訓與全文檢索

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 未知資訊不被寫成已確認事實
- [x] 投影檔被外部修改時產生協調事件而非靜默覆寫
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
