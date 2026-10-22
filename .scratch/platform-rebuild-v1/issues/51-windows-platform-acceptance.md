# 51: Windows 平台與原生整合驗收

Status: ready-for-human
Blocked by: 02, 18, 21, 24, 29, 30, 34, 36, 38, 39, 40, 41, 47, 48, 49, 50, 53

**What to build:** 在 Windows 11 Home／Pro × 25H2／26H2 四種組合上，以真實 Claude Code 與多個已登入帳號執行 issue #3 §15.3～15.10 的驗收清單與真實 Adapter 契約測試：乾淨安裝、多帳號、背景執行、完全退出、交接、崩潰恢復、監護、跨登入、非正常關機與磁碟滿。記錄 OS build、App、Claude Code 與相依版本，結論回寫 spec 的 Validation Required 項目；與 Claude 決策（D-NN）衝突時更新該決策並記錄原因。需要實體或虛擬機與真實帳號，因此由人執行。

**Blocked by:** 02 建立 GLOSSARY.md 與既有決策的 ADR；18 Worker 間訊息；21 Worktree 完成後自動清理；24 授權撤銷與啟動准入順序（R1-a）；29 突然限流與全部帳號受限；30 Controller 跨帳號交接；34 安裝程式、更新與解除安裝；36 修正與重試預算；38 Guard 與執行前檢查；39 Context Builder 與 context receipt；40 備份與還原演練；41 事件時間線與觀測指標；47 探索測試到可重現缺陷；48 已驗證動作重播與 no_live_model；49 驗證中的額度交接；50 Gate 失效與重驗

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 四種組合各有驗收紀錄與版本資訊
- [ ] spec 的 Validation Required 清單每項都有結論
- [ ] 與 D-NN 衝突的結果已回寫並說明

## Comments
