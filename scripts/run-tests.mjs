// 跑 scripts/ 下全部 test-*.ts（Node 22+ 的 --experimental-strip-types，零依赖）。
// 用法：pnpm test [关键字]   —— 带关键字只跑文件名包含它的测试
import { readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = dirname(fileURLToPath(import.meta.url));
const filter = process.argv[2] ?? "";
const tests = readdirSync(dir)
  .filter((f) => /^test-.*\.ts$/.test(f) && f.includes(filter))
  .sort();

let failed = [];
for (const f of tests) {
  const r = spawnSync(process.execPath, ["--experimental-strip-types", "--no-warnings", join(dir, f)], {
    encoding: "utf8",
  });
  const out = (r.stdout + r.stderr).trim();
  if (r.status === 0) {
    console.log(`✅ ${f}  ${out.split("\n").at(-1) ?? ""}`);
  } else {
    failed.push(f);
    console.log(`❌ ${f}\n${out.split("\n").slice(-15).join("\n")}\n`);
  }
}
console.log(`\n${tests.length - failed.length}/${tests.length} 通过`);
process.exit(failed.length ? 1 : 0);
