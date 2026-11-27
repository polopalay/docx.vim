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

pub fn apply_buffer_to_document(
    doc: &Document,
    new_text: &str,
    numbering: &std::collections::HashMap<(u32, u32), crate::numbering::LvlTemplate>,
) -> AppResult<Vec<u8>> {
    let mut doc = clone_doc(doc);

    let rendered = render::render(&doc, numbering);
    let old_text = rendered.text.trim_end_matches('\n');
    let new_text_trim = new_text.trim_end_matches('\n');
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let new_lines: Vec<&str> = new_text_trim.split('\n').collect();

    // `u` quá tay / xoá hết buffer rồi :w sẽ xoá trắng tài liệu.
    if new_text_trim.trim().is_empty() && !old_text.trim().is_empty() {
        return Err(AppError("NOT saved: buffer is empty".to_string()));
    }

    // Map line index (0-based) -> para_id (chỉ cho dòng có paragraph,
    // không phải border table).
    let mut line_to_pid: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    for (line, pid) in &rendered.para_map {
        line_to_pid.insert(*line - 1, pid.clone()); // 0-based
    }
    // Build cell_map_by_line: line 0-based -> Vec<(col, char_start_0based,
    // char_end_0based, pid)>. Dùng để extract per-cell text khi user edit
    // 1 dòng buffer trong table.
    let mut cell_map_by_line: std::collections::HashMap<usize, Vec<(u32, usize, usize, String)>> =
        std::collections::HashMap::new();
    for (line, col, cs, ce, pid) in &rendered.cell_map {
        cell_map_by_line
            .entry(*line - 1)
            .or_default()
            .push((*col, *cs - 1, *ce - 1, pid.clone()));
    }
    // Set các line index là structural marker (TABLE_START/ROW_SEP/TABLE_END).
    // Marker thì KHÔNG được phép xóa, KHÔNG được edit.
    let mut marker_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (idx, _) in old_lines.iter().enumerate() {
        if !line_to_pid.contains_key(&idx) {
            marker_lines.insert(idx);
        }
    }

    // Diff old_lines vs new_lines bằng LCS (longest common subsequence).
    // Output mapping: với mỗi index trong old, là (Keep(new_idx) | Delete).
    let ops = diff_lcs(&old_lines, &new_lines);

    // Phân tích ops thành 2 nhóm:
    // - Delete: paragraph_id bị xóa khỏi file.
    // - Edit (Keep với new != old): paragraph_id giữ nhưng text đổi.
    let mut deletes: Vec<String> = Vec::new();
    let mut edits: Vec<(String, String, String)> = Vec::new(); // (pid, new_text, old_text)
    // Track các marker bị Delete — sẽ kiểm tra sau khi đã biết những
    // paragraph nào bị xóa. Cho phép marker delete nếu paragraph kế nó
    // cũng bị xóa (= user xóa cả row trong table).
    let mut marker_deletes: Vec<usize> = Vec::new();
    let mut deleted_old_indices: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    // PRE-PROCESS table line edits: process per-table, per-row, per-col.
    //
    // Mỗi table có nhiều rows; mỗi row có nhiều cells (cols); mỗi cell có
    // N paragraphs cố định. Buffer render mỗi row dùng max(paragraphs)
    // dòng, các cells cùng row đặt cạnh nhau qua `│`. Save logic:
    //   1. Group paragraphs theo (table_id, row, col) từ doc.paragraphs.
    //   2. Group new_lines theo "table block" (contiguous lines bắt đầu
    //      bằng `│`).
    //   3. Pair table thứ N old với table thứ N new theo thứ tự.
    //   4. Trong mỗi table, pair từng row theo offset dòng buffer; mỗi
    //      cell apply head-align + tail-align để xử lý user thêm/xóa dòng.

    // Quick check: nếu buffer text == old rendered text → no edits at all,
    // skip toàn bộ pre-process để tránh false-positive edits cho file
    // không bị edit (vd round-trip).
    let buffer_unchanged = old_lines.len() == new_lines.len()
        && old_lines.iter().zip(new_lines.iter()).all(|(o, n)| o == n);
    if buffer_unchanged {
        // No edits — trả về XML gốc nguyên byte-for-byte.
        return Ok(doc.original_xml.clone());
    }

    // Build cells grouped by (table_id, row, col). table_id = số thứ tự
    // table trong doc (bắt đầu từ 1, reset khi gặp non-TableCell).
    use std::collections::BTreeMap;
    let mut cells_grouped: BTreeMap<(usize, u32, u32), Vec<String>> = BTreeMap::new();
    let mut pid_to_table: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    {
        let mut current_table_id: usize = 0;
        let mut last_was_table = false;
        for p in &doc.paragraphs {
            match &p.context {
                ParaContext::TableCell { row_idx, col_idx, .. } => {
                    if !last_was_table {
                        current_table_id += 1;
                    }
                    last_was_table = true;
                    cells_grouped
                        .entry((current_table_id, *row_idx, *col_idx))
                        .or_default()
                        .push(p.id.clone());
                    pid_to_table.insert(p.id.clone(), current_table_id);
                }
                _ => {
                    last_was_table = false;
                }
            }
        }
    }

    // Table trong buffer: table -> row (ngăn bởi '├') -> các dòng '│'.
    let old_blocks = table_blocks(&old_lines);
    let new_blocks = table_blocks(&new_lines);
    let text = |lines: &[&str], block: &[Vec<usize>]| -> Vec<Vec<String>> {
        block.iter().map(|row| row.iter().map(|&i| lines[i].to_string()).collect()).collect()
    };
    let tables_changed = old_blocks.len() != new_blocks.len()
        || old_blocks.iter().zip(&new_blocks).any(|(o, n)| text(&old_lines, o) != text(&new_lines, n));
    if tables_changed {
        let shape_error = || {
            AppError("NOT saved: adding/removing tables or table rows is not supported".to_string())
        };
        if old_blocks.len() != new_blocks.len() {
            return Err(shape_error());
        }
        for (tid, (old_block, new_block)) in old_blocks.iter().zip(&new_blocks).enumerate() {
            let tid = tid + 1; // table id đánh số từ 1
            if text(&old_lines, old_block) == text(&new_lines, new_block) {
                continue;
            }
            let mut rows_in_table: BTreeMap<u32, BTreeMap<u32, Vec<String>>> = BTreeMap::new();
            for ((t, row, col), pids) in &cells_grouped {
                if *t == tid {
                    rows_in_table.entry(*row).or_default().insert(*col, pids.clone());
                }
            }
            if old_block.len() != rows_in_table.len() || new_block.len() != rows_in_table.len() {
                return Err(shape_error());
            }
            for ((cells_in_row, old_row), new_row) in rows_in_table.values().zip(old_block).zip(new_block) {
                for (col, cell_pids) in cells_in_row {
                    let seg = |line: &str| {
                        line.split('│').nth(*col as usize + 1).map(|s| s.trim_matches(' ').to_string())
                    };
                    let old_segs: Vec<String> = old_row.iter().map(|&i| seg(old_lines[i]).unwrap_or_default()).collect();
                    let new_segs: Vec<String> = new_row.iter().map(|&i| seg(new_lines[i]).unwrap_or_default()).collect();
                    let (nc, m) = (cell_pids.len(), new_segs.len());
                    if m < nc {
                        // Chỉ cho phép xoá dòng TRỐNG trong cell; mọi thay đổi
                        // khác khi bớt dòng sẽ dồn nội dung sai chỗ.
                        let mut j = 0;
                        for (k, pid) in cell_pids.iter().enumerate() {
                            if j < m && old_segs.get(k) == Some(&new_segs[j]) {
                                j += 1;
                            } else if old_segs.get(k).is_some_and(|t| t.is_empty()) {
                                deletes.push(pid.clone());
                            } else {
                                return Err(shape_error());
                            }
                        }
                        if j < m {
                            return Err(shape_error());
                        }
                        continue;
                    }
                    // Slot k của buffer = paragraph k của cell. Dòng dư (gõ
                    // thêm) áp vào paragraph cuối từ dưới lên.
                    let mut pairs: Vec<(usize, usize)> = (0..nc).map(|k| (k, k)).collect();
                    for k in 1..=(m - nc).min(nc) {
                        if !new_segs[m - k].is_empty() {
                            pairs.retain(|p| p.1 != nc - k);
                            pairs.push((m - k, nc - k));
                        }
                    }
                    for (slot, para_idx) in pairs {
                        let (Some(old_seg), new_seg) = (old_segs.get(para_idx), &new_segs[slot]) else {
                            continue;
                        };
                        if old_seg == new_seg {
                            continue;
                        }
                        let cell_pid = &cell_pids[para_idx];
                        let Some(p) = doc.paragraphs.iter().find(|p| &p.id == cell_pid) else { continue };
                        let content = strip_rendered_prefix(p, old_seg, new_seg);
                        edits.push((cell_pid.clone(), content, old_seg.clone()));
                    }
                }
            }
        }
    }

    for (old_idx, op) in ops.iter().enumerate() {
        match op {
            DiffOp::Delete => {
                if marker_lines.contains(&old_idx) {
                    marker_deletes.push(old_idx);
                    continue;
                }
                if let Some(pid) = line_to_pid.get(&old_idx) {
                    if let Some(p) = doc.paragraphs.iter().find(|p| &p.id == pid) {
                        if matches!(p.context, ParaContext::TableCell { .. }) {
                            // Table line: đã được pre-process ở trên.
                            continue;
                        }
                    }
                    deletes.push(pid.clone());
                    deleted_old_indices.insert(old_idx);
                }
            }
            DiffOp::Keep(new_idx) => {
                let old_line = old_lines[old_idx];
                let new_line = new_lines[*new_idx];
                if old_line == new_line {
                    continue;
                }
                if marker_lines.contains(&old_idx) {
                    // Border/marker line bị edit (vd user vô tình thay đổi
                    // border, hoặc width col tự re-fit). Silent ignore —
                    // border sẽ được regenerate đúng khi save xong.
                    continue;
                }
                if let Some(pid) = line_to_pid.get(&old_idx) {
                    if let Some(p) = doc.paragraphs.iter().find(|p| &p.id == pid) {
                        if matches!(p.context, ParaContext::TableCell { .. }) {
                            continue; // pre-processed above
                        }
                    }
                    edits.push((pid.clone(), new_line.to_string(), old_line.to_string()));
                }
            }
        }
    }

    // Validate marker deletions: marker chỉ được phép xóa nếu CẢ paragraph
    // ngay trước HOẶC ngay sau nó cũng bị xóa (= xóa nguyên table row).
    // Nếu marker bị xóa "đứng lẻ" → báo lỗi (user vô tình xóa border).
    for &marker_idx in &marker_deletes {
        let prev_deleted = marker_idx > 0
            && (deleted_old_indices.contains(&(marker_idx - 1))
                || marker_lines.contains(&(marker_idx - 1)));
        let next_deleted = marker_idx + 1 < old_lines.len()
            && (deleted_old_indices.contains(&(marker_idx + 1))
                || marker_lines.contains(&(marker_idx + 1)));
        if !prev_deleted && !next_deleted {
            return Err(AppError(format!(
                "Line {} is a structural marker (table border) — cannot be deleted alone",
                marker_idx + 1
            )));
        }
    }

    // Detect INSERT (dòng new chưa được map). Hỗ trợ bằng cách tạo
    // paragraph mới kế thừa style từ paragraph KỀ. Logic:
    // - Walk new_lines theo thứ tự
    // - Mỗi new_idx không trong `kept`: là insert.
    // - Tìm anchor: paragraph kept gần nhất phía TRƯỚC → insert AFTER anchor.
    //   Nếu không có (insert ở đầu file): tìm paragraph kept đầu tiên SAU
    //   → insert BEFORE anchor.
    let kept_new_to_old: std::collections::HashMap<usize, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(old_idx, op)| match op {
            DiffOp::Keep(n) => Some((*n, old_idx)),
            _ => None,
        })
        .collect();
    let kept: std::collections::HashSet<usize> = kept_new_to_old.keys().copied().collect();

    // Tuples: (new_idx, anchor_pid, position, text)
    let mut inserts: Vec<(usize, String, InsertPosition, String)> = Vec::new();
    for new_idx in 0..new_lines.len() {
        if kept.contains(&new_idx) {
            continue;
        }
        // Dòng trong table được xử lý ở pre-process (nếu không áp được,
        // guard ensure_nothing_lost bên dưới sẽ từ chối lưu).
        if new_lines[new_idx].starts_with(['│', '┌', '└', '├']) {
            continue;
        }
        // Anchor chỉ được là paragraph ngoài table (không phải border/cell).
        let is_body = |j: &usize| {
            kept_new_to_old
                .get(j)
                .and_then(|o| line_to_pid.get(o))
                .and_then(|pid| doc.paragraphs.iter().find(|p| &p.id == pid))
                .is_some_and(|p| !matches!(p.context, ParaContext::TableCell { .. }))
        };
        let anchor_before = (0..new_idx).rev().find(|j| is_body(j));
        let anchor_after = (new_idx + 1..new_lines.len()).find(|j| is_body(j));

        // Helper: kiểm tra paragraph có phải list không
        let is_list_para = |line_idx: usize| -> bool {
            let old_idx = match kept_new_to_old.get(&line_idx) {
                Some(o) => *o,
                None => return false,
            };
            let pid = match line_to_pid.get(&old_idx) {
                Some(p) => p,
                None => return false,
            };
            doc.paragraphs
                .iter()
                .find(|p| &p.id == pid)
                .map(|p| {
                    p.is_list()
                })
                .unwrap_or(false)
        };

        // Smart anchor choice: nếu before là list mà after không phải list,
        // dùng after (Before position) → paragraph mới thuộc về block
        // sau, không kế thừa list style. Đây là case user nhấn O (Shift-O)
        // trên paragraph không-phải-list ngay sau list block.
        let (anchor_idx, position) = match (anchor_before, anchor_after) {
            (Some(before), Some(after)) => {
                if is_list_para(before) && !is_list_para(after) {
                    (Some(after), InsertPosition::Before)
                } else {
                    (Some(before), InsertPosition::After)
                }
            }
            (Some(before), None) => (Some(before), InsertPosition::After),
            (None, Some(after)) => (Some(after), InsertPosition::Before),
            (None, None) => (None, InsertPosition::After),
        };
        let anchor_pid = match anchor_idx {
            Some(an) => {
                let old_idx = kept_new_to_old[&an];
                if let Some(pid) = line_to_pid.get(&old_idx) {
                    pid.clone()
                } else {
                    continue;
                }
            }
            None => {
                // Không có anchor (vd mọi dòng đều bị thay) -> trước
                // paragraph ngoài table đầu tiên.
                if let Some(first) = doc.paragraphs.iter().find(|p| !matches!(p.context, ParaContext::TableCell { .. })) {
                    inserts.push((new_idx, first.id.clone(), InsertPosition::Before, new_lines[new_idx].to_string()));
                }
                continue;
            }
        };
        inserts.push((new_idx, anchor_pid, position, new_lines[new_idx].to_string()));
    }

    // Apply edits: update text của paragraph (đặt dirty=true)
    for (pid, new_line, old_line) in &edits {
        if let Some(p) = doc.paragraphs.iter_mut().find(|p| &p.id == pid) {
            let content = if matches!(p.context, ParaContext::TableCell { .. }) {
                new_line.clone() // đã tách khỏi border và bỏ prefix ở trên
            } else {
                strip_rendered_prefix(p, old_line, new_line)
            };
            set_paragraph_text(p, &content)?;
        }
    }

    // Auto-delete: nếu paragraph là LIST item (có numPr hoặc pStyle List*)
    // mà sau khi apply edit text TRỞ NÊN RỖNG, user có ý xóa nó (thường
    // nhấn dd hoặc backspace hết text). Word giữ empty list item nhưng
    // hành vi này thường không phải user muốn — họ muốn dòng list biến
    // mất. Detect và đẩy vào deletes.
    let mut auto_deletes: Vec<String> = Vec::new();
    for (pid, _new_line, _old_line) in &edits {
        if let Some(p) = doc.paragraphs.iter().find(|p| &p.id == pid) {
            let is_list = p.is_list();
            if !is_list {
                continue;
            }
            // Skip nếu paragraph có drawing (giữ ảnh)
            if p.runs.iter().any(|r| r.is_drawing) {
                continue;
            }
            // Tính text content hiện tại (sau apply_text_to_paragraph)
            let total_text: String = p.runs.iter()
                .filter(|r| !r.is_drawing)
                .map(|r| r.text.as_str())
                .collect();
            if total_text.trim().is_empty() {
                auto_deletes.push(pid.clone());
            }
        }
    }
    // Merge auto_deletes vào deletes
    for pid in auto_deletes {
        if !deletes.contains(&pid) {
            deletes.push(pid);
        }
    }

    // Build XML cho mỗi insert, kế thừa style từ parent paragraph. Group
    // theo parent: nhiều insert có cùng parent → emit theo thứ tự (Vim
    // line order).
    if !inserts.is_empty() {
        for (_new_idx, anchor_pid, position, text) in &inserts {
            let anchor = match doc.paragraphs.iter().find(|p| &p.id == anchor_pid) {
                Some(p) => p,
                None => continue,
            };
            // Skip insert nếu text trống VÀ anchor là list — tránh tạo
            // paragraph rỗng (user vô tình Enter thêm dòng rồi không gõ
            // gì hoặc đã backspace xong text). "Trống" ở đây = sau khi
            // strip list prefix (vd "• ", "1. ") chỉ còn whitespace.
            let is_list_anchor = anchor.is_list();
            if is_list_anchor {
                let stripped = strip_list_prefix(text);
                if stripped.trim().is_empty() {
                    continue;
                }
            }
            let xml = build_inherited_paragraph(anchor, text);
            doc.pending_inserts.push((anchor_pid.clone(), *position, xml));
        }
    }

    // Apply deletes: đánh dấu paragraph "to_delete" bằng cách CLEAR
    // byte_range range thành 0..0 và set dirty=false — sẽ bị skip khi
    // rebuild XML. Cách đơn giản: REMOVE khỏi doc.paragraphs.
    if !deletes.is_empty() {
        let del_set: std::collections::HashSet<String> = deletes.into_iter().collect();
        // Lưu byte ranges của paragraph bị xóa để skip trong rebuild
        let deleted_ranges: Vec<(usize, usize, String)> = doc
            .paragraphs
            .iter()
            .filter(|p| del_set.contains(&p.id))
            .map(|p| (p.byte_range.0, p.byte_range.1, p.id.clone()))
            .collect();
        doc.paragraphs.retain(|p| !del_set.contains(&p.id));
        // Lưu vào field tạm cho rebuild biết
        doc.deleted_ranges = deleted_ranges;
    }

    let xml = rebuild_document_xml(&doc)?;
    ensure_nothing_lost(&xml, &doc, &new_lines, numbering)?;
    Ok(xml)
}

