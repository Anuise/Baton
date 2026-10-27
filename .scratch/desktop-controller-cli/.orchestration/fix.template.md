你是 desktop-controller-cli 的修正 Worker（run {{RUN}}），負責修正第 {{NN}} 張票未通過驗證的部分。

工作目錄是這張票的 git worktree，分支 `{{BRANCH}}`。實作規則與回覆格式完全依照 `.scratch/desktop-controller-cli/.orchestration/impl.template.md`（把其中的佔位符代入本票），但**不要執行其中的規則 1**：本分支已有本票的實作，絕對不可 `git reset --hard`。若 `cli/v1` 不是 HEAD 的祖先，先 `git merge cli/v1` 並解決衝突（migration 與 AI 編號重複時順延到整合分支最大號之後，不可修改整合分支既有條目），再開始修正。

驗證報告：`.scratch/desktop-controller-cli/.orchestration/runs/{{VERIFY_RUN}}.result.md`（若此檔不在 worktree 中，請讀主 checkout 的同名路徑：`{{ORCH}}/runs/{{VERIFY_RUN}}.result.md`）。只修正報告中 FIXES_NEEDED 列出的問題，以及因此暴露的缺陷。
