# 46: Web 驗證 Adapter（Playwright）

Status: in_progress（Work-8，run R134-46-impl，2026-10-23 01:12）
Blocked by: 45

**What to build:** 實作 web Verification Adapter：以 Playwright 為基礎執行操作與程式斷言，需要語意判斷時使用受限的 judge（輸入取自當前畫面與固定預期，不採信行動者敘述）；秘密只以參照傳給 Runner、不進 Prompt；截圖、trace、錄影保存為 Artifact 並依政策遮蔽。tester-army/e2e 作為候選引擎，整合前驗證授權與契約，可延後。

**Blocked by:** 45 Verification Profile 與 CLI／API 驗證 Adapter

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 模型限流不被記為產品缺陷
- [ ] 秘密不出現在 Prompt 或證據中
- [ ] 測試 persona 與實際登入身分分開綁定
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
