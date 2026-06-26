//! Save logic: apply buffer text mới vào Document, rebuild XML minimal-diff.
//!
//! Cách apply:
//! 1. Render lại Document từ XML gốc -> biết mỗi paragraph hiển thị ở dòng
//!    nào (paramap).
//! 2. So sánh từng dòng buffer mới với render gốc:
//!    - Dòng KHÔNG có trong paramap (TABLE_START / ROW_SEP / TABLE_END):
//!      bắt buộc giữ nguyên (không cho user sửa).
//!    - Dòng CÓ trong paramap: nếu khác text -> update text của paragraph
//!      đó, set dirty=true.
//! 3. Số dòng phải khớp (line count guard).
//!
//! Rebuild XML: copy nguyên byte cho paragraph không dirty + cho mọi
//! khoảng khác (table outer XML, sectPr, headers...), chỉ re-emit XML
//! cho paragraph dirty.

use crate::error::{AppError, AppResult};
use crate::model::*;
use crate::render;
use crate::xml_emit;

pub fn apply_buffer_to_document(doc: &Document, new_text: &str) -> AppResult<Vec<u8>> {
    let mut doc = clone_doc(doc);

    let rendered = render::render(&doc);
    // Trim trailing newlines on both sides — tránh off-by-one khi text từ
    // file có \n cuối nhưng render output thì không, hoặc ngược lại.
    let old_text = rendered.text.trim_end_matches('\n');
    let new_text_trim = new_text.trim_end_matches('\n');
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let new_lines: Vec<&str> = new_text_trim.split('\n').collect();

    if old_lines.len() != new_lines.len() {
        return Err(AppError(format!(
            "Line count changed: was {}, now {}. Structure edits not supported.",
            old_lines.len(),
            new_lines.len()
        )));
    }

    let mut line_to_pid: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    for (line, pid) in &rendered.para_map {
        line_to_pid.insert(*line, pid.clone());
    }

    for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
        if old == new {
            continue;
        }
        let line_no = i + 1;
        let pid = match line_to_pid.get(&line_no) {
            Some(p) => p.clone(),
            None => {
                return Err(AppError(format!(
                    "Line {line_no} is a structural marker (table separator) and cannot be edited"
                )));
            }
        };
        let p_idx = doc
            .paragraphs
            .iter()
            .position(|p| p.id == pid)
            .ok_or_else(|| AppError(format!("Paragraph {pid} not found")))?;
        apply_text_to_paragraph(&mut doc.paragraphs[p_idx], new);
    }

    rebuild_document_xml(&doc)
}

fn apply_text_to_paragraph(p: &mut Paragraph, new_line: &str) {
    let prefix_len = render_prefix_len(p);
    let new_content: String = new_line.chars().skip(prefix_len).collect();

    if p.runs.is_empty() {
        if !new_content.is_empty() {
            p.runs.push(Run {
                text: new_content,
                style: RunStyle::default(),
                is_drawing: false,
            });
            p.dirty = true;
        }
        return;
    }

    // Nếu paragraph có drawing runs: KHÔNG để user sửa text vì sẽ làm mất
    // ảnh. Skip silently (user thấy text không đổi sau save).
    if p.runs.iter().any(|r| r.is_drawing) {
        return;
    }

    // 1 run: assign thẳng -> giữ style 100%.
    if p.runs.len() == 1 {
        if p.runs[0].text != new_content {
            p.runs[0].text = new_content;
            p.dirty = true;
        }
        return;
    }

    // Nhiều run: gán vào run đầu, xoá text các run khác (MVP).
    let original_concat: String = p.runs.iter().map(|r| r.text.as_str()).collect();
    if original_concat == new_content {
        return;
    }
    p.runs[0].text = new_content;
    for r in p.runs.iter_mut().skip(1) {
        r.text.clear();
    }
    p.dirty = true;
}

/// Phải khớp với prefix length trong render.rs
fn render_prefix_len(p: &Paragraph) -> usize {
    let context_len = match &p.context {
        ParaContext::TableCell { .. } => 2, // "├ "
        ParaContext::Sdt => 2,              // "┊ "
        ParaContext::Body => 0,
    };
    let indent_level = p
        .indent_twips
        .map(|t| (t.max(0) / 720) as usize)
        .unwrap_or(0);
    let indent_len = indent_level * 2; // "  " mỗi cấp
    let style_len = match p.para_style.as_deref() {
        Some(s) if s == "Heading1" || s == "Title" => 2,
        Some("Heading2") => 3,
        Some("Heading3") => 4,
        Some("Heading4") => 5,
        Some("Heading5") => 6,
        Some("Heading6") => 7,
        Some(s) if s.starts_with("ListBullet") => 2,
        Some(s) if s.starts_with("ListNumber") => 3,
        _ => 0,
    };
    context_len + indent_len + style_len
}

/// Rebuild document.xml — paragraph dirty thì emit XML mới, không dirty
/// thì copy byte gốc. Mọi khoảng XML khác (table outer, sdt, bookmark...)
/// giữ nguyên qua phép trừ giữa các byte_range.
pub fn rebuild_document_xml(doc: &Document) -> AppResult<Vec<u8>> {
    let mut out = Vec::with_capacity(doc.original_xml.len() + 512);

    // Phần "vỏ" trước <w:body> nội dung
    out.extend_from_slice(&doc.original_xml[..doc.body_inner_range.0]);

    // Trong body: tất cả paragraphs đã sắp theo byte_range tăng dần (vì
    // parser scan linearly). Ghép theo cách: copy mọi byte giữa các
    // paragraph, paragraph dirty thì emit mới, paragraph không dirty thì
    // copy byte gốc.
    let mut cursor = doc.body_inner_range.0;
    let mut sorted_paras: Vec<&Paragraph> = doc.paragraphs.iter().collect();
    sorted_paras.sort_by_key(|p| p.byte_range.0);

    for p in sorted_paras {
        if p.byte_range.0 > cursor {
            out.extend_from_slice(&doc.original_xml[cursor..p.byte_range.0]);
        }
        if p.dirty {
            out.extend_from_slice(xml_emit::emit_paragraph(p).as_bytes());
        } else {
            out.extend_from_slice(&doc.original_xml[p.byte_range.0..p.byte_range.1]);
        }
        cursor = p.byte_range.1;
    }
    if cursor < doc.body_inner_range.1 {
        out.extend_from_slice(&doc.original_xml[cursor..doc.body_inner_range.1]);
    }

    // Phần "vỏ" sau </w:body>
    out.extend_from_slice(&doc.original_xml[doc.body_inner_range.1..]);

    Ok(out)
}

fn clone_doc(doc: &Document) -> Document {
    Document {
        paragraphs: doc.paragraphs.clone(),
        body_inner_range: doc.body_inner_range,
        original_xml: doc.original_xml.clone(),
    }
}