/// Chữ/số của 1 dòng, bỏ prefix list ở đầu mỗi ô ("1.", "c)", "iv.") vì
/// chúng được đánh số lại khi xoá/chèn mục.
fn content_chars(line: &str) -> impl Iterator<Item = char> + '_ {
    line.split('│').flat_map(|seg| {
        let seg = seg.trim_start();
        let rest = match seg.split_once(' ') {
            Some((mark, rest))
                if !rest.trim().is_empty()
                    && !rest.starts_with(' ')
                    && mark.len() <= 12
                    && (mark.ends_with(['.', ')'])
                        || mark.chars().all(|c| c.is_ascii_digit() || c == '.')) =>
            {
                rest
            }
            _ => seg,
        };
        rest.chars().filter(|c| c.is_alphanumeric())
    })
}

/// Chốt chặn mất dữ liệu: mọi chữ/số trong buffer phải xuất hiện trong kết
/// quả render của XML sắp ghi. Thiếu = có dòng không áp được -> từ chối lưu.
fn ensure_nothing_lost(
    xml: &[u8],
    old: &Document,
    new_lines: &[&str],
    numbering: &std::collections::HashMap<(u32, u32), crate::numbering::LvlTemplate>,
) -> AppResult<()> {
    let mut saved = crate::parser::parse_document(xml)?;
    for p in saved.paragraphs.iter_mut() {
        for r in p.runs.iter_mut().filter(|r| r.is_drawing) {
            r.media_name = old
                .paragraphs
                .iter()
                .flat_map(|p| &p.runs)
                .find(|o| o.rel_id == r.rel_id && o.media_name.is_some())
                .and_then(|o| o.media_name.clone());
        }
    }
    let out = render::render(&saved, numbering).text;
    let mut have: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for c in out.split('\n').flat_map(content_chars) {
        *have.entry(c).or_insert(0) += 1;
    }
    for (i, line) in new_lines.iter().enumerate() {
        for c in content_chars(line) {
            match have.get_mut(&c) {
                Some(n) if *n > 0 => *n -= 1,
                _ => {
                    return Err(AppError(format!(
                        "NOT saved: text around line {} cannot be applied (new lines inside tables are not supported; use `o`)",
                        i + 1
                    )))
                }
            }
        }
    }
    Ok(())
}

