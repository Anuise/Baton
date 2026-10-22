VERDICT: FAIL

三條驗收條件都通過，建置與全部測試也都通過。判 FAIL 的原因是我實際重現了一個缺陷：使用者選「保留外部版本」之後，只要再編輯那個檔，就永遠無法從 UI 改回「以 Baton 版本覆寫」，每次都得到 409。這條路徑是 AI-363 和 `docs/development.md:1140` 寫明要支援的。

前置條件成立：`git merge-base --is-ancestor rebuild/v1 HEAD` 在驗證開始和結束時都成立，`rebuild/v1` 是 `f7c2480`。

```
CRITERIA:
- [PASS] 未知資訊不被寫成已確認事實 — 
  · Domain 擋住：沒有來源的 confirmed 回 UnverifiedClaim（crates/baton-domain/src/understanding.rs:282-284）；找不到的來源不論宣稱什麼狀態都拒絕（:268-273）；外部來源沒有版本也拒絕（:219-222）。
  · 資料表也擋：CHECK (status <> 'confirmed' OR json_array_length(sources) > 0)，見 crates/baton-store/src/lib.rs 的 migration project_understanding。
  · 版本和查驗時間由 Runtime 自己算：讀 Workspace 內的檔案取 SHA-256，時間取 clock.now()（crates/baton-api/src/understanding.rs:186-190），不採信 Agent 自報。
  · 沒給 status 時預設是 hypothesis（understanding.rs:105-115）。
  · 投影時假說標「假說（未確認）」，未知的名詞只列在「待確認（未知）」，不寫成定義（domain understanding.rs:654-677）。
  · 情境測試 unknown_information_is_never_recorded_or_projected_as_a_confirmed_fact 與 a_confirmed_item_whose_source_file_changes_is_flagged_for_recheck 通過（crates/baton-harness/tests/project_understanding.rs:254-313）。
- [PASS] 投影檔被外部修改時產生協調事件而非靜默覆寫 — 
  · 只有檔案仍等於 baseline_hash 才寫入；被修改、被刪除、或接入前就已存在，一律轉為 conflict 並記事件 understanding_projection_conflict_detected（domain understanding.rs:520-540）。
  · 在同一個交易中送一則工作事件給 Controller 對話（crates/baton-store/src/understanding.rs:244-246）。
  · 比對與寫檔用 projection_lock 序列化（api understanding.rs:299）。
  · 三個情境測試通過：外部修改（:317-384）、接入前就存在（:386-399）、ADR 被外部刪除（:401-439）。測試涵蓋不覆寫、只通知一次、重啟後協調仍在，以及檔案在檢視後又被改時回 409。
- [PASS] 以 Runtime harness（spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — 
  · crates/baton-harness/tests/project_understanding.rs 共 8 個測試，全部經真實 Axum API 操作，8 passed / 0 failed。
  · ai-decisions.md 新增 AI-362～AI-369，每條都有重要度（高／中／低）。
TESTS:
- `pnpm build`（release 版 Runtime、Web、Tauri）→ exit 0。有一則 unused import `StoreError` 的 warning，在 crates/baton-api/src/shared_settings.rs:20，本票沒動這個檔，不是本票造成的。
- `pnpm test` → exit 0。
  · Rust：cargo test --workspace --exclude baton-desktop，1455 passed / 0 failed / 8 ignored，其中 project_understanding 8/8 通過。
  · 前端：vitest 23 個檔、520 個測試全部通過。
- 缺陷重現：在 repo 外建一個暫存 crate，以 path 依賴 baton-domain，依序跑 plan_projection 與 reconcile：
  · 依序執行：首次投影 → 外部修改 → keep_external → 使用者再改檔 → overwrite，並照 UI 的做法帶 baseline_hash。
  · 輸出：`after keep: Detached` / `sync on detached changes record: false` / `overwrite with baseline_hash: Some("projection_changed")`。
  · 暫存 crate 已刪除，工作樹沒有變動（git status 是空的）。
FIXES_NEEDED:
1. 修正 detached 狀態下無法覆寫的問題：
   · 問題出在：UI 對 detached 檔送 `baseline_hash` 當 `expected_external_hash`（web/src/ProjectUnderstandingPage.tsx:242-244）；baseline_hash 是保留外部版本那一刻的雜湊。
   · 同時，plan_projection 遇到 detached 會直接回傳不變（crates/baton-domain/src/understanding.rs:464-466），所以 sync 永遠不會更新這個雜湊，視圖也沒有提供檔案目前的雜湊。
   · 結果：使用者保留外部版本後只要再編輯那個檔，「以 Baton 版本覆寫」就永遠回 409 `projection_changed`，即使按「重新檢查」也無法恢復。
   修正方式擇一：
   · (a) sync 時遇到 detached 檔，把目前檔案的雜湊寫進 `external_hash`，只更新狀態、不寫檔、不通知；UI 改送 `external_hash`。
   · (b) build_view 對 detached 檔即時讀出目前雜湊並放進視圖，UI 送這個值。
   不論哪一種，都要確保覆寫前使用者看過的就是目前的檔案版本。
2. 在 crates/baton-harness/tests/project_understanding.rs 補一個情境測試：keep_external → 外部再編輯檔案 → sync → 用視圖提供的雜湊做 overwrite，預期 200、in_sync，且檔案變成 Baton 版本。再測用過時的雜湊仍回 409。同時更新 web/src/ProjectUnderstandingPage.test.tsx:189-218 對 detached 送 baseline_hash 的斷言。
3. 在 ai-decisions.md 追加這次修正的決策並標重要度；如果 API 欄位的語意有變，同步更新 docs/development.md:1140 的說明。
```
