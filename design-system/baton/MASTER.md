# Baton 設計系統（MASTER，第 3 版「夜間跑道」）

> 實作或審查任何頁面時以本檔為準。token 的實際值在 `web/src/styles/tokens.css`，共用元件在 `web/src/styles/components.css`，外殼在 `web/src/styles/shell.css`。本檔說明「為什麼」與「什麼時候用哪個」。

## 1. 對象與主要工作

- **產品**：Windows 桌面 App（Tauri + React，WebView2），讓一個開發者同時調度多個 Claude Code 帳號長時間工作。
- **使用者**：一位專業開發者，繁體中文，不時切回來看一眼，要很快知道「有什麼要我決定」「現在誰在跑什麼」，再往下鑽到 Task、成果與證據。
- **主要工作**：看現況 → 回答裁定 → 檢查成果。設定是偶爾才做的事。
- **內容特性**：資訊密集、大量長文字（契約、理由、Windows 路徑、識別碼、雜湊、錯誤訊息），兩種主題都要完整。

## 2. 方向：夜間跑道

Baton 是接力棒：多個帳號像分道的跑者，額度用完就交棒給下一個。整個介面借用這個意象，但只在一個地方大膽：

1. **跑道欄（左側導覽）是唯一的深色大面積**，兩種主題都是深靛色。每個 Workspace 是一條跑道，選中的那條展開它的頁面。
2. **接力棒**：目前頁面左緣一根淡紫色短棒；換頁時沿跑道移過去（220ms，減少動態時直接出現）。這是全站唯一不由使用者觸發以外的動態，也是唯一的裝飾性元素。
3. **內容區安靜**：冷灰紙底、白色物件、群青強調色。不用漸層、物件不加陰影、不做 hover 位移，以留白、字級與圓角層次分層。

### 刻意避開的預設

| 預設 | 為什麼不用 | 改用 |
|---|---|---|
| 每個區塊都包成同一種圓角卡片 | 卡片湯：看不出什麼是「一個東西」、什麼只是分組 | 區段不加框（`.section`）；只有物件（Task、裁定、帳號）用 `.card`；同類清單用 `.row-list` |
| 標題上方的小標籤（eyebrow） | 模板感，資訊重複 | 不加；需要分組時用區段標題 |
| 「A · B · C」以中點串接的次要資訊 | 模板感，窄時斷得很難看 | `.meta`：項目之間用間距，各自不換行 |
| 等寬字型當裝飾標籤 | 模板感 | 等寬只給真正的程式碼、路徑、識別碼、雜湊 |
| 按鈕或連結文字後加「→」 | 模板感 | 動詞說清楚要做什麼 |
| 紫色漸層、玻璃擬態 | 不傳達資訊 | 純色 |

## 3. 色彩（tokens.css）

全部顏色只透過 token；不寫死色碼（跑道欄內少數以 `rgb(255 255 255 / n%)` 疊色是例外，只出現在 shell.css）。

| 角色 | 淺色 | 深色 | 用途 |
|---|---|---|---|
| `--color-bg` | `#F5F5F8` 紙 | `#121120` | 視窗底 |
| `--color-surface` | `#FFFFFF` | `#1B1A2D` | 物件、面板 |
| `--color-surface-sunken` | `#ECECF2` | `#151425` | 嵌入區：表單區、程式碼、看板欄 |
| `--color-text` | `#1C1A33` 墨 | `#F1F0FA` | 主要文字 |
| `--color-text-muted` | `#4F4C68` | `#ABA8C4` | 標籤、次要文字（sunken 上也可用） |
| `--color-text-subtle` | `#686583` | `#908CAB` | 計數、時間戳、placeholder |
| `--color-accent` | `#3F35BF` 群青 | `#AEA5FF` | 選取、連結、主要按鈕 |
| `--rail-bg` | `#221E4A` 夜靛 | `#1F1B45` | 跑道欄（深色模式比內容底色亮一階、帶靛色，並以分道線隔開） |
| `--rail-baton` | `#B9AFFF` 淡紫 | 同左 | 接力棒、跑道欄上的主要按鈕 |
| `--rail-badge-bg` | `#FFC24B` 琥珀 | 同左 | 跑道欄上的待裁定數 |

語意色（success／warning／danger）各有實色、`-subtle` 淡底、`-border` 邊框；驗證判定另有 `orange`（環境受阻）與 `magenta`（授權受阻）。所有文字對比 ≥ 4.5:1，控制項邊界與焦點 ≥ 3:1（已逐一計算）。

規則：

