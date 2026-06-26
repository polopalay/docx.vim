//! Render Document thành output cho Vim plugin. Hybrid approach:
//! - Mỗi paragraph = 1 dòng buffer.
//! - Paragraph trong table cell: prefix '├ ' (cell tiếp), hoặc ngắt row
//!   bằng dòng riêng '├─────┤' giữa các row.
//! - Paragraph trong sdt: prefix '┊ '.
//! - Heading: prefix '# '/'## '/'### ' theo level.
//! - Bullet: prefix '• '. Numbered: '1. '.
//! - Image run (is_drawing=true): hiển thị "[IMAGE]" trong nội dung.
//!
//! Metadata blocks @@STYLE@@ và @@PARAMAP@@ y hệt design cũ. Số dòng buffer
//! = số paragraph + số row separator (1 dòng cho mỗi <w:tr> kết thúc).

use crate::model::*;

pub struct Rendered {
    pub text: String,
    /// (buffer_line, char_start, char_end, bold, italic, size_pt, color_hex, font_name)
    pub style_meta: Vec<(
        usize, usize, usize, bool, bool,
        Option<f32>, Option<String>, Option<String>,
    )>,
    /// (buffer_line, para_id)
    pub para_map: Vec<(usize, String)>,
    /// (buffer_line, ilvl) — chỉ cho paragraph là list item (có numPr hoặc
    /// pStyle bắt đầu bằng "List"). Vim dùng để biết: Tab trên dòng này
    /// nên setilvl thay vì indent; `o` nên listadd.
    pub list_info: Vec<(usize, u32)>,
}

const ROW_SEPARATOR: &str = "├─────────────────────────────┤";
const TABLE_START: &str = "┌─── TABLE ───────────────────┐";
const TABLE_END: &str = "└─────────────────────────────┘";

pub fn render(doc: &Document) -> Rendered {
    let mut out = String::new();
    let mut style_meta = Vec::new();
    let mut para_map = Vec::new();
    let mut list_info: Vec<(usize, u32)> = Vec::new();
    let mut buffer_line: usize = 1;
    // Track xem mình đang ở trong table không, để emit TABLE_START/TABLE_END.
    let mut in_table_depth: u32 = 0;

    for (i, p) in doc.paragraphs.iter().enumerate() {
        // Phát hiện vào/ra table: so sánh context của p với p trước/sau.
        let current_in_table = matches!(p.context, ParaContext::TableCell { .. });
        let prev_in_table = i > 0
            && matches!(doc.paragraphs[i - 1].context, ParaContext::TableCell { .. });

        // Emit TABLE_START khi vừa vào table.
        if current_in_table && !prev_in_table {
            out.push_str(TABLE_START);
            out.push('\n');
            buffer_line += 1;
            in_table_depth = 1;
        }

        // Emit ROW_SEPARATOR trước paragraph nếu là first_in_row VÀ không
        // phải dòng đầu tiên của table (tránh ROW_SEPARATOR ngay sau
        // TABLE_START - thừa).
        if let ParaContext::TableCell {
            is_first_in_row, ..
        } = &p.context
        {
            if *is_first_in_row && prev_in_table {
                out.push_str(ROW_SEPARATOR);
                out.push('\n');
                buffer_line += 1;
            }
        }

        // Track list_info nếu paragraph là list item.
        let is_list = p.num_id.is_some()
            || p.para_style
                .as_deref()
                .map(|s| s.starts_with("List"))
                .unwrap_or(false);
        if is_list {
            list_info.push((buffer_line, p.num_ilvl.unwrap_or(0)));
        }

        render_paragraph(p, &mut out, &mut buffer_line, &mut style_meta, &mut para_map);

        // Emit TABLE_END khi paragraph tiếp theo KHÔNG còn trong table.
        let next_in_table = i + 1 < doc.paragraphs.len()
            && matches!(
                doc.paragraphs[i + 1].context,
                ParaContext::TableCell { .. }
            );
        if current_in_table && !next_in_table {
            out.push_str(TABLE_END);
            out.push('\n');
            buffer_line += 1;
            in_table_depth = 0;
        }
    }
    let _ = in_table_depth;

    if out.ends_with('\n') {
        out.pop();
    }

    Rendered {
        text: out,
        style_meta,
        para_map,
        list_info,
    }
}

fn render_paragraph(
    p: &Paragraph,
    out: &mut String,
    buffer_line: &mut usize,
    style_meta: &mut Vec<(
        usize, usize, usize, bool, bool,
        Option<f32>, Option<String>, Option<String>,
    )>,
    para_map: &mut Vec<(usize, String)>,
) {
    // Tính prefix theo context + para_style + indent.
    let context_prefix = match &p.context {
        ParaContext::TableCell { .. } => "├ ",
        ParaContext::Sdt => "┊ ",
        ParaContext::Body => "",
    };
    // Indent: 720 twips = 1 cấp. Mỗi cấp render thành "  " (2 spaces).
    // Trong save sẽ tính ngược: số "  " prefix -> indent_twips.
    let indent_level = p
        .indent_twips
        .map(|t| (t.max(0) / 720) as usize)
        .unwrap_or(0);
    let indent_prefix = "  ".repeat(indent_level);
    let style_prefix = match p.para_style.as_deref() {
        Some(s) if s == "Heading1" || s == "Title" => "# ",
        Some("Heading2") => "## ",
        Some("Heading3") => "### ",
        Some("Heading4") => "#### ",
        Some("Heading5") => "##### ",
        Some("Heading6") => "###### ",
        Some(s) if s.starts_with("ListBullet") => "• ",
        Some(s) if s.starts_with("ListNumber") => "1. ",
        _ => "",
    };
    out.push_str(context_prefix);
    out.push_str(&indent_prefix);
    out.push_str(style_prefix);

    let prefix_chars = context_prefix.chars().count()
        + indent_prefix.chars().count()
        + style_prefix.chars().count();
    let mut char_cursor: usize = prefix_chars;

    for run in &p.runs {
        if run.is_drawing {
            let placeholder = "[IMAGE]";
            let cstart = char_cursor + 1;
            let cend = char_cursor + placeholder.chars().count();
            out.push_str(placeholder);
            char_cursor += placeholder.chars().count();
            // Emit style nếu có (vd image với color/bold trong rPr — hiếm)
            if !run.style.is_default() {
                let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
                style_meta.push((
                    *buffer_line,
                    cstart,
                    cend,
                    run.style.bold,
                    run.style.italic,
                    size_pt,
                    run.style.color_hex.clone(),
                    run.style.font_name.clone(),
                ));
            }
            continue;
        }
        let run_text = &run.text;
        let run_len = run_text.chars().count();
        if run_len == 0 {
            continue;
        }
        if !run.style.is_default() {
            let cstart = char_cursor + 1;
            let cend = char_cursor + run_len;
            let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
            style_meta.push((
                *buffer_line,
                cstart,
                cend,
                run.style.bold,
                run.style.italic,
                size_pt,
                run.style.color_hex.clone(),
                run.style.font_name.clone(),
            ));
        }
        out.push_str(run_text);
        char_cursor += run_len;
    }

    para_map.push((*buffer_line, p.id.clone()));
    out.push('\n');
    *buffer_line += 1;
}
