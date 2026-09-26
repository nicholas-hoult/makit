// 性能埋点要最先执行：它被执行的时刻就是「JS 开始执行」（#218）
import "./perf";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, savedThemeId } from "./theme";

// render 前同步注入主题变量，避免浅色主题首屏闪一下深色
applyTheme(savedThemeId());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <App />,
);
