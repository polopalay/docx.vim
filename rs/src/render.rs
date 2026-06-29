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
use crate::numbering::LvlTemplate;
use std::collections::HashMap;

pub struct Rendered {
    pub text: String,
    /// (buffer_line, char_start, char_end, bold, italic, size_pt,
    ///  color_hex, font_name, highlight)
    pub style_meta: Vec<(
        usize, usize, usize, bool, bool,
        Option<f32>, Option<String>, Option<String>, Option<String>,
    )>,
    /// (buffer_line, para_id) — map dòng buffer chính với paragraph_id
    /// chính (cho non-table và col 0 của table).
    pub para_map: Vec<(usize, String)>,
    /// (buffer_line, ilvl) — chỉ cho paragraph là list item.
    pub list_info: Vec<(usize, u32)>,
    /// (buffer_line, col_idx, char_start, char_end, para_id) — cho mỗi
    /// dòng buffer table, track paragraph_id của TỪNG cell trong dòng đó.
    /// char_start/char_end là vùng text của cell (giữa các `│`).
    /// Save logic dùng để map text edit user gõ → đúng paragraph cell.
    pub cell_map: Vec<(usize, u32, usize, usize, String)>,
}

/// Render content của 1 cell (paragraph trong TableCell) ra string với
/// padding tới `cell_width` chars. Cũng push style_meta cho các runs.
/// Trả về string không bao gồm `│` borders (caller tự thêm).
fn render_cell_content(
    p: &Paragraph,
    ordinal: &str,
    cell_width: usize,
    buffer_line: usize,
    cell_start_char: usize,
    style_meta: &mut Vec<(
        usize, usize, usize, bool, bool,
        Option<f32>, Option<String>, Option<String>, Option<String>,
    )>,
) -> String {
    let mut s = String::new();
    // Indent
    let indent_level = p
        .indent_twips
        .map(|t| (t.max(0) / 720) as usize)
        .unwrap_or(0);
    for _ in 0..indent_level {
        s.push_str("  ");
    }
    // List prefix
    if !ordinal.is_empty() {
        s.push_str(ordinal);
        s.push(' ');
    } else if let Some(sty) = p.para_style.as_deref() {
        if sty.starts_with("ListBullet") {
            s.push_str("• ");
        }
    }
    let prefix_chars = s.chars().count();
    let mut char_cursor = prefix_chars;

    for run in &p.runs {
        if run.is_drawing {
            let placeholder = "[IMAGE]";
            let cstart = cell_start_char + char_cursor;
            let cend = cstart + placeholder.chars().count() - 1;
            if !run.style.is_default() {
                let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
                style_meta.push((
                    buffer_line, cstart, cend,
                    run.style.bold, run.style.italic, size_pt,
                    run.style.color_hex.clone(),
                    run.style.font_name.clone(),
                    run.style.highlight.clone(),
                ));
            }
            s.push_str(placeholder);
            char_cursor += placeholder.chars().count();
            continue;
        }
        let run_len = run.text.chars().count();
        if run_len == 0 {
            continue;
        }
        if !run.style.is_default() {
            let cstart = cell_start_char + char_cursor;
            let cend = cstart + run_len - 1;
            let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
            style_meta.push((
                buffer_line, cstart, cend,
                run.style.bold, run.style.italic, size_pt,
                run.style.color_hex.clone(),
                run.style.font_name.clone(),
                run.style.highlight.clone(),
            ));
        }
        s.push_str(&run.text);
        char_cursor += run_len;
    }
    // Pad to cell_width
    let actual_chars = char_cursor;
    let pad = cell_width.saturating_sub(actual_chars);
    for _ in 0..pad {
        s.push(' ');
    }
    s
}


fn paragraph_text_length(p: &Paragraph) -> usize {
    let mut total = 0usize;
    for run in &p.runs {
        if run.is_drawing {
            total += "[IMAGE]".chars().count();
        } else {
            total += run.text.chars().count();
        }
    }
    total
}

