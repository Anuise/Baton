# Baton 設計系統（MASTER）

> 規則：實作某一頁時，先看 `design-system/baton/pages/<頁面>.md`；有這個檔案就以它覆寫本檔，沒有就照本檔。
> 本檔由 ui-ux-pro-max 2.15.0 以 `--design-system --persist` 產生（原始輸出：`skill-raw-output.md`），再依 Baton 的產品性質改寫。
> 每項決定的「依據」欄：`Q<n>` 是 `skill-queries.md` 中的查詢編號；`QR §n` 是 skill 的 `references/quick-reference.md` 章節；`PR` 是 `references/pro-rules.md`；**自訂**表示 skill 沒有給出或給出的不適用，理由寫在同一列。

**產品**：Windows 桌面 App（Tauri + React，WebView2／Chromium），協調多個 Claude Code 帳號的派工、裁定、驗證、成果與知識。專業開發者工具、資訊密集、繁體中文、大量長文字（契約、說明、Windows 路徑、識別碼、雜湊、錯誤訊息）。
**風格**：Data-Dense Dashboard（Q3 的 Style）＋ Flat Design 的互動原則（Q2）：中性 slate 灰階、單一深藍強調色、細框不加陰影、緊湊但行高充足。
**技術限制**：純 CSS + token（`web/src/styles.css` 的 `:root`），不加 Tailwind、元件庫或網路字型；所有元件只取 token，不寫死色碼（QR §6 `color-semantic`、PR `Token-driven theming`）。
**密度**：`--density 8`（Dense / Dashboard）。

---

## 1. 不採用的 skill 建議（反駁）

| skill 建議 | 來源 | 不採用的理由 |
|---|---|---|
| Page Pattern：Enterprise Gateway（Hero／客戶 Logo／Contact Sales）、FAQ Landing、Product Demo | Q1–Q3 | 三者都是行銷網站的頁面結構；Baton 是常駐的桌面工作工具，沒有 Hero、CTA 轉換或銷售漏斗。 |
| Google Fonts（Fira Code + Fira Sans、JetBrains Mono + IBM Plex Sans、Inter、Noto Sans TC） | Q1–Q3、Q9 | 桌面 App 必須離線可用，不能 `@import` 網路字型；這些拉丁字型沒有中文字形，中文會落回系統字型，同一行中英混排兩種字型的筆畫粗細、x-height 對不齊。Noto Sans TC（Q9）方向正確，但自帶要多數 MB／字重，Windows 已內建同類的「Microsoft JhengHei UI」。 |
| 強調色 amber `#D97706` 當主要按鈕（CTA） | Q3 | Baton 的「待裁定」「警告」「已逾期」已用琥珀色；CTA 也用琥珀會讓「要你處理」與「可以按」混淆。維持單一強調色（QR §4 `primary-action`）。 |
| 前景色 `#1E3A8A`（深藍文字） | Q3 | 大量長文字用彩色前景會疲勞，也讓連結（強調色）失去辨識度；正文改中性 slate。 |
| 邊框 `#DBEAFE`（淡藍） | Q3 | 帶色邊框在資料密集畫面形成大量藍線，干擾語意色；改中性。 |
| 按鈕 `padding: 12px 24px`、圓角 8px；卡片 `padding: 24px`、圓角 12px、`shadow-md`；對話框圓角 16px、`padding: 32px` | Q3 Component Specs | 這是行銷頁尺度；密集工具每張卡片多佔 24px 就少看一張。維持 28px 控制項高度與 4／6／8px 圓角。 |
| 卡片 hover `transform: translateY(-2px)`、按鈕 hover `translateY(-1px)` | Q3 Component Specs | 與 skill 自己的反模式「Layout-shifting hovers」（同一份輸出的 Anti-Patterns）及 PR `Stable Interaction States` 衝突；清單卡片位移會讓滑鼠下的目標跳動。只換底色與框色。 |
| 輸入框 `font-size: 16px`、內文最少 16px | Q3 Inputs、Q15 Readable Font Size | 這兩條是避免 iOS 自動放大的行動裝置規則（Q15 原文 "on mobile"）；桌面 WebView 沒有這個行為。內文用 14px（中文在 Windows 100% 縮放下仍清楚），需要更大字的人以 Ctrl +／− 縮放。 |
| 「Light mode default」是反模式、Dark Mode (OLED) | Q1 | 開發者工具不能假設使用者偏好；兩種模式都要完整、跟隨系統（`prefers-color-scheme`）。只採 Q1 的深色 slate 底色層次。 |
| 觸控目標 44×44、safe area | PR、QR §2 | 適用行動裝置（PR 開頭明言）；桌面指標目標採 WCAG 2.2 的 24×24（QR §1 `web-target-size`）。 |
| `backdrop-filter: blur(4px)` 遮罩 | Q3 Modals | 對話框背後是 App 自己的畫面，模糊沒有傳達資訊（QR §4 `blur-purpose`），而且在大量文字畫面上成本高；用半透明遮罩即可。 |
| GSAP 動畫、stagger、spring | SKILL.md `--motion` | 沒有使用 `--motion`；工具型介面每個畫面最多 1–2 個有意義的過場（Q14 Excessive Motion），不引入動畫相依。 |
| Responsive：375／768px | Q3 Pre-Delivery Checklist | 桌面 App 視窗以 1024px 為設計下限（第 8 節）；375／768 不在支援範圍。 |

