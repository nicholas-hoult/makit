//! 解析 iTerm2 `.itermcolors`（XML plist）→ 源色（照搬 `src/itermcolors.ts`）。
//!
//! 文件结构：根 `<dict>` 里 `<key>Background Color</key><dict>…Red/Green/Blue Component…</dict>`
//! 这样 key / value 兄弟节点按顺序配对。颜色 dict 里的值是 `<real>` / `<integer>`，不会再嵌套 dict，
//! 所以不引 XML 库：认 `<key>名字</key>` 紧跟 `<dict>` 的就是一条颜色。
//!
//! 为什么单独测：导入的配色存进持久化，错了是「导进来的主题全黑 / 颜色错位」，而且
//! 只有用户自己的那份文件能复现。

use super::derive::ThemeSource;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItermColors {
    pub bg: String,
    pub fg: String,
    pub selection: Option<String>,
    /// 16 色，顺序同 ANSI 0..15；缺的用 fg 补
    pub ansi: Vec<String>,
}

fn to_hex2(n: f64) -> String {
    format!("{:02x}", (n.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8)
}

/// 取 `<tag>…</tag>` 之间的文本，返回 (内容, 结束标签之后的剩余)
fn take_element<'a>(s: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let s = s.trim_start();
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let rest = s.strip_prefix(&open)?;
    let end = rest.find(&close)?;
    Some((&rest[..end], &rest[end + close.len()..]))
}

/// 一个颜色 dict 的内容 → `#rrggbb`
fn read_color_dict(body: &str) -> String {
    let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
    let mut rest = body;
    while let Some(pos) = rest.find("<key>") {
        let Some((key, after)) = take_element(&rest[pos..], "key") else { break };
        let value = take_element(after, "real").or_else(|| take_element(after, "integer"));
        let num = value.and_then(|(v, _)| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        match key.trim() {
            "Red Component" => r = num,
            "Green Component" => g = num,
            "Blue Component" => b = num,
            _ => {}
        }
        rest = after;
    }
    format!("#{}{}{}", to_hex2(r), to_hex2(g), to_hex2(b))
}

pub fn parse_itermcolors(xml: &str) -> Result<ParsedItermColors, String> {
    let start = xml.find("<plist").ok_or("plist 根节点缺失")?;
    let body = &xml[start..];
    let dict_start = body.find("<dict>").ok_or("plist 根节点缺失")?;
    let mut rest = &body[dict_start + "<dict>".len()..];
    let mut colors: Vec<(String, String)> = Vec::new();
    while let Some(pos) = rest.find("<key>") {
        let Some((key, after)) = take_element(&rest[pos..], "key") else {
            return Err("XML 解析失败".into());
        };
        match take_element(after, "dict") {
            Some((dict, after_dict)) => {
                colors.push((key.trim().to_string(), read_color_dict(dict)));
                rest = after_dict;
            }
            None => rest = after,
        }
    }
    let get = |k: &str| colors.iter().find(|(n, _)| n == k).map(|(_, c)| c.clone());
    let (Some(bg), Some(fg)) = (get("Background Color"), get("Foreground Color")) else {
        return Err("缺少 Background/Foreground Color".into());
    };
    let ansi = (0..16).map(|i| get(&format!("Ansi {i} Color")).unwrap_or_else(|| fg.clone())).collect();
    Ok(ParsedItermColors { selection: get("Selection Color"), bg, fg, ansi })
}

/// 导入一个文件：id 为 `imported:<去掉扩展名的文件名>`（同名覆盖由调用方按 id 替换）
pub fn import_itermcolors(file_name: &str, xml: &str) -> Result<ThemeSource, String> {
    let parsed = parse_itermcolors(xml)?;
    let base = file_name.trim();
    let base = if base.to_ascii_lowercase().ends_with(".itermcolors") { &base[..base.len() - ".itermcolors".len()] } else { base };
    let name = if base.trim().is_empty() { "导入的配色".to_string() } else { base.trim().to_string() };
    Ok(ThemeSource {
        id: format!("imported:{name}"),
        name,
        bg: parsed.bg,
        fg: parsed.fg,
        ansi: parsed.ansi,
        selection: parsed.selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(key: &str, r: f64, g: f64, b: f64) -> String {
        format!(
            "<key>{key}</key>\n\t<dict>\n\t\t<key>Alpha Component</key>\n\t\t<real>1</real>\n\t\t<key>Blue Component</key>\n\t\t<real>{b}</real>\n\t\t<key>Color Space</key>\n\t\t<string>sRGB</string>\n\t\t<key>Green Component</key>\n\t\t<real>{g}</real>\n\t\t<key>Red Component</key>\n\t\t<integer>{r}</integer>\n\t</dict>\n"
        )
    }

    fn plist(entries: &str) -> String {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist>\n<plist version=\"1.0\">\n<dict>\n{entries}</dict>\n</plist>\n")
    }

    #[test]
    fn parses_bg_fg_selection_and_ansi() {
        let mut e = color("Background Color", 0.0, 0.5, 1.0);
        e += &color("Foreground Color", 1.0, 1.0, 1.0);
        e += &color("Selection Color", 0.2, 0.2, 0.2);
        e += &color("Ansi 1 Color", 1.0, 0.0, 0.0);
        let p = parse_itermcolors(&plist(&e)).unwrap();
        // 0.5*255 = 127.5 → 四舍五入 128 = 0x80（和 TS 的 Math.round 一致）
        assert_eq!(p.bg, "#0080ff");
        assert_eq!(p.fg, "#ffffff");
        assert_eq!(p.selection.as_deref(), Some("#333333"));
        assert_eq!(p.ansi.len(), 16);
        assert_eq!(p.ansi[1], "#ff0000");
        assert_eq!(p.ansi[0], "#ffffff", "缺的 ANSI 色用 fg 补");
    }

    #[test]
    fn missing_bg_or_fg_is_an_error() {
        let e = color("Background Color", 0.0, 0.0, 0.0);
        assert!(parse_itermcolors(&plist(&e)).unwrap_err().contains("Foreground"));
        assert!(parse_itermcolors("随便什么").is_err());
    }

    #[test]
    fn import_names_theme_after_file() {
        let mut e = color("Background Color", 0.0, 0.0, 0.0);
        e += &color("Foreground Color", 1.0, 1.0, 1.0);
        let t = import_itermcolors("Gruvbox Material.itermcolors", &plist(&e)).unwrap();
        assert_eq!((t.id.as_str(), t.name.as_str()), ("imported:Gruvbox Material", "Gruvbox Material"));
        assert_eq!(import_itermcolors(".itermcolors", &plist(&e)).unwrap().name, "导入的配色");
    }
}