/// Build top border: ┌────┬────┬────┐
fn build_table_top_border(widths: &[usize]) -> String {
    build_border(widths, '┌', '┬', '┐')
}
/// Build mid border: ├────┼────┼────┤
fn build_table_mid_border(widths: &[usize]) -> String {
    build_border(widths, '├', '┼', '┤')
}
/// Build bottom border: └────┴────┴────┘
fn build_table_bottom_border(widths: &[usize]) -> String {
    build_border(widths, '└', '┴', '┘')
}
fn build_border(widths: &[usize], left: char, mid: char, right: char) -> String {
    if widths.is_empty() {
        // Fallback nếu chưa có col widths (shouldn't happen)
        return format!("{left}{}{right}", "─".repeat(28));
    }
    let mut s = String::with_capacity(widths.iter().sum::<usize>() + widths.len() * 4);
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        // Cell = "─" × (w + 2) cho padding 1 space mỗi bên
        for _ in 0..(*w + 2) {
            s.push('─');
        }
        if i + 1 < widths.len() {
            s.push(mid);
        }
    }
    s.push(right);
    s
}

pub fn render(
    doc: &Document,
    numbering: &HashMap<(u32, u32), LvlTemplate>,
) -> Rendered {
    let mut out = String::new();
    let mut style_meta = Vec::new();
    let mut para_map = Vec::new();
    let mut list_info: Vec<(usize, u32)> = Vec::new();
    let mut cell_map: Vec<(usize, u32, usize, usize, String)> = Vec::new();
    let mut buffer_line: usize = 1;
    let mut in_table_depth: u32 = 0;

    // Pre-compute ordinal (số thứ tự + prefix template) cho mỗi list
    // paragraph. Dùng numbering.xml template (LvlTemplate.render) để emit
    // đúng "•", "-", "1.", "a)", "i.", "1.1.", v.v. theo định nghĩa Word.
    // Counter dict[(list_key, ilvl)] -> số hiện tại (1-based).
    // - list_key = num_id (nếu có) hoặc para_style (vd "ListNumber").
    // - Counter của sub-level RESET khi level cao hơn tăng.
    // - Non-list paragraph không reset counter (list có thể bị ngắt).
    let mut ordinals: Vec<String> = Vec::with_capacity(doc.paragraphs.len());
    let mut counters: std::collections::HashMap<String, Vec<u32>> =
        std::collections::HashMap::new();
    for p in &doc.paragraphs {
        let is_list = p.num_id.is_some()
            || p.num_ilvl.is_some()
            || p.para_style
                .as_deref()
                .map(|s| s.starts_with("List"))
                .unwrap_or(false);
        if !is_list {
            ordinals.push(String::new());
            continue;
        }
        let key = p
            .num_id
            .map(|n| format!("n{n}"))
            .unwrap_or_else(|| format!("s{}", p.para_style.as_deref().unwrap_or("")));
        let ilvl = p.num_ilvl.unwrap_or(0) as i32;
        let counter_vec = counters.entry(key.clone()).or_default();
        while counter_vec.len() < (ilvl as usize + 1) {
            counter_vec.push(0);
        }
        for level in (ilvl as usize + 1)..counter_vec.len() {
            counter_vec[level] = 0;
        }
        counter_vec[ilvl as usize] += 1;
        // Intermediate levels = 0 -> bump lên 1 để render đẹp.
        // Vd khi user Tab từ "1." straight tới ilvl=2 mà không có item
        // ở ilvl=1 trước, counter_vec = [1, 0, 1] -> "1.0.1" trông xấu.
        // Bump các 0 thành 1: counter_vec = [1, 1, 1] -> "1.1.1".
        for level in 0..(ilvl as usize) {
            if counter_vec[level] == 0 {
                counter_vec[level] = 1;
            }
        }
        // Build prefix dùng template từ numbering.xml nếu có.
        let prefix = if let (Some(nid), tpl_opt) = (
            p.num_id,
            p.num_id.and_then(|nid| numbering.get(&(nid, ilvl as u32))),
        ) {
            let _ = nid;
            if let Some(tpl) = tpl_opt {
                tpl.render(counter_vec, ilvl as usize)
            } else {
                // numId có nhưng numbering.xml không có entry — fallback
                // join với dấu '.'
                counter_vec[..=(ilvl as usize)]
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(".")
            }
        } else {
            // Không có numId (style-based numbering từ pStyle) — dùng
            // heuristic theo style name.
            let use_bullet = p
                .para_style
                .as_deref()
                .map(|s| s.starts_with("ListBullet"))
                .unwrap_or(false);
            if use_bullet {
                "•".to_string()
            } else {
                counter_vec[..=(ilvl as usize)]
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(".")
            }
        };
        ordinals.push(prefix);
    }

    // Pre-compute column widths cho mỗi table (table_start_idx -> Vec<col_width>).
    // Mục đích: render table grid với cells thẳng cột.
    // Cấu trúc: với mỗi paragraph TableCell, gom theo (table_start_idx, col_idx)
    // tính max chars của text trong cell đó.
    let mut table_col_widths: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    {
        let mut current_table_start: Option<usize> = None;
        for (i, p) in doc.paragraphs.iter().enumerate() {
            match &p.context {
                ParaContext::TableCell { col_idx, .. } => {
                    let start = if let Some(s) = current_table_start {
                        // Vẫn trong table cũ?
                        let prev_is_table = i > 0
                            && matches!(doc.paragraphs[i - 1].context, ParaContext::TableCell { .. });
                        if prev_is_table { s } else { current_table_start = Some(i); i }
                    } else {
                        current_table_start = Some(i);
                        i
                    };
                    let text_len = paragraph_text_length(p);
                    let widths = table_col_widths.entry(start).or_insert_with(Vec::new);
                    let ci = *col_idx as usize;
                    while widths.len() <= ci {
                        widths.push(0);
                    }
                    if text_len > widths[ci] {
                        widths[ci] = text_len;
                    }
                }
                _ => {
                    current_table_start = None;
                }
            }
        }
        // Clamp width tối thiểu 4 chars/col (tránh col quá hẹp). Không
        // clamp max — vì nếu text dài hơn cell width, render sẽ tràn ra
        // ngoài và lệch border. User vẫn có thể scroll ngang trong Vim.
        for widths in table_col_widths.values_mut() {
            for w in widths.iter_mut() {
                if *w < 4 {
                    *w = 4;
                }
            }
        }
    }

    // Track table_start_idx khi loop chính chạy
    let mut current_table_start_for_render: Option<usize> = None;

    // Walk paragraphs với "row batching": collect paragraphs cùng row Word
    // (cùng row_idx, cùng table) vào batch, emit cả batch cùng lúc dưới
    // dạng grid rows. Mỗi row Word = max(số paragraphs trong cell) dòng
    // buffer; mỗi dòng buffer hiển thị các cells cùng row tại "slot N".
    let mut i: usize = 0;
    while i < doc.paragraphs.len() {
        let p = &doc.paragraphs[i];
        let current_in_table = matches!(p.context, ParaContext::TableCell { .. });
        let prev_in_table = i > 0
            && matches!(doc.paragraphs[i - 1].context, ParaContext::TableCell { .. });

        if !current_in_table {
            // Non-table paragraph: emit như cũ.
            // Trước đó nếu vừa ra khỏi table → emit bottom border
            if prev_in_table {
                let widths = current_table_start_for_render
                    .and_then(|s| table_col_widths.get(&s))
                    .cloned()
                    .unwrap_or_default();
                out.push_str(&build_table_bottom_border(&widths));
                out.push('\n');
                buffer_line += 1;
                in_table_depth = 0;
                current_table_start_for_render = None;
            }
            let is_list = p.num_id.is_some()
                || p.num_ilvl.is_some()
                || p.para_style
                    .as_deref()
                    .map(|s| s.starts_with("List"))
                    .unwrap_or(false);
            if is_list {
                list_info.push((buffer_line, p.num_ilvl.unwrap_or(0)));
            }
            let ordinal = &ordinals[i];
            render_paragraph(
                p, ordinal, &[], &mut out, &mut buffer_line, &mut style_meta, &mut para_map,
            );
            i += 1;
            continue;
        }

        // Vào table mới
        if !prev_in_table {
            current_table_start_for_render = Some(i);
            let widths = table_col_widths.get(&i).cloned().unwrap_or_default();
            out.push_str(&build_table_top_border(&widths));
            out.push('\n');
            buffer_line += 1;
            in_table_depth = 1;
        }

        // Collect paragraphs cùng row Word: walk tiếp cho đến khi
        // is_first_in_row=true (của paragraph sau) hoặc out of table.
        let current_row_idx = match &p.context {
            ParaContext::TableCell { row_idx, .. } => *row_idx,
            _ => 0,
        };
        let mut row_paras: Vec<usize> = vec![i];
        let mut j = i + 1;
        while j < doc.paragraphs.len() {
            match &doc.paragraphs[j].context {
                ParaContext::TableCell { row_idx, is_first_in_row, .. } => {
                    if *row_idx != current_row_idx || (*is_first_in_row && doc.paragraphs[j].context.is_first_in_cell_field()) {
                        break;
                    }
                    row_paras.push(j);
                    j += 1;
                }
                _ => break,
            }
        }

        // Emit ROW_SEPARATOR nếu không phải row đầu của table.
        if current_row_idx > 0 {
            let widths = current_table_start_for_render
                .and_then(|s| table_col_widths.get(&s))
                .cloned()
                .unwrap_or_default();
            out.push_str(&build_table_mid_border(&widths));
            out.push('\n');
            buffer_line += 1;
        }

        // Group row_paras theo col_idx → cells[col_idx] = Vec<paragraph_idx>
        let mut cells_in_row: std::collections::BTreeMap<u32, Vec<usize>> =
            std::collections::BTreeMap::new();
        for &pi in &row_paras {
            if let ParaContext::TableCell { col_idx, .. } = &doc.paragraphs[pi].context {
                cells_in_row.entry(*col_idx).or_default().push(pi);
            }
        }
        // Số dòng buffer cho row này = max paragraphs trong 1 cell.
        let max_paras_in_cell = cells_in_row.values().map(|v| v.len()).max().unwrap_or(1);

        let col_widths = current_table_start_for_render
            .and_then(|s| table_col_widths.get(&s).cloned())
            .unwrap_or_default();

        for slot in 0..max_paras_in_cell {
            // Build dòng buffer: với mỗi col (theo width order), nếu cell
            // có paragraph ở slot N, render text của nó; còn lại empty.
            let mut line_str = String::new();
            line_str.push('│');
            // Lưu para_id chính cho dòng này: dùng cell đầu tiên có
            // paragraph ở slot N. Cũng track cell_map per col cho save.
            let mut line_para_id: Option<String> = None;
            for col in 0..col_widths.len() {
                let cell_paras = cells_in_row.get(&(col as u32));
                let para_idx = cell_paras.and_then(|v| v.get(slot)).copied();
                let cell_width = col_widths[col];
                // Vị trí char_start của cell text (1-based, sau '│ ').
                // line_str trước khi push space: có `│` của col này. Sau
                // push space, cell text bắt đầu.
                line_str.push(' ');
                let cell_text_start_char = line_str.chars().count() + 1; // 1-based
                if let Some(pi) = para_idx {
                    let p_cell = &doc.paragraphs[pi];
                    if line_para_id.is_none() {
                        line_para_id = Some(p_cell.id.clone());
                    }
                    let is_list_c = p_cell.num_id.is_some()
                        || p_cell.num_ilvl.is_some()
                        || p_cell.para_style
                            .as_deref()
                            .map(|s| s.starts_with("List"))
                            .unwrap_or(false);
                    if is_list_c {
                        list_info.push((buffer_line, p_cell.num_ilvl.unwrap_or(0)));
                    }
                    let cell_content_with_styles = render_cell_content(
                        p_cell, &ordinals[pi], cell_width,
                        buffer_line, cell_text_start_char,
                        &mut style_meta,
                    );
                    line_str.push_str(&cell_content_with_styles);
                    // Track cell_map: vùng text của cell = [start, start+cell_width-1]
                    let cell_text_end_char = cell_text_start_char + cell_width - 1;
                    cell_map.push((
                        buffer_line,
                        col as u32,
                        cell_text_start_char,
                        cell_text_end_char,
                        p_cell.id.clone(),
                    ));
                } else {
                    for _ in 0..cell_width {
                        line_str.push(' ');
                    }
                }
                line_str.push(' ');
                line_str.push('│');
            }
            out.push_str(&line_str);
            out.push('\n');
            if let Some(pid) = line_para_id {
                para_map.push((buffer_line, pid));
            }
            buffer_line += 1;
        }

        // Advance i tới j (sau row hiện tại)
        i = j;
    }

    // Emit bottom border nếu cuối file vẫn còn trong table
    if in_table_depth > 0 {
        let widths = current_table_start_for_render
            .and_then(|s| table_col_widths.get(&s))
            .cloned()
            .unwrap_or_default();
        out.push_str(&build_table_bottom_border(&widths));
        out.push('\n');
        buffer_line += 1;
    }
    let _ = in_table_depth;
    let _ = in_table_depth;

    if out.ends_with('\n') {
        out.pop();
    }

    Rendered {
        text: out,
        style_meta,
        para_map,
        list_info,
        cell_map,
    }
}

