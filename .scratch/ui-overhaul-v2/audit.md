# Baton UI 稽核與改版計畫（v2）

## 視覺方向摘要（給使用者）

1. **字很多也不會撐破版面**：現在 7 個頁面在長文字下會出現水平捲軸、整頁被撐到 1100～2200px 寬；改版後任何頁面在 1024px 寬都不會左右捲動（原因與修法已實測，見 C1）。
2. **清單只放摘要，全文在詳情**：卡片標題最多 2 行、說明最多 3 行再加「展開」；識別碼、雜湊、路徑改成等寬字並縮短（`task_7b5deb…5c4a`），旁邊附「複製」，詳情裡才完整顯示。
3. **長段落有閱讀寬度**：段落每行約 46 個中文字為上限，不再橫跨整個 1400px 螢幕；內文 14px、行高 1.65（中文需要比英文更寬的行距）。
4. **顏色退到狀態標示**：不再整張卡片塗黃、整段錯誤用紅色粗體；改成左側細色條 + 狀態徽章，正文維持黑字。
5. **配色**：從現在的灰 + 紫藍（Linear 風）改成 slate 冷灰 + 深藍單一強調色（ui-ux-pro-max「Data-Dense Dashboard」）；淺、深色兩套都重算過對比，最淡的文字從現在不合格的 2.7:1（淺）／3.5:1（深）提高到 4.5:1 以上。
6. **更安靜的元件**：按鈕和卡片拿掉陰影；每張卡片只留一個主要按鈕，其餘收進「更多」；徽章不換行（`待裁定 999+`）。
7. **一致的版面骨架**：每頁標題只寫頁名（Workspace 名稱在頁簽上方顯示一次），側欄只負責選 Workspace，沒有樣式的 5 個區塊（專案理解、排程、共同設定、登入啟動、平台）補齊。

設計系統：`design-system/baton/MASTER.md`（每項決定附 skill 查詢依據：`design-system/baton/skill-queries.md`）。

---

## 0. 方法與證據

