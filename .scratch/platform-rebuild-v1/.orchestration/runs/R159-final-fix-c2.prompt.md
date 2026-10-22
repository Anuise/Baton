你是 platform-rebuild-v1 最終 review 的修正 Worker（run R159-final-fix-c2），在分支 `rebuild/final-fix-c` 上修正驗證未通過的部分（這是本分支的第 1 次修正，上限 3 次）。

實作規則、執行方式限制與回覆格式完全依照 `.scratch/platform-rebuild-v1/.orchestration/final-fix.template.md`（分支代入 `rebuild/final-fix-c`），但**不要 `git reset --hard`**：本分支已有修正，若 `rebuild/v1` 不是 HEAD 的祖先，先 `git merge rebuild/v1` 並解衝突（AI 編號重複時把本分支的條目順延到整合分支最大號之後，同步更新程式與文件中的引用；不可修改整合分支既有條目）。

驗證報告：`/c/Users/User/Documents/au-program/issue-orchestrator/.scratch/platform-rebuild-v1/.orchestration/runs/R158-final-verify-c.result.md`。只處理 FIXES_NEEDED 第 1、2 項，以及因此暴露的缺陷。第 3 項由主控另開票處理，不必做。本項目允許修改 `crates/baton-runtime` 的測試檔（跨範圍，請在 NOTES 說明）。

驗收時請用專用 TEMP（例如 `TEMP=%TEMP%\baton-final-c`）跑測試，確認結束後沒有殘留。
