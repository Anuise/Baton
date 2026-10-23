# 最終 review 待修清單

1. 可能有不穩定的 Rust 測試：R005（票 01 整合）第一次 `pnpm test` 的 Rust 部分回結束碼 101，輸出未保存；之後三次 `cargo test --workspace --exclude baton-desktop` 都通過。最終 review 時留意是否有依賴時序或共用資源的測試。
2. 不穩定測試（承第 1 項）：R011（票 07 實作）整套 `cargo test` 第一次時 `verification_handoff::after_a_crash_the_successor_stops_the_old_test_processes_and_reruns_the_unfinished_case` 失敗一次，單獨重跑 3 次與第二次整套都通過。可能就是第 1 項那個失敗。
3. 不穩定測試：R009（票 05 實作）高負載整包跑時 `baton-store` 的 `lifecycle::tests::concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other` 失敗一次，單獨重跑 3 次與整包重跑都通過。
4. 操作手冊（票 04）要寫明 `"$BATON_MCP_SHIM" wait` 的用法與結束碼 0～4（票 06 的 AI 決策；R014 驗證者提出）。票 04 與 06 平行開發，若票 04 合併時手冊還沒寫，最終修正要補上。
