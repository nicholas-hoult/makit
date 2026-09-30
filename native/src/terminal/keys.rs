//! 按键 → 写给 PTY 的字节（#221）。
//!
//! 为什么单独测：这张表错一格，界面上的表现是「按了没反应」或「按出乱码」——
//! 比如方向键在 claude 的全屏界面里（应用光标模式）必须发 `ESC O A` 而不是 `ESC [ A`，
//! 发错了 claude 的选择菜单就上下不动；Ctrl+C 发错了就停不下正在跑的命令。
//! 这些只能靠真按键一个个试，所以把映射抽成和 GPUI 无关的纯函数钉在测试里。
//!
//! 约定：返回 `None` 的按键不归终端直接处理 —— 可打印字符交给输入法（中文输入要走
//! 输入法的上屏回调），⌘ 组合键留给应用快捷键（复制、粘贴、新标签）。
//! 表参考 Zed `crates/terminal/src/mappings/keys.rs`（gpui 0.2.2 对应的 69e2130）。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

pub fn key_to_bytes(key: &str, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    if mods.cmd {
        return None;
    }
    let b = |s: &str| Some(s.as_bytes().to_vec());
    let plain = !mods.ctrl && !mods.alt && !mods.shift;

    match key {
        "enter" if mods.shift || mods.alt => return b("\x1b\r"),
        "enter" => return b("\r"),
        "tab" if mods.shift => return b("\x1b[Z"),
        "tab" if !mods.ctrl && !mods.alt => return b("\t"),
        "escape" => return b("\x1b"),
        "backspace" if mods.alt => return b("\x1b\x7f"),
        "backspace" if mods.ctrl => return b("\x08"),
        "backspace" => return b("\x7f"),
        "space" if mods.ctrl => return Some(vec![0]),
        "left" if mods.alt && !mods.ctrl && !mods.shift => return b("\x1bb"),
        "right" if mods.alt && !mods.ctrl && !mods.shift => return b("\x1bf"),
        _ => {}
    }

    // 方向键 / Home / End：没修饰键时看应用光标模式，有修饰键走 xterm 的 `CSI 1;<码> X`
    let cursor_final = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(f) = cursor_final {
        return if plain {
            Some(if app_cursor { format!("\x1bO{f}") } else { format!("\x1b[{f}") }.into_bytes())
        } else {
            Some(format!("\x1b[1;{}{f}", modifier_code(mods)).into_bytes())
        };
    }

    // `CSI <n> ~` 一族
    let tilde = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return Some(if plain { format!("\x1b[{n}~") } else { format!("\x1b[{n};{}~", modifier_code(mods)) }.into_bytes());
    }
    let ss3 = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(f) = ss3 {
        return Some(if plain { format!("\x1bO{f}") } else { format!("\x1b[1;{}{f}", modifier_code(mods)) }.into_bytes());
    }

    // Ctrl + 字母 / 符号 → 控制字符（caret notation）
    if mods.ctrl && !mods.alt {
        let mut chars = key.chars();
        if let (Some(ch), None) = (chars.next(), chars.next()) {
            let ch = ch.to_ascii_lowercase();
            let code = match ch {
                'a'..='z' => Some(ch as u8 - b'a' + 1),
                '@' | '2' => Some(0),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' | '7' => Some(0x1f),
                '?' | '8' => Some(0x7f),
                _ => None,
            };
            if let Some(c) = code {
                return Some(vec![c]);
            }
        }
    }
    None
}

/// 程序开了鼠标上报（SGR 1006）时，滚轮要编码成鼠标事件发给它，而不是滚我们自己的回看：
/// `CSI < 64/65 ; 列 ; 行 M`（列、行从 1 起）。不这么做，开了鼠标模式的全屏程序里滚轮没反应
pub fn sgr_wheel(up: bool, col: usize, row: usize) -> Vec<u8> {
    format!("\x1b[<{};{};{}M", if up { 64 } else { 65 }, col + 1, row + 1).into_bytes()
}

/// 鼠标按键（上报给开了鼠标模式的程序用）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// 程序开了鼠标上报（DECSET 1000/1002/1003）时，点击 / 拖动 / 滚轮编码成鼠标事件发给它（claude 全屏界面、vim、htop）。
/// `sgr`：程序开了 1006（`CSI < b;x;y M/m`，坐标无上限）；否则走 X10 老格式（`CSI M` + 三个 32 起的字节，
/// 坐标超过 222 编不出来，返回 None）。`motion` = 拖动 / 移动（按键码 +32）。修饰键：Shift 4、Alt 8、Ctrl 16。
/// 列、行从 0 起传进来，编码时 +1。
pub fn mouse_report(btn: MouseBtn, pressed: bool, motion: bool, col: usize, row: usize, mods: Mods, sgr: bool) -> Option<Vec<u8>> {
    let mut code: u32 = match btn {
        MouseBtn::Left => 0,
        MouseBtn::Middle => 1,
        MouseBtn::Right => 2,
        MouseBtn::WheelUp => 64,
        MouseBtn::WheelDown => 65,
    };
    if motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    if sgr {
        return Some(format!("\x1b[<{code};{};{}{}", col + 1, row + 1, if pressed { 'M' } else { 'm' }).into_bytes());
    }
    // X10：松开统一报 3（不知道是哪个键）
    if !pressed {
        code = (code & !0b11) | 3;
    }
    if col > 222 || row > 222 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code as u8, 33 + col as u8, 33 + row as u8])
}