/// Trả về text sau prefix. Nếu không match prefix, trả nguyên text (đã
/// trim leading whitespace).
fn strip_list_prefix(text: &str) -> String {
    let trimmed = text.trim_start();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut consumed = 0usize;
    // Pattern 1/2: digits + optional dots + space
    if !chars.is_empty() && chars[0].is_ascii_digit() {
        let mut i = 0;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
            i += 1;
        }
        if i < chars.len() && chars[i] == ' ' {
            consumed = i + 1;
        }
    }
    // Pattern 3: bullet/dash/star + space
    if consumed == 0 && chars.len() >= 2 {
        let first = chars[0];
        if matches!(first, '•' | '▪' | '◆' | '✓' | '✗' | '○' | '·' | '-' | '*' | '+' | 'o')
            && chars[1] == ' '
        {
            consumed = 2;
        }
    }
    chars[consumed..].iter().collect()
}


/// từ parent paragraph. Text content = `text` (đã được caller strip
/// alignment padding nếu cần). Dùng cho insert qua Enter trong Insert mode.
fn build_inherited_paragraph(parent: &Paragraph, text: &str) -> String {
    // Bỏ prefix list user gõ theo mẫu dòng trên: render sẽ tự thêm lại.
    let clean = if parent.is_list() {
        strip_list_prefix(text)
    } else {
        text.trim_start().to_string()
    };
    let run = if clean.is_empty() {
        String::new()
    } else {
        format!(
            "<w:r>{}{}</w:r>",
            xml_emit::inherited_rpr(parent),
            xml_emit::serialize_text(&clean)
        )
    };
    format!("<w:p>{}{run}</w:p>", xml_emit::inherited_ppr(parent, false))
}