---

## 2. 色彩

依據：Q3（primary／secondary／background／muted-foreground）、Q1（深色 slate 層次）、QR §6 `color-dark-mode`、`color-accessible-pairs`、Q8、PR `Light/Dark Mode Contrast`。
對比值以 WCAG 2.x 相對亮度公式計算（文字需 ≥ 4.5:1，控制項邊界與焦點需 ≥ 3:1）。

### 2.1 淺色（`:root`）

| Token | 值 | 用途 | 對比（對 surface `#FFFFFF`，另註者除外） | 依據 |
|---|---|---|---|---|
| `--color-bg` | `#F8FAFC` | 視窗底色 | — | Q3 Background |
| `--color-surface` | `#FFFFFF` | 卡片、面板、側欄 | — | Q3 Card |
| `--color-surface-raised` | `#FFFFFF` | 對話框、浮層（以 `shadow-md` 區分） | — | 現行保留 |
| `--color-surface-sunken` | `#F1F5F9` | 卡片內嵌區（程式碼、看板欄、次要資訊） | — | 自訂：Q3 Muted `#E9EEF6` 帶藍，改中性 slate-100 |
| `--color-surface-hover` | `#F1F5F9` | 可點元素滑過 | — | 自訂（同上） |
| `--color-overlay` | `rgb(15 23 42 / 45%)` | 對話框遮罩 | — | 自訂：以 text 色為基底 |
| `--color-border` | `#E2E8F0` | 分隔線、卡片框 | 1.23（裝飾性分隔，不受 3:1 限制） | 自訂 |
| `--color-border-strong` | `#CBD5E1` | 次要按鈕框、看板欄框 | 1.48（按鈕以文字辨識，屬 WCAG 1.4.11 例外） | 自訂 |
| `--color-control-border` | `#7D8CA3` | **新增**：輸入框、選單、核取方塊的邊界 | 3.41（bg 上 3.26） | 自訂：WCAG 1.4.11 要求辨識控制項的邊界 ≥ 3:1；現行 `#d2d4da` 只有 1.48 |
| `--color-text` | `#0F172A` | 主要文字 | 17.85（bg 17.06） | Q1 Background 值當淺色文字；QR §6 `contrast-readability` |
| `--color-text-muted` | `#475569` | 次要文字、標籤、表頭 | 7.58（sunken 6.92） | Q3 Muted Foreground |
| `--color-text-subtle` | `#64748B` | 最淡的文字：計數、placeholder、時間戳 | 4.76（bg 4.55）；**不可放在 sunken 上（4.34）**，sunken 上改用 muted | 自訂：現行 `#9a9ea6` 只有 2.69，不合格 |
| `--color-accent` | `#1E40AF` | 唯一強調色：選取、連結、主要按鈕底 | 8.72；在 accent-subtle 上 8.01 | Q3 Primary |
| `--color-accent-hover` | `#1E3A8A` | 主要按鈕滑過 | 10.36 | 自訂（Q3 Foreground 值） |
| `--color-accent-subtle` | `#EFF6FF` | 選取列底色、資訊提示底 | — | 自訂 |
| `--color-accent-border` | `#BFDBFE` | 選取列框、資訊提示框 | — | 自訂 |
| `--color-on-accent` | `#FFFFFF` | accent 底上的文字 | 8.72 | Q3 On Primary |
| `--color-focus` | `#3B82F6` | **新增**：鍵盤焦點外框 | 3.68（bg 3.52） | Q3 Secondary；Q8 Focus Appearance 需 ≥ 3:1 |
| `--color-success` ／ `-subtle` ／ `-border` | `#15803D` ／ `#F0FDF4` ／ `#BBF7D0` | 通過、運作中 | 5.02；在 subtle 上 4.79 | 自訂：Q3 未給 success |
| `--color-warning` ／ `-subtle` ／ `-border` | `#B45309` ／ `#FFFBEB` ／ `#FDE68A` | 待裁定、逾期、需注意 | 5.02；在 subtle 上 4.84 | 自訂 |
| `--color-danger` ／ `-subtle` ／ `-border` | `#B91C1C` ／ `#FEF2F2` ／ `#FECACA` | 失敗、錯誤、破壞性動作 | 6.47；在 subtle 上 5.91 | 自訂：Q3 Destructive `#DC2626` 在白底只有 4.83、淡紅底更低，改深一階 |
| `--color-orange` ／ `-subtle` | `#C2410C` ／ `#FFF7ED` | 只用於驗證判定（環境受阻） | 5.18；在 subtle 上 4.88 | 現行用途保留，值調整 |
| `--color-purple` ／ `-subtle` | `#6D28D9` ／ `#F5F3FF` | 只用於驗證判定（授權受阻） | 7.10；在 subtle 上 6.48 | 現行用途保留，值調整 |