- **截圖**：`node scripts/ui-screenshots.mjs --label baseline`（commit `347e5bd`），輸出到 `C:\Users\User\Documents\au-program\Baton\.scratch\ui-shots\baseline\`：16 頁 × 2 寬（1440×900、1024×768）× 2 主題 × 2 資料集（`normal`、`long`）＝ 128 張，全部成功；console error 0 張；**水平捲軸 22 張**（`results.json`）。截圖檔名格式 `<頁面>__<資料集>__<主題>__<寬>x<高>.png`，下文直接引用檔名。
- **實際檢視**：每頁至少看過 `long`／1024 與 `normal`／1440 的淺色；`long`／1440 用於長頁面的分段裁切；深色抽看 `tasks__normal__dark__1440x900.png`、`decisions__normal__dark__1024x768.png`（深色版面與淺色一致，問題只在 token 值，見 C9）。裁切圖在 `.scratch/ui-shots/baseline-crops/`（不 commit）。
- **水平捲軸的源頭**：`results.json` 的 `offenders` 只列出被撐開的外層（h2、tabs），看不出源頭。另寫診斷腳本 `.scratch/ui-shots/diag/find-overflow.mjs`（不 commit，不改正式程式碼與截圖工具）：以 Playwright 開預覽頁，量每個元素的 min-content 寬度，找出「比可用寬度寬、而且子元素都沒有那麼寬」的元素；再以 `addStyleTag` 注入候選 CSS 重量 `scrollWidth`。結果見 C1。
- **程式位置**：行號以 commit `347e5bd` 的 `web/src/styles.css`（2955 行）為準。
- 嚴重度：**高**＝版面破壞（水平捲軸、內容被撐爆或幾乎無法閱讀）或可及性不合格；**中**＝明顯難看或難讀、但資訊仍取得到；**低**＝細節不一致。

---

## 1. 跨頁面的共通問題

### C1（高）長 token、`<select>`、`<pre>` 把整頁撐出水平捲軸

- **證據**：`results.json` 的 22 張 `horizontalScroll: true`，全部是 `long` 資料集：
  `controller__long__{light,dark}__1024x768.png`（1098px）、`authorization__long__{light,dark}__1024x768.png`（1087px）、`setup__long__{light,dark}__1024x768.png`（1139px）、`decisions__long__{light,dark}__{1024x768,1440x900}.png`（2168px）、`exploration__long__…__{1024x768,1440x900}.png`（1505px）、`knowledge__long__…`（1488px）、`timeline__long__…`（2211px）。
- **源頭**（診斷腳本的 min-content 量測）：

  | 頁面 | 撐寬的元素 | min-content |
  |---|---|---|
  | controller | `label > select`（換成帳號；選項是完整帳號名稱） | 765px |
  | authorization | `section.model-overrides label > select` ×3（模型選項） | 752px |
  | decisions | `.decision-permission details > pre`（工具輸入 JSON，`white-space: pre`）、`label > select`（「選擇要連結的請求」） | 2403px／1573px |
  | timeline | `.timeline-filter label > select`（依 Task 過濾，選項是完整 Task 標題） | 1695px |
  | knowledge | `.understanding-items`（沒有空白的長 key） | 1155px |
  | exploration | `.exploration-group-suspected` 內不能斷的 token | 1172px |
  | setup | `.setup-guidance` 的長 URL 與 `--location "C:\…"` 指令、`.profile-quota` 段落 | 1115px |

- **原因**：
  1. `.main { flex: 1 }`（styles.css:475）沒有 `min-width: 0`，flex 子元素最小寬度＝內容的 min-content。
  2. `.tab-panel`（586）、`.setup-screen`（600）與各頁的 `display: grid` 容器都沒有寫欄軌，隱含的 `auto` 欄以最寬子元素為準；`.decision-layout`（1417）寫 `minmax(220px, 1fr) 2fr`，`2fr` 的最小值是 `auto`。
  3. 原生 `<select>` 以最長選項為最小寬度；基礎規則（241）沒有 `max-width`。
  4. `overflow-wrap: anywhere` 只散在 19 條個別規則中，沒有全站規則；`ProjectUnderstandingPage.tsx` 的元件在 styles.css 中沒有任何規則。
- **修法**（已以注入下列 CSS 驗證：上述 11 個「頁面 × 寬度」組合的 `scrollWidth` 全部降到 1024／1440，即視窗寬度；以淺色量測——深色只覆寫色彩 token（styles.css:87–127），版面相同，所以涵蓋全部 22 張）：
  ```css
  .main { min-width: 0; }
  .tab-panel, .setup-screen { grid-template-columns: minmax(0, 1fr); }
  .main :where(section, article, form, fieldset, label, div, details, dl, dd) { min-width: 0; }
  .main select { max-width: 100%; min-width: 0; }
  .main pre { max-width: 100%; }
  .decision-layout { grid-template-columns: minmax(240px, 320px) minmax(0, 1fr); }
  .main :where(p, li, dd, dt, h2, h3, h4, h5, td, th, span, summary, label, a, code) { overflow-wrap: anywhere; }
  .timeline-filter { flex-wrap: wrap; }
  .timeline-filter label { display: flex; flex: 1 1 280px; align-items: center; gap: 8px; min-width: 0; }
  .timeline-filter select { flex: 1 1 0; width: 0; min-width: 0; }
  ```
  正式實作改用 MASTER.md 6.2／6.4 的規則（`minmax(0, …)` 寫在各容器上，而不是對 `div` 全面加 `min-width: 0`），並由程式把 `<select>` 選項文字縮短到 60 字以內（MASTER.md 6.1）。

### C2（中）每頁標題重複完整 Workspace 名稱

- **證據**：`controller__long__light__1024x768.png`「主控台：長文字壓力測試專用標記的 Workspace 第 1 號：…（含舊版遷移資料）」折成 2 行；`tasks__long__light__1024x768.png` 同樣 2 行並把「建立 Task」擠成兩行；`verification__long__light__1024x768.png`、`workflows__long__light__1024x768.png`、`authorization__long__light__1024x768.png`、`reviews__long__light__1440x900.png`、`knowledge__long__light__1440x900.png`、`timeline__long__light__1440x900.png` 皆同。
- **原因**：各頁元件的 h2 都是 `<頁名>：{workspace.name}`（例：ControllerPage、TaskBoard、DecisionCenter、ArtifactsPage 的「成果與驗證：筆記」）。
- **修法**：h2 只寫頁名；`App.tsx` 在 Workspace 頁簽上方加一行情境列（名稱 `.truncate` + `title`、縮短路徑），完整名稱在側欄。順便統一頁名與頁簽名（`artifacts__normal__light__1440x900.png`：頁簽「成果」、標題卻是「成果與驗證」）。

### C3（高）側欄的 Workspace 項目在長文字下失控

- **證據**：`workspace__long__light__1440x900.png`、`controller__long__light__1024x768.png`：名稱按鈕折成 4 行；徽章「待裁定 1284」被擠成 3 行的橢圓；路徑以 `overflow-wrap: anywhere` 折成 8 行；每個項目還有「封存」、暫停提示框、2 個核取方塊 + 2 個「?」。`workspace__long__light__1440x900.png` 的主區只有一行提示，整頁仍高 5803px——高度全部來自側欄。
- **原因**：`WorkspaceSidebar.tsx`；`.workspace-list li`（422）是單欄 grid，`.decision-badge`（1396）沒有 `white-space: nowrap`／`flex-shrink: 0`，`.workspace-path`（433）完整換行，`.workspace-paused`／`.workspace-auto-resume`（1201–1222）放在每個項目內。
- **修法**：項目＝名稱（`clamp-2`，`flex: 1; min-width: 0`）＋徽章（`nowrap`、`999+`、`aria-label` 完整數字）；路徑 1 行中間省略 + `title`；封存與自動續跑移到情境列的「Workspace 設定」選單（或主控台頁），側欄只保留「完全退出後暫停中」這類需要立即處理的狀態（1 行 + 按鈕）。

### C4（高）長文字的處理方式只有「全部攤開」一種

- **證據**：`tasks__long__light__1440x900.png`（卡片把 Goal 全文、3 層巢狀的排隊原因、缺少項目、預算與 worktree 錯誤全部攤開，單張卡片超過 1000px 高）、`knowledge__long__light__1440x900.png`（頁面高 26309px：每張知識卡把內文、觸發條件、根因、解法全文展開）、`decisions__long__light__1440x900.png`（清單項把問題全文、Task 全名、metadata 全部換行，一張 20 多行）。
- **原因**：元件直接輸出完整欄位；styles.css 沒有任何 `line-clamp` 或截斷規則（`grep text-overflow\|line-clamp` 0 筆）。
- **修法**：MASTER.md 6.1 的分類表：清單中標題 2 行、段落 3 行 + 展開、錯誤只顯示摘要句 + 技術細節收合、識別碼／雜湊／路徑縮短 + 複製；詳情中完整。新增共用元件 `LongText`（clamp + 展開）與 `textFormat.ts`（`shortId`、`shortHash`、`shortPath`、`formatCount`）。

### C5（中）長段落沒有閱讀寬度上限

- **證據**：`reviews__long__light__1440x900.png`（審查理由每行約 65 個中文字、1080px）、`knowledge__long__light__1440x900.png`（每行約 95 字）、`decisions__long__light__1440x900.png`（修正 C1 後詳情區仍約 1100px 寬）。
- **原因**：只有表單容器有 `max-width: 720px`（`.authorization` 644、`.model-overrides` 702、`.task-form` 997），段落沒有上限。
- **修法**：段落類元素 `max-inline-size: 46em`（MASTER.md 6.3）。

### C6（中）識別碼、雜湊、路徑的呈現不一致

- **證據**：`reviews__long__light__1440x900.png`「版本 996fb…→ e3b0c…」用內文字型、全長；`artifacts__long__light__1024x768.png`「Task：task_7b5deb026d9841b0cced7314aa0f9e8d…」全長、內文字型，雜湊卻是等寬；`decisions__long__light__1440x900.png` 選項「撤回整合並重新派工」下方一整行裸 SHA-256。另一方面 `DecisionCenter.tsx:639`、`TaskBoard.tsx:184`、`SharedSettingsPage.tsx:149` 已各自寫了 `slice(0, 12)`。
- **修法**：統一用 `textFormat.ts` 縮短 + `.mono` + 複製按鈕；詳情中完整等寬、`overflow-wrap: anywhere`（MASTER.md 6.1）。

### C7（中）grid 容器把按鈕拉滿整列

- **證據**：`authorization__normal__light__1440x900.png`「確認並保存」「儲存模型設定」寬 720px；`knowledge__normal__light__1440x900.png`「新增」滿版；`accounts__normal__light__1440x900.png`「新增 Profile」「以既有目錄建立」寬 270px；`artifacts__long__light__1024x768.png`「全部版本（3）」滿版。
- **原因**：按鈕是 grid 子元素，`justify-items` 預設 `stretch`：`.authorization`（641）、`.model-overrides fieldset`（715）、`.knowledge-form`（2011）、`.profile-form`（1194）、`.artifact-card`（1761）。
- **修法**：grid 中的按鈕 `justify-self: start`；表單動作放 `.form-actions`（MASTER.md 5.1）。

### C8（中）長內容整塊上語意色

- **證據**：`setup__long__light__1024x768.png`（每個工具整張黃卡，含 3 段長說明與指令）、`knowledge__long__light__1440x900.png`（候選卡整張黃底虛線框）、`tasks__long__light__1440x900.png`（同一張卡疊黃框「缺少項目」＋紅框預算＋紅框 worktree）、`reviews__long__light__1440x900.png`（「已逾期：<200 字錯誤>」整段橘色粗體）、`settings__long__light__1024x768.png`（還原演練失敗項整行紅色粗體）。
- **原因**：`.setup-tool-missing/too_old/unknown`（1039）、`.knowledge-candidate`（1935）、`.task-missing`（935）、`.task-budget-exhausted`／`.task-workdir-failed`（888–895）、`p.review-badge`（2571，套在整段原因上，ReviewPage.tsx:99）、`.backup-check-failed`（2091）。
- **修法**：MASTER.md 2.3：左側 3px 色條 + 狀態徽章，正文用 text 色；淡色底只給 2 行以內的提示。

### C9（中）色彩對比不合格

- **證據**（計算值）：淺色 `--color-text-subtle #9a9ea6` 對白底 **2.69:1**（用在 placeholder（260）、看板欄計數 `.task-count`（794）、`.status::before`（344））；深色 `#6c6f78` 對 surface **3.50:1**；輸入框邊界 `--color-border-strong #d2d4da` 對白底 1.48:1（WCAG 1.4.11 要求 3:1）。
- **修法**：MASTER.md 第 2 節的 token（subtle 4.76／4.90，新增 `--color-control-border` 3.41／3.29，新增 `--color-focus`）。