#[derive(Debug)]
enum DiffOp {
    /// Dòng old được giữ, ứng với index trong new.
    Keep(usize),
    /// Dòng old bị xóa.
    Delete,
}

/// Các table trong buffer: mỗi table là list row, mỗi row là list chỉ số
/// dòng '│'. Dòng '├' ngăn 2 row.
fn table_blocks(lines: &[&str]) -> Vec<Vec<Vec<usize>>> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<Vec<usize>>> = None;
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with(['│', '┌', '└', '├']) {
            blocks.extend(current.take());
            continue;
        }
        let rows = current.get_or_insert_with(|| vec![Vec::new()]);
        if line.starts_with('├') {
            rows.push(Vec::new());
        } else if line.starts_with('│') {
            rows.last_mut().unwrap().push(i);
        }
    }
    blocks.extend(current);
    blocks
}

/// LCS diff. Mỗi old[i] -> Keep(j) hoặc Delete. Trong 1 hunk thay thế, dòng
/// cũ được ghép với dòng mới giống nhất (giữ thứ tự) để paragraph giữ nguyên
/// định danh/format khi user sửa; dòng mới dư ra = insert.
fn diff_lcs(old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_tail = old.len().min(new.len()) - head;
    let tail = old.iter().rev().zip(new.iter().rev()).take(max_tail).take_while(|(a, b)| a == b).count();
    let a = &old[head..old.len() - tail];
    let b = &new[head..new.len() - tail];
    // ponytail: bảng O(a*b) trên phần giữa; đổi sang Myers nếu sửa 2 đầu file
    // rất lớn làm tốn RAM.
    let mut dp = vec![0u32; (a.len() + 1) * (b.len() + 1)];
    let w = b.len() + 1;
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let mut matches: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matches.push((a.len(), b.len()));

    let mut mid: Vec<DiffOp> = (0..a.len()).map(|_| DiffOp::Delete).collect();
    let (mut i0, mut j0) = (0, 0);
    for (i1, j1) in matches {
        if i1 < a.len() {
            mid[i1] = DiffOp::Keep(head + j1);
        }
        let table = |s: &str| s.starts_with(['│', '┌', '└', '├']);
        let pairs = (i1 - i0).min(j1 - j0);
        let (mut oi, mut nj) = (i0, j0);
        for k in 0..pairs {
            let left = pairs - k - 1;
            // Bên dài hơn được chọn ứng viên giống nhất, chừa chỗ cho phần còn lại.
            if i1 - i0 <= j1 - j0 {
                let best = (nj..j1 - left)
                    .rev()
                    .max_by(|x, y| text_similarity(a[oi], b[*x]).total_cmp(&text_similarity(a[oi], b[*y])))
                    .unwrap_or(nj);
                if table(a[oi]) == table(b[best]) {
                    mid[oi] = DiffOp::Keep(head + best);
                }
                oi += 1;
                nj = best + 1;
            } else {
                let best = (oi..i1 - left)
                    .rev()
                    .max_by(|x, y| text_similarity(a[*x], b[nj]).total_cmp(&text_similarity(a[*y], b[nj])))
                    .unwrap_or(oi);
                if table(a[best]) == table(b[nj]) {
                    mid[best] = DiffOp::Keep(head + nj);
                }
                oi = best + 1;
                nj += 1;
            }
        }
        i0 = i1 + 1;
        j0 = j1 + 1;
    }

    let mut ops: Vec<DiffOp> = (0..head).map(DiffOp::Keep).collect();
    ops.extend(mid);
    ops.extend((0..tail).map(|k| DiffOp::Keep(new.len() - tail + k)));
    ops
}

