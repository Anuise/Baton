# 08: 共同設定匯入與版本化快照（S3）

Status: completed
Blocked by: 07

**What to build:** 從使用者既有 Claude Code 設定匯入共同基準（偏好、指示、hooks、MCP、plugins），顯示來源與差異，使用者採用後建立不可變設定版本；不改寫使用者平常手動使用的設定，不複製任何登入資料。為 Run 准入提供「取得某版本快照並寫成 Run 專屬唯讀設定目錄」的能力（spec D-09）。

**Blocked by:** 07 帳號 Profile 與首次啟動精靈

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 每次採用建立新版本，舊版本內容不被修改
- [x] 匯入結果不含任何登入秘密（以測試掃描驗證）
- [x] 可為指定 Run 產生唯讀的設定快照目錄，內容與版本雜湊一致
- [x] 使用者原始設定檔在匯入前後位元組相同
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