### 2.2 深色（`@media (prefers-color-scheme: dark)`）

| Token | 值 | 對比（對 surface `#1B2336`，另註者除外） | 依據 |
|---|---|---|---|
| `--color-bg` | `#0F172A` | — | Q1 Background |
| `--color-surface` | `#1B2336` | — | Q1 Card |
| `--color-surface-raised` | `#222B40` | — | 自訂：比 surface 亮一階 |
| `--color-surface-sunken` | `#141C2F` | — | 自訂：介於 bg 與 surface |
| `--color-surface-hover` | `#272F42` | — | Q1 Muted |
| `--color-overlay` | `rgb(0 0 0 / 60%)` | — | 現行保留 |
| `--color-border` | `#334155` | 1.51 | 自訂：Q1 Border `#475569` 用在密集分隔線太亮 |
| `--color-border-strong` | `#475569` | 2.07 | Q1 Border |
| `--color-control-border` | `#64748B` | 3.29 | 自訂（WCAG 1.4.11） |
| `--color-text` | `#F8FAFC` | 14.98（bg 17.06） | Q1 Foreground |
| `--color-text-muted` | `#94A3B8` | 6.11（bg 6.96、sunken 6.62、hover 5.21） | Q1 Muted Foreground |
| `--color-text-subtle` | `#8391A7` | 4.90 | 自訂：現行 `#6c6f78` 只有 3.50 |
| `--color-accent` | `#60A5FA` | 6.16（bg 7.02）；在 accent-subtle 上 5.56 | 自訂：Q3 Primary 在深色底上太暗，取同色相亮階 |
| `--color-accent-hover` | `#93C5FD` | 8.69 | 自訂 |
| `--color-accent-subtle` ／ `-border` | `#1E2A4A` ／ `#2B4C8C` | text 在 subtle 上 13.51 | 自訂 |
| `--color-on-accent` | `#0F172A` | 在 accent 上 7.02 | 自訂 |
| `--color-focus` | `#60A5FA` | 6.16 | 自訂 |
| `--color-success` ／ `-subtle` ／ `-border` | `#4ADE80` ／ `#14301F` ／ `#166534` | 8.99；在 subtle 上 8.19 | 自訂 |
| `--color-warning` ／ `-subtle` ／ `-border` | `#FBBF24` ／ `#3A2A12` ／ `#854D0E` | 9.39；在 subtle 上 8.28 | 自訂 |
| `--color-danger` ／ `-subtle` ／ `-border` | `#F87171` ／ `#3B1D24` ／ `#991B1B` | 5.66；在 subtle 上 5.47 | 自訂 |
| `--color-orange` ／ `-subtle` | `#FB923C` ／ `#3A2216` | 6.92；在 subtle 上 6.53 | 自訂 |
| `--color-purple` ／ `-subtle` | `#C4B5FD` ／ `#2A2346` | 8.49；在 subtle 上 7.97 | 自訂 |

### 2.3 色彩使用規則

1. 語意色只標示狀態；**長內容不整塊上色**。狀態以「左側 3px 色條 + 狀態徽章（文字）」表達，內文維持 surface 底與 text 色。淡色底（`*-subtle`）只用於 2 行以內的提示框（自訂，見 audit C8；QR §6 `color-not-decorative-only` 要求顏色之外還有文字）。
2. 錯誤與警告訊息的正文用 `--color-text`，只有開頭的狀態詞（「未通過」「已逾期」）用語意色與 600 字重；整段粗體彩色字在長訊息下難讀（自訂，見 audit C8）。
3. 不以顏色單獨傳達（QR §1 `color-not-only`）：狀態點旁一律有文字。

---

## 3. 字型與字級

### 3.1 字型堆疊（自訂；依 Q9 的方向，反駁 Q1–Q3 的網路字型）

```css
--font-sans: "Segoe UI Variable Text", "Segoe UI", "Microsoft JhengHei UI", "Microsoft JhengHei",
             "PingFang TC", "Noto Sans TC", system-ui, sans-serif;
--font-mono: "Cascadia Mono", "Cascadia Code", Consolas, "Microsoft JhengHei UI", monospace;
```