### C10（中）長段落用 12px

- **證據**：`tasks__long__light__1024x768.png`、`artifacts__long__light__1024x768.png`、`decisions__long__light__1440x900.png` 的卡片內文。
- **原因**：`.task-card { font-size: var(--font-size-xs) }`（827）、`.artifact-card`（1770）、`.decision-task/.decision-meta/.decision-detail-meta`（1464–1471）、`.timeline-list`（2360）。
- **修法**：12px 只用於徽章、計數、時間戳；卡片內文 13px、段落 14px／1.65（MASTER.md 3.2）。

### C11（低）垂直節奏不一致

- **證據**：`decisions__normal__dark__1024x768.png` 頁簽到標題約 60px（其他頁約 40px）；`artifacts__normal__light__1440x900.png` 第一層頁簽到第二層膠囊約 40px；`accounts__normal__light__1440x900.png`「首次設定已完成」上下各一大段空白；`settings__normal__light__1440x900.png` 各區塊沒有容器、只靠 h2 分隔，而備份演練有框。
- **原因**：`.decision-center { margin-top: 16px }`（1405）疊加 `.tab-panel` 的 `padding-top`（589）；`.tab-panel` gap 24px 也套在頁簽與內容之間；設定頁元件沒有共同容器。
- **修法**：MASTER.md 4.1 的間距層次；頁面區塊一律用同一種 section 樣式。

### C12（中）徽章、計數會折行

- **證據**：`workspace__long__light__1440x900.png`「待裁定 1284」3 行；`knowledge__long__light__1440x900.png`「候選，尚未驗證」2 行。頁簽上的 `1284` 徽章（`tasks__long__light__1024x768.png`）目前沒折，但同一規則也沒有保護。
- **原因**：`.decision-badge`（1396）、`.knowledge-badge`（1926）、`.tab-badge`（550）沒有 `white-space: nowrap`。
- **修法**：MASTER.md 5.4。

### C13（低）5 個元件完全沒有樣式

- **證據**：`knowledge__normal__light__1440x900.png`（專案理解：「purpose已確認」「GLOSSARY.md第 1 版已同步」文字黏在一起）、`accounts__normal__light__1440x900.png`（排程的兩個 radio 選項接成同一行、共同設定版本清單「06:13偏好 3」）、`settings__normal__light__1440x900.png`（登入啟動、平台狀態）。
- **原因**：`ProjectUnderstandingPage.tsx`（`understanding-*`、`projection-*`）、`SchedulingSettings.tsx`（`scheduling-*`）、`SharedSettingsPage.tsx`（`settings-*`、`shared-settings-page`）、`LoginStartupSettings.tsx`（`login-startup-*`）、`PlatformPage.tsx`（`platform-page`）的 class 在 styles.css 中都是 0 條規則（`grep -c` 確認）。
- **修法**：依 MASTER.md 元件規則補齊（歸入各頁的工作包）。

---

## 2. 逐頁問題

### 2.1 首次啟動精靈（`setup`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-SETUP-1 | 高 | `setup__long__light__1024x768.png`、`setup__long__dark__1024x768.png` | 水平捲軸 1139px，標題列底色只到 1024px、內容超出。源頭：官方說明的長 URL、winget 指令中的長路徑、`.profile-quota` 段落（C1）；`.setup-screen`（600）沒有欄軌。 | C1；`.setup-guidance code`（1058）與 URL 用 `overflow-wrap: anywhere`；單行指令 `pre-wrap`。 |
| P-SETUP-2 | 中 | `setup__long__light__1024x768.png` | 每個工具整張黃卡（`.setup-tool-missing/too_old/unknown`，1039），長原因段落也是黃底（C8）。 | 中性卡片 + 左側 warning 色條。 |
| P-SETUP-3 | 中 | `setup__long__light__1024x768.png`、`setup__normal__light__1440x900.png` | 狀態「缺少」「版本過舊」只是卡片中的一行普通文字，不像狀態。 | 標題列放狀態徽章（MASTER.md 5.4）。 |
| P-SETUP-4 | 低 | `setup__long__light__1024x768.png` | 「官方說明」完整 URL 以 12px 灰字佔 2 行。 | 顯示網域 + 連結文字「官方說明」，完整 URL 放 `title`。 |

