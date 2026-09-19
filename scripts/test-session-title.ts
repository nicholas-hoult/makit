/**
 * 会话标题规则：**tab / ⌘K / 拖拽影像共用同一个出口，且长度由 CSS 决定不由数据层决定**。
 *
 * 这条不变量之前完全没有测试覆盖，而 `deriveSessionTabLabel` 是标题的唯一出口 ——
 * 它错一个字段优先级，整个 app 里会话的名字就全错。
 *
 * 最要紧的是「不截断」那几条：三个消费点都已经有 CSS ellipsis（tab max-width:180px、
 * drag ghost 280px、palette 随容器），数据层再剪一刀只会比 CSS 更早剪短，而三处可用
 * 宽度不同，一个数字不可能同时对三处正确。旧实现在这里写的是 `truncate(…, 24)`。
 */
import { flatten, truncate, deriveSessionTabLabel } from "../src/sessionTitle.ts";

let pass = 0;
const fails: string[] = [];

function eq(name: string, actual: unknown, expected: unknown) {
  if (actual === expected) pass++;
  else fails.push(`${name}\n      期望 ${JSON.stringify(expected)}\n      实际 ${JSON.stringify(actual)}`);
}

function meta(o: Partial<Parameters<typeof deriveSessionTabLabel>[0]>) {
  return { display_name: "", first_user_msg: "", short_id: "abc1234", ...o };
}

// ---- 字段优先级 ----------------------------------------------------------
eq(
  "display_name 优先于首条消息",
  deriveSessionTabLabel(meta({ display_name: "重构登录流程", first_user_msg: "帮我看看这段" })),
  "重构登录流程",
);
eq(
  "display_name 为空时兜到首条消息（不是直接跳 short_id）",
  deriveSessionTabLabel(meta({ first_user_msg: "帮我看看这段" })),
  "帮我看看这段",
);
eq(
  "display_name 只有空白算空",
  deriveSessionTabLabel(meta({ display_name: "   ", first_user_msg: "帮我看看这段" })),
  "帮我看看这段",
);
eq(
  "两者都空时用 [short_id]",
  deriveSessionTabLabel(meta({})),
  "[abc1234]",
);
eq(
  "首条消息只有空白也算空",
  deriveSessionTabLabel(meta({ first_user_msg: "\n\t  " })),
  "[abc1234]",
);

// ---- 不截断（本次改动的核心） --------------------------------------------
// 60 个汉字：远超旧实现的 24 字上限，也超过 tab 条 180px 能显示的宽度。
// 后者不影响这条断言 —— 剪到多少是 CSS 的事，这里的契约是「原样交出去」。
const LONG = "帮我看一下这个函数为什么在并发情况下会偶发返回空值我怀疑是缓存那块的问题但是不确定要不要加锁";
eq("长首条消息不截断", deriveSessionTabLabel(meta({ first_user_msg: LONG })), LONG);
eq("长 display_name 不截断", deriveSessionTabLabel(meta({ display_name: LONG })), LONG);
eq("不截断时也不该追加省略号", deriveSessionTabLabel(meta({ first_user_msg: LONG })).includes("…"), false);

// codex 场景端到端：后端把首条消息（截到 120 字符）直接塞进 display_name，
// 走第一档。claude 场景：display_name 空，走第二档。**同一条消息两条路必须同名** ——
// 旧实现里前者 120 字后者 24 字，同一个 ⌘K 列表里两种会话长度差 5 倍。
eq(
  "同一条消息走 display_name 档和首条消息档结果一致",
  deriveSessionTabLabel(meta({ display_name: LONG })),
  deriveSessionTabLabel(meta({ first_user_msg: LONG })),
);

// ---- 压平空白（数据清洗，必须做） ----------------------------------------
eq(
  "首条消息里的换行被压成单空格",
  deriveSessionTabLabel(meta({ first_user_msg: "第一行\n第二行" })),
  "第一行 第二行",
);
eq(
  "display_name 里的换行也被压平（codex 的首条消息只 trim 过两端，内部换行会留着）",
  deriveSessionTabLabel(meta({ display_name: "修一下\n\tthis bug" })),
  "修一下 this bug",
);
eq(
  "连续空格 / 制表符压成一个空格",
  deriveSessionTabLabel(meta({ first_user_msg: "a  \t  b" })),
  "a b",
);
eq("首尾空白被去掉", deriveSessionTabLabel(meta({ first_user_msg: "  x  " })), "x");

// ---- flatten / truncate 本身 --------------------------------------------
eq("flatten 不截断", flatten(LONG), LONG);
// truncate 目前没有活的消费点（见 sessionTitle.ts 里的说明），测它是为了锁住语义：
// 万一以后有人拿它去做标题，红的应该是这里，而不是用户看到标题被剪短。
eq("truncate 仍然截断并加省略号", truncate("abcdef", 3), "abc…");
eq("truncate 边界：正好等于上限不加省略号", truncate("abc", 3), "abc");
eq("truncate 也压平空白", truncate("a\nb", 100), "a b");

if (fails.length) {
  console.error(`✗ 会话标题规则失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 会话标题规则全部通过（${pass} 项）`);
