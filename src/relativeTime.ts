/**
 * 全应用统一的紧凑相对时间。
 *
 * 之前有两套：通知面板是 `2m / 3h / 5d`，侧栏用的是 Rust 侧 `humanize_duration`
 * 产出的 `2 分钟前 / 3 小时前`。后者放在 session 行的右列有两个问题：
 *   1. 宽度不定（"刚刚" 2 字 ~ "10 个月前" 5 字），右列没有稳定基线，整列看着在抖；
 *   2. 它占掉的横向空间是从同一行的标题里扣的 —— 侧栏只有 280px，标题本来就不够。
 * 两位数以内的紧凑写法宽度基本恒定，代价只是"前"字没了，而"这是时间"由位置说明。
 *
 * 月/年两档是照 humanize_duration 的分档补的：session 列表里真有半年前的会话，
 * 只到 d 会显示成 200d。
 */
export function relativeTime(tsMs: number): string {
  const diff = Math.floor((Date.now() - tsMs) / 1000);
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)}d`;
  if (diff < 86400 * 365) return `${Math.floor(diff / (86400 * 30))}mo`;
  return `${Math.floor(diff / (86400 * 365))}y`;
}