### 2.2 Workspace（未選擇，`workspace`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-WS-1 | 低 | `workspace__normal__light__1440x900.png` | 空狀態只有一行灰字「請從左側選擇或建立 Workspace」，靠左上、沒有動作（`.workspace-hint`，596）。 | MASTER.md 5.7：標題 + 說明 + 「建立 Workspace」按鈕（聚焦側欄表單）。 |
| P-WS-2 | 高 | `workspace__long__light__1440x900.png` | 側欄項目失控（C3）。 | 見 C3。 |

### 2.3 主控台（`controller`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-CTRL-1 | 高 | `controller__long__light__1024x768.png`、`controller__long__dark__1024x768.png` | 水平捲軸 1098px，源頭是「換成帳號」`<select>`（選項含完整帳號名稱與額度），`.controller-row label`（2167）。 | C1；選項文字縮短（帳號名前段 + 額度）。 |
| P-CTRL-2 | 中 | `controller__long__light__1024x768.png` | 標題重複 Workspace 全名，折成 2 行（C2）。 | C2。 |
| P-CTRL-3 | 低 | `controller__normal__light__1440x900.png` | 內容擠在左側約 500px 的窄欄，右側 900px 空白；額度以「5h 未知｜7d 未知｜執行中 Worker 1」純文字與直線分隔，「結束」按鈕與換帳號表單之間用一條短分隔線。 | 事實用 `.controller-facts`（2110）的兩欄 dl，額度改成 3 個小指標（數值 + 標籤）；動作集中在頁面標題列右側。 |
| P-CTRL-4 | 低 | `controller__long__light__1024x768.png` | `.controller-facts` 是 `max-content 1fr`（2112），長 dd 時 1fr 的最小值為 auto。 | MASTER.md 6.4 第 5 條。 |

### 2.4 工作（`tasks`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-TASK-1 | 高 | `tasks__long__light__1440x900.png`、`tasks__long__light__1024x768.png` | 卡片攤開全部細節：Goal 全文、排隊原因 3 層巢狀清單（瀏覽器預設 40px 縮排，在 240px 欄內只剩約 150px 給文字）、缺少項目、預算、worktree 錯誤；單張卡片超過 1000px 高，整頁 8206px（C4）。`TaskBoard.tsx` 的卡片、`.task-card`（819）、`.task-missing`（935）、`.task-budget`（878）。 | 卡片＝標題 `clamp-2` + 種類／Goal 1 行 + 一行狀態（「排隊中：額度已滿」）+ 主要動作；細節收進 `<details>` 或點卡片開詳情；清單縮排改 16px。 |
| P-TASK-2 | 高 | `tasks__long__light__1024x768.png` | 卡片內文 12px（`.task-card`，827）（C10）。 | 13px，段落 14px。 |
| P-TASK-3 | 中 | `tasks__normal__light__1440x900.png`、`tasks__normal__dark__1440x900.png` | 每張卡 4～5 個按鈕（派工、編輯契約、新增相依、取消、契約版本歷史）排成 2～3 列，佔卡片一半高度（`.task-actions`，979）。 | 1 個主要動作 + 「更多」選單（MASTER.md 5.1）；「取消」放選單內並以 danger 色。 |
| P-TASK-4 | 中 | `tasks__normal__light__1440x900.png` | 看板最右欄「等待…」被裁到只剩 2 個字，沒有任何「還有欄位」的提示（`.task-columns { overflow-x: auto }`，802）。 | 欄寬 280px；看板左右緣漸層提示；欄標題列加總數。 |
| P-TASK-5 | 中 | `tasks__long__light__1440x900.png` | 同一張卡疊黃框與紅框（C8）。 | 狀態徽章 + 左色條。 |
| P-TASK-6 | 中 | `tasks__long__light__1024x768.png` | 標題 2 行並把「建立 Task」按鈕擠成兩行（C2；`.task-board-header`，766，按鈕沒有 `flex: none`／`nowrap`）。 | C2；按鈕 `flex: none; white-space: nowrap`。 |
| P-TASK-7 | 低 | `tasks__long__light__1024x768.png` | 整合佇列每項把完整 Task 標題與分支名全文列出（`.integration-queue`，919）。 | 標題 `clamp-1`、分支 `.mono` 縮短。 |

### 2.5 工作／編輯契約（`tasks-contract`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-TC-1 | 高 | `tasks-contract__long__light__1440x900.png` | textarea 固定約 2 行高（`.task-form textarea { min-height: 48px }`，1020），多段落的契約內容只露出 2 行，要在小框內捲動才能讀。 | `field-sizing: content; min-height: 3lh; max-height: 16lh`（MASTER.md 5.5）。 |
| P-TC-2 | 中 | `tasks-contract__normal__light__1440x900.png` | 欄位之間沒有間距：textarea 底緣直接接下一個 label。fieldset 是預設 block（272），只有 `label` 是 grid（1010）。 | fieldset 內 `display: grid; gap: 12px`。 |
| P-TC-3 | 中 | `tasks-contract__normal__light__1440x900.png` | fieldset legend 已是「目標」，內部 label 又寫「目標：模式」「目標：內容」「目標：引用（選填）」（`ContractEditor.tsx` 約第 201–236 行），6 個 fieldset 各重複 3～4 次。 | 視覺上只寫「模式」「內容」「引用（選填）」，可存取名稱用 `aria-label` 保留完整名稱（測試以 label 查詢時一併更新）。 |
| P-TC-4 | 低 | `tasks-contract__normal__light__1440x900.png` | 「模式」只有 2～3 個選項，卻是 700px 滿版下拉。 | 選單 `width: auto; max-width: 16em`。 |
| P-TC-5 | 中 | `tasks-contract__long__light__1440x900.png` | 編輯器以行內表單插在整合佇列與看板之間，看板被推到 2000px 以下；編輯時看不到正在編的卡片。 | 改為對話框（MASTER.md 5.6，`min(880px, …)`），或看板右側的詳情面板。 |

