# 07: Controller CLI 的完全退出與當掉恢復

Status: ready-for-agent
Blocked by: 03

**What to build:** 只關 App 視窗時 Controller CLI 不受影響；完全退出時 Runtime 結束 Controller 宿主與 `claude`，退出進度列出這一項；Runtime 當掉恢復後，非終態 Controller Run 先查驗程序：不存活標為 interrupted，存活者舊憑證失效並提示使用者重開。之後可用同帳號接續原生 Session。User stories 68～71。

**Blocked by:** 03

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [ ] 關閉 App 視窗（Runtime 仍在）不影響 Controller Run
- [ ] 完全退出時 Controller 宿主被結束，Run 進入 ended，退出進度顯示這一項
- [ ] Runtime 崩潰重啟：宿主不存活 → interrupted；存活 → 舊憑證失效、頁簽提示重新開啟
- [ ] 恢復後重新開啟同帳號時啟動規格含 `--continue`
- [ ] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過
- [ ] AI 決策已追加至 ai-decisions.md

## Comments
