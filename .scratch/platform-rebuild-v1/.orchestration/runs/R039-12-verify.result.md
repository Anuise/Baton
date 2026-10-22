第 12 張票驗證通過：四條驗收條件都成立，`pnpm test` 與 `pnpm build` 都成功，也沒有找到違反 spec 的缺陷。工作目錄全程保持乾淨（`git status --short` 沒有輸出）。

```
VERDICT: PASS
CRITERIA:
- [PASS] 成果寫入中途中斷後重建 Runtime，不出現已登記但內容不完整的成果 — 寫入順序（crates/baton-api/src/artifacts.rs:132-152）：先確認是目前的寫入所有者，再呼叫 write_artifact_atomic；manifest 等檔案落地之後才在同一個交易提交。真實實作在 crates/baton-ports/src/workspace_fs.rs 的 write_file_atomic：先寫 `.partial` 暫存檔 → sync_all → rename，由 production_ports.rs 接上。fake 可以在三個點模擬崩潰：MidWrite、BeforeRename、AfterRename（crates/baton-harness/src/fakes.rs）。情境測試在 crates/baton-harness/tests/artifacts.rs:303-370：每個崩潰點都會 harness.restart()，之後斷言 artifacts 為空，殘留檔案以 staging 或 unregistered 列在 unregistered_files，之後重新登記得到 v1 且內容完整。
- [PASS] manifest 雜湊與檔案一致；同一成果新版本不覆寫舊版本 — manifest 記錄 sha256、size、storage_path、run 與世代、task（crates/baton-store/src/artifacts.rs，migration 11）。artifact 與 artifact_version 兩張表有觸發器禁止 UPDATE 和 DELETE，storage_path 設為 UNIQUE（crates/baton-store/src/lib.rs）。每個版本都寫到獨立的 blob_<隨機> 檔。測試：tests/artifacts.rs:169 檢查雜湊與大小和磁碟檔一致；:217 檢查 v2 不改動 v1 的檔案，兩個版本都能下載；:250 檢查直接改資料庫會被拒；:373 檢查竄改後的內容在預覽與下載時回 409 artifact_integrity_failed。
- [PASS] 未授權呼叫者無法下載成果 — 預覽、下載與列表都在管理 API 之下，由 guard 要求桌面角色。tests/artifacts.rs:443 驗證以下情況都拿不到內容：無憑證回 401、猜測的憑證回 401、缺少 x-baton-client 標頭回 403、過期憑證回 401、Agent 憑證（包括登記該成果的 Worker 自己）回 403 role_not_allowed。另外 :499 測試跨 Workspace 一律回 404，:524 測試 Agent 不能登記到其他 Workspace 或 Run（403 agent_scope_mismatch）。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/artifacts.rs 共 17 個測試，全部通過；它們用真實的 Store、SQLite 與 Axum，只替換 Port，並涵蓋經由 MCP shim 登記（:621）。ai-decisions.md 新增 AI-103 到 AI-108，都標了重要度（高、中、中、中、低、低），只有追加，沒有修改既有條目。ancestry：`git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK，表示整合分支是 HEAD 的祖先。
TESTS: `pnpm test` 的結束碼是 0。Rust 部分執行 `cargo test --workspace --exclude baton-desktop`，全部通過：harness 的 artifacts 17 個、baton-client 的 artifacts_client 2 個、domain 193 個、store 48 個、mcp-shim 的 scenario 14 個等，沒有失敗。baton-runtime 有 3 個 ignored，是本票之前就有、需要真實 Claude Code 的測試。前端 vitest 12 個檔案、173 個測試全部通過。`pnpm build` 的結束碼是 0，產出 target/release/baton-runtime.exe 與 baton-desktop.exe。
FIXES_NEEDED: none
```

另外有兩點不影響判定的觀察：
- 原子改名之後沒有對所在目錄做 fsync。Windows 上沒有標準作法，這點不影響「已登記就一定完整」，因為 manifest 只在 rename 成功後才提交。
- 未登記的殘留檔案目前只會顯示，不會被清理。實作者已在 AI-104 記錄這是刻意的，清理留給後續的票。