1. 語意色只標示狀態；**長內容不整塊上色**。狀態以「左側 3px 色條（`.stripe-*`）＋狀態徽章（文字）」表達。
2. 淡底（`.callout-*`、`.badge-*`）只給兩行內的提示與徽章。
3. 不以顏色單獨傳達：狀態點（`.dot`）旁一定有文字。
4. 琥珀色＝要你處理（待裁定、逾期、警告）；群青＝可以按、目前所在。兩者不混用。

## 4. 字型與字級

- `--font-sans`：Segoe UI Variable Text → 微軟正黑體 UI。全站內文。
- `--font-display`：Segoe UI Variable Display → 微軟正黑體 UI。只給頁面標題（h2 in `.page-header`）、空狀態主標、對話框標題、產品名稱。
- `--font-mono`：Cascadia Mono → Consolas → 微軟正黑體 UI。只給程式碼、路徑、識別碼、雜湊。
- 桌面 App 必須離線可用，不載入網路字型。

| token | 字級／行高 | 用途 |
|---|---|---|
| `--text-caption` | 12／1.4 | 徽章、計數、時間戳、跑道欄群組名稱。**不可用於段落** |
| `--text-meta` | 13／1.5 | 控制項、表格、次要資訊、標籤（dt） |
| `--text-body` | 14／1.7 | 一切段落與長內容；全站基準 |
| `--text-title-sm` | 14／1.45 粗 | 物件標題（Task、條目） |
| `--text-title` | 16／1.4 粗 | 區段標題（h3） |
| `--text-page` | 24／1.25 特粗 display | 頁面標題（h2） |
| `--text-hero` | 32／1.2 特粗 display | 首次設定主標 |

字重：400、600（UI 強調）、700（頁面標題）。微軟正黑體只有 400／700，600 在中文會落到 700。數字欄一律 `font-variant-numeric: tabular-nums`。段落 `text-wrap: pretty`，標題 `text-wrap: balance`。閱讀寬度 `--measure`（44em）、表單 `--measure-form`（46em）。

## 5. 間距、圓角、陰影、動態

- 間距以 4px 為基準：`--space-1`…`--space-9` ＝ 2／4／6／8／12／16／24／32／48。標題與內容 8–12px，區段之間 32px（`.page` 的 gap）。
- 圓角有層次：`sm` 4（行內 code、核取方塊）、`md` 6（控制項、嵌入區）、`lg` 10（物件、面板、清單外框）、`xl` 14（對話框）、`full`（徽章）。
- 陰影只給浮在內容上的東西（`--shadow-popover`：選單、HelpTip、對話框）。
- 控制項高 30px（小按鈕 24px）。
- 動態：只對色彩、透明度、transform 做 120ms 過渡；接力棒 220ms。不做進場動畫、不做 hover 位移。`prefers-reduced-motion` 時全部歸零。

## 6. 版面與資訊架構

```
┌──────────────┬───────────────────────────────────────────────┐
│ Baton        │ （通知：儲存故障、可以更新）                    │
│ Workspace 新增│ 筆記  C:\work\notes  複製路徑     ← 情境列       │
│ ┃筆記        │ 工作                         [建立 Task]  ← h2  │
│ ┃C:\…  待裁定2│                                                │
│  文件        │ 區段 h3                                         │
│  C:\…\docs  │ 內容……                                          │
│ ──────────── │                                                │
│  進行        │                                                │
│  ▌Controller │                                                │
│   工作 …     │                                                │
│  產出／脈絡／規則…                                              │
│ ──────────── │                                                │
│ 帳號 共同設定 App 設定                                          │
│ ● Runtime…   │                                                │
└──────────────┴───────────────────────────────────────────────┘
```

- 跑道欄 248px（1100px 以下 216px），sticky。由上到下：產品名稱 → Workspace 清單（每個 Workspace 一條「跑道」，名稱一行、路徑與待裁定數一行）→ 選中的 Workspace 的頁面（清單之後，讓其他 Workspace 與它們的待裁定數永遠在上方）→ 欄底固定的平台頁面與狀態。中段內容多時自己捲動，換頁時目前頁面會被捲進可視範圍。主區隨文件捲動，最寬 1600px，左右 32px（窄時 24px）。設計下限 1024×768。
- 內容靠左對齊。數字欄靠右。不置中整頁內容（首次設定除外）。

### 頁面與名稱（跑道欄的按鈕文字＝頁面 h2，一字不差）

| 群組 | 頁面 h2 | 元件 |
|---|---|---|
| 進行 | Controller、工作、流程、裁定、時間線 | ControllerPage、TaskBoard、WorkflowPage、DecisionCenter、TimelinePage |
| 產出 | 成果、審查、驗證、探索測試 | ArtifactsPage、ReviewPage、VerificationPage、ExplorationPage |
| 脈絡 | 知識、專案理解 | KnowledgePage、ProjectUnderstandingPage |
| 規則 | 授權、決策權限、模型設定 | AuthorizationPanel、DecisionAuthorityPanel、ModelOverridesPanel |
| 平台 | 帳號、共同設定、App 設定 | App 組合（SetupWizard＋AccountsPage＋SchedulingSettings）、SharedSettingsPage、App 組合（LoginStartup／Update／Backup／Recovery／Platform／ModelCatalogSettings） |