### 2.6 裁定（`decisions`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-DEC-1 | 高 | `decisions__long__light__1024x768.png`、`decisions__long__light__1440x900.png`（深色同） | 水平捲軸 2168px（兩種寬度都有）。源頭：權限提示的「工具輸入」`<pre>`（2403px）、「選擇要連結的請求」`<select>`（1573px）；`.decision-layout { grid-template-columns: minmax(220px, 1fr) 2fr }`（1417）。 | C1（已驗證）；`<pre>` 放在 `max-height: 24lh; overflow: auto` 的捲動框。 |
| P-DEC-2 | 高 | `decisions__long__light__1440x900.png` | 清單欄只剩約 190px：每個清單項把問題全文、Task 全名、metadata 全部換行，一張 20 多行，第一屏只看得到 2 項（`.decision-list li button`，1439；`.decision-task`／`.decision-meta`，1464）。 | 清單欄 `minmax(260px, 340px)`；問題 `clamp-3`、Task 名 `clamp-1` muted、緊急程度與逾期改徽章同一行。 |
| P-DEC-3 | 中 | `decisions__long__light__1440x900.png` | 詳情區的段落橫跨約 1570px（修正 C1 後仍約 1100px）（C5）。`.decision-detail`（1489）。 | 段落 46em。 |
| P-DEC-4 | 中 | `decisions__long__light__1440x900.png` | 選項「撤回整合並重新派工」的取捨文字是一整行裸 SHA-256；權限提示的路徑、工具名以內文字型全長顯示（C6）。 | `.mono` + 縮短 + 複製；詳情中的長路徑 `overflow-wrap: anywhere`。 |
| P-DEC-5 | 低 | `decisions__normal__dark__1024x768.png`、`decisions__normal__light__1440x900.png` | 頁簽與標題間距比其他頁多 16px（`.decision-center { margin-top }`，1405）（C11）。 | 移除。 |
| P-DEC-6 | 中 | `decisions__long__light__1440x900.png` | 標題重複 Workspace 全名（C2）。 | C2。 |

### 2.7 成果與品質／成果（`artifacts`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-ART-1 | 中 | `artifacts__long__light__1024x768.png` | 「全部版本（3）」按鈕被拉滿整張卡片（`.artifact-card` 是 grid，1761）（C7）。 | `justify-self: start`，改低調按鈕。 |
| P-ART-2 | 中 | `artifacts__long__light__1024x768.png`、`artifacts__normal__light__1440x900.png` | 「Task：task_7b5deb…（全長）」顯示識別碼而不是 Task 標題；產生者 Run 也是全長識別碼（`.artifact-task`，1796）（C6）。 | 顯示 Task 標題（`clamp-1`），識別碼縮短放旁邊。 |
| P-ART-3 | 低 | `artifacts__long__light__1024x768.png` | 「1234567892 位元組」沒有千分位或單位換算（Q19）。 | `1.15 GB（1,234,567,892 位元組）`。 |
| P-ART-4 | 低 | `artifacts__normal__light__1440x900.png` | 卡片 12px（`.artifact-card` 1770）、有陰影（1769）；頁名「成果與驗證」與頁簽「成果」不一致（C2、C10）。 | C10；頁名改「成果」。 |

### 2.8 成果與品質／審查（`reviews`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-REV-1 | 中 | `reviews__long__light__1440x900.png` | 「已逾期：<長錯誤>」整段橘色粗體（`p.review-badge`，2571／2586；ReviewPage.tsx:99 把原因放在同一個 `<p>`）（C8）。 | 狀態詞做成徽章，原因用一般文字。 |
| P-REV-2 | 中 | `reviews__long__light__1440x900.png` | 版本雜湊（`996fb… → e3b0c…`）用內文字型、全長；規範來源路徑全長（C6）。 | `.mono` 縮短 + 複製。 |
| P-REV-3 | 低 | `reviews__long__light__1440x900.png` | 段落每行約 65 字（C5）。 | 46em。 |
| — | — | `reviews__normal__light__1440x900.png`、`reviews__long__light__1024x768.png` | 沒有水平捲軸；審查軸以左框線區分、dl 兩欄對齊，結構本身沒有問題。 | — |

### 2.9 成果與品質／驗證（`verification`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| — | — | `verification__long__light__1024x768.png`、`verification__long__light__1440x900.png` | 長文字資料下沒有水平捲軸、沒有被撐開的元素；本頁版面在兩種寬度都穩定。 | — |
| P-VER-1 | 低 | `verification__long__light__1024x768.png` | Feature Map 的功能名與其下「驗收準則／旅程」同一縮排，只靠粗體區分（`.verification-features li`，2665）。 | 功能名為 h5，準則縮排 12px 並以 muted 標籤。 |
| P-VER-2 | 低 | `verification__normal__light__1440x900.png` | 三層框（`.verification-section` > `.verification-bundle` > `.verification-uncovered`，2616／2679）。 | 內層改上框線分隔，只保留最外層卡片框。 |
| P-VER-3 | 低 | `verification__normal__light__1440x900.png` | 欄名中英混用（「build」「candidate revision」與「需求版本」並列）。 | 文案統一（不影響 CSS）。 |

### 2.10 成果與品質／探索（`exploration`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-EXP-1 | 高 | `exploration__long__light__1024x768.png`、`exploration__long__light__1440x900.png`（深色同） | 水平捲軸 1505px。源頭在「疑點」群組內不能斷的 token；只有 `.exploration-attempt`（2946）有 `overflow-wrap`，`.exploration-section`／`.exploration-card`（2871）是沒有欄軌的 grid。 | C1（全站斷行規則已驗證可消除）。 |
| P-EXP-2 | 中 | `exploration__normal__light__1440x900.png` | 三層巢狀框：群組（左色條）> 疑點卡 > 重現紀錄。 | 重現紀錄改上框線分隔的清單。 |
| P-EXP-3 | 低 | `exploration__normal__light__1440x900.png` | 同一張卡的兩組 dl 標籤欄寬不同（「Task」欄約 50px、「確認的 Evidence Bundle」欄約 110px），內容欄不對齊。 | dl 欄軌 `minmax(6em, max-content)`，同卡共用一個 grid（MASTER.md 6.4）。 |
| P-EXP-4 | 低 | `exploration__normal__light__1440x900.png` | 狀態徽章「回歸失敗」獨佔標題下一行。 | 放進標題列（MASTER.md 5.2）。 |

