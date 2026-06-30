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

    // Group old table buffer lines by table_id (theo paragraph cell đầu
    // tiên trên mỗi dòng).
    let mut table_buffer_lines: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut sorted_old_lines: Vec<usize> = cell_map_by_line.keys().copied().collect();
    sorted_old_lines.sort();
    for old_line in &sorted_old_lines {
        if let Some(cells) = cell_map_by_line.get(old_line) {
            if let Some((_, _, _, pid)) = cells.first() {
                if let Some(tid) = pid_to_table.get(pid) {
                    table_buffer_lines.entry(*tid).or_default().push(*old_line);
                }
            }
        }
    }

    // Group new buffer table lines by table block. 1 table block =
    // contiguous lines bắt đầu bằng `│` HOẶC `┌`/`└`/`├`/`┤` (borders).
    // Khi gặp dòng KHÔNG phải table char → kết thúc table block.
    let is_table_line = |s: &str| -> bool {
        s.starts_with('│')
            || s.starts_with('┌')
            || s.starts_with('└')
            || s.starts_with('├')
    };
    let mut new_table_groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group: Vec<usize> = Vec::new();
    for (idx, line) in new_lines.iter().enumerate() {
        if is_table_line(line) {
            if line.starts_with('│') {
                current_group.push(idx);
            }
            // Border lines: keep in current group (do not break) — chúng
            // thuộc về cùng table.
        } else if !current_group.is_empty() {
            new_table_groups.push(std::mem::take(&mut current_group));
        }
    }
    if !current_group.is_empty() {
        new_table_groups.push(current_group);
    }

    // Pair table_buffer_lines[tid] với new_table_groups[i].
    let table_ids: Vec<usize> = table_buffer_lines.keys().copied().collect();
    for (table_pos, tid) in table_ids.iter().enumerate() {
        let old_table_lines = &table_buffer_lines[tid];
        let new_table_lines = match new_table_groups.get(table_pos) {
            Some(g) => g,
            None => continue,
        };
        // Group cells của table này theo row.
        let mut rows_in_table: BTreeMap<u32, BTreeMap<u32, Vec<String>>> = BTreeMap::new();
        for ((t2, row, col), pids) in &cells_grouped {
            if t2 == tid {
                rows_in_table.entry(*row).or_default().insert(*col, pids.clone());
            }
        }
        // Process row-by-row, advance cursors trong old/new lines.
        let mut new_line_cursor = 0usize;
        let mut old_line_cursor = 0usize;
        let rows_count = rows_in_table.len();
        let mut row_iter_idx = 0usize;
        for cells_in_row in rows_in_table.values() {
            let max_paras = cells_in_row.values().map(|v| v.len()).max().unwrap_or(0);
            let old_slice_end = (old_line_cursor + max_paras).min(old_table_lines.len());
            // Cho ROW CUỐI: lấy hết new_table_lines còn lại (xử lý overflow
            // do user gõ tràn). Cho rows giữa: dùng max_paras.
            let is_last_row = row_iter_idx + 1 == rows_count;
            let new_slice_end = if is_last_row {
                new_table_lines.len()
            } else {
                (new_line_cursor + max_paras).min(new_table_lines.len())
            };
            row_iter_idx += 1;
            let new_row_lines = &new_table_lines[new_line_cursor..new_slice_end];

            for (col, cell_pids) in cells_in_row {
                let segment_idx = (*col as usize) + 1;
                let nc = cell_pids.len();
                let m = new_row_lines.len();
                // Tính col_unchanged trước: nếu multiset text col 1 trong
                // new buffer == multiset paragraphs cell text → user
                // không edit col này (có thể chỉ shift do thay đổi row
                // khác). Skip toàn bộ head-align + tail-align cho col.
                let col_new_texts: Vec<String> = new_row_lines.iter()
                    .map(|li| {
                        let line = new_lines[*li];
                        let s: Vec<&str> = line.split('│').collect();
                        if segment_idx < s.len() {
                            s[segment_idx].trim().to_string()
                        } else {
                            String::new()
                        }
                    })
                    .collect();
                let col_old_texts: Vec<String> = cell_pids.iter()
                    .map(|pid| {
                        doc.paragraphs.iter()
                            .find(|p| &p.id == pid)
                            .map(|p| p.runs.iter().filter(|r| !r.is_drawing).map(|r| r.text.as_str()).collect::<String>())
                            .unwrap_or_default()
                    })
                    .collect();
                // Multiset compare: KEEP empty strings để detect xóa
                // dòng empty (user dd dòng empty trong cell). Nếu remove
                // empty thì xóa empty không detect được.
                // Nhưng cũng giới hạn: chỉ count empty trong RANGE đầu
                // tiên = số paragraphs cell (Nc). Tail empty (m > Nc)
                // thường là padding render, không count.
                let mut old_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for t in &col_old_texts {
                    *old_counts.entry(t.clone()).or_insert(0) += 1;
                }
                let mut new_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                // Chỉ count first Nc entries của new (mirror size old)
                // — bỏ qua tail padding nếu m > Nc.
                for t in col_new_texts.iter().take(nc) {
                    *new_counts.entry(t.clone()).or_insert(0) += 1;
                }
                // Nếu new buffer NGẮN HƠN cell paragraphs (m < nc): user
                // đã xóa dòng. Tính total entries trong new = m.
                // Old có nc entries. Sự khác biệt = số entries bị thiếu.
                let col_unchanged = old_counts == new_counts && m >= nc;
                if col_unchanged {
                    continue;
                }

                // Head-align
                for (slot, cell_pid) in cell_pids.iter().enumerate() {
                    if slot >= m {
                        continue;
                    }
                    // Check old buffer line == new buffer line at this
                    // slot: nếu giống nhau → no edit cần thiết. Tránh
                    // false-positive khi render giữa lần open vs save có
                    // chênh lệch (vd width col tự re-fit).
                    let old_slot = old_line_cursor + slot;
                    if old_slot < old_table_lines.len() {
                        let old_buf_idx = old_table_lines[old_slot];
                        let new_buf_idx = new_row_lines[slot];
                        if old_lines[old_buf_idx] == new_lines[new_buf_idx] {
                            continue;
                        }
                    }
                    let old_text: String = doc.paragraphs.iter()
                        .find(|p| &p.id == cell_pid)
                        .map(|p| p.runs.iter().filter(|r| !r.is_drawing).map(|r| r.text.as_str()).collect::<String>())
                        .unwrap_or_default();
                    let new_line = new_lines[new_row_lines[slot]];
                    let segs: Vec<&str> = new_line.split('│').collect();
                    let new_text: String = if segment_idx < segs.len() {
                        segs[segment_idx].trim().to_string()
                    } else {
                        continue;
                    };
                    if new_text == old_text {
                        continue;
                    }
                    // Bảo vệ wipe: empty new + non-empty old + buffer dài
                    // hơn cell, kiểm tail chứa old_text → skip.
                    if new_text.is_empty() && !old_text.is_empty() && m > nc {
                        let found_in_tail = (nc..m).any(|extra_slot| {
                            let line = new_lines[new_row_lines[extra_slot]];
                            let s: Vec<&str> = line.split('│').collect();
                            segment_idx < s.len() && s[segment_idx].trim() == old_text
                        });
                        if found_in_tail {
                            continue;
                        }
                    }
                    edits.push((cell_pid.clone(), new_text, old_text));
                }
                // Tail-align: chỉ với non-empty new_text
                if m > nc {
                    let extra = m - nc;
                    for k in 1..=extra.min(nc) {
                        let buffer_slot = m - k;
                        let para_idx = nc - k;
                        // Skip nếu dòng tương đương trong old chưa đổi.
                        let old_slot = old_line_cursor + para_idx;
                        if old_slot < old_table_lines.len() {
                            let old_buf_idx = old_table_lines[old_slot];
                            let new_buf_idx = new_row_lines[buffer_slot];
                            if old_lines[old_buf_idx] == new_lines[new_buf_idx] {
                                continue;
                            }
                        }
                        let cell_pid = &cell_pids[para_idx];
                        let new_line = new_lines[new_row_lines[buffer_slot]];
                        let segs: Vec<&str> = new_line.split('│').collect();
                        let new_text: String = if segment_idx < segs.len() {
                            segs[segment_idx].trim().to_string()
                        } else {
                            continue;
                        };
                        if new_text.is_empty() {
                            continue;
                        }
                        let old_text: String = doc.paragraphs.iter()
                            .find(|p| &p.id == cell_pid)
                            .map(|p| p.runs.iter().filter(|r| !r.is_drawing).map(|r| r.text.as_str()).collect::<String>())
                            .unwrap_or_default();
                        if new_text == old_text {
                            continue;
                        }
                        edits.retain(|(pid, _, _)| pid != cell_pid);
                        edits.push((cell_pid.clone(), new_text, old_text));
                    }
                }
                // DELETE: nếu user buffer NGẮN HƠN cell paragraphs (m < nc),
                // user đã xóa dòng. Tìm paragraph empty cuối cell để
                // xóa. Ưu tiên xóa empty paragraphs (giữ non-empty).
                if m < nc {
                    let to_delete_count = nc - m;
                    // Lấy danh sách indices của empty paragraphs (theo
                    // text run thực), từ CUỐI ngược lên.
                    let mut empty_indices: Vec<usize> = Vec::new();
                    for (idx, pid) in cell_pids.iter().enumerate().rev() {
                        let txt: String = doc.paragraphs.iter()
                            .find(|p| &p.id == pid)
                            .map(|p| p.runs.iter().filter(|r| !r.is_drawing).map(|r| r.text.as_str()).collect::<String>())
                            .unwrap_or_default();
                        if txt.is_empty() {
                            empty_indices.push(idx);
                            if empty_indices.len() == to_delete_count {
                                break;
                            }
                        }
                    }
                    // Add to deletes
                    for idx in empty_indices {
                        let pid = cell_pids[idx].clone();
                        if !deletes.contains(&pid) {
                            deletes.push(pid);
                        }
                    }
                }
            }
            old_line_cursor = old_slice_end;
            new_line_cursor = new_slice_end;
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
        // Tìm anchor "before" (paragraph kept phía trước trong new buffer)
        let mut anchor_before: Option<usize> = None;
        for j in (0..new_idx).rev() {
            if kept.contains(&j) {
                anchor_before = Some(j);
                break;
            }
        }
        // Tìm anchor "after" (paragraph kept phía sau)
        let mut anchor_after: Option<usize> = None;
        for j in (new_idx + 1)..new_lines.len() {
            if kept.contains(&j) {
                anchor_after = Some(j);
                break;
            }
        }
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
                    p.num_id.is_some()
                        || p.num_ilvl.is_some()
                        || p.para_style
                            .as_deref()
                            .map(|s| s.starts_with("List"))
                            .unwrap_or(false)
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
                // Không có kept paragraph nào làm anchor (vd file mới chỉ
                // có 1 paragraph trống mà user gõ đè hết → old[0] bị Delete,
                // không còn Keep). Fallback: dùng paragraph ĐẦU TIÊN trong
                // doc làm anchor, insert Before nó. Như vậy text user gõ
                // được thêm vào trước paragraph gốc, kế thừa style của nó.
                if let Some(first_para) = doc.paragraphs.first() {
                    // Đẩy insert này thành Before paragraph đầu tiên.
                    inserts.push((
                        new_idx,
                        first_para.id.clone(),
                        InsertPosition::Before,
                        new_lines[new_idx].to_string(),
                    ));
                    continue;
                }
                // Doc hoàn toàn rỗng (không paragraph nào) — không thể
                // insert. Bỏ qua dòng này (hiếm gặp).
                continue;
            }
        };
        inserts.push((new_idx, anchor_pid, position, new_lines[new_idx].to_string()));
    }

    // Apply edits: update text của paragraph (đặt dirty=true)
    for (pid, new_line, old_line) in &edits {
        if let Some(p) = doc.paragraphs.iter_mut().find(|p| &p.id == pid) {
            apply_text_to_paragraph(p, new_line, old_line);
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
            let is_list = p.num_id.is_some()
                || p.num_ilvl.is_some()
                || p.para_style
                    .as_deref()
                    .map(|s| s.starts_with("List"))
                    .unwrap_or(false);
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
            // Skip insert nếu anchor là table cell paragraph. Lý do tương
            // tự như edit/delete: refactor render table mới gộp cells,
            // anchor lookup không chính xác cho table → tránh insert vào
            // sai vị trí trong cell.
            if matches!(anchor.context, ParaContext::TableCell { .. }) {
                continue;
            }
            // Skip insert nếu text trống VÀ anchor là list — tránh tạo
            // paragraph rỗng (user vô tình Enter thêm dòng rồi không gõ
            // gì hoặc đã backspace xong text). "Trống" ở đây = sau khi
            // strip list prefix (vd "• ", "1. ") chỉ còn whitespace.
            let is_list_anchor = anchor.num_id.is_some()
                || anchor.num_ilvl.is_some()
                || anchor.para_style
                    .as_deref()
                    .map(|s| s.starts_with("List"))
                    .unwrap_or(false);
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

    rebuild_document_xml(&doc)
}

/// Apply text mới đã clean (đã strip prefix/border) vào paragraph.
/// Giữ style của first run (cho table cells).
fn apply_clean_text(p: &mut Paragraph, new_text: &str) {
    if p.runs.is_empty() {
        if !new_text.is_empty() {
            p.runs.push(Run {
                text: new_text.to_string(),
                style: RunStyle::default(),
                is_drawing: false,
                rel_id: None,
                byte_range: None,
        media_name: None,
            drawing_xml: None,
            });
            p.dirty = true;
        }
        return;
    }
    // Skip nếu có drawing run (giữ ảnh)
    if p.runs.iter().any(|r| r.is_drawing) {
        return;
    }
    // Gán toàn bộ text mới vào run đầu, xóa các runs khác. Giữ style.
    let first_style = p.runs[0].style.clone();
    p.runs.clear();
    if !new_text.is_empty() {
        p.runs.push(Run {
            text: new_text.to_string(),
            style: first_style,
            is_drawing: false,
            rel_id: None,
            byte_range: None,
    media_name: None,
        drawing_xml: None,
        });
    }
    p.dirty = true;
}


/// box-drawing dùng render table grid: │ ─ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼.
/// Strip cả whitespace dính vào để clean.
fn strip_table_border_chars(s: &str) -> String {
    let is_border = |c: char| {
        matches!(
            c,
            '│' | '─' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼'
        )
    };
    let trimmed = s.trim_matches(|c: char| is_border(c) || c.is_whitespace());
    trimmed.to_string()
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
    use std::fmt::Write;
    let mut s = String::with_capacity(128 + text.len());

    // Strip prefix (ordinal/indent/style markers) khỏi text — vì khi
    // render lại, prefix sẽ được tự thêm theo style của paragraph mới.
    // Chỉ strip nếu text đầu trông GIỐNG list prefix (digit + dot + space,
    // hoặc bullet char + space). Tránh strip nhầm với text thường.
    let mut clean = text.trim_start().to_string();
    let is_list_parent = parent.num_id.is_some()
        || parent.num_ilvl.is_some()
        || parent
            .para_style
            .as_deref()
            .map(|s| s.starts_with("List"))
            .unwrap_or(false);
    if is_list_parent {
        let chars: Vec<char> = clean.chars().collect();
        // Pattern 1: "N.M.K. " (digits + dots + trailing space)
        // Pattern 2: "1 " (digit + space — python-docx style không có dot)
        // Pattern 3: "• " hoặc "▪ " hoặc "o " (bullet char + space)
        let mut consumed = 0usize;
        // Try pattern 1/2: digits + optional dots + space
        if !chars.is_empty() && chars[0].is_ascii_digit() {
            let mut i = 0;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // Phải có space NGAY sau số (đảm bảo là prefix, không phải text)
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
        if consumed > 0 {
            clean = chars[consumed..].iter().collect();
        }
    }

    s.push_str("<w:p>");
    // pPr: kế thừa các thuộc tính paragraph
    let has_ppr = parent.para_style.is_some()
        || parent.alignment.is_some()
        || parent.indent_twips.is_some()
        || parent.num_id.is_some()
        || parent.num_ilvl.is_some();
    if has_ppr {
        s.push_str("<w:pPr>");
        if let Some(style) = &parent.para_style {
            if style.starts_with("List") {
                write!(s, "<w:pStyle w:val=\"{}\"/>", xml_emit::xml_escape_attr(style)).ok();
            }
        }
        if let Some(num_id) = parent.num_id {
            let ilvl = parent.num_ilvl.unwrap_or(0);
            write!(
                s,
                "<w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>"
            )
            .ok();
        } else if let Some(ilvl) = parent.num_ilvl {
            write!(s, "<w:numPr><w:ilvl w:val=\"{ilvl}\"/></w:numPr>").ok();
        }
        if let Some(ind) = parent.indent_twips {
            if ind != 0 {
                write!(s, "<w:ind w:left=\"{ind}\"/>").ok();
            }
        }
        if let Some(jc) = &parent.alignment {
            write!(s, "<w:jc w:val=\"{}\"/>", xml_emit::xml_escape_attr(jc)).ok();
        }
        s.push_str("</w:pPr>");
    }
    // Run: nếu clean rỗng → paragraph trống. Nếu có text → emit run với
    // text content + rPr kế thừa từ first run của parent (font/bold/italic/
    // size/color/highlight) — đảm bảo dòng mới không bị default về VnTime
    // hay font khác lạ.
    if !clean.is_empty() {
        let rpr = build_first_run_rpr_inline(parent);
        if rpr.is_empty() {
            write!(
                s,
                "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                xml_emit::xml_escape_text(&clean)
            )
            .ok();
        } else {
            write!(
                s,
                "<w:r><w:rPr>{}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                rpr,
                xml_emit::xml_escape_text(&clean)
            )
            .ok();
        }
    }
    s.push_str("</w:p>");
    s
}

/// Build rPr inner XML từ first run (non-drawing) của parent paragraph.
/// Dùng cho insert paragraph mới — kế thừa font/bold/italic/size/color/
/// highlight để dòng mới không dùng Word default (vd VnTime).
fn build_first_run_rpr_inline(parent: &Paragraph) -> String {
    use std::fmt::Write as _;
    let first = match parent.runs.iter().find(|r| !r.is_drawing) {
        Some(r) => r,
        None => return String::new(),
    };
    let st = &first.style;
    if st.is_default() {
        return String::new();
    }
    let mut s = String::new();
    if let Some(font) = &st.font_name {
        write!(
            s,
            "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\"/>",
            f = xml_emit::xml_escape_attr(font)
        )
        .ok();
    }
    if st.bold {
        s.push_str("<w:b/>");
    }
    if st.italic {
        s.push_str("<w:i/>");
    }
    if let Some(sz) = st.size_half_pt {
        write!(s, "<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>").ok();
    }
    if let Some(color) = &st.color_hex {
        write!(s, "<w:color w:val=\"{}\"/>", xml_emit::xml_escape_attr(color)).ok();
    }
    if let Some(hl) = &st.highlight {
        write!(s, "<w:highlight w:val=\"{}\"/>", xml_emit::xml_escape_attr(hl)).ok();
    }
    s
}


#[derive(Debug)]
enum DiffOp {
    /// Dòng old được giữ, ứng với index trong new.
    Keep(usize),
    /// Dòng old bị xóa.
    Delete,
}

/// LCS-based diff. Trả về list ops cùng độ dài với old_lines.
/// Mỗi old[i] hoặc map sang new[j] (Keep) hoặc bị xóa (Delete).
/// Dòng new không được map sẽ là "inserted" — caller check riêng.
fn diff_lcs(old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    let m = old.len();
    let n = new.len();
    // Build LCS table
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in 0..m {
        for j in 0..n {
            if old[i] == new[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }
    // Backtrack to build ops on old
    let mut ops: Vec<DiffOp> = Vec::with_capacity(m);
    let mut i = m;
    let mut j = n;
    let mut tmp: Vec<DiffOp> = Vec::new();
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            tmp.push(DiffOp::Keep(j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            tmp.push(DiffOp::Delete);
            i -= 1;
        } else {
            // Insert in new — không tạo op trên old, chỉ giảm j.
            j -= 1;
        }
    }
    while i > 0 {
        tmp.push(DiffOp::Delete);
        i -= 1;
    }
    // tmp đang ngược, reverse
    tmp.reverse();
    ops.extend(tmp);

    // Strategy quan trọng cho use case này: LCS có thể "match" 1 dòng
    // text-edit (old_line khác new_line nhưng cùng position) bằng cách
    // mark Delete + Insert. Mình muốn nó là Keep + edit thay vì Delete +
    // Insert (vì user thường edit text in-place, không xóa rồi thêm
    // mới).
    //
    // Heuristic post-process: nếu có sequence (Delete at i, Insert at j)
    // với i+1 == ops sau hoặc gần nhau, GIỮ paragraph cùng vị trí và treat
    // là edit. Để đơn giản: scan ops, nếu pattern "Delete liên tiếp" mà
    // new cũng có dòng tương ứng (cùng index), CONVERT thành Keep.
    //
    // Cài đặt: nếu len(ops) == old.len() và new.len() == số Keep + số
    // Insert chưa map, mình map từng Delete với Insert kế tiếp ở cùng
    // vị trí relative. Bỏ qua optimize này nếu users muốn behavior LCS
    // thuần (mất ID khi text đổi lớn).
    //
    // Để giữ ID stable khi user chỉ edit text: pair Delete + Insert
    // ở cùng "relative position" → Keep.
    pair_delete_insert(&mut ops, m, n, old, new);

    ops
}

/// Convert "Delete tại old_i + Insert ở new_j gần nhất AND text similar"
/// thành Keep(j). Pair theo proximity + text similarity — đảm bảo edit
/// (Delete + Insert cùng vị trí, text gần giống) giữ ID, nhưng PURE
/// insert (text hoàn toàn khác) không bị nhầm.
fn pair_delete_insert(
    ops: &mut Vec<DiffOp>,
    _m: usize,
    n: usize,
    old_lines: &[&str],
    new_lines: &[&str],
) {
    let mapped: std::collections::HashSet<usize> = ops
        .iter()
        .filter_map(|op| match op {
            DiffOp::Keep(j) => Some(*j),
            _ => None,
        })
        .collect();
    let unmapped: Vec<usize> = (0..n).filter(|j| !mapped.contains(j)).collect();
    if unmapped.is_empty() {
        return;
    }

    let mut keep_count_before: Vec<usize> = Vec::with_capacity(ops.len());
    let mut cnt = 0;
    for op in ops.iter() {
        keep_count_before.push(cnt);
        if matches!(op, DiffOp::Keep(_)) {
            cnt += 1;
        }
    }

    let mut available: std::collections::BTreeSet<usize> = unmapped.iter().copied().collect();
    for (i, op) in ops.iter_mut().enumerate() {
        if !matches!(op, DiffOp::Delete) {
            continue;
        }
        let target = keep_count_before[i];
        let old_text = old_lines.get(i).copied().unwrap_or("");
        // Tìm candidate gần nhất trong available
        let lower = available.range(..=target).next_back().copied();
        let upper = available.range(target..).next().copied();
        // Score mỗi candidate = distance + (1 - similarity) * 10.
        // Chỉ pair candidate có score ≤ 2 (heuristic).
        let mut best: Option<(usize, f64)> = None;
        for cand_opt in [lower, upper].iter().copied() {
            if let Some(cand) = cand_opt {
                let new_text = new_lines.get(cand).copied().unwrap_or("");
                let dist = if cand >= target { cand - target } else { target - cand };
                let sim = text_similarity(old_text, new_text);
                // Score thấp = match tốt. Distance 0 + sim 1.0 = 0. Sim 0 + dist 0 = 10.
                let score = dist as f64 + (1.0 - sim) * 10.0;
                if best.map(|(_, s)| score < s).unwrap_or(true) {
                    best = Some((cand, score));
                }
            }
        }
        if let Some((j, score)) = best {
            // Pair chỉ khi score đủ thấp (heuristic). Distance=0 + sim=0.8
            // -> score 2.0 — pair. Distance=0 + sim=0.0 -> score 10.0 —
            // không pair.
            if score <= 3.0 {
                *op = DiffOp::Keep(j);
                available.remove(&j);
            }
        }
    }
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

fn apply_text_to_paragraph(p: &mut Paragraph, new_line: &str, old_line: &str) {
    // Nếu paragraph là TableCell: text đã được pre-extracted clean
    // (caller đã split bởi `│` và trim). Apply trực tiếp, không strip
    // prefix list/align.
    if matches!(p.context, ParaContext::TableCell { .. }) {
        let _ = old_line;
        apply_clean_text(p, new_line);
        return;
    }
    // Nếu paragraph có alignment center/right, render thêm spaces phía
    // trước cho căn. Trim leading whitespace để strip padding đó trước
    // khi extract content.
    let trimmed_line: &str = match p.alignment.as_deref() {
        Some("center") | Some("centre") | Some("right") => new_line.trim_start(),
        _ => new_line,
    };

    // Tính prefix CHÍNH XÁC bằng cách so sánh với old_line: prefix là
    // common prefix giữa old_line và những gì paragraph render. Đơn giản:
    // lấy mọi text content thực của paragraph cũ, tìm vị trí xuất hiện
    // trong old_line — phần trước đó = prefix.
    let original_text: String = p.runs.iter()
        .filter(|r| !r.is_drawing)
        .map(|r| r.text.as_str())
        .collect();
    let prefix_len = if !original_text.is_empty() {
        // Tìm vị trí original_text trong old_line (sau khi trim_start cho
        // align center/right).
        let old_trimmed = match p.alignment.as_deref() {
            Some("center") | Some("centre") | Some("right") => old_line.trim_start(),
            _ => old_line,
        };
        match old_trimmed.find(original_text.as_str()) {
            Some(idx) => old_trimmed[..idx].chars().count(),
            None => render_prefix_len(p),
        }
    } else {
        render_prefix_len(p)
    };
    let raw_content: String = trimmed_line.chars().skip(prefix_len).collect();
    // Strip border chars (table render artifacts) khỏi text content.
    let new_content = strip_table_border_chars(&raw_content);

    // Nếu paragraph cũ có text chứa border chars (corrupted từ render
    // cũ), KHÔNG apply edit text mới — giữ XML gốc nguyên để tránh
    // re-corrupt. User cần dùng tool ngoài để clean trước khi edit
    // dòng đó.
    if original_text.chars().any(|c| matches!(c, '│' | '─' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼')) {
        return;
    }

    if p.runs.is_empty() {
        if !new_content.is_empty() {
            p.runs.push(Run {
                text: new_content,
                style: RunStyle::default(),
                is_drawing: false,
                rel_id: None,
                byte_range: None,
        media_name: None,
            drawing_xml: None,
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

/// Phải khớp với prefix length trong render.rs.
/// Lưu ý: render dùng ordinal động (2.3.1.), nên prefix length CŨNG động.
/// Để khớp, mình tính dựa trên cấu trúc paragraph TRONG SOURCE và ilvl
/// (giả định ordinal có pattern N.M.K. với N chữ số 1-9, tách bằng '.').
/// Cách tính: char_count = (ilvl + 1) * 2 (mỗi level góp "N." nhập 2
/// chars, plus 1 space cuối) — gần đúng cho số 1 chữ số. Để CHÍNH XÁC,
/// mình re-compute ordinal khi cần. Tạm dùng heuristic + verify lại bằng
/// cách lấy prefix thật từ render trong apply_buffer_to_document.
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
    let indent_len = indent_level * 2;
    // List item (có numbering source nào đó) → ordinal-based length.
    // Mọi list source: inline numPr (num_id), inline ilvl alone, hoặc
    // style-based numbering (ListNumber, ListBullet).
    let is_numbered_list = p.num_id.is_some()
        || p.num_ilvl.is_some()
        || p
            .para_style
            .as_deref()
            .map(|s| s.starts_with("ListNumber"))
            .unwrap_or(false);
    let is_bullet = p
        .para_style
        .as_deref()
        .map(|s| s.starts_with("ListBullet"))
        .unwrap_or(false);
    let style_len = if is_numbered_list && !is_bullet {
        // Heuristic gần đúng (numbering.xml template có thể có "•", "-",
        // "1.1.", v.v. — không biết exact length từ đây). Dùng worst case
        // (ilvl+1)*2 + 1 = "N.M.K. " hoặc tương đương.
        let ilvl = p.num_ilvl.unwrap_or(0) as usize;
        (ilvl + 1) * 2 + 1
    } else if is_bullet {
        2 // "• "
    } else {
        match p.para_style.as_deref() {
            Some(s) if s == "Heading1" || s == "Title" => 2,
            Some("Heading2") => 3,
            Some("Heading3") => 4,
            Some("Heading4") => 5,
            Some("Heading5") => 6,
            Some("Heading6") => 7,
            _ => 0,
        }
    };
    // Alignment padding (center/right) — phải khớp với render.rs.
    // Để tránh tính lại text_len chính xác (cần access runs), dùng
    // heuristic: trim leading whitespace của new_line khi apply text
    // (xem apply_text_to_paragraph).
    context_len + indent_len + style_len
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
