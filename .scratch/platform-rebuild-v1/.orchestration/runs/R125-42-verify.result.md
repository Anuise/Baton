R123 回報的缺陷已修好，三條驗收條件都通過，建置與全部測試也都通過，判 PASS。驗證全程唯讀，工作樹沒有變動。

```
VERDICT: PASS
CRITERIA:
- [PASS] 未知資訊不被寫成已確認事實 —
  · Domain 擋住：沒有來源就標 confirmed，回 UnverifiedClaim（crates/baton-domain/src/understanding.rs:283）。
  · 資料表也擋：crates/baton-store/src/lib.rs:1366 有 CHECK (status <> 'confirmed' OR json_array_length(sources) > 0)。
  · 情境測試 unknown_information_is_never_recorded_or_projected_as_a_confirmed_fact 與 a_confirmed_item_whose_source_file_changes_is_flagged_for_recheck 通過。
  · 本次修正 80e84e5 沒有動到這條的程式路徑。
- [PASS] 投影檔被外部修改時產生協調事件而非靜默覆寫 —
  · 原本的協調流程沒變，用 projection_lock 序列化：比對 → 開啟協調 → 在同一個交易中通知 Controller（crates/baton-api/src/understanding.rs:298-317）。
  · R123 的缺陷已修：
    - 保留外部版本（detached）的檔案，重新比對時只把目前的雜湊記到 external_hash，revision 加 1；不寫檔、不記事件、不通知（crates/baton-domain/src/understanding.rs:464-471）。
    - 選「保留外部版本」時，也把當下的雜湊記到 external_hash（:608）。
    - UI 覆寫一律送 external_hash（web/src/ProjectUnderstandingPage.tsx:243）。
    - 覆寫時仍要核對雜湊，舊的雜湊照樣被拒絕（:584）。
  · 新的情境測試 a_kept_external_glossary_edited_again_can_still_be_overwritten_with_the_hash_from_the_view 通過（crates/baton-harness/tests/project_understanding.rs:387-430），涵蓋：
    - 保留外部版本後再編輯檔案，sync 不會寫檔。
    - Controller 只收到最初那一次通知。
    - 用舊的雜湊覆寫回 409 projection_changed，檔案不變。
    - 用視圖提供的雜湊覆寫回 200，狀態變 in_sync，檔案變成 Baton 版本。
  · Domain 單元測試 a_detached_file_edited_again_is_not_written_or_reported_but_its_current_hash_is_tracked_for_overwrite 通過，也涵蓋檔案被刪除時 external_hash 記成 None。
  · 前端測試的斷言已改成送 external_hash（web/src/ProjectUnderstandingPage.test.tsx:189-220）。
- [PASS] 以 Runtime harness（spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md —
  · crates/baton-harness/tests/project_understanding.rs 共 9 個測試，全部通過。
  · ai-decisions.md 有 AI-362～AI-369，加上這次的 AI-370（重要度：中）（:3334），每條都標了重要度。
  · docs/development.md:1132 與 :1140 已同步改成 external_hash 的新語意。
TESTS:
- 前置條件：`git merge-base --is-ancestor rebuild/v1 HEAD` 成立，驗證開始和結束時都檢查過，rebuild/v1 是 e3ae2cb。
- `pnpm build` → exit 0：release 版 Runtime、Web、Tauri（baton-desktop.exe）都建好。
  · 唯一的 warning 是 unused import `StoreError`，在 crates/baton-api/src/shared_settings.rs:20。這個檔和 rebuild/v1 相同，不是本票造成的。
- `pnpm test` → exit 0：
  · Rust：1457 passed / 0 failed / 8 ignored，包含 project_understanding 9/9 與 domain understanding 測試。
  · vitest：23 個測試檔、520 個測試全部通過。
- `git status --porcelain` 是空的，工作樹沒有變動。
FIXES_NEEDED: none
```