### 2.11 更多／知識（`knowledge`，含專案理解）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-KN-1 | 高 | `knowledge__long__light__1024x768.png`、`knowledge__long__light__1440x900.png`（深色同） | 水平捲軸 1488px，源頭是專案理解的 `ul.understanding-items`（沒有空白的長 key）；`ProjectUnderstandingPage.tsx` 沒有任何 CSS（C13）。 | C1；`.understanding-key` 用 `.mono` + `anywhere`。 |
| P-KN-2 | 高 | `knowledge__long__light__1440x900.png` | 每張知識卡攤開內文、觸發條件、根因、解法全文，頁面高 26309px（C4）（`.knowledge-card`，1896；`.knowledge-body { white-space: pre-wrap }`，1959）。 | 卡片＝標題 + 狀態 + 摘要 `clamp-3` +「展開」；教訓的 dl 收進 `<details>`。 |
| P-KN-3 | 中 | `knowledge__long__light__1440x900.png` | 候選卡整張黃底虛線框（`.knowledge-candidate`，1935）（C8）。 | 左側 warning 色條 + 徽章。 |
| P-KN-4 | 中 | `knowledge__long__light__1440x900.png` | 徽章「候選，尚未驗證」折成 2 行（`.knowledge-badge`，1926）（C12）。 | `nowrap`，文字改「未驗證」。 |
| P-KN-5 | 中 | `knowledge__normal__light__1440x900.png` | 專案理解區塊無樣式：「purpose已確認」「GLOSSARY.md第 1 版已同步」字黏在一起，h2 用瀏覽器預設間距（C13）。 | 補 `understanding-*`／`projection-*` 樣式：key 等寬、狀態徽章、來源一行 muted。 |
| P-KN-6 | 低 | `knowledge__normal__light__1440x900.png` | 「新增條目」8 個欄位常駐展開在清單與專案理解之間，送出鈕滿版（`.knowledge-form`，2011）（C7）。 | 收在「新增條目」按鈕後（或對話框），按鈕 `justify-self: start`。 |
| P-KN-7 | 低 | `knowledge__long__light__1440x900.png` | 段落每行約 95 字（C5）。 | 46em。 |

### 2.12 更多／流程（`workflows`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| — | — | `workflows__long__light__1024x768.png`、`workflows__long__light__1440x900.png` | 沒有水平捲軸；dl 兩欄在長文字下仍對齊。 | — |
| P-WF-1 | 中 | `workflows__long__light__1024x768.png` | 「Router 的理由」在摘要 dl 中全文顯示（約 17 行，含完整路徑、URL）（`.workflow-summary dd`，2466）。 | 摘要中 `clamp-3` + 展開；路徑與 URL 依 MASTER.md 6.1。 |
| P-WF-2 | 低 | `workflows__normal__light__1440x900.png` | 階段、嘗試以多層項目符號清單呈現，層次靠縮排與圓點（`.workflow-stage-list`／`.workflow-attempts`，2531）。 | 階段改有狀態徽章的列（標題列 + 狀態），嘗試列為表格（時間、結果、說明）。 |

### 2.13 更多／時間線（`timeline`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-TL-1 | 高 | `timeline__long__light__1024x768.png`、`timeline__long__light__1440x900.png`（深色同） | 水平捲軸 2211px，源頭是「依 Task 過濾」`<select>`（1695px）；`.timeline-filter`（2336）是 flex，label 與 select 都不能縮小。 | C1 的 `.timeline-filter` 規則（已驗證）+ 選項文字縮短。 |
| P-TL-2 | 中 | `timeline__long__light__1440x900.png` | 「服務端額度」在 220px 指標卡中列出 8 個帳號全名，卡片 470px 高，其他卡 95px，整列網格高低不齊（`.timeline-metric`，2318）。 | 額度改成指標網格下方的小表格（帳號名 `clamp-1`、五小時、七日兩個數字欄）。 |
| P-TL-3 | 中 | `timeline__normal__light__1440x900.png`、`timeline__long__light__1440x900.png` | 指標卡的數值與說明同字級、同字重，沒有主從（`.timeline-metric dd`，2332；整張卡 12px，2325）。 | 數值 20px／600／tabular-nums，標籤 caption muted。 |
| P-TL-4 | 低 | `timeline__long__light__1440x900.png` | 事件列是 `flex-wrap`（2363），長 Task 名把 Run 擠到第二行，各列欄位不對齊。 | grid：時間 9.5em｜類別 5em｜類型 14ch 等寬｜內容 `minmax(0, 1fr)`。 |

### 2.14 更多／授權（`authorization`，含模型設定）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-AUTH-1 | 高 | `authorization__long__light__1024x768.png`、`authorization__long__dark__1024x768.png` | 水平捲軸 1087px，源頭是模型設定的 3 個 `<select>`（752px；`.model-overrides label`，723）。 | C1。 |
| P-AUTH-2 | 中 | `authorization__long__light__1024x768.png` | 「沿用舊授權的 Run」清單：Task 全名粗體 3 行；「暫停／取消」按鈕有時接在文字行尾、有時另起一行；兩個停用按鈕黏在一起。`.authorization-legacy-runs`（AuthorizationPanel.tsx:354）沒有任何 CSS。 | 每列 grid：標題 `clamp-2`｜狀態｜按鈕組（`flex: none; gap: 8px`）。 |
| P-AUTH-3 | 中 | `authorization__normal__light__1440x900.png` | 「確認並保存」「儲存模型設定」被拉滿 720px（C7）。 | `.form-actions`。 |
| P-AUTH-4 | 低 | `authorization__long__light__1024x768.png` | 授權設定與模型設定的標題都重複 Workspace 全名（C2）。 | C2。 |

