import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, savedThemeId } from "./theme";

// render 前同步注入主题变量，避免浅色主题首屏闪一下深色
applyTheme(savedThemeId());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <App />,
);
