# ui-ux-pro-max 查詢紀錄

MASTER.md 每一項決定以這裡的查詢編號（Q1…）或 reference 章節作為依據。
Skill：`ui-ux-pro-max` 2.15.0（`C:\Users\User\.claude\plugins\cache\ui-ux-pro-max-skill\ui-ux-pro-max\2.15.0\.claude\skills\ui-ux-pro-max\`），
腳本：`python "<skill>\scripts\search.py" …`，2026-10-28 執行。查詢字串只用通用詞，不含專案私有資料。

`skill-raw-output.md` 是 Q3 以 `--persist` 寫出的原始 MASTER.md，保留作為對照；MASTER.md 已依本產品改寫。

## 設計系統查詢（`--design-system --density 8 -p "Baton"`）

| # | 查詢 | 回傳的 Pattern／Style | 色彩 | 字型 | 採用 |
|---|------|------------------------|------|------|------|
| Q1 | `developer tool dashboard dense professional` | FAQ/Documentation Landing／Dark Mode (OLED)（Light not-recommended） | 深色 slate：bg `#0F172A`、card `#1B2336`、muted `#272F42`、muted-fg `#94A3B8`、border `#475569`、accent `#22C55E` | JetBrains Mono + IBM Plex Sans | 只採深色模式的 slate 底色層次；Pattern（行銷 FAQ 頁）與「不建議淺色」不採用 |
| Q2 | `developer productivity tool task management` | Product Demo + Features／Flat Design | teal `#0D9488` + orange `#EA580C` | Inter | Flat Design 的「無漸層、簡單 hover、150–200ms」採用；Pattern 與配色不採用 |
| Q3 | `data-dense admin dashboard desktop`（`--persist`） | Enterprise Gateway／**Data-Dense Dashboard** | primary `#1E40AF`、secondary `#3B82F6`、accent `#D97706`、bg `#F8FAFC`、fg `#1E3A8A`、muted `#E9EEF6`、muted-fg `#475569`、border `#DBEAFE`、destructive `#DC2626` | Fira Code + Fira Sans | 風格（Data-Dense Dashboard）、primary／secondary／bg／muted-fg、density 8 間距採用；Pattern、amber CTA、藍色前景、藍色邊框、Google Fonts 不採用（理由見 MASTER.md） |

三次查詢的 Pattern 都是行銷頁（FAQ landing、Product demo、Enterprise gateway），沒有一個是桌面工具的版面模式，所以 Pattern 一律不採用。

## 領域查詢（`--domain ux`／`typography`、`--stack react`）