### 2.15 帳號（`accounts`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-ACC-1 | 中 | `accounts__long__light__1024x768.png`、`accounts__normal__light__1440x900.png` | 方案上限表單的 label 與輸入框黏在一起（「五小時方案上限[ ]」）（`.quota-limits`，1182）。 | 同列 label + 控制項 `gap: 6px`；輸入框 `width: 12em`（MASTER.md 5.5）。 |
| P-ACC-2 | 中 | `accounts__normal__light__1440x900.png` | 兩個新增 Profile 表單沒有標題、label 貼著輸入框、送出鈕拉滿 270px（`.profile-form`，1194）（C7）。 | 加小標題「新增帳號」「以既有目錄建立」；`.form-actions`。 |
| P-ACC-3 | 中 | `accounts__normal__light__1440x900.png` | 排程「並行容量」的兩個 radio 選項接成同一行文字（`SchedulingSettings.tsx` 的 `scheduling-*` 沒有 CSS）（C13）。 | 每個選項一列：radio + 名稱（600）+ 說明（muted）。 |
| P-ACC-4 | 低 | `accounts__normal__light__1440x900.png` | 共同設定版本清單文字黏連（「06:13偏好 3」）（`SharedSettingsPage.tsx` 的 `settings-*` 沒有 CSS）（C13）。 | 版本列 grid：版本｜雜湊（mono）｜時間｜計數。 |
| P-ACC-5 | 低 | `accounts__long__light__1024x768.png` | 帳號卡片 6～8 行事實逐行排列；狀態「可派工」是一行綠字，帳號 email 與核對時間混在一句（`.profile-list li`，1075）。 | 標題列放名稱 + 狀態徽章；事實改兩欄 dl。 |
| P-ACC-6 | 低 | `accounts__normal__light__1440x900.png` | 「首次設定已完成」上下各一大段空白（C11）。 | 間距層次。 |

### 2.16 設定（`settings`）

| # | 嚴重度 | 截圖 | 問題與原因 | 修法 |
|---|---|---|---|---|
| P-SET-1 | 中 | `settings__normal__light__1440x900.png` | 「登入時自動啟動」與「平台狀態」沒有樣式：核取方塊與說明緊貼、說明不是次要色（`LoginStartupSettings.tsx`、`PlatformPage.tsx` 沒有 CSS）（C13）。 | 補樣式；說明 muted、與控制項 4px。 |
| P-SET-2 | 中 | `settings__normal__light__1440x900.png` | 各區塊沒有一致容器：只有還原演練有框，其餘是裸文字（C11）。 | 每個設定區塊同一種 section 卡片。 |
| P-SET-3 | 中 | `settings__long__light__1024x768.png` | 還原演練失敗項整行紅色粗體，包含 3 行長錯誤（`.backup-check-failed`，2091）（C8）。 | 「未通過」徽章 + 一般文字；技術細節收合。 |
| P-SET-4 | 低 | `settings__normal__light__1440x900.png` | 更新資訊以 4 個「目前版本：…」段落呈現（`.update-settings p`，2423）。 | 兩欄 dl；版本號 `.mono`。 |

---

## 3. 改版分包

### 3.1 先做的結構調整：把 styles.css 依區域拆檔

現在 2955 行全在一個檔案，同一區域的規則還分散在不同位置（例：狀態列在 324–387、1352–1395、2239–2246 三處；側欄的 `.workspace-paused` 在帳號區 1201）。多個工作包並行時很容易改到同一段。**WP1 的第一個 commit 只搬移、不改值**，拆成：

| 檔案 | 內容（baseline 行號） | 擁有者 |
|---|---|---|
| `web/src/styles/tokens.css` | 1–127 | WP1 |
| `web/src/styles/base.css` | 129–311 | WP1 |
| `web/src/styles/components.css` | 新增（長文字工具、按鈕變體、徽章、`.form-actions`、`.table-wrap`、`LongText`）＋ 1223–1269（HelpTip）、1270–1351（完全退出遮罩）、2226–2237（`.primary-action`） | WP1 |
| `web/src/styles/shell.css` | 317–388（`.app`、狀態列、平台提示）、389–474（`.body`、側欄、Workspace 清單與表單）、475–478（`.main`）、480–599（標題列、通知、頁簽、`.tab-panel`、`.workspace-hint`）、600–640（`.setup-screen`、`.workspace-*`）、1201–1222（`.workspace-paused`、`.workspace-auto-resume`）、1352–1404（狀態列項目、`.decision-badge`）、2239–2246（`.status-recovery*::before`）、2391–2417（更新通知） | WP2（`.main`、`.tab-panel`、`.setup-screen` 的 min-width／欄軌由 WP1 先改） |
| `web/src/styles/accounts-settings.css` | 1024–1200（首次精靈、帳號、Profile、額度）、2040–2099（備份）、2247–2284（崩潰恢復）、2418–2436（更新設定）＋新增排程、共同設定、登入啟動、平台頁 | WP2 |
| `web/src/styles/tasks.css` | 760–1023（看板、Task 表單與契約編輯器）、2100–2225（主控台） | WP3 |
| `web/src/styles/decisions.css` | 1405–1746 | WP4 |
| `web/src/styles/quality.css` | 1747–1842（成果）、2548–2606（審查）、2607–2861（驗證）、2862–2955（探索） | WP5 |
| `web/src/styles/more.css` | 641–759（授權、模型設定）、1843–2039（知識）、2285–2389（時間線）、2437–2547（流程）＋新增專案理解 | WP6 |

`web/src/styles.css` 改為依上表順序 `@import` 各檔（Vite 會內嵌）。搬移會改變不同區域間「同權重規則」的先後；拆檔 commit 的驗證條件是：以 `--label wp1-split` 重拍，128 張與 baseline **逐像素相同**（可用 PIL 比對），有差異就調整 import 順序或把衝突規則留在原檔順序。

### 3.2 工作包

