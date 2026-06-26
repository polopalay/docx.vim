//! Generate XML cho 1 paragraph đã sửa. Output cần valid theo schema
//! WordprocessingML.

use crate::model::*;
use std::fmt::Write;

pub fn emit_paragraph(p: &Paragraph) -> String {
    let mut s = String::with_capacity(256);
    s.push_str("<w:p>");

    let has_ppr = p.para_style.is_some()
        || p.alignment.is_some()
        || p.indent_twips.is_some()
        || p.num_id.is_some()
        || p.num_ilvl.is_some();
    if has_ppr {
        s.push_str("<w:pPr>");
        if let Some(style) = &p.para_style {
            write!(s, "<w:pStyle w:val=\"{}\"/>", xml_escape_attr(style)).ok();
        }
        if let Some(num_id) = p.num_id {
            // Inline numbering: <w:numPr> với cả ilvl và numId.
            let ilvl = p.num_ilvl.unwrap_or(0);
            write!(
                s,
                "<w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>"
            )
            .ok();
        } else if let Some(ilvl) = p.num_ilvl {
            // Style-based numbering (vd pStyle=ListNumber) với ilvl
            // override — chỉ emit <w:ilvl>, không emit numId (kế thừa từ
            // style trong styles.xml).
            write!(s, "<w:numPr><w:ilvl w:val=\"{ilvl}\"/></w:numPr>").ok();
        }
        if let Some(ind) = p.indent_twips {
            // <w:ind w:left="720"/>. Chỉ emit nếu khác 0.
            if ind != 0 {
                write!(s, "<w:ind w:left=\"{ind}\"/>").ok();
            }
        }
        if let Some(jc) = &p.alignment {
            write!(s, "<w:jc w:val=\"{}\"/>", xml_escape_attr(jc)).ok();
        }
        s.push_str("</w:pPr>");
    }

    for run in &p.runs {
        emit_run(run, &mut s);
    }

    s.push_str("</w:p>");
    s
}

fn emit_run(run: &Run, s: &mut String) {
    // Drawing runs không nên được dirty/re-emit ở MVP này. Nếu vẫn được
    // gọi, emit run rỗng để không crash file.
    if run.is_drawing {
        s.push_str("<w:r><w:t/></w:r>");
        return;
    }

    s.push_str("<w:r>");
    if !run.style.is_default() {
        s.push_str("<w:rPr>");
        // Thứ tự element trong <w:rPr> không bắt buộc nhưng Word ưa thứ tự
        // chuẩn: rFonts -> b -> i -> sz/szCs -> color -> highlight.
        if let Some(font) = &run.style.font_name {
            // Emit cả ascii, hAnsi, cs để cover mọi script. Word render
            // cùng 1 font cho tất cả vùng (latin/CJK/complex).
            write!(
                s,
                "<w:rFonts w:ascii=\"{n}\" w:hAnsi=\"{n}\" w:cs=\"{n}\"/>",
                n = xml_escape_attr(font)
            )
            .ok();
        }
        if run.style.bold {
            s.push_str("<w:b/>");
        }
        if run.style.italic {
            s.push_str("<w:i/>");
        }
        if let Some(sz) = run.style.size_half_pt {
            write!(s, "<w:sz w:val=\"{sz}\"/>").ok();
            write!(s, "<w:szCs w:val=\"{sz}\"/>").ok();
        }
        if let Some(c) = &run.style.color_hex {
            write!(s, "<w:color w:val=\"{c}\"/>").ok();
        }
        if let Some(h) = &run.style.highlight {
            write!(s, "<w:highlight w:val=\"{}\"/>", xml_escape_attr(h)).ok();
        }
        s.push_str("</w:rPr>");
    }

    let parts: Vec<&str> = run.text.split('\u{2028}').collect();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            s.push_str("<w:br/>");
        }
        if part.is_empty() {
            continue;
        }
        let needs_preserve = part.starts_with(|c: char| c.is_whitespace())
            || part.ends_with(|c: char| c.is_whitespace());
        if needs_preserve {
            s.push_str("<w:t xml:space=\"preserve\">");
        } else {
            s.push_str("<w:t>");
        }
        s.push_str(&xml_escape_text(part));
        s.push_str("</w:t>");
    }

    s.push_str("</w:r>");
}

fn xml_escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn xml_escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}