- 每個 Workspace 頁面元件自己輸出 `.page` 與 `.page-header`（h2 ＝ 上表名稱，主要動作在右）。
- 「帳號」「App 設定」由 App 輸出 h2，裡面的元件各自是一個區段（h3）。
- 頁面名稱的由來：「Controller」不叫「主控台」（GLOSSARY 把「主控」列為 Controller 的避免用語）；「App 設定」與「共同設定」（所有帳號共用的 Claude Code 設定）區分。
- 頁面內的區塊用 h3；物件標題用 h4 或 `.row-head` 的第一個子元素。

## 7. 元件詞彙（components.css）

| 需要 | 用 | 不要 |
|---|---|---|
| 一頁 | `<section className="page">` + `<header className="page-header"><h2>…</h2><div className="page-actions">…</div></header>` | 自己寫 flex 標題列 |
| 頁中一個區塊 | `<section className="section">` + `.section-heading`（h3 ＋ `.count` ＋ 可選 `.section-actions`，說明用 `<p>`） | 把每個區塊包成 `.card` |
| 一個東西（Task、裁定、帳號、Run） | `.card`（＋`.card-header`、可加 `.stripe-*`） | 用 `.card` 包分組或表單 |
| 一串同類物件 | `.row-list > li`（＋`.row-head`、`.meta`） | 一排獨立的卡片 |
| 一組設定或表單 | `.panel`（獨立放在紙底上）或 `.inset`（放在 surface 裡的下沉區） | 卡片裡再放卡片 |
| 標籤：值 | `<dl className="facts">` | 自己排兩欄 |
| 次要資訊一列 | `<p className="meta">` 內放 `<span>` | 「A · B · C」 |
| 兩行內的狀態說明 | `.callout` + `.callout-{accent,success,warning,danger}`，開頭狀態詞用 `.callout-title` | 整段彩色粗體字 |
| 可能超過兩行的警示（錯誤、阻擋原因、平台警告） | `.notice` + `.notice-{warning,danger,accent}`，狀態詞用 `.notice-title`；細節收進 `<details>` | 長文字放進淡底 `.callout` |
| 狀態徽章 | `.badge` + `.badge-{success,warning,danger,accent,orange,magenta}`；中性分類標籤 `.badge-plain` | 用顏色當唯一訊號 |
| 狀態點＋文字 | `.dot` + `.dot-{success,warning,danger,accent}` | 單獨的色點 |
| 額度、進度 | `.meter > span[style=width]`，數字另外寫出 | 只有長條沒有數字 |
| 互斥選項（各有說明） | `.choice-list > label.choice` | 一排擠在一起的 radio |
| 欄位 | `.field`（label → 控制項 → `.field-hint` → `.field-error`），並排用 `.field-row` | |
| 按鈕 | 預設＝次要；`type="submit"`／`.btn-primary`＝主要（每區最多一個）；`.btn-secondary` 把送出鈕改回次要；`.btn-quiet` 第三順位；`.btn-danger` 破壞性；`.btn-sm` 行內小按鈕 | 一區多個主要按鈕 |
| 第二順位以後的動作 | `.more-menu`（`<details>` 浮層） | |
| 空清單 | `.empty-note`（一行） | 加框的空白卡片 |
| 整頁空狀態 | `.empty-state`（標題＋一句下一步＋一個動作） | |
| 長文字、識別碼、路徑 | `LongText`、`ShortValue`、`PathText`、`CopyButton`（`LongText.tsx`） | 直接截斷沒有完整取得方式 |
| 解釋性小字 | `HelpTip`（「?」） | 每個欄位下一行灰字 |
| 兩欄並排的小面板 | `.split` | |
| 對話框 | `.overlay` + `.dialog-panel`（＋`.dialog-actions`）；舊名 `.exit-overlay`／`.exit-progress`／`.exit-actions` 同義 | |

### 7.1 一致性規則（第一輪審查後補充）