| 包 | 範圍 | 負責檔案 | 對應問題 | 先後 |
|---|---|---|---|---|
| **WP1 基礎** | token、全站斷行與 min-width、共用元件樣式、長文字工具 | `styles/tokens.css`、`styles/base.css`、`styles/components.css`；`shell.css` 中只改 `.main`、`.tab-panel`、`.setup-screen` 三條；新增 `web/src/textFormat.ts`（+ 測試）、`web/src/LongText.tsx`（+ 測試） | C1、C5（工具）、C6（工具）、C7（規則）、C9、C10（token）、C12（徽章規則）、MASTER 第 2–6 節 | **最先**，完成後其他包才開始 |
| **WP2 外殼、側欄、帳號與設定** | 狀態列、標題列與頁簽、Workspace 情境列、側欄、更新通知；首次精靈、帳號、排程、共同設定、設定頁各區塊 | `styles/shell.css`、`styles/accounts-settings.css`；`App.tsx`、`StatusBar.tsx`、`WorkspaceSidebar.tsx`、`UpdateNotice.tsx`、`SetupWizard.tsx`、`AccountsPage.tsx`、`SchedulingSettings.tsx`、`SharedSettingsPage.tsx`、`LoginStartupSettings.tsx`、`BackupSettings.tsx`、`RecoveryPanel.tsx`、`PlatformPage.tsx`、`UpdateSettings.tsx` | C2（情境列）、C3、C11、C13（排程、共同設定、登入啟動、平台）、P-WS-1/2、P-SETUP-1～4、P-ACC-1～6、P-SET-1～4 | WP1 之後，與 WP3～WP6 並行 |
| **WP3 主控台與工作** | 主控台、看板卡片、整合佇列、契約編輯器（改對話框） | `styles/tasks.css`；`ControllerPage.tsx`、`TaskBoard.tsx`、`ContractEditor.tsx` | C2（本頁 h2）、C4、C8、C10、P-CTRL-1～4、P-TASK-1～7、P-TC-1～5 | WP1 之後，與 WP2、WP4～WP6 並行 |
| **WP4 裁定** | 裁定清單與詳情、權限提示、批次與連結 | `styles/decisions.css`；`DecisionCenter.tsx` | C2、C4、C5、C6、P-DEC-1～6 | WP1 之後，並行 |
| **WP5 成果與品質** | 成果、審查、驗證、探索 | `styles/quality.css`；`ArtifactsPage.tsx`、`ReviewPage.tsx`、`VerificationPage.tsx`、`ExplorationPage.tsx` | C2、C5、C6、C7、C8、P-ART-1～4、P-REV-1～3、P-VER-1～3、P-EXP-1～4 | WP1 之後，並行 |
| **WP6 更多** | 知識、專案理解、流程、時間線、授權、模型設定 | `styles/more.css`；`KnowledgePage.tsx`、`ProjectUnderstandingPage.tsx`、`WorkflowPage.tsx`、`TimelinePage.tsx`、`AuthorizationPanel.tsx`、`ModelOverridesPanel.tsx` | C2、C4、C5、C7、C8、C12、C13（專案理解）、P-KN-1～7、P-WF-1～2、P-TL-1～4、P-AUTH-1～4 | WP1 之後，並行 |

**不重疊的保證**：WP2～WP6 各自只寫自己的 CSS 檔與自己列出的 `.tsx`（含對應的 `.test.tsx`），彼此沒有共同檔案。共用規則（按鈕、徽章、長文字工具）只在 WP1；並行期間發現需要新的共用規則時，先寫在自己的檔案內並在回報中列出，由後續整理收進 `components.css`，不直接改 `components.css`。C2 的「h2 只寫頁名」由各頁所屬的包改自己的元件；情境列由 WP2 在 `App.tsx` 加——兩者獨立，順序不拘（情境列尚未上線時，側欄的選取狀態仍標示目前的 Workspace）。

### 3.3 各包的完成條件（以截圖工具對照）

共通：`pnpm --filter web test` 與 `pnpm --filter web build` 通過；以 `node scripts/ui-screenshots.mjs --label <包名>`（只拍自己的頁面時加 `--pages`）重拍，與 `baseline` 同檔名對照。

| 包 | 可驗證的完成條件 |
|---|---|
| WP1 | (1) 拆檔 commit：`--label wp1-split` 128 張與 `baseline` 逐像素相同。(2) 完成後 `--label wp1`：`results.json` 中 `horizontalScroll: true` 為 **0 張**（baseline 22 張）。(3) tokens.css 的值與 MASTER.md 第 2 節逐一相同。(4) `textFormat.ts` 有單元測試：`shortId("task_7b5deb026d9841b0cced7314…")` 為 `task_7b5deb…` + 後 4 碼、`shortPath` 保留磁碟與最後 2 段、`formatCount(1284)` 為 `999+`。(5) `workspace__long__light__1440x900.png` 的待裁定徽章是一行。 |
| WP2 | `--pages setup,workspace,accounts,settings,controller`（controller 用來看情境列）：(1) 側欄每個 Workspace 項目在 `long` 資料集中高度 ≤ 96px（名稱 2 行 + 徽章 + 1 行路徑），`workspace__long__light__1440x900.png` 整頁高度從 5803px 降到 2400px 以下（`long` 資料集有 15 個 Workspace：15 × 96px + 建立表單 + 頂部通知）；(2) 情境列在 `long` 下 1 行；(3) `accounts__normal__light__1440x900.png` 排程選項各佔一列、共同設定版本列欄位分開；(4) `settings__normal__light__1440x900.png` 各區塊同一種容器；(5) 無水平捲軸。 |
| WP3 | `--pages controller,tasks,tasks-contract`：(1) `tasks__long__light__1440x900.png` 每張卡片高度 ≤ 260px、每張卡最多 2 個可見按鈕；整頁高度明顯低於 baseline 的 8206px；(2) `tasks-contract__long__*` 的契約 textarea 顯示完整內容（高度隨內容，最多 16 行）且以對話框呈現，看板不被推下；(3) h2 只有頁名；(4) 無水平捲軸。 |
| WP4 | `--pages decisions`：(1) `decisions__long__light__1440x900.png` 清單欄寬 260～340px，每個清單項 ≤ 6 行；(2) 詳情段落寬 ≤ 46em；(3) SHA-256 以縮短等寬形式出現；(4) 無水平捲軸（baseline 2168px）。 |
| WP5 | `--pages artifacts,reviews,verification,exploration`：(1) 「全部版本」不再滿版；(2) 審查的逾期原因不是整段彩色粗體；(3) 雜湊、識別碼等寬縮短；(4) 探索卡片最多兩層框；(5) 無水平捲軸（baseline exploration 1505px）。 |
| WP6 | `--pages knowledge,workflows,timeline,authorization`：(1) `knowledge__long__light__1440x900.png` 整頁高度低於 baseline 26309px 的三分之一，卡片摘要 3 行 + 展開；(2) 「候選」徽章一行；(3) 專案理解的狀態與名稱分開；(4) 時間線指標卡高度一致、額度移到表格；(5) 授權與模型設定的送出鈕不滿版；(6) 無水平捲軸（baseline knowledge 1488px、timeline 2211px、authorization 1087px）。 |