- 拉丁字在前、CJK 在後：英數用 Segoe UI（Windows 11 有 Variable 版，字距較好），中文落到「微軟正黑體 UI」（UI 版行距較緊、適合介面）。Q9 建議的 Noto Sans TC 保留在堆疊中作為已安裝時的備援，不打包。
- 等寬字型的 CJK 備援放 JhengHei UI，避免路徑中的中文資料夾名落到新細明體。
- 移除現行的 `ui-monospace`（Windows 的 Chromium 不認得，沒有作用）。
- 字重只用 400／600：JhengHei 沒有 500，500 與 300 在多數機器上會被合成或退回。

### 3.2 字級階層

依據：Q15 Font Size Scale（一致級距；Q15 的 16px 下限是行動規則，不採用）、QR §6 `weight-hierarchy`、Q5 Line Height（1.5–1.75）。中文需要比拉丁字更高的行高（自訂：CJK 字面框滿版、沒有 x-height 留白）。

| Token | 字級 | 行高 | 字重 | 用途 |
|---|---|---|---|---|
| `--text-caption` | 12px | 1.4 | 400／600 | **只用於**徽章、計數、時間戳、狀態列。不可用於段落。 |
| `--text-meta` | 13px | 1.5 | 400 | 控制項（按鈕、輸入、頁簽）、表格儲存格、卡片的次要資訊、標籤（dt） |
| `--text-body` | 14px | 1.65 | 400 | 一切段落與長內容（契約、理由、錯誤說明、知識內文）；全站基準 |
| `--text-title-sm` | 14px | 1.4 | 600 | 區塊標題（h3／h4）、卡片標題 |
| `--text-title` | 16px | 1.35 | 600 | 頁面標題（h2） |
| `--text-title-lg` | 20px | 1.3 | 600 | 全畫面精靈、空狀態主標 |
| 等寬 | 0.92em | 繼承 | 400 | 程式碼、路徑、識別碼、雜湊；`<pre>` 內行高 1.5 |

- 現行 `.task-card { font-size: var(--font-size-xs) }`（styles.css:827）、`.decision-task/.decision-meta`（styles.css:1464）把長段落放在 12px，違反上表；卡片內文最小 13px。
- 數字欄、計數、百分比、版本號：`font-variant-numeric: tabular-nums`（QR §6 `number-tabular`）；大數字加千分位（Q19 Number Formatting）。
- 標題：短標題可加 `text-wrap: balance`（Q5 Heading Line Balance），長標題（> 2 行）不加。
- 段落：`text-wrap: pretty`（WebView2 支援；不支援時自然退回），減少中文段尾孤字。

---

## 4. 間距、圓角、陰影、層級、動態

### 4.1 間距（Q3 `--density 8` 表 + 自訂）

| Token | 值 | 用途 |
|---|---|---|
| `--space-1` | 2px | 徽章內距（上下） |
| `--space-2` | 4px | 圖示與文字、標籤與欄位 |
| `--space-3` | 6px | 控制項內部、同列小元素（自訂：Q3 表沒有 6px，保留給 28px 控制項） |
| `--space-4` | 8px | 標準間距、清單項之間、標題與內容 |
| `--space-5` | 12px | 卡片內距、欄位之間、卡片之間 |
| `--space-6` | 16px | fieldset 之間、窄視窗的頁面內距 |
| `--space-7` | 24px | 區塊之間、寬視窗的頁面內距 |
| `--space-8` | 32px | 頁面主要分段 |

與 Q3 density 8 表（2／4／8／12／16／24／32）一一對應，只多 6px。規則（PR `Section spacing hierarchy`）：同一層級的元素間距一致；**標題與它的內容 8px，與上一個區塊 24px**（標題靠近它所屬的內容）。現行 token 名稱沿用，不改名。

### 4.2 圓角（自訂，反駁 Q3 的 8／12／16）

`--radius-sm: 4px`（行內 code、核取方塊）、`--radius-md: 6px`（按鈕、輸入、卡片）、`--radius-lg: 8px`（對話框、浮層）、`--radius-full`（徽章、膠囊頁簽）。與現行相同。

### 4.3 陰影（Q2 Flat「No gradients/shadows」+ QR §4 `elevation-consistent`）

| Token | 淺 | 深 | 用途 |
|---|---|---|---|
| `--shadow-sm` | `0 1px 2px rgb(15 23 42 / 6%)` | `0 1px 2px rgb(0 0 0 / 30%)` | 只用於浮在內容上的小元件 |
| `--shadow-md` | `0 8px 24px rgb(15 23 42 / 14%), 0 1px 3px rgb(15 23 42 / 8%)` | `0 8px 24px rgb(0 0 0 / 45%), 0 1px 3px rgb(0 0 0 / 30%)` | 對話框、下拉浮層、HelpTip |

卡片與按鈕**不加陰影**，只用邊框分層（現行 `:where(button)`（styles.css:211）與 `.task-card`（styles.css:826）都有 `shadow-sm`，改版移除）。

### 4.4 層級（Q20 Z-Index Management）

