# 待最終 code-review 修正的已知問題

主控在執行期間收到、尚未派票處理的問題。最終 code-review 的修正 run 必須一併處理本檔所列項目。

## 1. 權限自動允許的私有位址判斷不完整（票 22，permission.rs）
- 來源：2026-10-22 背景安全審查（中）。
- 問題：判斷 URL 主機是否為私有位址時，只拆解「四段十進位」的 IPv4。整數、十六進位、八進位 IPv4（例如 `http://2130706433/`、`0x7f.1`）、IPv6（`[::1]`、`[fd00::]`）、結尾帶點的 `localhost.`、以及解析到私有位址的網域名稱，都會落到最後的 `true` 而被當成公開位址自動允許，形成 SSRF。
- 修法：以 `url::Url` 解析並用 `host()`；`Ipv4`／`Ipv6` 用標準函式排除 loopback、private、link-local、unspecified、CGNAT（100.64/10）、unique-local；網域先去掉結尾點並轉小寫再比對 `localhost`／`.localhost`／`.local`；非標準寫法的 IPv4 一律回傳無法歸類；網域名稱不自動允許，除非在明確的白名單內。補單元測試涵蓋上述每一種繞過。