/// Text similarity 0.0-1.0 dựa trên longest common substring length /
/// max(len_a, len_b). Simple heuristic, không tốn quá nhiều CPU.
fn text_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return if a.is_empty() && b.is_empty() { 1.0 } else { 0.0 };
    }
    // Đếm characters chung (multiset intersection).
    let mut counts_a: std::collections::HashMap<char, usize> =
        std::collections::HashMap::new();
    for c in a.chars() {
        *counts_a.entry(c).or_insert(0) += 1;
    }
    let mut common = 0;
    for c in b.chars() {
        if let Some(n) = counts_a.get_mut(&c) {
            if *n > 0 {
                common += 1;
                *n -= 1;
            }
        }
    }
    let max_len = a.chars().count().max(b.chars().count());
    common as f64 / max_len as f64
}

/// Text hiển thị của paragraph (không gồm prefix list/heading).
fn display_text(p: &Paragraph) -> String {
    p.runs
        .iter()
        .map(|r| if r.is_drawing { render::image_placeholder(r) } else { r.text.clone() })
        .collect()
}

/// Bỏ prefix render (ordinal, "# ", indent...) khỏi dòng user sửa, dựa trên
/// dòng cũ thực tế: prefix = phần của `old_line` đứng trước text.
fn strip_rendered_prefix(p: &Paragraph, old_line: &str, new_line: &str) -> String {
    let old_text = display_text(p);
    // Text nằm CUỐI dòng render -> tìm từ phải, kẻo text như "1" khớp vào
    // số thứ tự "1." ở prefix.
    let prefix = if old_text.is_empty() {
        old_line
    } else {
        old_line
            .rfind(old_text.as_str())
            .or_else(|| old_line.rfind(old_text.trim_matches(' ')))
            .map(|i| &old_line[..i])
            .unwrap_or("")
    };
    let centered = matches!(p.alignment.as_deref(), Some("center") | Some("centre") | Some("right"));
    let (prefix, line) = if centered {
        (prefix.trim_start(), new_line.trim_start())
    } else {
        (prefix, new_line)
    };
    match line.strip_prefix(prefix) {
        Some(rest) => rest.to_string(),
        None if prefix.trim().is_empty() => line.to_string(),
        None => strip_list_prefix(line.trim_start_matches(['#', ' '])),
    }
}

