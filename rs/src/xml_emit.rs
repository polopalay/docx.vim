//! Re-emit 1 paragraph đã sửa bằng cách VÁ trên XML gốc: chỉ thay đúng
//! <w:t>/<w:tab>/<w:br> bị đổi text và đúng thuộc tính pPr/rPr bị đổi. Mọi
//! thứ khác (hyperlink, field, bookmark, underline, sectPr...) giữ nguyên byte.

use crate::model::*;
use crate::parser;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

const PPR_ORDER: &[&str] = &[
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr",
    "suppressLineNumbers", "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap",
    "overflowPunct", "topLinePunct", "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd",
    "snapToGrid", "spacing", "ind", "contextualSpacing", "mirrorIndents", "suppressOverlap", "jc",
    "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl", "divId", "cnfStyle",
    "rPr", "sectPr", "pPrChange",
];
const RPR_ORDER: &[&str] = &[
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike",
    "outline", "shadow", "emboss", "imprint", "noProof", "snapToGrid", "vanish", "webHidden",
    "color", "spacing", "w", "kern", "position", "sz", "szCs", "highlight", "u", "effect", "bdr",
    "shd", "fitText", "vertAlign", "rtl", "cs", "em", "lang", "eastAsianLayout", "specVanish",
    "oMath",
];

