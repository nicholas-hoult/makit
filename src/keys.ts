/**
 * 全局快捷键的修饰键规则。
 *
 * 这个 app 只跑在 macOS 上，而 macOS 上有一条硬约束：**⌃ 在终端里是数据，不是修饰键**。
 * ⌃L 清屏、⌃R 反查历史、⌃K 删到行尾、⌃A/⌃E 行首行尾、⌃U 删整行、⌃W 删一词、⌃C 中断、
 * ⌃D EOF、⌃I 就是 Tab 字符 —— 这些都是要原样送进 PTY 的字节，不是给 app 用的按键。
 *
 * 原来全项目 9 处写的是 `e.metaKey || e.ctrlKey`，那是 Windows/Linux 的 "mod 键" 写法
 * （那边 Ctrl 才是主修饰键）。搬到 macOS 上就变成：每个 ⌘X 快捷键同时也吃掉 ⌃X。
 * 而这些 handler 全挂在**捕获阶段**并 `preventDefault + stopPropagation`（为了抢在 xterm
 * 之前拿到事件），所以它们赢得干干净净 —— 用户按 ⌃L 想清屏，屏没清，侧栏还跳了一下。
 *
 * 这个坑之前局部踩过一次并修过（⌃[ 在终端里就是 ESC，被切 tab 抢掉导致 vim 退不出插入
 * 模式，见 App.tsx 括号那段注释），但只修了那一处，规则没立起来。这里把它立成一条：
 * **全局快捷键一律只认 ⌘。**
 *
 * 唯一例外是 ⌃Tab / ⌃⇧Tab（切 tab）—— 那是浏览器 / VS Code / iTerm2 的通用肌肉记忆，
 * 而且 `e.code === "Tab"` 和 ⌃I（`e.code === "KeyI"`）是两个不同的键，不会串。
 * 例外走它自己那段 `e.ctrlKey && !e.metaKey && e.code === "Tab"`，不经过这个谓词。
 */
export function isCmd(e: { metaKey: boolean; ctrlKey: boolean }): boolean {
  // 连 ⌘⌃X 也一并排除：这个 app 没有任何 ⌘⌃ 组合快捷键，
  // 手滑同时按到时宁可什么都不做，也不要触发一个用户没想要的操作。
  return e.metaKey && !e.ctrlKey;
}
