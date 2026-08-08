// pane 落点图标 —— 切 pane 时中央浮出的那张牌上的图标
//
// 图标**按 pane 序号固定**：第 1 个 pane 永远是这一套里的第 1 个图标，第 2 个永远是第 2 个……
// 序号就是 ⌥⌘N 里的 N。固定是这件事的全部意义：每次随机的话图标只是装饰噪点，
// 绑到按键的那个数上之后它才成了 pane 的身份标记 —— 看一眼形状比读名字快，
// 而那个数正是你按键时脑子里想的数。也因此不按 containerId 哈希：那样更稳
// （关掉别的 pane 也不变）但和 ⌥⌘N 对不上，等于另立了一套只有它自己知道的编号。
//
// 为什么不做进主题（theme.ts）：一个主题在本项目里的定义是"一份 .itermcolors 等量的
// 数据"—— bg / fg / ANSI 16 色，其余 UI 变量全部由它推导。往里塞 emoji 会破坏这个
// 不变量：导入的 .itermcolors 里不可能有图标字段，于是导入主题必然缺一块、得有回退，
// 而"配色"和"用哪一套图标"本来也是两个互不影响的轴（浅色主题配灵兽没有任何不协调）。
// 所以它是设置面板里自己的一项，和主题并列，各自持久化。
//
// 一套之内九个必须**同类**：混进一个火箭，它就是这一列里唯一的非生物，眼睛会先去找它
// 而不是找"第几个"——图标能当序号用的前提是九个地位相同。要换就整套换，这也是
// 一套一个数组、而不是九个独立槽位的原因。

export type PaneIconSet = {
  id: string;
  name: string;
  /** 九个（对应 ⌥⌘1~⌥⌘9）。空数组表示不显示图标 */
  icons: readonly string[];
};

export const PANE_ICON_SETS: readonly PaneIconSet[] = [
  { id: "beasts", name: "灵兽", icons: ["🐉", "🐯", "🦊", "🐳", "🦉", "🐝", "🦄", "🐙", "🐺"] },
  { id: "flowers", name: "花木", icons: ["🌸", "🌹", "🌻", "🌷", "🌺", "🌼", "🪷", "💐", "🌾"] },
  { id: "fruits", name: "果园", icons: ["🍎", "🍊", "🍋", "🍇", "🍓", "🍑", "🥝", "🍒", "🥭"] },
  // 不想要 emoji 那种热闹感时用这套：九个同形不同色，只剩"第几个"这一个信息
  { id: "dots", name: "色点", icons: ["🔴", "🟠", "🟡", "🟢", "🔵", "🟣", "🟤", "⚫️", "⚪️"] },
  { id: "none", name: "不显示", icons: [] },
];

const KEY = "makit-pane-icons";
const DEFAULT_ID = "beasts";

export function savedPaneIconSetId(): string {
  try {
    const id = localStorage.getItem(KEY);
    if (id && PANE_ICON_SETS.some((s) => s.id === id)) return id;
  } catch {
    /* localStorage 不可用时用默认套 */
  }
  return DEFAULT_ID;
}

export function savePaneIconSetId(id: string) {
  try {
    localStorage.setItem(KEY, id);
  } catch {
    /* 存不进去就只影响下次启动，不值得打断切换 */
  }
}

export function paneIconsFor(id: string): readonly string[] {
  return (PANE_ICON_SETS.find((s) => s.id === id) ?? PANE_ICON_SETS[0]).icons;
}
