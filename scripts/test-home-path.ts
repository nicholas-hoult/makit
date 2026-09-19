/**
 * 路径里的 home 缩写成 `~`（#192）。
 *
 * 为什么单独测：原实现把 home 写死成开发者本机的 `/Users/<名字>`，换一台电脑缩写就失效，
 * 还把电脑用户名带进了公开代码。改成运行时取 home 之后，边界全在这个纯函数里：
 * `/Users/me` 不能把 `/Users/meg/x` 当成自己的子目录缩写成 `~g/x`；home 还没取到时原样返回。
 * 错了在 UI 上就是侧栏卡片里的路径显示成别人家的 `~g/...`，或者整条长路径不缩写。
 */
import { shortenHome } from "../src/homePath.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  if (got !== want) {
    console.error(`✗ ${name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
    process.exit(1);
  }
}

eq("home 下的子目录缩写成 ~/", shortenHome("/Users/me/RustProjects/makit", "/Users/me"), "~/RustProjects/makit");
eq("home 本身缩写成 ~", shortenHome("/Users/me", "/Users/me"), "~");
eq("前缀相同的别人家目录不缩写", shortenHome("/Users/meg/x", "/Users/me"), "/Users/meg/x");
eq("home 外的路径原样", shortenHome("/tmp/a", "/Users/me"), "/tmp/a");
eq("home 末尾带斜杠也能认", shortenHome("/Users/me/a", "/Users/me/"), "~/a");
eq("home 还没取到（空）→ 原样", shortenHome("/Users/me/a", ""), "/Users/me/a");

console.log(`✓ home 路径缩写全部通过（${n} 项）`);
