# 05: Controller 帳號排除、額度顯示與換帳號重開

Status: completed
Blocked by: 03

**What to build:** Controller CLI 開著時，該 Profile 不被 Scheduler 分派給 Worker（既有 Worker 不受影響），結束後自動恢復。Controller 頁簽持續顯示 Controller 帳號的 5h／7d 額度，接近上限時提示；Profile 選單顯示每個 Profile 的額度與執行中 Worker 數。同帳號重新開啟時以 `--continue` 接回原生 Session；「換帳號重開」結束目前 CLI 並以新 Profile 開新 Session（不接續）。User stories 19、33～35、38～40。

**Blocked by:** 03

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [x] Controller Run 非終態期間，Scheduler 不把該 Profile 分給新的 Worker；已在執行的 Worker 繼續執行；Run 結束後該 Profile 恢復為候選
- [x] Controller 頁簽與 Profile 選單顯示額度與執行中 Worker 數；達到 draining 門檻時顯示提示
- [x] 同一 Workspace 以同一 Profile 重開時啟動規格含 `--continue`；換 Profile 時不含
- [x] 「換帳號重開」先結束目前 Controller Run 再開新的，兩者在時間線上可追查
- [x] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；Controller 頁簽元件測試涵蓋額度與換帳號
- [x] AI 決策已追加至 ai-decisions.md

## Comments