`--z-sticky: 10`（固定的標題列）、`--z-popover: 20`（HelpTip、選單）、`--z-overlay: 30`（對話框與遮罩）、`--z-toast: 40`（全域通知、完全退出進度）。

### 4.5 動態（Q2、Q14、QR §7）

- 只對 `color`、`background-color`、`border-color`、`opacity`、`transform` 做 120ms `ease-out` 過渡；不對 width／height 做動畫（QR §7 `transform-performance`）。
- `@media (prefers-reduced-motion: reduce)` 下一律 `transition: none`（Q14）。

---

## 5. 元件規則

### 5.1 按鈕（QR §4 `primary-action`、Q21、QR §1 `web-target-size`）

| 種類 | 樣式 | 用法 |
|---|---|---|
| 主要 `.btn-primary`（含現行 `button[type=submit]`、`.primary-action`） | accent 底、on-accent 字 | **每個區域最多一個**（一張卡、一個表單、一個頁面標題列） |
| 次要（預設 `button`） | surface 底、`border-strong` 框、text 字 | 一般動作 |
| 低調 `.btn-quiet` | 無框、無底，滑過 surface-hover | 卡片內第三順位以後的動作、「展開」「複製」 |
| 危險 `.btn-danger` | surface 底、danger 字與框；確認對話框內才用 danger 實心 | 取消 Task、封存（QR §8 `destructive-emphasis`、`confirmation-dialogs`） |

- 尺寸：高 28px（`--text-meta`），水平內距 12px；圖示按鈕 28×28；最小可點範圍 24×24。
- 停用：`opacity: .5` + `cursor: not-allowed` + `disabled` 屬性（Q21）；能說明原因時，原因用文字放在按鈕旁，不只靠 title。
- **按鈕不可被 grid 拉滿整列**：放在 `display: grid` 容器裡的按鈕加 `justify-self: start`；表單底部的動作放 `.form-actions`（flex、`gap: 8px`、`flex-wrap: wrap`）。（自訂，見 audit C7）
- 按鈕文字不換行：`white-space: nowrap`；一組按鈕放不下時整組換行，不讓單一按鈕折成兩行。
- 一張卡片的動作超過 3 個時：第一順位顯示為按鈕，其餘收進「更多」選單（`<details>` 浮層），見 audit P-TASK-3。

### 5.2 卡片

- `surface` 底、1px `border`、`radius-md`、內距 12px、無陰影；卡片之間 12px。
- 卡片結構固定為三段：**標題列**（標題 + 狀態徽章，flex；標題 `flex: 1; min-width: 0`）→ **摘要**（最多 3 行，見長文字策略）→ **細節**（`<details>` 收合）→ 動作列。
- 狀態以左側 3px 色條（`border-left`）＋ 徽章表示（見 2.3），不用整卡淡色底。
- 可點的整張卡片：滑過換 `border-strong`，選取時 `accent-border` + `accent-subtle` 底；不位移（反駁 Q3 的 hover transform）。

### 5.3 表格（Q7、Q17、QR §5 `horizontal-scroll`）

- 外層 `.table-wrap { overflow-x: auto; max-width: 100%; }`，表格本身 `width: 100%`。
- 資料表用 `table-layout: fixed` 並以 `<colgroup>` 指定欄寬：識別碼欄 `12ch`（顯示縮短形式）、時間欄 `9.5em`、狀態欄 `7em`、數字欄依位數並 `text-align: right; font-variant-numeric: tabular-nums`，**剩下的寬度全給一個文字欄**（`width: auto`）。
- 儲存格 `overflow-wrap: anywhere; vertical-align: top`；表頭 `--text-meta`、muted、600、`white-space: nowrap`。
- 列高：上下 6px（密度 8）；滑過整列 surface-hover（Q3 Key Effects「row highlighting on hover」）。
- 只有欄位超過 6 欄時才允許水平捲動；6 欄以內要能在 1024px 寬內完整顯示。

### 5.4 徽章與狀態標籤（Q4／Q6 Compact Label Overflow、QR §5 `chip-collection-reflow`）

- `display: inline-flex; white-space: nowrap; flex-shrink: 0;` 高 18px、`--text-caption`、`radius-full`、水平內距 6px。
- **計數徽章**：`tabular-nums`；超過 999 顯示 `999+`，完整數字放在可存取名稱（`aria-label="待裁定 1,284 項"`）與 `title`（Q6 Contextual Live Badge Updates）。
- **文字徽章**（例：「候選，尚未驗證」）：一行，最多 6 個全形字；放不下時改用短詞（「未驗證」），不截斷、不換行。
- 多個徽章並排時整組換行（`flex-wrap: wrap; gap: 4px`），不縮小單一徽章。
- 語意：成功／警告／危險／中性四種；底 `*-subtle`、字語意色、框 `*-border`。

