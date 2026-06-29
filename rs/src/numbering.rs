//! Parser cho word/numbering.xml — file lưu định nghĩa số/prefix cho mỗi
//! list trong DOCX. Render dùng để biết:
//!   - numId 1, ilvl 0 -> prefix "•" hay "1." hay "▪" hay "a."?
//!
//! Cấu trúc numbering.xml (rút gọn):
//!   <w:abstractNum w:abstractNumId="N">
//!     <w:lvl w:ilvl="0">
//!       <w:numFmt w:val="bullet|decimal|lowerLetter|upperRoman|..."/>
//!       <w:lvlText w:val="•"/>  hoặc "%1." hoặc "%1.%2." cho multi-level
//!     </w:lvl>
//!     ...
//!   </w:abstractNum>
//!   <w:num w:numId="1">
//!     <w:abstractNumId w:val="N"/>
//!   </w:num>
//!
//! Mục tiêu: build HashMap (numId, ilvl) -> LvlTemplate.

use crate::error::AppResult;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct LvlTemplate {
    /// Format: "bullet", "decimal", "lowerLetter", "upperLetter",
    /// "lowerRoman", "upperRoman", "none", v.v.
    pub num_fmt: String,
    /// Template text: "•", "▪", "%1.", "%1.%2.", "%1)", v.v.
    /// %N được thay bằng số/chữ tương ứng ở level N (1-based trong DOCX).
    pub lvl_text: String,
}

impl LvlTemplate {
    /// Render prefix cuối cùng cho list item.
    /// `counters` = ordinals theo level (1-based), vd [2, 3, 1] = "2.3.1".
    /// `ilvl` = level hiện tại (0-based).
    pub fn render(&self, counters: &[u32], ilvl: usize) -> String {
        if self.num_fmt == "bullet" {
            // Bullet: dùng lvl_text trực tiếp (• ▪ ▫ o –).
            // Một số file dùng ký tự private-use area của Wingdings (vd
            // U+F0B7 = bullet trong Wingdings) — convert về "•" cho gọn.
            return remap_wingdings(&self.lvl_text);
        }
        // Numbered: thay %1, %2, ... bằng counter format theo numFmt
        let mut out = String::with_capacity(self.lvl_text.len() + 8);
        let mut chars = self.lvl_text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                if let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                    chars.next();
                    let level = (d as usize).saturating_sub(1);
                    let n = counters.get(level).copied().unwrap_or(1);
                    out.push_str(&format_number(n, &self.num_fmt_for_level(level, ilvl)));
                    continue;
                }
            }
            out.push(c);
        }
        out
    }

    /// Trả về numFmt áp dụng cho level cụ thể. Hiện đơn giản: dùng cùng
    /// numFmt cho mọi level. Word phức tạp hơn (mỗi <w:lvl> có numFmt
    /// riêng) — nếu cần chính xác hơn, lookup theo level.
    fn num_fmt_for_level(&self, _level: usize, _current_ilvl: usize) -> String {
        self.num_fmt.clone()
    }
}

/// Format 1 số theo numFmt.
fn format_number(n: u32, fmt: &str) -> String {
    match fmt {
        "decimal" => n.to_string(),
        "decimalZero" => format!("{n:02}"),
        "lowerLetter" => letter_format(n, false),
        "upperLetter" => letter_format(n, true),
        "lowerRoman" => roman_format(n, false),
        "upperRoman" => roman_format(n, true),
        "none" => String::new(),
        _ => n.to_string(),
    }
}

/// 1 -> "a", 2 -> "b", ..., 26 -> "z", 27 -> "aa", ...
fn letter_format(n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let mut s = String::new();
    let mut x = n;
    while x > 0 {
        x -= 1;
        let c = (b'a' + (x % 26) as u8) as char;
        s.insert(0, if upper { c.to_ascii_uppercase() } else { c });
        x /= 26;
    }
    s
}

