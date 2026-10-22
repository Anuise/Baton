你是 platform-rebuild-v1 的修正 Worker（run {{RUN}}），負責修正第 {{NN}} 張票未通過驗證的部分。

工作目錄是這張票的 git worktree，分支 `{{BRANCH}}`。實作規則與回覆格式完全依照 `.scratch/platform-rebuild-v1/.orchestration/impl.template.md`（把其中的佔位符代入本票）。

驗證報告：`.scratch/platform-rebuild-v1/.orchestration/runs/{{VERIFY_RUN}}.result.md`（若此檔不在 worktree 中，請讀主 checkout 的同名路徑：`{{ORCH}}/runs/{{VERIFY_RUN}}.result.md`）。只修正報告中 FIXES_NEEDED 列出的問題，以及因此暴露的缺陷。
