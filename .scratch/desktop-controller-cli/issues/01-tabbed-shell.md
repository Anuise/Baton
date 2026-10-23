# 01: 桌面 App 頁簽外殼

Status: completed
Blocked by: None（可立即開始）

**What to build:** 把桌面 App 改成頁簽式：頂層 `Workspace`｜`帳號`｜`設定`；選中 Workspace 後主區以九個頁簽（Controller、工作、裁定、成果、審查與驗證、知識、流程、時間線、授權）一次只顯示一個功能。左側 Workspace 清單只負責選擇 Workspace。現有各功能元件原樣放進對應頁簽，只調整外層容器、標題層級與間距。Controller 頁簽本票先放佔位內容（票 03 填入）；舊的「對話」暫時保留為第十個頁簽，由票 08 移除。User stories 1～17。

**Blocked by:** None（可立即開始）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [x] 頂層三個頁簽；任何時候主區只顯示一個功能，不再把帳號、排程、更新、備份等設定往下疊
- [x] Workspace 下有 Controller、工作、裁定、成果、審查與驗證（內含審查／驗證／探索小頁簽）、知識（知識＋專案理解）、流程、時間線、授權（授權＋模型覆寫）頁簽
- [x] `帳號` 頁簽含帳號、排程、共同設定；`設定` 頁簽含跨登入啟動、更新、備份、恢復、平台狀態
- [x] 裁定頁簽與左側 Workspace 名稱旁顯示待裁定數徽章，回答裁定後更新
- [x] 切換 Workspace 時停在同一頁簽；新選 Workspace 預設 Controller 頁簽；未選 Workspace 時顯示提示文字
- [x] 重新開啟 App 回到上次的頂層頁簽、Workspace 與頁簽（本機儲存讀不到時用預設值，不報錯）
- [x] 沒有可用 Account Profile 時首次精靈以全畫面顯示；狀態列、更新通知、完全退出進度不受頁簽影響
- [x] 既有各頁面的元件測試仍全部通過；新增 App 外殼的元件測試涵蓋上述行為
- [x] AI 決策已追加至 ai-decisions.md

## Comments
