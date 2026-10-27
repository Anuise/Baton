---
name: product-design-decision
description: 對 Issue Orchestrator 已選定的具體產品設計決策逐輪比較方案、檢查跨模組一致性及驗收完整度。由探索階段選定決策後觸發，亦適用本流程的 decision、review 與候選方案回覆；未選範圍時先探索。
---

# 逐輪決策審查

先讀 [共用契約](../product-design-review/references/context.md)，取得本回合快照與使用者已選決策。若只有領域／模組，讀取 [探索 skill](../product-design-discovery/SKILL.md) 補足選擇；不自行指定問題。

直接指定已 Synced 的 Decision ID 時先呈現現況，不自動重開審查。只有使用者明確要求重審，或有共用契約列出的重審事由時，才建立候選修訂；保留既有已核准版本與 ID，候選版本另追蹤狀態。

## 每輪審查

1. 明示 Domain／Module／Decision ID、來源、目前定義、重要性、依賴與已知條件。
2. 找出模糊定義、缺漏、隱含假設、需求必要性、責任重疊、邊界／失敗流程、維護擴充成本、過度設計及商業交付風險。
3. 至少比較兩個合理方案；只有一個可行時，說明其他選項不可行的原因。以表格比較機制、優缺點、架構影響、實作／維運成本、可靠性、安全性、擴充性及適用階段，聚焦真正相關的差異。
4. 提出清楚的推薦、理由與代價。若使用者偏好有重大問題，提出有證據的反對理由，保留其產品裁定權。
5. 只提出一個核心決策問題，接受採用方案、自訂、暫定、保持原設計、延後、補資料或導航，等待回答；回答僅確認其實際涵蓋範圍。
6. 顯示當前狀態與距離 Ready for Approval 尚缺的條件。有候選方案時，同回合執行下述一致性檢查，不以補問無關問題延長流程。

## 候選決策的一致性檢查

逐項檢查與已確認產品原則、同模組決策、其他領域、API／資料模型／狀態機、Runtime／Agent 生命週期、第三方整合、安全／可靠性／資料、安裝／部署／升級、測試驗收、既有決策有效性、MVP／版本邊界及關聯模組重審需求的關係。

各影響分類為 Compatible、Extension、Revision Required、Conflict 或 Breaking Change。衝突需附來源、影響程度、可行解法、建議及是否需重新決策。需變更其他模組時列為待核准修訂，不能默默改寫；阻塞衝突未解決時不轉入同步。

## 完成度關卡

僅在下列條件全部有明確內容或有理由的不適用說明時，標示 Ready for Approval：目標與非目標清楚、合理替代方案已比較、正常及主要失敗流程已定義、責任與架構契約明確、一致性已檢查、重大風險已處理或明確接受、驗收可客觀測試、遺留問題不阻塞。

仍需討論時保持 In Discussion；需研究／實驗／PoC 時為 Validation Required，列出驗證問題與成功判準，不將假設當成事實。

達標時依 [Decision Record](../product-design-review/references/decision-record.md) 整理完整記錄，接著實際讀取 [同步 skill](../product-design-sync/SKILL.md) 製作修改預覽。這次自動銜接只到預覽與授權關卡，不包含寫入。導航與狀態交接見 [入口](../product-design-review/SKILL.md)。
