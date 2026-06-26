//! Parser DOCX hybrid approach. Quét toàn bộ document.xml linearly, KHÔNG
//! phân biệt top-level vs nested. Track 1 stack context:
//!   - In body? track via `in_body`.
//!   - Trong table? `table_depth` tăng khi gặp <w:tbl>, giảm khi </w:tbl>.
//!   - Trong cell? `in_cell` bool, set khi vào <w:tc>, reset khi </w:tc>.
//!   - Trong row? `in_row` bool tương tự cho <w:tr>.
//!   - Trong sdt? `sdt_depth` tăng/giảm theo <w:sdtContent>.
//!
//! Khi gặp <w:p>, đăng ký Paragraph với context tính từ stack hiện tại.
//! Parser xử lý cả Start (`<w:p>...</w:p>`) lẫn Empty (`<w:p/>`).
//!
//! Mỗi paragraph nội bộ vẫn được parse runs/styles. Recursive khi cần
//! không bị borrow checker vì paragraph được parse tới End <w:p> trước khi
//! tiếp tục main loop — depth của outer parser tự cân bằng qua biến `depth`.

use crate::error::{AppError, AppResult};
use crate::model::*;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

pub fn parse_document(xml_bytes: &[u8]) -> AppResult<Document> {
    let mut reader = Reader::from_reader(xml_bytes);
    reader.trim_text(false);

    let mut doc = Document::default();
    doc.original_xml = xml_bytes.to_vec();

    let mut buf = Vec::new();
    let mut in_body = false;
    let mut table_depth: u32 = 0;
    // Stack track new row events — khi gặp <w:p> đầu tiên trong row mới,
    // set is_first_in_row = true rồi clear flag.
    let mut row_just_started = false;
    let mut sdt_depth: u32 = 0;
    let mut next_para_id: usize = 0;

    loop {
        let event_pos_before = reader.buffer_position();
        buf.clear();
        let evt = reader.read_event_into(&mut buf)?;

        // Trích info cần dùng từ event ra biến local trước khi drop event,
        // vì recursive parse_paragraph cần mượn lại `buf`.
        let action: ScanAction = match &evt {
            Event::Start(e) => classify_start(e),
            Event::Empty(e) => classify_empty(e),
            Event::End(e) => classify_end(e),
            Event::Eof => ScanAction::Eof,
            _ => ScanAction::Other,
        };
        drop(evt);

        match action {
            ScanAction::BodyStart => {
                in_body = true;
                doc.body_inner_range.0 = reader.buffer_position();
            }
            ScanAction::BodyEnd => {
                doc.body_inner_range.1 = event_pos_before;
                in_body = false;
            }
            ScanAction::TblStart => {
                table_depth += 1;
            }
            ScanAction::TblEnd => {
                if table_depth > 0 {
                    table_depth -= 1;
                }
            }
            ScanAction::TrStart => {
                row_just_started = true;
            }
            ScanAction::TrEnd => {
                // Đánh dấu paragraph cuối cùng trong row vừa kết thúc.
                mark_last_in_row(&mut doc);
            }
            ScanAction::SdtStart => {
                sdt_depth += 1;
            }
            ScanAction::SdtEnd => {
                if sdt_depth > 0 {
                    sdt_depth -= 1;
                }
            }
            ScanAction::PStart => {
                if !in_body {
                    continue;
                }
                // Compute context
                let ctx = current_context(table_depth, sdt_depth, row_just_started);
                let first_in_row = matches!(
                    &ctx,
                    ParaContext::TableCell {
                        is_first_in_row: true,
                        ..
                    }
                );
                if first_in_row {
                    row_just_started = false;
                }
                parse_paragraph_body(
                    &mut reader,
                    &mut buf,
                    event_pos_before,
                    &mut next_para_id,
                    &mut doc,
                    ctx,
                )?;
            }
            ScanAction::PEmpty => {
                if !in_body {
                    continue;
                }
                let ctx = current_context(table_depth, sdt_depth, row_just_started);
                let first_in_row = matches!(
                    &ctx,
                    ParaContext::TableCell {
                        is_first_in_row: true,
                        ..
                    }
                );
                if first_in_row {
                    row_just_started = false;
                }
                let byte_end = reader.buffer_position();
                let para = Paragraph {
                    id: format!("P{}", next_para_id),
                    runs: Vec::new(),
                    para_style: None,
                    alignment: None,
                    indent_twips: None,
                    num_id: None,
                    num_ilvl: None,
                    byte_range: (event_pos_before, byte_end),
                    dirty: false,
                    context: ctx,
                };
                next_para_id += 1;
                doc.paragraphs.push(para);
            }
            ScanAction::Eof => break,
            ScanAction::Other => {}
        }
    }

    Ok(doc)
}

fn current_context(table_depth: u32, sdt_depth: u32, row_just_started: bool) -> ParaContext {
    if table_depth > 0 {
        ParaContext::TableCell {
            table_depth,
            is_first_in_row: row_just_started,
            is_last_in_row: false,
        }
    } else if sdt_depth > 0 {
        ParaContext::Sdt
    } else {
        ParaContext::Body
    }
}