| # | 查詢 | 結果（Issue → Do） | 用在 |
|---|------|--------------------|------|
| Q4 | `long text truncation overflow` `--domain ux -n 4` | Truncation（Medium）→ 截斷要有展開；**Essential Text Truncation（Critical）**→ 標題、動作、錯誤、識別名稱要能完整取得；**Long Token Wrapping（High）**→ `.token { min-inline-size: 0; overflow-wrap: anywhere; }`，不要對一般文字用 `word-break: break-all`；**Compact Label Overflow（High）**→ 徽章一行、`nowrap` + 可縮的 label，完整值要讓鍵盤與指標都拿得到，不能只靠 hover tooltip | 長文字策略全節 |
| Q5 | `line length readability` `--domain ux -n 3` | Line Height → 內文 1.5–1.75；Line Length → 每行 65–75 字元；Heading Line Balance → 短標題可用 `text-wrap: balance` | 行高、閱讀寬度 |
| Q6 | `badge chip label wraps` `--domain ux -n 3` | Compact Label Overflow（同 Q4）；Contextual Live Badge Updates → 計數變更以完整語句宣告；Input Labels → 每個輸入要有可見 label | 徽章、表單 |
| Q7 | `table column width overflow` `--domain ux -n 3` | Table Handling → 外層 `overflow-x: auto`；Overflow Hidden → 不要盲目 `overflow: hidden`；Container Width → 文字 65–75ch | 表格欄寬 |
| Q8 | `dark mode contrast` `--domain ux -n 3` | Contrast Readability；Color Contrast → 一般文字 4.5:1；Focus Appearance → 2px 外框、3:1 狀態對比 | 對比值、焦點 |
| Q9 | `CJK chinese font` `--domain typography -n 3` | Chinese Simplified（Noto Sans SC）；**Chinese Traditional（Noto Serif TC + Noto Sans TC）**；Calistoga + Inter | 字型堆疊（改用系統的同類字型，理由見 MASTER.md） |
| Q10 | `system font stack offline` `--domain typography -n 2` | Plus Jakarta Sans（system fallback）、Inter（system-first）——兩者都仍以 Google Fonts 為主 | 不採用（沒有可離線的 CJK 方案） |
| Q11 | `empty state` `--domain ux -n 2` | Empty States → 說明 + 動作，不要空白；Active State → 目前位置要標示 | 空狀態 |
| Q12 | `modal dialog focus` `--domain ux -n 2` | Focus States → 對話框內也要有可見焦點；Focus Appearance | 對話框 |
| Q13 | `text overflow min-width flex` `--stack react -n 3` | **0 筆**（closest: next, test, context） | — |
| Q13b | 重試 `list rendering performance` `--stack react -n 2` | Use keys properly；generic list components——與版面無關 | React stack 沒有版面相關準則；長文字的實作改以 Q4（ux）為依據 |
| Q14 | `reduced motion` `--domain ux -n 2` | Reduced Motion → `prefers-reduced-motion`；Excessive Motion → 每個畫面最多 1–2 個動畫 | 動態 |
| Q15 | `font size scale` `--domain ux -n 3` | Font Size Scale → 一致的級距（12 14 16 18 24 32）；Readable Font Size → 行動裝置內文最少 16px | 字級（16px 是行動裝置規則，見 MASTER.md 的反駁） |
| Q16 | `primary action hierarchy` `--domain ux -n 3` | Breadcrumbs、Heading Hierarchy、Font Size Scale——**沒有命中主要／次要動作** | 改用 quick-reference §4 `primary-action` |
| Q17 | `data table density` `--domain ux -n 3` | Table Handling（同 Q7）、Bulk Actions、Auto-Play Video（離題） | 表格 |
| Q18 | `tooltip keyboard full text` `--domain ux -n 3` | Streaming、Alt Text、Keyboard Navigation——**沒有命中 tooltip** | 改用 Q4 Compact Label Overflow 的「不可只靠 hover」 |
| Q19 | `number tabular figures` `--domain ux -n 2` | 1 筆：Number Formatting → 千分位或縮寫 | 數字格式 |
| Q20 | `z-index modal overlay` `--domain ux -n 2` | Stacking Context；Z-Index Management → 固定級距（10 20 30 50） | 層級 |
| Q21 | `disabled state` `--domain ux -n 2` | Disabled States → 降低不透明度並換游標；Active State | 按鈕、表單 |

## Reference

- `references/quick-reference.md`：§1 Accessibility（`color-contrast`、`focus-states`、`web-target-size` 24×24）、§4 Style（`primary-action`、`elevation-consistent`、`state-clarity`）、§5 Layout（`horizontal-scroll`、`z-index-management`、`compact-label-overflow`、`chip-collection-reflow`）、§6 Typography & Color（`line-height`、`line-length`、`number-tabular`、`truncation-strategy`、`long-token-wrapping`、`color-dark-mode`）、§7 Animation（`duration-timing`、`transform-performance`、`reduced-motion`）、§8 Forms（`input-labels`、`error-placement`、`disabled-states`、`read-only-distinction`、`confirmation-dialogs`）。
- `references/pro-rules.md`：檔案開頭明言適用 native／mobile；只引用與平台無關的「Light/Dark Mode Contrast」與「Layout & Spacing」兩張表（`Border and divider visibility`、`Token-driven theming`、`Readable text measure`、`Section spacing hierarchy`）。觸控 44pt、safe area 等行動規則不適用於桌面 App。