### 5.5 表單（QR §8 `input-labels`、`error-placement`、`input-helper-text`、`read-only-distinction`）

- 欄位結構：label（`--text-meta`、600）→ 控制項 → 說明（`--text-meta`、muted）→ 錯誤（danger、`aria-describedby` 連到控制項）。label 與控制項間 4px，欄位之間 12px，fieldset 之間 16px。
- 控制項：高 28px、`control-border` 框、`radius-md`；`max-width: 100%; min-width: 0`（避免 `<select>` 被最長選項撐寬，見 6.4）。
- 控制項寬度依內容性質，不一律滿版：短值（數字、token 上限）`width: 12em`；選單 `width: 100%; max-width: 32em`；長文字 textarea 滿版但受閱讀寬度上限（6.3）。
- **textarea 依內容長高**：`field-sizing: content; min-height: 3lh; max-height: 16lh; overflow-y: auto`（WebView2 為 Chromium，支援 `field-sizing`）；不支援時維持 `rows` 屬性的高度。
- 同列的 label + 控制項（例：帳號頁的方案上限）用 `display: inline-flex; align-items: center; gap: 6px`，label 與控制項不得貼齊。
- fieldset 已經以 legend 說明分組時，內部 label 不重複分組名稱（例：fieldset「目標」內的 label 只寫「模式」「內容」）；需要完整可存取名稱時用 `aria-label` 或視覺隱藏文字補上。
- 焦點：`outline: 2px solid var(--color-focus); outline-offset: 2px`（Q8、Q12）。

### 5.6 對話框（Q12、QR §1 `escape-routes`、`focus-not-obscured`）

- `<dialog>` 以 `showModal()` 開啟；`surface-raised` 底、`radius-lg`、`shadow-md`、內距 16px；寬 `min(640px, 100vw - 48px)`，需要並排比較時 `min(880px, 100vw - 48px)`。
- 結構：標題列（標題 + 關閉鈕）→ 內容（`max-height: calc(100vh - 160px); overflow-y: auto`）→ 動作列（靠右：次要在左、主要在右；危險動作與主要動作分開）。
- 遮罩用 `--color-overlay`，不加 blur（反駁 Q3）。Esc 與關閉鈕都能離開；有未儲存內容時先確認（QR §8 `sheet-dismiss-confirm`）。
- 開啟時焦點移到第一個可操作欄位，關閉後回到觸發按鈕。
- 現況：完全退出進度（`.exit-overlay`）是唯一的遮罩層；「編輯契約」是在看板上方展開的行內表單，不是對話框（見 audit P-TC-5）。

### 5.7 空狀態（Q11）

- 置中於內容區、上方留 1/4 高度，最大寬度 40em：一行標題（`--text-title-sm`）＋ 一行說明（muted）＋ 一個主要動作（若有可做的事）。
- 例：「請從左側選擇或建立 Workspace」應加上「建立 Workspace」按鈕（聚焦側欄表單的目錄欄位）。
- 區塊內的空清單（「沒有延後的裁定」）只用一行 muted 文字，不加框。

### 5.8 頁簽

- 第一層：底線式，選取時 accent 底線 2px + text 色 600。第二層：膠囊式（現行保留）。
- 頁簽文字與徽章 `white-space: nowrap`；頁簽列放不下時水平捲動（`overflow-x: auto; scrollbar-width: thin`），不換成兩列。

---

## 6. 長文字策略

依據：Q4（Truncation、**Essential Text Truncation（Critical）**、**Long Token Wrapping（High）**、**Compact Label Overflow（High）**）、Q5、Q7、QR §6 `truncation-strategy`（優先換行，截斷要有完整文字的取得方式）、`long-token-wrapping`、QR §5 `horizontal-scroll`。

### 6.1 內容分類與處理方式