/// Đánh dấu paragraph cuối cùng (theo thứ tự gặp) đang ở cùng row level
/// hiện tại là is_last_in_row = true. Vì paragraph đã được push xong, mình
/// chỉnh ngược lại trên paragraph cuối nếu nó ở trong cell.
fn mark_last_in_row(doc: &mut Document) {
    if let Some(last) = doc.paragraphs.last_mut() {
        if let ParaContext::TableCell {
            is_last_in_row, ..
        } = &mut last.context
        {
            *is_last_in_row = true;
        }
    }
}

enum ScanAction {
    BodyStart,
    BodyEnd,
    TblStart,
    TblEnd,
    TrStart,
    TrEnd,
    SdtStart,
    SdtEnd,
    PStart,
    PEmpty,
    Eof,
    Other,
}

fn classify_start(e: &BytesStart<'_>) -> ScanAction {
    match local_name(e.name().as_ref()) {
        b"body" => ScanAction::BodyStart,
        b"tbl" => ScanAction::TblStart,
        b"tr" => ScanAction::TrStart,
        b"sdtContent" => ScanAction::SdtStart,
        b"p" => ScanAction::PStart,
        _ => ScanAction::Other,
    }
}

fn classify_empty(e: &BytesStart<'_>) -> ScanAction {
    match local_name(e.name().as_ref()) {
        b"p" => ScanAction::PEmpty,
        _ => ScanAction::Other,
    }
}

fn classify_end(e: &quick_xml::events::BytesEnd<'_>) -> ScanAction {
    match local_name(e.name().as_ref()) {
        b"body" => ScanAction::BodyEnd,
        b"tbl" => ScanAction::TblEnd,
        b"tr" => ScanAction::TrEnd,
        b"sdtContent" => ScanAction::SdtEnd,
        _ => ScanAction::Other,
    }
}

fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// Parse 1 paragraph từ <w:p> đến </w:p>. Khi return, reader đã đọc qua
/// thẻ đóng </w:p>.
fn parse_paragraph_body(
    reader: &mut Reader<&[u8]>,
    buf: &mut Vec<u8>,
    byte_start: usize,
    next_id: &mut usize,
    doc: &mut Document,
    context: ParaContext,
) -> AppResult<()> {
    let id = format!("P{}", *next_id);
    *next_id += 1;

    let mut runs: Vec<Run> = Vec::new();
    let mut para_style: Option<String> = None;
    let mut alignment: Option<String> = None;
    let mut indent_twips: Option<i32> = None;
    let mut num_id: Option<u32> = None;
    let mut num_ilvl: Option<u32> = None;
    let mut depth: i32 = 1; // đã consume <w:p> Start

    // State within paragraph:
    let mut in_ppr = false;
    let mut in_rpr = false;
    let mut in_numpr = false;
    let mut current_run: Option<Run> = None;
    let mut in_text = false;
    let mut text_buf = String::new();
    // True nếu run hiện tại chứa drawing/pict (image).
    let mut current_has_drawing = false;
    let mut drawing_depth: i32 = 0;

    loop {
        buf.clear();
        let evt = reader.read_event_into(buf)?;
        match evt {
            Event::Start(e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                depth += 1;
                match local.as_slice() {
                    b"pPr" => in_ppr = true,
                    b"pStyle" if in_ppr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            para_style = Some(v);
                        }
                    }
                    b"jc" if in_ppr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            alignment = Some(v);
                        }
                    }
                    b"ind" if in_ppr => {
                        // <w:ind w:left="720"/> -> indent.
                        if let Some(v) = attr_val(&e, b"w:left") {
                            indent_twips = v.parse().ok();
                        } else if let Some(v) = attr_val(&e, b"w:start") {
                            indent_twips = v.parse().ok();
                        }
                    }
                    b"numPr" if in_ppr => {
                        in_numpr = true;
                    }
                    b"numId" if in_numpr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            num_id = v.parse().ok();
                        }
                    }
                    b"ilvl" if in_numpr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            num_ilvl = v.parse().ok();
                        }
                    }
                    b"r" => {
                        current_run = Some(Run {
                            text: String::new(),
                            style: RunStyle::default(),
                            is_drawing: false,
                        });
                        current_has_drawing = false;
                    }
                    b"rPr" => in_rpr = true,
                    b"b" if in_rpr => set_bold(&e, &mut current_run, true),
                    b"i" if in_rpr => set_italic(&e, &mut current_run, true),
                    b"sz" if in_rpr => set_sz(&e, &mut current_run),
                    b"color" if in_rpr => set_color(&e, &mut current_run),
                    b"highlight" if in_rpr => set_highlight(&e, &mut current_run),
                    b"rFonts" if in_rpr => set_font(&e, &mut current_run),
                    b"t" => in_text = true,
                    b"drawing" | b"pict" | b"object" => {
                        current_has_drawing = true;
                        drawing_depth += 1;
                    }
                    _ => {
                        if drawing_depth > 0 {
                            drawing_depth += 1;
                        }
                    }
                }
            }
            Event::Empty(e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                match local.as_slice() {
                    b"pStyle" if in_ppr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            para_style = Some(v);
                        }
                    }
                    b"jc" if in_ppr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            alignment = Some(v);
                        }
                    }
                    b"ind" if in_ppr => {
                        if let Some(v) = attr_val(&e, b"w:left") {
                            indent_twips = v.parse().ok();
                        } else if let Some(v) = attr_val(&e, b"w:start") {
                            indent_twips = v.parse().ok();
                        }
                    }
                    b"numId" if in_numpr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            num_id = v.parse().ok();
                        }
                    }
                    b"ilvl" if in_numpr => {
                        if let Some(v) = attr_val(&e, b"w:val") {
                            num_ilvl = v.parse().ok();
                        }
                    }
                    b"b" if in_rpr => set_bold(&e, &mut current_run, true),
                    b"i" if in_rpr => set_italic(&e, &mut current_run, true),
                    b"sz" if in_rpr => set_sz(&e, &mut current_run),
                    b"color" if in_rpr => set_color(&e, &mut current_run),
                    b"highlight" if in_rpr => set_highlight(&e, &mut current_run),
                    b"rFonts" if in_rpr => set_font(&e, &mut current_run),
                    b"tab" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\t');
                        }
                    }
                    b"br" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\u{2028}');
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_text && drawing_depth == 0 {
                    text_buf.push_str(&t.unescape()?.into_owned());
                }
            }
            Event::End(e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                depth -= 1;
                match local.as_slice() {
                    b"pPr" => in_ppr = false,
                    b"rPr" => in_rpr = false,
                    b"numPr" => in_numpr = false,
                    b"t" => {
                        in_text = false;
                        if let Some(run) = current_run.as_mut() {
                            run.text.push_str(&text_buf);
                            text_buf.clear();
                        }
                    }
                    b"drawing" | b"pict" | b"object" => {
                        if drawing_depth > 0 {
                            drawing_depth -= 1;
                        }
                    }
                    b"r" => {
                        if let Some(mut run) = current_run.take() {
                            run.is_drawing = current_has_drawing;
                            current_has_drawing = false;
                            // Giữ run nếu có text hoặc có style hoặc có drawing
                            if !run.text.is_empty() || !run.style.is_default() || run.is_drawing {
                                runs.push(run);
                            }
                        }
                    }
                    b"p" => {
                        let byte_end = reader.buffer_position();
                        let para = Paragraph {
                            id,
                            runs,
                            para_style,
                            alignment,
                            indent_twips,
                            num_id,
                            num_ilvl,
                            byte_range: (byte_start, byte_end),
                            dirty: false,
                            context,
                        };
                        doc.paragraphs.push(para);
                        return Ok(());
                    }
                    _ => {
                        if drawing_depth > 0 {
                            drawing_depth -= 1;
                        }
                    }
                }
                if depth <= 0 {
                    return Err(AppError(
                        "Unbalanced XML: closing tag beyond paragraph".to_string(),
                    ));
                }
            }
            Event::Eof => {
                return Err(AppError("EOF inside <w:p>".to_string()));
            }
            _ => {}
        }
    }
}