/// 1 -> "i", 2 -> "ii", 4 -> "iv", v.v.
fn roman_format(n: u32, upper: bool) -> String {
    let pairs = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"),
        (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut s = String::new();
    let mut x = n;
    for &(val, sym) in &pairs {
        while x >= val {
            s.push_str(sym);
            x -= val;
        }
    }
    if upper { s.to_uppercase() } else { s }
}

/// Wingdings/Symbol font characters trong private-use area thường được
/// dùng cho bullet. Convert sang Unicode tương đương để render được trong
/// terminal mà không cần Wingdings font.
fn remap_wingdings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let mapped = match c {
            '\u{F0B7}' => '•',   // Wingdings bullet
            '\u{F0A7}' => '▪',   // Wingdings small filled square
            '\u{F076}' => '✓',   // Wingdings check
            '\u{F0FC}' => '✗',   // Wingdings cross
            '\u{F0D8}' => '◆',   // Wingdings diamond
            '\u{F0A8}' => '▫',   // Wingdings empty square
            // 'o' lowercase thường dùng làm bullet level 2
            _ => c,
        };
        out.push(mapped);
    }
    out
}

/// Parse numbering.xml -> map (numId, ilvl) -> LvlTemplate.
/// Trả về HashMap rỗng nếu không có numbering.xml hoặc parse fail.
pub fn parse_numbering(xml: &[u8]) -> AppResult<HashMap<(u32, u32), LvlTemplate>> {
    let mut out: HashMap<(u32, u32), LvlTemplate> = HashMap::new();
    // 2 maps trung gian:
    //   abstract_levels[abstractNumId][ilvl] = LvlTemplate
    //   num_to_abstract[numId] = abstractNumId
    let mut abstract_levels: HashMap<u32, HashMap<u32, LvlTemplate>> = HashMap::new();
    let mut num_to_abstract: HashMap<u32, u32> = HashMap::new();

    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut buf = Vec::new();

    // State trong khi scan:
    let mut in_abstract_num: Option<u32> = None;
    let mut in_num: Option<u32> = None;
    let mut current_lvl: Option<u32> = None;
    let mut current_num_fmt: Option<String> = None;
    let mut current_lvl_text: Option<String> = None;

    loop {
        buf.clear();
        let evt = reader.read_event_into(&mut buf)?;
        match evt {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                let local = local_name(name.as_ref());
                match local {
                    b"abstractNum" => {
                        if let Some(v) = attr_val(&e, b"w:abstractNumId") {
                            if let Ok(id) = v.parse::<u32>() {
                                in_abstract_num = Some(id);
                                abstract_levels.entry(id).or_default();
                            }
                        }
                    }
                    b"num" => {
                        if let Some(v) = attr_val(&e, b"w:numId") {
                            if let Ok(id) = v.parse::<u32>() {
                                in_num = Some(id);
                            }
                        }
                    }
                    b"abstractNumId" if in_num.is_some() => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            if let Ok(aid) = v.parse::<u32>() {
                                if let Some(nid) = in_num {
                                    num_to_abstract.insert(nid, aid);
                                }
                            }
                        }
                    }
                    b"lvl" if in_abstract_num.is_some() => {
                        if let Some(v) = attr_val(&e, b"w:ilvl") {
                            current_lvl = v.parse().ok();
                            current_num_fmt = None;
                            current_lvl_text = None;
                        }
                    }
                    b"numFmt" if current_lvl.is_some() => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            current_num_fmt = Some(v);
                        }
                    }
                    b"lvlText" if current_lvl.is_some() => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            current_lvl_text = Some(v);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                match local.as_slice() {
                    b"lvl" => {
                        if let (Some(aid), Some(ilvl)) = (in_abstract_num, current_lvl) {
                            let tpl = LvlTemplate {
                                num_fmt: current_num_fmt.take().unwrap_or_else(|| "decimal".to_string()),
                                lvl_text: current_lvl_text.take().unwrap_or_else(|| "%1.".to_string()),
                            };
                            abstract_levels.entry(aid).or_default().insert(ilvl, tpl);
                        }
                        current_lvl = None;
                    }
                    b"abstractNum" => in_abstract_num = None,
                    b"num" => in_num = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Merge num_to_abstract + abstract_levels -> final map
    for (num_id, abstract_id) in &num_to_abstract {
        if let Some(levels) = abstract_levels.get(abstract_id) {
            for (ilvl, tpl) in levels {
                out.insert((*num_id, *ilvl), tpl.clone());
            }
        }
    }

    Ok(out)
}

fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

fn attr_val(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == key {
            return Some(String::from_utf8_lossy(&attr.value).into_owned());
        }
    }
    None
}
