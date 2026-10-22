結論是 FAIL。建置與全部測試都通過，`rebuild/v1` 也確實是 HEAD 的祖先；但有兩條驗收條件不成立。

1. **權限提示**：在「可信專案開發（P3）」範本下，我用暫存探針程式實測了 `permission::evaluate`。好幾個超出授權的操作被自動允許，沒有建立裁定請求，包括改寫 Git 歷史、寫到 Workspace 外、全域安裝，以及寫進 `.git/hooks`。
2. **自動恢復排程**：裁定後 Task 只回到 `ready`，沒有重新放進派工佇列。Supervisor 只處理佇列裡的項目，所以使用者仍得手動派工；情境測試本身也是手動派工的。AI-105 說「由票 19 的 Scheduler 看到 `ready` 後處理」，與票 19 的實作不符。

```
VERDICT: FAIL
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成功（輸出 ANCESTOR_OK；rebuild/v1 = 9e49a8d，已在 HEAD 656ffc2 的歷史中）
- [PASS] Agent 憑證無法提交正式裁定 — 回答端點只在 /api/v1/ 下（crates/baton-api/src/decisions.rs:21-24），guard 依前綴只放行桌面角色（crates/baton-api/src/guard.rs:153）；Domain 的 answer 另外檢查 decided_by（crates/baton-domain/src/decision.rs:679 起）；資料庫有 `CHECK (decided_by = 'desktop')`（crates/baton-store/src/lib.rs:364）；Agent API 只有 request_decision 和 permission_prompt，沒有回答的工具（crates/baton-api/src/lib.rs:168-169）。情境測試 `an_agent_credential_cannot_submit_a_decision_or_read_the_decision_center` 驗證回 403 role_not_allowed（crates/baton-harness/tests/decisions.rs:434），通過
- [FAIL] 裁定後受影響 Task 自動恢復排程；無關 Task 從未被阻擋 — 後半成立：只有 blocking 中的 Task 進 waiting_decision；另一個 Task 一直是 running（decisions.rs:262）；多個請求阻擋同一 Task 時要全部回答才恢復（decision.rs:720 起，測試 `a_task_stays_waiting_until_every_request_blocking_it_is_answered`）。前半不成立：回答後 Task 只回到 `ready`，沒有寫入 dispatch_request。Supervisor::schedule_queue 只處理 dispatch_request（crates/baton-runtime/src/supervisor.rs:123-131）；派出時 Task 已被移出佇列（crates/baton-store/src/tasks.rs:277），所以不會自動再派。情境測試靠手動 `POST …/dispatch` 才產生新 Run（crates/baton-harness/tests/decisions.rs:263）。AI-105「由票 19 的 Scheduler 看到 ready 後處理」（ai-decisions.md:973）與實作不符
- [PASS] 逾時不視為同意 — deadline 只算出 overdue 標示（decision.rs is_overdue／view），沒有任何程式路徑因時間回答請求或恢復 Task；`a_deadline_passing_is_only_a_label_and_never_consent` 推進 fake Clock、重啟 Runtime 後請求仍是 pending；`a_permission_request_is_never_released_by_time_passing` 放三天後權限提示仍回 deny（decisions.rs:524-532）
- [FAIL] 權限提示在授權內自動允許、範圍外產生裁定請求（以 fake Adapter 驗證） — 基本流程和測試都正確（授權內回 allow 並記 permission_auto_allowed；範圍外建立 allow_once/deny 請求並回「等待裁定」；allow_once 只消耗一次）。但詞法判定有漏洞。我在 repo 外建了暫存 crate，用 P3 範本實際呼叫 `permission::evaluate`，以下操作都得到 `Allow`：
    • `git commit --amend --no-edit` → Allow WriteFiles。違反 P3 寫入授權的限制「不修改 Git 歷史」（permission.rs:146 只看子命令）
    • `git diff --output=C:/Users/me/AppData/Roaming/evil.txt` → Allow ReadFiles。實際會寫檔到 Workspace 外，只需要讀取權限，研究範本也會放行（permission.rs:145）
    • `npm install --location=global typescript`、`npm install --global=true typescript` → Allow InstallProjectDependencies。實際是高風險的全域安裝，P3 沒有授權（permission.rs:126 只比對 `-g`、`--global` 完全相同的字串）
    • Write `.git./hooks/pre-commit`、`.git /config` → Allow WriteFiles。Win32 會去掉路徑元件結尾的點與空白，實際寫進 .git（permission.rs:226 只比對 `== ".git"`）
    • `cargo test --manifest-path C:/other/Cargo.toml` → Allow RunBuildTest。違反限制「僅執行專案內建置與測試指令」
  對照組：`.git/config` 正確判為 OutOfScope
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/decisions.rs 共 18 個情境，使用真實 SQLite、真實 Axum、兩組憑證，以 scripted fake Adapter 推進 Worker，全部通過；AI-103～AI-109 已追加並標示重要度（AI-105 內容需依上面第三項修正）
TESTS:
- `pnpm test`（Rust 為 `cargo test --workspace --exclude baton-desktop`，加上 `pnpm --filter web test`）→ EXIT=0；Rust 合計 567 passed、0 failed、3 ignored（live_ 測試）；其中 decisions.rs 18 passed、decisions_client.rs 通過；前端 vitest 12 個檔案、173 個測試全部通過
- `pnpm build` → EXIT=0，產出 target/release/baton-desktop.exe，沒有 warning
- 權限判定探針：在 repo 外的暫存 crate 以 path 依賴 baton-domain，對 P3 範本呼叫 `permission::evaluate`，結果見上；探針已刪除
- `git status --porcelain` 為空，沒有修改任何追蹤中的檔案
FIXES_NEEDED:
1. crates/baton-domain/src/permission.rs 收緊判定，每一項都在 permission.rs 加單元測試，並在 harness decisions.rs 至少加一個情境（P3 範本下這些指令要回 waiting_decision 並建立請求）：
   a. git：`commit` 帶 `--amend`（以及 `--fixup`、`--squash`、`-C`、`--reset-author` 這類改寫選項）時判為無法歸類；`status/diff/log/show` 帶 `--output`／`--output=` 或 `-o` 時判為無法歸類。比較穩的做法是 git 子命令後面只要有不在白名單裡的旗標，就判為無法歸類。
   b. npm/pnpm/yarn/bun：`--global=…`、`--location=global`、`--location global`、`-g=…` 判為 GlobalInstall；`--prefix`、`-C`、`--dir` 這類指到其他目錄的旗標判為無法歸類。
   c. cargo：帶 `--manifest-path`、`-Z`、`--config` 時判為無法歸類（或要求路徑落在授權目錄內）。
   d. 路徑元件比對前先去掉結尾的 `.` 與空白（模擬 Win32 正規化），並拒絕含 `:`（ADS）或 `\\?\` 前綴的元件，避免 `.git.`、`.git ` 繞過 .git 限制。
2. 裁定後真正自動恢復排程：在 answer 的同一個交易裡，把恢復成 ready 的 Task 寫回 dispatch_request（priority 沿用 Task／契約，或用原本派工的 priority）。如果舊 Run 還持有 execution lease，請確認排程會讓它在佇列中等待，不會直接丟掉，必要時補上對應的等待原因。harness 測試改成：回答後不手動 POST dispatch，只呼叫 `harness.supervise()`，就應該看到新的 Run（受影響的 Task 回到 running）；無關 Task 不受影響。
3. 更新 ai-decisions.md 的 AI-105（拿掉「由票 19 的 Scheduler 看到 ready 後處理」這個錯誤前提，改寫成實際的重新排隊行為）和 AI-103（補上第 1 項的規則），並同步 docs/development.md「裁定中心與權限提示轉接」一節。
```