fn render_paragraph(
    p: &Paragraph,
    ordinal: &str,
    col_widths: &[usize],
    out: &mut String,
    buffer_line: &mut usize,
    style_meta: &mut Vec<(
        usize, usize, usize, bool, bool,
        Option<f32>, Option<String>, Option<String>, Option<String>,
    )>,
    para_map: &mut Vec<(usize, String)>,
) {
    // Render TableCell as grid row: "│ <padding to col 0> │ <text> │ <padding to col N> │"
    // Mọi paragraph trong cell có cùng (col_idx, row_idx); paragraph thứ 2+
    // trong cell render ở dòng riêng nhưng vẫn align cùng cột.
    if let ParaContext::TableCell { col_idx, .. } = &p.context {
        let ci = *col_idx as usize;
        out.push('│');
        // Emit empty cells trước col của paragraph
        for k in 0..ci {
            let w = col_widths.get(k).copied().unwrap_or(4);
            out.push(' ');
            for _ in 0..w {
                out.push(' ');
            }
            out.push(' ');
            out.push('│');
        }
        // Cell chứa paragraph: " <text padded to w> │"
        out.push(' ');
        let cell_width = col_widths.get(ci).copied().unwrap_or(4);
        // Tính prefix (ordinal/indent/style) + text
        let mut cell_content = String::new();
        // Indent
        let indent_level = p
            .indent_twips
            .map(|t| (t.max(0) / 720) as usize)
            .unwrap_or(0);
        for _ in 0..indent_level {
            cell_content.push_str("  ");
        }
    // List prefix: ordinal đã chứa cả punctuation từ numbering template
    // (vd "1.", "•", "a)"). Chỉ thêm space sau.
    if !ordinal.is_empty() {
        cell_content.push_str(ordinal);
        cell_content.push(' ');
    } else if let Some(sty) = p.para_style.as_deref() {
        if sty.starts_with("ListBullet") {
            cell_content.push_str("• ");
        }
    }
        // Text content
        let prefix_chars_in_cell = cell_content.chars().count();
        let text_start_byte_in_out = out.len();
        out.push_str(&cell_content);
        // Track style ranges
        let cell_text_start_char_in_line = char_count_after_last_newline(out);
        let mut char_cursor_in_cell = prefix_chars_in_cell;
        for run in &p.runs {
            if run.is_drawing {
                let placeholder = "[IMAGE]";
                let cstart = cell_text_start_char_in_line - prefix_chars_in_cell + char_cursor_in_cell + 1;
                let cend = cstart + placeholder.chars().count() - 1;
                if !run.style.is_default() {
                    let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
                    style_meta.push((
                        *buffer_line, cstart, cend,
                        run.style.bold, run.style.italic, size_pt,
                        run.style.color_hex.clone(),
                        run.style.font_name.clone(),
                        run.style.highlight.clone(),
                    ));
                }
                out.push_str(placeholder);
                char_cursor_in_cell += placeholder.chars().count();
                continue;
            }
            let run_len = run.text.chars().count();
            if run_len == 0 {
                continue;
            }
            if !run.style.is_default() {
                let cstart = cell_text_start_char_in_line - prefix_chars_in_cell + char_cursor_in_cell + 1;
                let cend = cstart + run_len - 1;
                let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
                style_meta.push((
                    *buffer_line, cstart, cend,
                    run.style.bold, run.style.italic, size_pt,
                    run.style.color_hex.clone(),
                    run.style.font_name.clone(),
                    run.style.highlight.clone(),
                ));
            }
            out.push_str(&run.text);
            char_cursor_in_cell += run_len;
        }
        let _ = text_start_byte_in_out;
        // Pad text to cell_width
        let actual_text_chars = char_cursor_in_cell;
        let pad = cell_width.saturating_sub(actual_text_chars);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push(' ');
        out.push('│');
        // Emit empty cells sau col của paragraph
        for k in (ci + 1)..col_widths.len() {
            let w = col_widths.get(k).copied().unwrap_or(4);
            out.push(' ');
            for _ in 0..w {
                out.push(' ');
            }
            out.push(' ');
            out.push('│');
        }
        para_map.push((*buffer_line, p.id.clone()));
        out.push('\n');
        *buffer_line += 1;
        return;
    }

    // Non-table paragraph: original render flow + alignment padding
    let context_prefix = match &p.context {
        ParaContext::Sdt => "┊ ",
        _ => "",
    };
    let indent_level = p
        .indent_twips
        .map(|t| (t.max(0) / 720) as usize)
        .unwrap_or(0);
    let indent_prefix = "  ".repeat(indent_level);
    let numbered_prefix = if !ordinal.is_empty() {
        // Ordinal đã chứa punctuation từ template — chỉ thêm space.
        Some(format!("{ordinal} "))
    } else {
        None
    };
    let style_prefix: &str = if let Some(np) = numbered_prefix.as_deref() {
        np
    } else {
        match p.para_style.as_deref() {
            Some(s) if s == "Heading1" || s == "Title" => "# ",
            Some("Heading2") => "## ",
            Some("Heading3") => "### ",
            Some("Heading4") => "#### ",
            Some("Heading5") => "##### ",
            Some("Heading6") => "###### ",
            Some(s) if s.starts_with("ListBullet") => "• ",
            _ => "",
        }
    };

    // Pre-compute text length để tính alignment padding.
    // Total content chars = context + indent + style_prefix + sum(run text).
    let text_only_chars: usize = p
        .runs
        .iter()
        .map(|r| if r.is_drawing { 7 } else { r.text.chars().count() })
        .sum();
    let content_chars = context_prefix.chars().count()
        + indent_prefix.chars().count()
        + style_prefix.chars().count()
        + text_only_chars;

    // Alignment padding: center -> (PAGE_WIDTH - content) / 2 spaces trước.
    // right -> PAGE_WIDTH - content spaces trước. Giả định PAGE_WIDTH = 80
    // (chuẩn terminal). Không áp dụng cho left/both/None.
    const PAGE_WIDTH: usize = 80;
    let align_pad = match p.alignment.as_deref() {
        Some("center") | Some("centre") => {
            if content_chars < PAGE_WIDTH {
                (PAGE_WIDTH - content_chars) / 2
            } else {
                0
            }
        }
        Some("right") => {
            if content_chars < PAGE_WIDTH {
                PAGE_WIDTH - content_chars
            } else {
                0
            }
        }
        _ => 0,
    };
    for _ in 0..align_pad {
        out.push(' ');
    }
    out.push_str(context_prefix);
    out.push_str(&indent_prefix);
    out.push_str(style_prefix);

    let prefix_chars = align_pad
        + context_prefix.chars().count()
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
            if !run.style.is_default() {
                let size_pt = run.style.size_half_pt.map(|h| h as f32 / 2.0);
                style_meta.push((
                    *buffer_line, cstart, cend,
                    run.style.bold, run.style.italic, size_pt,
                    run.style.color_hex.clone(),
                    run.style.font_name.clone(),
                    run.style.highlight.clone(),
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
                *buffer_line, cstart, cend,
                run.style.bold, run.style.italic, size_pt,
                run.style.color_hex.clone(),
                run.style.font_name.clone(),
                run.style.highlight.clone(),
            ));
        }
        out.push_str(run_text);
        char_cursor += run_len;
    }

    para_map.push((*buffer_line, p.id.clone()));
    out.push('\n');
    *buffer_line += 1;
}

/// Đếm số ký tự từ ký tự `\n` cuối cùng (hoặc đầu chuỗi) đến hết `s`.
/// Dùng để xác định vị trí char start của text cell trong dòng buffer.
fn char_count_after_last_newline(s: &str) -> usize {
    match s.rfind('\n') {
        Some(idx) => s[idx + 1..].chars().count(),
        None => s.chars().count(),
    }
}
