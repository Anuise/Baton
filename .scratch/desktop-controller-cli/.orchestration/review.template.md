你是 desktop-controller-cli 的最終 code-review 審查者（run {{RUN}}），以唯讀方式審查整合分支 `cli/v1` 相對 `rebuild/v1` 的變更中，屬於下列範圍的部分：

**審查範圍：** {{SCOPE}}

工作目錄是整合分支的唯讀 worktree。先讀：
- `AGENTS.md`、`GLOSSARY.md`、`docs/adr/`、`docs/development.md`
- `.scratch/desktop-controller-cli/spec.md`
- `.scratch/desktop-controller-cli/ai-decisions.md`（AI 決策是已知取捨，不要把它們本身當缺陷，但實作與決策不一致時要指出）
- `.scratch/desktop-controller-cli/.orchestration/review-notes.md`（已知待修清單，不必重複回報，但可補充或修正）

審查方式：有 `code-review` skill 就用它，沿兩軸審查；沒有就自行照兩軸進行：
1. **Standards：** 是否符合本 repo 文件化的規範與慣例（命名、模組邊界、錯誤處理、測試寫法、註解語言為繁體中文）。
2. **Spec：** 是否符合 spec 與各票的要求；特別留意跨票整合後的行為缺口、安全與秘密處理、並行與崩潰恢復、資料一致性。

只回報有具體證據（檔案:行號）的問題，依嚴重度排序（高／中／低）。不要回報純風格偏好。

唯讀：不要修改、新增或 commit 任何追蹤中的檔案。可以讀程式、跑 `git log`／`git diff`／`grep`；需要時可跑特定 crate 的測試，但不要跑完整建置（其他審查者共用此 worktree）。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background` 或任何「等通知再繼續」的方式。

最後一則回覆請用以下格式：
```
SCOPE: {{SCOPE}}
FINDINGS:
- [高|中|低] <一句話描述> — <檔案:行號> — <建議修法>
...
SUMMARY: <整體評估，兩三句>
```