/// xterm 修饰键编码：1 + Shift(1) + Alt(2) + Ctrl(4)
fn modifier_code(m: Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, cmd: false };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const CMD: Mods = Mods { cmd: true, ..NONE };

    fn k(key: &str, m: Mods) -> Option<Vec<u8>> {
        key_to_bytes(key, m, false)
    }

    #[test]
    fn basic_named_keys() {
        assert_eq!(k("enter", NONE), Some(b"\r".to_vec()));
        assert_eq!(k("tab", NONE), Some(b"\t".to_vec()));
        assert_eq!(k("tab", SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(k("escape", NONE), Some(b"\x1b".to_vec()));
        assert_eq!(k("backspace", NONE), Some(b"\x7f".to_vec()));
        assert_eq!(k("backspace", ALT), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(k("delete", NONE), Some(b"\x1b[3~".to_vec()));
        assert_eq!(k("pageup", NONE), Some(b"\x1b[5~".to_vec()));
        assert_eq!(k("f1", NONE), Some(b"\x1bOP".to_vec()));
        assert_eq!(k("f5", NONE), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn shift_or_alt_enter_is_meta_enter_for_claude_newline() {
        // claude 把 ESC CR 当「换行不提交」（它的 /terminal-setup 给 iTerm 配的就是这个）
        assert_eq!(k("enter", SHIFT), Some(b"\x1b\r".to_vec()));
        assert_eq!(k("enter", ALT), Some(b"\x1b\r".to_vec()));
    }

    #[test]
    fn arrows_follow_app_cursor_mode() {
        assert_eq!(key_to_bytes("up", NONE, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes("up", NONE, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_to_bytes("left", NONE, true), Some(b"\x1bOD".to_vec()));
        assert_eq!(key_to_bytes("home", NONE, false), Some(b"\x1b[H".to_vec()));
        assert_eq!(key_to_bytes("end", NONE, true), Some(b"\x1bOF".to_vec()));
    }

    #[test]
    fn modified_arrows_use_xterm_modifier_code() {
        assert_eq!(k("up", SHIFT), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(k("right", CTRL), Some(b"\x1b[1;5C".to_vec()));
        // ⌥←/→ 按 macOS 习惯跳词（iTerm「Natural Text Editing」同款）
        assert_eq!(k("left", ALT), Some(b"\x1bb".to_vec()));
        assert_eq!(k("right", ALT), Some(b"\x1bf".to_vec()));
    }

    #[test]
    fn ctrl_letters_are_caret_notation() {
        assert_eq!(k("c", CTRL), Some(vec![0x03]));
        assert_eq!(k("a", CTRL), Some(vec![0x01]));
        assert_eq!(k("z", CTRL), Some(vec![0x1a]));
        assert_eq!(k("space", CTRL), Some(vec![0x00]));
        assert_eq!(k("[", CTRL), Some(vec![0x1b]));
        assert_eq!(k("\\", CTRL), Some(vec![0x1c]));
    }

    #[test]
    fn sgr_wheel_is_one_based() {
        assert_eq!(sgr_wheel(true, 0, 0), b"\x1b[<64;1;1M".to_vec());
        assert_eq!(sgr_wheel(false, 9, 4), b"\x1b[<65;10;5M".to_vec());
    }

    #[test]
    fn mouse_reports() {
        let r = |b, p, m, c, rw, mods, sgr| mouse_report(b, p, m, c, rw, mods, sgr);
        assert_eq!(r(MouseBtn::Left, true, false, 0, 0, NONE, true), Some(b"\x1b[<0;1;1M".to_vec()), "SGR 按下");
        assert_eq!(r(MouseBtn::Left, false, false, 9, 4, NONE, true), Some(b"\x1b[<0;10;5m".to_vec()), "SGR 松开是小写 m");
        assert_eq!(r(MouseBtn::Left, true, true, 2, 3, NONE, true), Some(b"\x1b[<32;3;4M".to_vec()), "拖动 +32");
        assert_eq!(r(MouseBtn::WheelUp, true, false, 0, 0, CTRL, true), Some(b"\x1b[<80;1;1M".to_vec()), "滚轮 64 + Ctrl 16");
        assert_eq!(r(MouseBtn::Right, true, false, 0, 0, NONE, false), Some(vec![0x1b, b'[', b'M', 34, 33, 33]), "X10 按下");
        assert_eq!(r(MouseBtn::Right, false, false, 0, 0, NONE, false), Some(vec![0x1b, b'[', b'M', 35, 33, 33]), "X10 松开报 3");
        assert_eq!(r(MouseBtn::Left, true, false, 300, 0, NONE, false), None, "X10 编不出大坐标");
        assert_eq!(r(MouseBtn::Left, true, false, 300, 0, NONE, true).map(|v| v.len()), Some(11), "SGR 没有上限（ESC[<0;301;1M）");
    }

    #[test]
    fn printable_and_cmd_keys_are_not_handled_here() {
        // 可打印字符交给输入法，否则中文输入法的拼音会被当成英文字母直接发出去
        assert_eq!(k("a", NONE), None);
        assert_eq!(k("a", SHIFT), None);
        assert_eq!(k("space", NONE), None);
        assert_eq!(k("a", ALT), None);
        // ⌘ 组合键留给应用
        assert_eq!(k("c", CMD), None);
        assert_eq!(k("enter", CMD), None);
    }
}