- **頁寬**：頁面根 `.page` 一律不設寬度上限，標題列與主要動作永遠對齊主區右緣。區段內容依性質限寬：表單 `--measure-form`；以文字為主的清單與說明（知識條目、專案理解、授權能力、Controller 狀態、成果清單）`--content-narrow`；資料型的物件與表格（看板、裁定收件匣、審查、探索、流程、時間線、帳號、共同設定的版本、App 設定的面板）用滿主區寬度。
- **計數**：`.count` 只放數字，不加括號或單位，放在標題元素外面；0 筆時一律顯示 0（整組隱藏只用在「本來就不常出現」的分組，例如已延後）。收合摘要的計數也用同樣寫法：「已過期的審查 1」。需要讓輔助技術讀到計數時，用外層 region 的 `aria-label`（例如「可處理（4）」）。
- **琥珀色只給要你處理的事**（待裁定、缺項要你補、逾期、需要你接受的成果）。系統自己在等的狀態（排隊、等待相依、等待額度）用中性（`.badge-plain`、`.stripe-muted`）。同一個狀態在清單、詳情、總覽用同一個色調（共用同一個 tone 函式）。
- **列內動作**：`.row-list` 與看板卡片裡的動作用次要或低調按鈕；主要按鈕留給頁首或區段裡唯一的動作。
- **對話框按鈕**：依 Windows 慣例，動作列靠右，**主要動作在前（左）、取消／關閉在後（右）**。
- **「更多」**：第二順位以後的動作一律用 `.more-menu`（`<summary>` 帶 ▾）；放在列尾時加 `.more-menu-end`。
- **展開長文字**：LongText 一律用收合摘要樣式（`.long-text-disclosure`），與 `<details>` 的「▶ 詳細資訊」一致。
- **提示與長警示的分工**：固定的短句（兩行內，例如「尚未確認授權」「額度已接近上限」）一律用 `.callout-*` 加 `.callout-title`；內容來自 Runtime、可能很長的錯誤與原因才用 `.notice-*`。同一個狀態在不同頁用同一種。`.notice` 放在物件卡、清單列或面板裡時自動只留左側色條。
- **空清單**：`.empty-note` 是一行說明，不加句號；整頁或大區塊才用 `.empty-state`，有頁首的頁面裡它的標題自動降為區段標題字級。
- **收合摘要的名稱**一律「詳細資訊」（不用「細節」）；計數放在摘要文字後面的 `.count`。
- **機器代碼**（事件類型、reason code、設定鍵）不當主要文字：顯示中文名稱，代碼放在 `title` 或「細節」裡；沒有中文名稱時才顯示代碼，並用 `.mono` 與次要色。
- **物件標題**一律 `--text-title-sm`；區段標題 `--text-title`。
- **面板內的 fieldset** 不加框（用 legend 或區段標題分組）；物件卡裡不再放有框的子卡片，用細線或留白分隔。
- **路徑**一律 `PathText`／`ShortValue`／`.mono`；版本號、雜湊、識別碼用 `.mono`。

工具 class：`.mono`、`.tabular`、`.nowrap`、`.truncate`、`.clamp-1/2/3`、`.pre-wrap`、`.measure`、`.measure-form`、`.no-shrink`、`.visually-hidden`。

各頁專屬樣式放在該頁的 CSS 檔（`tasks.css`、`decisions.css`、`quality.css`、`more.css`、`accounts-settings.css`），以頁面根 class 開頭（例：`.task-board .task-column`），只寫共用元件沒有的部分。

## 8. 文字

- 從使用者的角度命名：說「Task 卡住了，需要你決定」，不說「blocked_on_decision」。
- 按鈕用動詞，說清楚會發生什麼：「送出裁定」「建立 Task」「換帳號重開」。同一個動作從頭到尾同名（按鈕「封存」→ 訊息「已封存」）。
- 錯誤說明發生什麼、怎麼處理，不道歉、不含糊。空狀態告訴人下一步。
- 句子式大小寫、不加多餘的形容詞；不在標題裡強調單一詞。
- 詞彙依 `GLOSSARY.md`（Workspace、Goal、Task、Run、Artifact 稱「成果」、Decision Request 稱「裁定」）。

## 9. 品質底線

- 鍵盤：所有互動元素可聚焦，焦點外框清楚（`--color-focus`，跑道欄內 `--rail-focus`）；Esc 收起浮層並把焦點還給觸發者。
- 1024×768 與 1440×900、淺色與深色、normal 與 long 資料集都不可出現水平捲軸、文字重疊或被裁切而沒有完整取得方式。
- 長 token（路徑、雜湊、URL）一律可斷；flex／grid 子元素要能縮小（`min-width: 0`）。
- `prefers-reduced-motion` 時沒有動態。
- 截圖檢查：`node scripts/ui-screenshots.mjs --label <名稱> [--pages id,id]`，輸出到 `.scratch/ui-shots/<名稱>/`（`index.html` 並排、`results.json` 記錄水平捲軸與 console error）。頁面 id 見 `web/src/preview/pages.ts`。
