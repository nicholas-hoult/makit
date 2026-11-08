//! 本机会话接口的屏幕快照（#299）：把 alacritty 的网格转成 `makit_core::link::Screen`，给别的程序原样显示终端画面。
//! 只取当前屏（不含回滚历史，也不管用户在本机往上翻到了哪里），宽字符的占位格丢掉。

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use makit_core::link::{render_ansi_line, Screen, ScreenCell, ScreenColor};

/// 前景 / 背景等语义色交给对方的默认色；16 个基本色按序号；真彩色原样。
/// 为什么单独测：映射错了，手机上的终端视图会把「红色的报错」显示成默认色，或把整屏背景涂黑。
pub fn color_of(c: Color) -> ScreenColor {
    match c {
        Color::Spec(rgb) => ScreenColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => ScreenColor::Indexed(i),
        Color::Named(n) => named(n),
    }
}

fn named(n: NamedColor) -> ScreenColor {
    let i = n as usize;
    if i < 16 {
        return ScreenColor::Indexed(i as u8);
    }
    // 暗色变体按对应的基本色
    let dim = match n {
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        _ => return ScreenColor::Default,
    };
    ScreenColor::Indexed(dim)
}

pub fn cell_of(c: &Cell) -> ScreenCell {
    ScreenCell {
        ch: c.c,
        fg: color_of(c.fg),
        bg: color_of(c.bg),
        bold: c.flags.contains(Flags::BOLD),
        dim: c.flags.contains(Flags::DIM),
        italic: c.flags.contains(Flags::ITALIC),
        underline: c.flags.intersects(Flags::ALL_UNDERLINES),
        inverse: c.flags.contains(Flags::INVERSE),
    }
}

/// 当前屏的快照。调用方持有终端锁。
pub fn snapshot<T>(term: &Term<T>) -> Screen {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let lines = (0..rows)
        .map(|y| {
            let row = &grid[Line(y as i32)];
            let cells: Vec<ScreenCell> = (0..cols).map(|x| &row[Column(x)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(cell_of).collect();
            render_ansi_line(&cells)
        })
        .collect();
    let cursor = grid.cursor.point;
    Screen { cols, rows, cursor_x: cursor.column.0, cursor_y: cursor.line.0.max(0) as usize, lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    #[test]
    fn colors_map_to_what_a_terminal_emulator_understands() {
        assert_eq!(color_of(Color::Named(NamedColor::Red)), ScreenColor::Indexed(1));
        assert_eq!(color_of(Color::Named(NamedColor::BrightWhite)), ScreenColor::Indexed(15));
        assert_eq!(color_of(Color::Named(NamedColor::DimGreen)), ScreenColor::Indexed(2));
        // 语义色交给对方的默认色，不能变成黑色
        assert_eq!(color_of(Color::Named(NamedColor::Foreground)), ScreenColor::Default);
        assert_eq!(color_of(Color::Named(NamedColor::Background)), ScreenColor::Default);
        assert_eq!(color_of(Color::Indexed(208)), ScreenColor::Indexed(208));
        assert_eq!(color_of(Color::Spec(Rgb { r: 1, g: 2, b: 3 })), ScreenColor::Rgb(1, 2, 3));
    }

    #[test]
    fn cell_flags_carry_over() {
        let mut c = Cell::default();
        c.c = 'x';
        c.flags = Flags::BOLD | Flags::INVERSE | Flags::UNDERCURL;
        let s = cell_of(&c);
        assert_eq!((s.ch, s.bold, s.inverse, s.underline, s.italic, s.dim), ('x', true, true, true, false, false));
        assert_eq!(cell_of(&Cell::default()), ScreenCell::default());
    }
}