/// 1 element con trực tiếp: (tên local, byte start, byte end) trong chuỗi cha.
pub fn children(xml: &str) -> Vec<(String, usize, usize)> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut open: (String, usize) = (String::new(), 0);
    loop {
        let before = reader.buffer_position();
        let local = |n: &[u8]| {
            let n = String::from_utf8_lossy(n).into_owned();
            n.rsplit(':').next().unwrap_or("").to_string()
        };
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if depth == 1 {
                    open = (local(e.name().as_ref()), before);
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) => {
                if depth == 1 {
                    out.push((local(e.name().as_ref()), before, reader.buffer_position()));
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    out.push((open.0.clone(), open.1, reader.buffer_position()));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// `<x .../>` -> `<x ...></x>` để chèn con vào được.
fn expand(elem: &str) -> String {
    if !elem.ends_with("/>") || !children_is_empty_root(elem) {
        return elem.to_string();
    }
    let name: String = elem[1..].chars().take_while(|c| !c.is_whitespace() && *c != '/').collect();
    format!("{}></{name}>", &elem[..elem.len() - 2])
}

fn children_is_empty_root(elem: &str) -> bool {
    matches!(Reader::from_str(elem).read_event(), Ok(Event::Empty(_)))
}

/// Thay / xoá / chèn (đúng thứ tự schema) element con `name` của `parent`.
fn set_child(parent: &str, order: &[&str], name: &str, new: Option<&str>) -> String {
    let mut s = expand(parent);
    let kids = children(&s);
    if let Some((_, a, b)) = kids.iter().find(|k| k.0 == name) {
        s.replace_range(*a..*b, new.unwrap_or(""));
        return s;
    }
    let Some(new) = new else { return s };
    let rank = |n: &str| order.iter().position(|o| *o == n).unwrap_or(order.len());
    let at = kids
        .iter()
        .find(|k| rank(&k.0) > rank(name))
        .map(|k| k.1)
        .unwrap_or_else(|| s.rfind("</").unwrap_or(s.len()));
    s.insert_str(at, new);
    s
}

/// Đổi riêng attribute w:left của <w:ind>, giữ hanging/right...
fn set_indent(ppr: &str, twips: Option<i32>) -> String {
    let old = children(&expand(ppr))
        .iter()
        .find(|k| k.0 == "ind")
        .map(|k| expand(ppr)[k.1..k.2].to_string());
    let mut ind = old.unwrap_or_else(|| "<w:ind/>".to_string());
    for key in [" w:left=\"", " w:start=\""] {
        if let Some(a) = ind.find(key) {
            if let Some(len) = ind[a + key.len()..].find('"') {
                ind.replace_range(a..a + key.len() + len + 1, "");
            }
        }
    }
    if let Some(t) = twips {
        ind.insert_str("<w:ind".len(), &format!(" w:left=\"{t}\""));
    }
    let keep = ind.contains('=');
    set_child(ppr, PPR_ORDER, "ind", keep.then_some(ind.as_str()))
}

fn patch_rpr(rpr: &str, old: &RunStyle, new: &RunStyle) -> String {
    let mut s = rpr.to_string();
    let mut set = |name: &str, xml: Option<String>| s = set_child(&s, RPR_ORDER, name, xml.as_deref());
    if old.font_name != new.font_name {
        set("rFonts", new.font_name.as_ref().map(|f| {
            let f = xml_escape_attr(f);
            format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\"/>")
        }));
    }
    if old.bold != new.bold {
        set("b", new.bold.then(|| "<w:b/>".to_string()));
    }
    if old.italic != new.italic {
        set("i", new.italic.then(|| "<w:i/>".to_string()));
    }
    if old.size_half_pt != new.size_half_pt {
        set("sz", new.size_half_pt.map(|v| format!("<w:sz w:val=\"{v}\"/>")));
        set("szCs", new.size_half_pt.map(|v| format!("<w:szCs w:val=\"{v}\"/>")));
    }
    if old.color_hex != new.color_hex {
        set("color", new.color_hex.as_ref().map(|v| format!("<w:color w:val=\"{}\"/>", xml_escape_attr(v))));
    }
    if old.highlight != new.highlight {
        set("highlight", new.highlight.as_ref().map(|v| format!("<w:highlight w:val=\"{}\"/>", xml_escape_attr(v))));
    }
    s
}

/// Text buffer -> chuỗi <w:t>/<w:tab/>/<w:br/>. \f = ngắt trang, \v = ngắt cột.
pub fn serialize_text(text: &str) -> String {
    let mut out = String::new();
    let mut chunk = String::new();
    let flush = |chunk: &mut String, out: &mut String| {
        if chunk.is_empty() {
            return;
        }
        let preserve = if chunk.starts_with(char::is_whitespace) || chunk.ends_with(char::is_whitespace) {
            " xml:space=\"preserve\""
        } else {
            ""
        };
        out.push_str(&format!("<w:t{preserve}>{}</w:t>", xml_escape_text(chunk)));
        chunk.clear();
    };
    for c in text.chars() {
        let tag = match c {
            '\t' => "<w:tab/>",
            '\u{2028}' => "<w:br/>",
            '\u{000C}' => "<w:br w:type=\"page\"/>",
            '\u{000B}' => "<w:br w:type=\"column\"/>",
            _ => {
                chunk.push(c);
                continue;
            }
        };
        flush(&mut chunk, &mut out);
        out.push_str(tag);
    }
    flush(&mut chunk, &mut out);
    out
}

/// Thay đoạn khác nhau (sau khi bỏ prefix/suffix chung) giữa các atom của 1 run.
fn patch_run_text(run: &str, new_text: &str) -> String {
    // (start, end, text) của từng atom trong run
    let mut atoms: Vec<(usize, usize, Vec<char>)> = Vec::new();
    for (name, a, b) in children(run) {
        let xml = &run[a..b];
        let text = match name.as_str() {
            "t" => {
                let mut r = Reader::from_str(xml);
                let mut t = String::new();
                loop {
                    match r.read_event() {
                        Ok(Event::Text(x)) => t.push_str(&x.unescape().unwrap_or_default()),
                        Ok(Event::Eof) | Err(_) => break,
                        _ => {}
                    }
                }
                t
            }
            "tab" => "\t".to_string(),
            "br" => parser::break_char(xml).to_string(),
            _ => continue,
        };
        atoms.push((a, b, text.chars().collect()));
    }
    let old: Vec<char> = atoms.iter().flat_map(|x| x.2.iter().copied()).collect();
    let new: Vec<char> = new_text.chars().collect();
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suf = old.len().min(new.len()) - pre;
    let suf = old.iter().rev().zip(new.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
    let (cut_a, cut_b) = (pre, old.len() - suf);
    let mid: String = new[pre..new.len() - suf].iter().collect();

    let mut out = run.to_string();
    if atoms.is_empty() {
        let mut s = expand(run);
        let at = s.rfind("</").unwrap_or(s.len());
        s.insert_str(at, &serialize_text(new_text));
        return s;
    }
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut pos = 0usize;
    let mut inserted = false;
    for (a, b, text) in &atoms {
        let (s, e) = (pos, pos + text.len());
        pos = e;
        let reaches = if cut_b > cut_a { cut_a < e } else { cut_a <= e };
        let takes_mid = !inserted && reaches;
        let overlaps = s < cut_b && e > cut_a;
        if !overlaps && !(takes_mid && !mid.is_empty()) {
            if takes_mid {
                inserted = true;
            }
            continue;
        }
        let mut t: String = text.iter().take(cut_a.saturating_sub(s)).collect();
        if takes_mid {
            t.push_str(&mid);
            inserted = true;
        }
        t.extend(text.iter().skip(cut_b.saturating_sub(s)));
        edits.push((*a, *b, serialize_text(&t)));
    }
    for (a, b, x) in edits.into_iter().rev() {
        out.replace_range(a..b, &x);
    }
    out
}

pub fn emit_paragraph(p: &Paragraph, original_xml: &[u8]) -> String {
    const WRAP: &str = "<w:body>";
    let orig = String::from_utf8_lossy(&original_xml[p.byte_range.0..p.byte_range.1]).into_owned();
    let wrapped = format!("{WRAP}{orig}</w:body>");
    let Some(old) = parser::parse_document(wrapped.as_bytes())
        .ok()
        .and_then(|d| d.paragraphs.into_iter().next())
    else {
        return orig;
    };

    // Vá các run (từ cuối lên đầu để offset không lệch), rồi append run mới.
    let mut out = orig.clone();
    let mut appended = String::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for run in &p.runs {
        let Some((a, b)) = run.byte_range else {
            appended.push_str(&new_run(run));
            continue;
        };
        let (a, b) = (a - p.byte_range.0, b - p.byte_range.0);
        let Some(o) = old.runs.iter().find(|o| o.byte_range.map(|r| r.0) == Some(a + WRAP.len())) else {
            continue;
        };
        if let Some(drawing) = &run.drawing_xml {
            edits.push((a, b, drawing.clone()));
            continue;
        }
        let mut xml = orig[a..b].to_string();
        if run.text != o.text && !run.is_drawing {
            xml = patch_run_text(&xml, &run.text);
        }
        if run.style != o.style {
            xml = expand(&xml);
            let old_rpr = children(&xml).into_iter().find(|k| k.0 == "rPr");
            let rpr = old_rpr.as_ref().map(|k| xml[k.1..k.2].to_string()).unwrap_or_else(|| "<w:rPr></w:rPr>".into());
            let new_rpr = patch_rpr(&rpr, &o.style, &run.style);
            match old_rpr {
                Some((_, x, y)) => xml.replace_range(x..y, &new_rpr),
                None => {
                    let at = xml.find('>').map(|i| i + 1).unwrap_or(0);
                    xml.insert_str(at, &new_rpr);
                }
            }
        }
        if xml != orig[a..b] {
            edits.push((a, b, xml));
        }
    }
    edits.sort_by_key(|e| e.0);
    for (a, b, x) in edits.into_iter().rev() {
        out.replace_range(a..b, &x);
    }

    // pPr
    let ppr_changed = p.para_style != old.para_style
        || p.alignment != old.alignment
        || p.indent_twips != old.indent_twips
        || p.num_id != old.num_id
        || p.num_ilvl != old.num_ilvl;
    if !appended.is_empty() || ppr_changed {
        out = expand(&out);
    }
    if !appended.is_empty() {
        let at = out.rfind("</").unwrap_or(out.len());
        out.insert_str(at, &appended);
    }
    if ppr_changed {
        let found = children(&out).into_iter().find(|k| k.0 == "pPr");
        let mut ppr = found.as_ref().map(|k| out[k.1..k.2].to_string()).unwrap_or_else(|| "<w:pPr></w:pPr>".into());
        if p.para_style != old.para_style {
            let x = p.para_style.as_ref().map(|v| format!("<w:pStyle w:val=\"{}\"/>", xml_escape_attr(v)));
            ppr = set_child(&ppr, PPR_ORDER, "pStyle", x.as_deref());
        }
        if p.num_id != old.num_id || p.num_ilvl != old.num_ilvl {
            let id = p.num_id.map(|v| format!("<w:numId w:val=\"{v}\"/>")).unwrap_or_default();
            let x = (p.num_id.is_some() || p.num_ilvl.is_some()).then(|| {
                format!("<w:numPr><w:ilvl w:val=\"{}\"/>{id}</w:numPr>", p.num_ilvl.unwrap_or(0))
            });
            ppr = set_child(&ppr, PPR_ORDER, "numPr", x.as_deref());
        }
        if p.indent_twips != old.indent_twips {
            ppr = set_indent(&ppr, p.indent_twips);
        }
        if p.alignment != old.alignment {
            let x = p.alignment.as_ref().map(|v| format!("<w:jc w:val=\"{}\"/>", xml_escape_attr(v)));
            ppr = set_child(&ppr, PPR_ORDER, "jc", x.as_deref());
        }
        match found {
            Some((_, a, b)) => out.replace_range(a..b, &ppr),
            None => {
                let at = out.find('>').map(|i| i + 1).unwrap_or(0);
                out.insert_str(at, &ppr);
            }
        }
    }
    out
}

/// <w:pPr> cho paragraph MỚI kế thừa từ `p`: list/indent/căn lề, và pStyle
/// nếu là list (hoặc heading khi `keep_heading`). Rỗng nếu không có gì.
pub fn inherited_ppr(p: &Paragraph, keep_heading: bool) -> String {
    let mut s = String::new();
    if let Some(v) = p.para_style.as_deref().filter(|v| {
        v.starts_with("List") || (keep_heading && (v.starts_with("Heading") || *v == "Title"))
    }) {
        s.push_str(&format!("<w:pStyle w:val=\"{}\"/>", xml_escape_attr(v)));
    }
    if p.num_id.is_some() || p.num_ilvl.is_some() {
        let id = p.num_id.map(|v| format!("<w:numId w:val=\"{v}\"/>")).unwrap_or_default();
        s.push_str(&format!("<w:numPr><w:ilvl w:val=\"{}\"/>{id}</w:numPr>", p.num_ilvl.unwrap_or(0)));
    }
    if let Some(v) = p.indent_twips.filter(|v| *v != 0) {
        s.push_str(&format!("<w:ind w:left=\"{v}\"/>"));
    }
    if let Some(v) = &p.alignment {
        s.push_str(&format!("<w:jc w:val=\"{}\"/>", xml_escape_attr(v)));
    }
    if s.is_empty() { s } else { format!("<w:pPr>{s}</w:pPr>") }
}

/// <w:rPr> của 1 style; rỗng nếu style mặc định.
pub fn rpr_xml(style: &RunStyle) -> String {
    if style.is_default() {
        return String::new();
    }
    patch_rpr("<w:rPr></w:rPr>", &RunStyle::default(), style)
}

/// rPr kế thừa từ run chữ đầu tiên (font/đậm/màu...) cho dòng mới.
pub fn inherited_rpr(p: &Paragraph) -> String {
    p.runs.iter().find(|r| !r.is_drawing).map(|r| rpr_xml(&r.style)).unwrap_or_default()
}

/// Run mới hoàn toàn (ảnh vừa chèn, hoặc run đầu tiên của paragraph rỗng).
fn new_run(run: &Run) -> String {
    if let Some(xml) = &run.drawing_xml {
        return xml.clone();
    }
    format!("<w:r>{}{}</w:r>", rpr_xml(&run.style), serialize_text(&run.text))
}
pub fn xml_escape_text(s: &str) -> String {
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

pub fn xml_escape_attr(s: &str) -> String {
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
