/**
 * 回归测试：src/ 里不许出现 `window.confirm` / `alert` / `prompt`。
 *
 * 为什么这值得一条专门的测试 —— 这是一类**静默失效**，代码看起来完全正常：
 *
 * wry 的 `WryWebViewUIDelegate` 只实现了三个 WKUIDelegate 方法（windowWillClose:、
 * runOpenPanelWithParameters:、requestMediaCapturePermissionForOrigin:），**没有**
 * runJavaScriptConfirmPanelWithMessage: / runJavaScriptAlertPanelWithMessage:。
 * WebKit 的约定是：UI delegate 不实现，就不弹框 —— `confirm()` 立刻返回 false，
 * `alert()` 什么都不做。不抛异常、不打日志、不留任何痕迹。
 *
 * 实际后果（#归档不了）：`handleToggleArchive` 里「运行中的 session 归档需二次确认」
 * 那个 confirm 恒为 false，于是运行中的 session 永远走不到 `archive_session`，
 * 而且不报错、不弹 toast。用户看到的是「点了归档没反应」，代码审查看到的是
 * 「一个再普通不过的二次确认」。三处 alert 同理 —— 导入失败、通知未授权、Hook
 * 安装结果，全都在对着空气说话。
 *
 * 所以这条不变量只能靠静态检查钉住：正确的做法是 @tauri-apps/plugin-dialog 的
 * 异步 confirm/message（它走 IPC 到 Rust 侧弹原生框，不依赖 WKUIDelegate）。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.(ts|tsx)$/.test(name)) out.push(p);
  }
  return out;
}

// 匹配调用，不匹配注释里提到的名字 —— 这些文件里正好有几处注释在解释「为什么不能用」。
// 前置的 (^|[^.\w]) 是为了不误伤 `foo.confirm(` 这类同名成员方法；`window.` 单独列出。
const PATTERNS: [string, RegExp][] = [
  ["window.confirm", /window\s*\.\s*confirm\s*\(/],
  ["window.alert", /window\s*\.\s*alert\s*\(/],
  ["window.prompt", /window\s*\.\s*prompt\s*\(/],
  ["裸 confirm(", /(^|[^.\w"'`])confirm\s*\(/],
  ["裸 alert(", /(^|[^.\w"'`])alert\s*\(/],
  ["裸 prompt(", /(^|[^.\w"'`])prompt\s*\(/],
];

// 允许的例外：从插件 import 进来并改了名的那个 confirm。它是解决方案本身。
const ALLOW = /confirm\s+as\s+confirmDialog|message\s+as\s+messageDialog|confirmDialog\s*\(|messageDialog\s*\(/;

const files = walk("src");
check(`扫到源文件（得到 ${files.length} 个）`, files.length > 10);

for (const file of files) {
  const lines = readFileSync(file, "utf8").split("\n");
  lines.forEach((line, i) => {
    const code = line.trim();
    // 跳过整行注释。行尾注释里出现这些词的情况本仓库没有，真出现了宁可误报。
    if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) return;
    if (ALLOW.test(code)) return;
    for (const [name, re] of PATTERNS) {
      check(`${file}:${i + 1} 用了 ${name} —— 在这个 webview 里恒静默失效：${code.slice(0, 80)}`, !re.test(code));
    }
  });
}

console.log(fails.length === 0 ? `✅ ${pass} 项通过` : `❌ ${fails.length} 项失败：\n` + fails.join("\n"));
process.exit(fails.length === 0 ? 0 : 1);