| 內容類別 | 例 | 清單／卡片中 | 詳情／編輯中 | 理由 |
|---|---|---|---|---|
| **標題**（必要文字） | Task 標題、裁定問題、知識標題、帳號名稱 | **限 2 行**（`clamp-2`）＋ 整張卡片可點開詳情；`title` 放完整文字 | **完整換行**，不截斷 | Q4 Essential Text：清單中的完整取得路徑是「點開詳情」，不是 hover |
| **頁面標題中的 Workspace 名稱** | 「主控台：<Workspace 名稱>」 | — | 頁面標題只寫頁名；Workspace 名稱在頁簽列上方的情境列顯示一次，1 行截斷 + `title`，完整名稱在側欄 | 同一個長名稱在每頁重複成 2 行標題（audit C2） |
| **段落**（說明、理由、契約內容、知識內文、Router 理由、Goal） | — | **限 3 行**（`clamp-3`）＋「展開」低調按鈕 | 完整換行，受閱讀寬度上限 | Q4 Truncation「line-clamp with expand」 |
| **錯誤與警告**（必要文字） | storage fault、驗證失敗原因、還原演練失敗 | 第一句（摘要）完整顯示；技術細節（exit code、完整路徑、SQLite 代碼）收進「技術細節」`<details>` | 完整 | Q4 Essential Text：錯誤不可只剩省略號 |
| **識別碼** | task_…、run_…、ws_…、vbun_… | **中間省略**：前 10 + `…` + 後 4（`task_7b5deb…5c4a`），等寬；旁邊有「複製」低調按鈕，`title` 放完整值 | 完整顯示，等寬，`overflow-wrap: anywhere` | 識別碼的辨識度在頭尾 |
| **雜湊**（SHA-256、commit） | e3b0c442… | 前 12 字（沿用 TaskBoard.tsx:184 `shortHash`）+ 複製 | 完整，等寬，`overflow-wrap: anywhere` | 同上 |
| **路徑**（Windows 路徑、URL） | `C:\Users\…\report.md` | 1 行，**保留開頭磁碟與結尾 2 段，省略中間**（`C:\…\fixtures\report.md`），等寬，`title` 完整 + 複製 | 完整，等寬，`overflow-wrap: anywhere` | 路徑最有用的是開頭與檔名 |
| **指令與 JSON**（`<pre>`） | 工具輸入、Profile JSON、安裝指令 | 收在 `<details>` 裡 | 結構化資料（JSON）：`white-space: pre; overflow: auto; max-height: 24lh`；單行指令：`pre-wrap` + `overflow-wrap: anywhere` | 程式碼不可折行改變意義；外層必須 `min-width: 0` |
| **徽章、計數** | 待裁定 1284 | 一行不換行，999+ | 一行 | Q4／Q6 Compact Label |
| **選單選項** | Task、帳號下拉 | 選單 `max-width: 100%; min-width: 0`；選項文字由程式縮短為 60 字以內（標題前段 + 識別碼後 4 碼） | — | 原生 `<select>` 以最長選項為最小寬度（audit C1） |

**判斷原則**：能換行就換行；只有在「清單中、高度必須可預期」時才截斷，而且截斷一定有完整文字的取得路徑——點開詳情、展開按鈕或複製按鈕。`title` 只是輔助，不能是唯一路徑（Q4 Compact Label：不可只靠 hover tooltip）。

### 6.2 斷行規則（CSS）

```css
/* 全站：長 token 不撐破版面 */
:where(body) { overflow-wrap: break-word; }
:where(p, li, dt, dd, td, th, h1, h2, h3, h4, h5, summary, label, legend) { overflow-wrap: anywhere; }

/* 等寬的識別碼、路徑、雜湊、URL：可在任何字元斷開 */
.mono, :where(code, kbd, samp) { font-family: var(--font-mono); overflow-wrap: anywhere; }

/* 單行截斷（必須另有完整文字的取得路徑） */
.truncate { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* 多行截斷 */
.clamp-2, .clamp-3 { display: -webkit-box; -webkit-box-orient: vertical; overflow: hidden; }
.clamp-2 { -webkit-line-clamp: 2; line-clamp: 2; }
.clamp-3 { -webkit-line-clamp: 3; line-clamp: 3; }
```

- **不使用** `word-break: break-all`（Q4 Long Token Wrapping 明列為反模式）：中文本來就逐字可斷，`break-all` 只會讓英文單字在中間斷開。
- `overflow-wrap: anywhere` 與 `break-word` 的差別：`anywhere` 會把斷點算進 min-content，**flex／grid 子元素才真的能縮小**；所以放在會進入 flex／grid 的文字元素上。現行只在 19 條個別規則中加了 `anywhere`（例：styles.css:435、828、2370），沒有全站規則。
- 中文標點避頭尾由瀏覽器處理（`line-break: auto`），不另設 `strict`（嚴格模式在窄欄會留下大段空白）。

### 6.3 閱讀寬度上限（Q5 Line Length、Q7 Container Width、PR `Readable text measure`）

- 段落與長內容：`max-inline-size: 46em`（14px 下約 644px，中文約 46 字／行；英文約 90 字元）。Q5 的 65–75 字元是拉丁字的建議；中文以每行 35–50 字為宜（自訂）。
- 表單：`max-inline-size: 48em`（取代現行 `.task-form`、`.authorization`、`.model-overrides` 的 `max-width: 720px`，改用 em 讓 Ctrl+ 縮放時一起變）。
- 清單、表格、看板、卡片網格**不設**上限，填滿可用寬度；但卡片**內**的段落仍受 46em 限制。

### 6.4 flex／grid 子元素 min-width（Q4 Long Token Wrapping「let flex or grid text children shrink」）

flex 與 grid 子元素的 `min-width` 預設是 `auto`（＝內容的 min-content），一個不能斷的長 token、`<pre>` 或 `<select>` 就會把整個欄位、進而整頁撐寬。baseline 的 22 張水平捲軸截圖全部出自這個原因（audit C1）。規則：