/// Ghi text mới vào các run, chỉ đụng đoạn khác nhau để giữ style từng run.
/// Ảnh/object là khối nguyên: placeholder của nó phải còn nguyên.
fn set_paragraph_text(p: &mut Paragraph, new_text: &str) -> AppResult<()> {
    if display_text(p) == new_text {
        return Ok(());
    }
    // Chia theo ảnh: (run range trước ảnh, text mới tương ứng)
    let mut rest = new_text.to_string();
    let mut groups: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut from = 0;
    for (i, r) in p.runs.iter().enumerate().filter(|(_, r)| r.is_drawing) {
        let mark = render::image_placeholder(r);
        let at = rest.find(&mark).ok_or_else(|| {
            AppError(format!("{}: do not edit/remove {mark} (use :DocxDeleteFile)", p.id))
        })?;
        groups.push((from..i, rest[..at].to_string()));
        rest = rest[at + mark.len()..].to_string();
        from = i + 1;
    }
    groups.push((from..p.runs.len(), rest));

    let mut changed = false;
    let mut append: Option<String> = None;
    for (gi, (range, text)) in groups.iter().enumerate() {
        let runs = &mut p.runs[range.clone()];
        let old: Vec<char> = runs.iter().flat_map(|r| r.text.chars()).collect();
        let new: Vec<char> = text.chars().collect();
        if old == new {
            continue;
        }
        changed = true;
        if runs.is_empty() {
            // Chỉ nối được vào CUỐI paragraph.
            if gi + 1 != groups.len() {
                return Err(AppError(format!("{}: cannot type directly before an image", p.id)));
            }
            append = Some(text.clone());
            continue;
        }
        let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let max_suf = old.len().min(new.len()) - pre;
        let suf = old.iter().rev().zip(new.iter().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
        let (cut_a, cut_b) = (pre, old.len() - suf);
        let mid: String = new[pre..new.len() - suf].iter().collect();
        let mut pos = 0;
        let mut inserted = false;
        for r in runs.iter_mut() {
            let t: Vec<char> = r.text.chars().collect();
            let (s, e) = (pos, pos + t.len());
            pos = e;
            // Gõ chèn ở ranh giới -> nối vào run trước; thay thế -> run chứa
            // ký tự bị thay (giữ đúng style).
            let reaches = if cut_b > cut_a { cut_a < e } else { cut_a <= e };
            let takes = !inserted && reaches && (!t.is_empty() || old.is_empty());
            if !takes && !(s < cut_b && e > cut_a) {
                continue;
            }
            let mut out: String = t.iter().take(cut_a.saturating_sub(s)).collect();
            if takes {
                out.push_str(&mid);
                inserted = true;
            }
            out.extend(t.iter().skip(cut_b.saturating_sub(s)));
            r.text = out;
        }
        if !inserted && !mid.is_empty() {
            runs[0].text.insert_str(0, &mid);
        }
    }
    if let Some(text) = append {
        p.runs.push(Run { text, ..Run::default() });
    }
    p.dirty |= changed;
    Ok(())
}

/// Rebuild document.xml — paragraph dirty thì emit XML mới, không dirty
/// thì copy byte gốc. Mọi khoảng XML khác (table outer, sdt, bookmark...)
/// giữ nguyên qua phép trừ giữa các byte_range.
pub fn rebuild_document_xml(doc: &Document) -> AppResult<Vec<u8>> {
    let mut out = Vec::with_capacity(doc.original_xml.len() + 512);

    // Phần "vỏ" trước <w:body> nội dung
    out.extend_from_slice(&doc.original_xml[..doc.body_inner_range.0]);

    // Build set của tất cả paragraph still present + paragraph được phép
    // hiển thị (chưa bị delete). Walk byte-by-byte:
    // - Khi cursor đạt byte_range.0 của 1 paragraph còn lại: emit nó.
    // - Khi cursor đạt byte_range.0 của 1 paragraph đã delete: SKIP qua,
    //   set cursor = byte_range.1 (không copy).
    // - Còn lại: copy byte gốc (whitespace giữa, sectPr, table outer...).
    let mut all_events: Vec<(usize, usize, EventKind)> = Vec::new();
    for p in &doc.paragraphs {
        all_events.push((p.byte_range.0, p.byte_range.1, EventKind::Paragraph(p)));
    }
    for (s, e, pid) in &doc.deleted_ranges {
        all_events.push((*s, *e, EventKind::Deleted(pid.clone())));
    }
    all_events.sort_by_key(|(s, _, _)| *s);

    // Group inserts by anchor_pid -> list of (position, xml).
    let mut inserts_by_anchor: std::collections::HashMap<&str, Vec<(InsertPosition, &str)>> =
        std::collections::HashMap::new();
    for (anchor_pid, position, xml) in &doc.pending_inserts {
        inserts_by_anchor
            .entry(anchor_pid.as_str())
            .or_default()
            .push((*position, xml.as_str()));
    }

    let mut cursor = doc.body_inner_range.0;
    for (start, end, kind) in &all_events {
        if *start > cursor {
            out.extend_from_slice(&doc.original_xml[cursor..*start]);
        }
        match kind {
            EventKind::Paragraph(p) => {
                // Emit "Before" inserts trước paragraph
                if let Some(xmls) = inserts_by_anchor.get(p.id.as_str()) {
                    for (pos, xml) in xmls {
                        if *pos == InsertPosition::Before {
                            out.extend_from_slice(xml.as_bytes());
                        }
                    }
                }
                if p.dirty {
                    out.extend_from_slice(xml_emit::emit_paragraph(p, &doc.original_xml).as_bytes());
                } else {
                    out.extend_from_slice(&doc.original_xml[p.byte_range.0..p.byte_range.1]);
                }
                // Emit "After" inserts sau paragraph
                if let Some(xmls) = inserts_by_anchor.get(p.id.as_str()) {
                    for (pos, xml) in xmls {
                        if *pos == InsertPosition::After {
                            out.extend_from_slice(xml.as_bytes());
                        }
                    }
                }
            }
            EventKind::Deleted(pid) => {
                // Paragraph này đã bị xóa nội dung, NHƯNG vẫn có thể có
                // pending inserts neo vào nó (vd file mới: P0 trống bị
                // xóa, user gõ text → inserts Before P0). Emit cả Before
                // lẫn After inserts, chỉ bỏ qua nội dung paragraph gốc.
                if let Some(xmls) = inserts_by_anchor.get(pid.as_str()) {
                    for (pos, xml) in xmls {
                        if *pos == InsertPosition::Before {
                            out.extend_from_slice(xml.as_bytes());
                        }
                    }
                    for (pos, xml) in xmls {
                        if *pos == InsertPosition::After {
                            out.extend_from_slice(xml.as_bytes());
                        }
                    }
                }
            }
        }
        cursor = *end;
    }
    if cursor < doc.body_inner_range.1 {
        out.extend_from_slice(&doc.original_xml[cursor..doc.body_inner_range.1]);
    }

    // Phần "vỏ" sau </w:body>
    out.extend_from_slice(&doc.original_xml[doc.body_inner_range.1..]);

    Ok(out)
}

enum EventKind<'a> {
    Paragraph(&'a Paragraph),
    /// Paragraph đã xóa, mang theo id để vẫn emit pending inserts neo vào nó.
    Deleted(String),
}

fn clone_doc(doc: &Document) -> Document {
    Document {
        paragraphs: doc.paragraphs.clone(),
        body_inner_range: doc.body_inner_range,
        original_xml: doc.original_xml.clone(),
        deleted_ranges: doc.deleted_ranges.clone(),
        pending_inserts: doc.pending_inserts.clone(),
    }
}
