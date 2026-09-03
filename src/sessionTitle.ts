/**
 * 会话标题的显示层规则。
 *
 * 抽成独立模块的唯一目的是**能测** —— 这三个函数原来定义在 App 组件里（虽然是纯函数、
 * 不闭包任何状态），测试脚本进不去。它承担的不变量是「同一条会话在 tab / ⌘K / 拖拽影像
 * 里叫同一个名字，且这个名字由哪个字段决定是确定的」。
 *
 * 后端那半（`display_name` 的优先级：rename > customTitle > agentName > 留空）在
 * `lib.rs` 的 `parse_session` 里，有它自己的 Rust 测试。这里只管拿到 SessionMeta 之后。
 */

/** 标题函数只需要这三个字段，不引 App.tsx 的 SessionMeta（会绕成循环依赖） */
export type TitleFields = {
  display_name: string;
  first_user_msg: string;
  short_id: string;
};

/**
 * 压平空白：换行 / 制表符 / 连续空格一律收成单个空格。
 *
 * 这一步是**数据清洗，不是显示决策**，所以必须做：首条用户消息经常是多行的（贴了一段
 * 代码、或者带缩进的需求），原样塞进单行容器里 `white-space: nowrap` 虽然不会换行，
 * 但制表符和连续空格会在标题中间留出莫名的大洞。
 */
export function flatten(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

/**
 * 压平 + 截断。
 *
 * 注意：目前**没有活的消费点** —— 唯一的两处调用在 App.tsx 的 `renderSessionCard` 里，
 * 而那个函数零引用（侧栏早已换成 SessionTree.tsx），构建时被整块摇掉。留着它是因为
 * 「有长度上限」和「无长度上限」是两个不同的需求：`deriveSessionTabLabel` 那种交给
 * CSS 裁剪的单行标题不需要它，而多行预览（真要恢复的话）需要。
 */
export function truncate(text: string, n = 100): string {
  const flat = flatten(text);
  return flat.length <= n ? flat : flat.slice(0, n) + "…";
}

/**
 * SessionMeta → 显示名。tab 标题 / ⌘K 面板 / 拖拽影像 / 落点提示**共用这一个出口**。
 *
 * `display_name` 为空不代表「没名字」，而是后端在说「我这儿没有比首条用户消息更好的
 * 名字」（见 lib.rs 里那段注释），所以第二档兜到首条消息，而不是直接跳到 short_id。
 *
 * 这里**故意不做长度截断**。三个消费点全都已经有 CSS 裁剪了（tab `max-width:180px`、
 * drag ghost `280px`、palette 随容器，都带 `text-overflow: ellipsis`），在这里再剪一刀
 * 只会「比 CSS 更早地剪短」—— 那是拿数据层去替显示层做决定，而每个消费点的可用宽度
 * 根本不一样，一个数字不可能同时对三处正确。
 *
 * 原来这里是 `truncate(first_user_msg, 24)`，代价最实的地方是 ⌘K 面板：那是找会话的
 * 主入口、行宽是 tab 的两三倍，而开头相似的会话（「帮我看一下这个函数为什么…」）在
 * 24 字处还没分岔，列表里看起来是一模一样的两行。同一个列表里 codex 会话却能显示到
 * 120 字 —— 因为它的首条消息是后端直接塞进 `display_name` 的，走的是上面第一档，
 * 压根不经过那次截断。
 */
export function deriveSessionTabLabel(s: TitleFields): string {
  if (s.display_name && s.display_name.trim()) {
    return flatten(s.display_name);
  }
  if (s.first_user_msg && s.first_user_msg.trim()) {
    return flatten(s.first_user_msg);
  }
  return `[${s.short_id}]`;
}