1. 版面骨架：`.main { min-width: 0 }`；所有「一欄」的 grid 容器明寫 `grid-template-columns: minmax(0, 1fr)`，多欄寫 `minmax(0, Nfr)`——**不寫裸的 `1fr`／`2fr`**（裸 `fr` 的最小值是 `auto`，例：styles.css:1417 `.decision-layout`）。
2. flex 列中「放文字、應該縮小的那一個」：`flex: 1 1 auto; min-width: 0`；「不該縮小的」（按鈕、徽章、時間）：`flex: none`。
3. 表單控制項：`min-width: 0; max-width: 100%`；在 flex 中的 `<select>` 用 `flex: 1 1 0; width: 0; min-width: 0`。
4. `<pre>`、`<table>`、看板等需要自己捲動的元素：自己 `overflow: auto`，**而且它的每一層祖先都要能縮小**（`min-width: 0`）。
5. dl 的兩欄 grid：`grid-template-columns: minmax(6em, max-content) minmax(0, 1fr)`，dt 最多 `12em` 並允許換行（現行多處寫 `max-content 1fr`，例：styles.css:372、1505）。

### 6.5 表格欄寬

見 5.3：`table-layout: fixed` + `<colgroup>`；只給一個文字欄彈性寬度；識別碼欄放縮短形式；表頭不換行；儲存格 `anywhere`。

### 6.6 驗證方式

改版後以 `node scripts/ui-screenshots.mjs --label <名稱>` 重跑：`results.json` 中 `horizontalScroll` 必須全部為 `false`（baseline 為 22 張 `true`），並對照 `long` 資料集的截圖檢查上表的每一類內容。

---

## 7. 版面骨架

- 外殼：狀態列（28px）→ 標題列（44px：產品名 + 頂層頁簽）→ 全域通知 → 主體（側欄 + 主區）。
- 側欄：寬 260px（≤ 1100px 時 232px）；只放 Workspace 的選擇清單與建立表單。每個項目：名稱（2 行截斷）+ 待裁定徽章（999+）＋ 1 行縮短路徑；封存、自動續跑等管理動作移出側欄（見 audit C3）。
- 主區：內距 24px（≤ 1100px 時 16px）；頁簽列上方加一行 Workspace 情境列（名稱 1 行截斷 + 縮短路徑）。
- 頁面：頁面標題列（h2 頁名 + 主要動作靠右）→ 內容；區塊之間 24px。

---

## 8. 響應式：1024px 寬（設計下限）

依據：QR §5 `breakpoint-consistency`、`horizontal-scroll`；Q3 Checklist 的 1024／1440（375／768 不採用，見第 1 節）。

斷點只有一個：`@media (max-width: 1100px)`（1024 視窗與略寬的視窗都落在這一段）。

| 區域 | ≥ 1101px | ≤ 1100px（含 1024） |
|---|---|---|
| 側欄 | 260px | 232px；Workspace 路徑隱藏，只留名稱與徽章 |
| 主區內距 | 24px | 16px |
| 裁定（清單 + 詳情） | `minmax(260px, 340px) minmax(0, 1fr)` | `minmax(220px, 280px) minmax(0, 1fr)` |
| 工作看板欄寬 | 280px，水平捲動 | 260px，水平捲動；看板左右緣加漸層提示還有欄位 |
| 觀測指標網格 | `repeat(auto-fill, minmax(200px, 1fr))` | 同左（自動減為 3 欄） |
| 表單 | 48em 上限 | 滿版（可用寬度已小於 48em） |
| dl 標籤欄 | `minmax(6em, max-content)`，最多 12em | 最多 9em |
| 頁簽列 | 一列 | 一列，放不下時水平捲動 |

1024px 時主區可用寬度約 1024 − 232 − 32 = 760px，**任何頁面不得出現水平捲軸**（看板、`<pre>`、寬表格自己的捲動除外）。

---

## 9. 交付前檢查（QR §1–§3 + 本產品）

- [ ] `long` 資料集 × 1024／1440 × 淺／深：`results.json` 沒有 `horizontalScroll: true`
- [ ] 文字對比 ≥ 4.5:1、控制項邊界與焦點 ≥ 3:1（第 2 節的值）
- [ ] 徽章不換行；計數 999+；長標題 2 行截斷且可點開詳情
- [ ] 每個區域最多一個主要按鈕；grid 中的按鈕沒有被拉滿
- [ ] 長段落 ≤ 46em；卡片段落 3 行 + 展開
- [ ] 識別碼／路徑／雜湊等寬、縮短、可複製；詳情中完整
- [ ] 焦點外框可見；`prefers-reduced-motion` 下無過渡
- [ ] 不使用 emoji 當圖示（Q3 Anti-Patterns）
