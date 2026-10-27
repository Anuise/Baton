import "@testing-library/jest-dom/vitest";

import { configure } from "@testing-library/react";

// findBy／waitFor 預設只等 1 秒，多套測試同時跑時首次渲染就可能超過；放寬等待上限（不改斷言），
// 仍低於 vitest 每個測試 5 秒的上限，失敗時看得到 Testing Library 的 DOM 輸出
configure({ asyncUtilTimeout: 3000 });