fn set_bold(e: &BytesStart<'_>, run: &mut Option<Run>, _default: bool) {
    if let Some(r) = run.as_mut() {
        r.style.bold = attr_val(e, b"w:val").map(|v| !is_false(&v)).unwrap_or(true);
    }
}
fn set_italic(e: &BytesStart<'_>, run: &mut Option<Run>, _default: bool) {
    if let Some(r) = run.as_mut() {
        r.style.italic = attr_val(e, b"w:val").map(|v| !is_false(&v)).unwrap_or(true);
    }
}
fn set_sz(e: &BytesStart<'_>, run: &mut Option<Run>) {
    if let Some(r) = run.as_mut() {
        if let Some(v) = attr_val(e, b"w:val") {
            r.style.size_half_pt = v.parse().ok();
        }
    }
}
fn set_color(e: &BytesStart<'_>, run: &mut Option<Run>) {
    if let Some(r) = run.as_mut() {
        if let Some(v) = attr_val(e, b"w:val") {
            if v != "auto" {
                r.style.color_hex = Some(v.to_uppercase());
            }
        }
    }
}
fn set_highlight(e: &BytesStart<'_>, run: &mut Option<Run>) {
    if let Some(r) = run.as_mut() {
        if let Some(v) = attr_val(e, b"w:val") {
            if v != "none" {
                r.style.highlight = Some(v);
            }
        }
    }
}
fn set_font(e: &BytesStart<'_>, run: &mut Option<Run>) {
    if let Some(r) = run.as_mut() {
        // Ưu tiên ascii (font cho chữ latin), fallback hAnsi (high ANSI).
        let v = attr_val(e, b"w:ascii").or_else(|| attr_val(e, b"w:hAnsi"));
        if let Some(name) = v {
            r.style.font_name = Some(name);
        }
    }
}

fn attr_val(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == key {
            return Some(String::from_utf8_lossy(&attr.value).into_owned());
        }
    }
    None
}

fn is_false(v: &str) -> bool {
    matches!(v, "false" | "0" | "off")
}
