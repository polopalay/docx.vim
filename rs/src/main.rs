//! Binary CLI cho DOCX plugin. Subcommands tương tự excel_rs:
//!   docx open <file.docx>           -> in bảng text + metadata @@STYLE@@ @@PARAMAP@@
//!   docx save <file.docx> <tmp.txt> -> lưu lại file.docx từ tmp.txt
//!   docx setstyle <file.docx> <para_id> <bold|italic|size|color> <value>
//!     -> đổi style của TOÀN BỘ runs trong 1 paragraph (vd. ":ExcelBold")
//!
//! save logic làm minimal-diff: parse text mới, so sánh với paragraph gốc;
//! paragraph nào identical -> giữ nguyên byte; paragraph nào khác -> rebuild
//! XML mới chỉ cho paragraph đó.

mod error;
mod model;
mod parser;
mod render;
mod save;
mod xml_emit;
mod zip_io;

use error::{AppError, AppResult};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: docx <command> [args...]");
        eprintln!("Commands: open, save, setstyle");
        return ExitCode::from(1);
    }
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn dispatch(args: &[String]) -> AppResult<()> {
    let cmd = args[1].as_str();
    match cmd {
        "open" => {
            let path = require_arg(args, 2, "docx_file")?;
            cmd_open(path)
        }
        "save" => {
            let path = require_arg(args, 2, "docx_file")?;
            let tmp = require_arg(args, 3, "tmp_file")?;
            cmd_save(path, tmp)
        }
        "setstyle" => {
            let path = require_arg(args, 2, "docx_file")?;
            let para_ids = require_arg(args, 3, "para_ids")?;
            let attr = require_arg(args, 4, "attr")?;
            let value = require_arg(args, 5, "value")?;
            cmd_setstyle(path, para_ids, attr, value)
        }
        "listadd" => {
            // Chèn 1 paragraph rỗng NGAY SAU paragraph có id = para_id.
            // Paragraph mới kế thừa numId + ilvl từ source nếu source là
            // list item. Dùng cho `o` trên list item.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_listadd(path, para_id)
        }
        "listdel" => {
            // Xoá paragraph có id = para_id. Dùng cho `dd` trên dòng list
            // hoặc khi user xoá dòng nào đó.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_listdel(path, para_id)
        }
        "listexit" => {
            // Thoát khỏi list: xoá num_id + num_ilvl của paragraph. Dùng
            // khi user Enter trên list item rỗng đã ở ilvl=0.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_listexit(path, para_id)
        }
        _ => Err(AppError(format!("Unknown command: {cmd}"))),
    }
}

fn require_arg<'a>(args: &'a [String], idx: usize, name: &str) -> AppResult<&'a str> {
    args.get(idx)
        .map(|s| s.as_str())
        .ok_or_else(|| AppError(format!("Missing argument: {name}")))
}

fn cmd_open(path: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;
    let r = render::render(&doc);
    println!("{}", r.text);
    println!("@@STYLE@@");
    for (line, cs, ce, bold, italic, size_pt, color, font) in &r.style_meta {
        let size_str = size_pt.map(|s| format!("{s}")).unwrap_or_else(|| "-".to_string());
        let color_str = color.as_deref().unwrap_or("-");
        let font_str = font.as_deref().unwrap_or("-");
        println!(
            "{line}\t{cs}\t{ce}\t{b}\t{i}\t{size_str}\t{color_str}\t{font_str}",
            b = *bold as u8,
            i = *italic as u8,
        );
    }
    println!("@@END@@");
    println!("@@PARAMAP@@");
    for (line, pid) in &r.para_map {
        println!("{line}\t{pid}");
    }
    println!("@@PARAMAPEND@@");
    println!("@@LISTINFO@@");
    for (line, ilvl) in &r.list_info {
        println!("{line}\t{ilvl}");
    }
    println!("@@LISTINFOEND@@");
    Ok(())
}

fn cmd_save(path: &str, tmp_path: &str) -> AppResult<()> {
    let new_text = std::fs::read_to_string(tmp_path)?;
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;
    let new_xml = save::apply_buffer_to_document(&doc, &new_text)?;
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    Ok(())
}

fn cmd_setstyle(path: &str, para_ids: &str, attr: &str, value: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let mut doc = parser::parse_document(&xml)?;

    // Comma-separated list of paragraph IDs (giống Excel "B2,C3,D5").
    let ids: Vec<&str> = para_ids.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let mut hit = 0;
    for id in ids {
        for p in doc.paragraphs.iter_mut() {
            if p.id == id {
                apply_style_attr(p, attr, value)?;
                p.dirty = true;
                hit += 1;
                break;
            }
        }
    }
    if hit == 0 {
        return Err(AppError(format!("No paragraphs matched: {para_ids}")));
    }

    let new_xml = save::rebuild_document_xml(&doc)?;
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    Ok(())
}

// ---------- STRUCTURE EDIT COMMANDS ----------
//
// 3 lệnh này thay đổi SỐ paragraph trong document — khác setstyle/save
// vốn chỉ sửa nội dung paragraph hiện có. Cách thực hiện: chèn/xoá byte
// thẳng trong document.xml gốc, KHÔNG re-emit toàn bộ — vẫn giữ
// minimal-diff cho mọi paragraph khác.

fn cmd_listadd(path: &str, para_id: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;

    // Tìm paragraph source
    let src = doc
        .paragraphs
        .iter()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Build XML của paragraph mới: nếu source là list item, kế thừa numId
    // + ilvl; nếu không, paragraph thường rỗng.
    let new_para_xml = if let Some(num_id) = src.num_id {
        let ilvl = src.num_ilvl.unwrap_or(0);
        format!(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr></w:p>"
        )
    } else {
        // Paragraph thường rỗng. Giữ alignment + indent nếu có (tiện cho
        // user khi `o` trên paragraph thường — dòng mới cùng format).
        let mut ppr_inner = String::new();
        if let Some(ind) = src.indent_twips {
            if ind != 0 {
                ppr_inner.push_str(&format!("<w:ind w:left=\"{ind}\"/>"));
            }
        }
        if let Some(jc) = &src.alignment {
            ppr_inner.push_str(&format!("<w:jc w:val=\"{jc}\"/>"));
        }
        if ppr_inner.is_empty() {
            "<w:p/>".to_string()
        } else {
            format!("<w:p><w:pPr>{ppr_inner}</w:pPr></w:p>")
        }
    };

    // Chèn vào XML gốc ngay SAU </w:p> của source paragraph.
    let mut new_xml: Vec<u8> = Vec::with_capacity(xml.len() + new_para_xml.len());
    new_xml.extend_from_slice(&xml[..src.byte_range.1]);
    new_xml.extend_from_slice(new_para_xml.as_bytes());
    new_xml.extend_from_slice(&xml[src.byte_range.1..]);

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    Ok(())
}

fn cmd_listdel(path: &str, para_id: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;

    let target = doc
        .paragraphs
        .iter()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Xoá đoạn byte_range của paragraph khỏi XML gốc.
    let mut new_xml: Vec<u8> = Vec::with_capacity(xml.len());
    new_xml.extend_from_slice(&xml[..target.byte_range.0]);
    new_xml.extend_from_slice(&xml[target.byte_range.1..]);

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    Ok(())
}

fn cmd_listexit(path: &str, para_id: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let mut doc = parser::parse_document(&xml)?;

    let p_idx = doc
        .paragraphs
        .iter()
        .position(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Thoát list: xoá numId + ilvl. Paragraph thành paragraph thường.
    doc.paragraphs[p_idx].num_id = None;
    doc.paragraphs[p_idx].num_ilvl = None;
    doc.paragraphs[p_idx].dirty = true;

    let new_xml = save::rebuild_document_xml(&doc)?;
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    Ok(())
}

fn apply_style_attr(p: &mut model::Paragraph, attr: &str, value: &str) -> AppResult<()> {
    match attr {
        "togglebold" => {
            let any_not_bold = p.runs.iter().any(|r| !r.style.bold);
            for r in p.runs.iter_mut() {
                r.style.bold = any_not_bold;
            }
        }
        "toggleitalic" => {
            let any_not_italic = p.runs.iter().any(|r| !r.style.italic);
            for r in p.runs.iter_mut() {
                r.style.italic = any_not_italic;
            }
        }
        "size" => {
            let pt: f32 = value
                .parse()
                .map_err(|_| AppError(format!("Invalid size: {value}")))?;
            let half = (pt * 2.0).round() as u32;
            for r in p.runs.iter_mut() {
                r.style.size_half_pt = Some(half);
            }
        }
        "color" => {
            let v = parse_color(value)?;
            for r in p.runs.iter_mut() {
                r.style.color_hex = v.clone();
            }
        }
        "highlight" => {
            // Highlight value: tên màu chuẩn ("yellow", "green", "cyan",
            // "magenta", "blue", "red", "darkBlue", "darkCyan", "darkGreen",
            // "darkMagenta", "darkRed", "darkYellow", "darkGray",
            // "lightGray", "black", "white", "none"). DOCX nhận chính các
            // tên này cho <w:highlight>.
            let lower = value.trim().to_lowercase();
            let v = if lower == "none" {
                None
            } else {
                Some(value.trim().to_string())
            };
            for r in p.runs.iter_mut() {
                r.style.highlight = v.clone();
            }
        }
        "font" => {
            // Font name — bất kỳ string nào, vd "Times New Roman",
            // "Roboto", "Arial". Word sẽ render bằng font đó nếu hệ thống
            // có cài, hoặc fallback.
            let v = value.trim();
            let opt = if v.is_empty() || v.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(v.to_string())
            };
            for r in p.runs.iter_mut() {
                r.style.font_name = opt.clone();
            }
        }
        "indent" => {
            // Indent: value là số nguyên cấp (-N hoặc +N), tăng/giảm theo
            // bậc 720 twips (0.5 inch). Vd "indent +1" -> tăng 1 cấp;
            // "indent -1" -> giảm 1 cấp; "indent 0" -> reset về 0;
            // "indent 2" (không dấu) -> set thẳng 2 cấp.
            let trimmed = value.trim();
            let current_level = p
                .indent_twips
                .map(|t| (t.max(0) / 720) as i32)
                .unwrap_or(0);
            let new_level: i32 = if let Some(stripped) = trimmed.strip_prefix('+') {
                let delta: i32 = stripped
                    .parse()
                    .map_err(|_| AppError(format!("Invalid indent: {value}")))?;
                current_level + delta
            } else if trimmed.starts_with('-') {
                let delta: i32 = trimmed
                    .parse()
                    .map_err(|_| AppError(format!("Invalid indent: {value}")))?;
                current_level + delta // delta âm sẵn
            } else {
                trimmed
                    .parse()
                    .map_err(|_| AppError(format!("Invalid indent: {value}")))?
            };
            let new_level = new_level.max(0); // không cho âm
            p.indent_twips = if new_level == 0 {
                None
            } else {
                Some(new_level * 720)
            };
        }
        "align" => {
            // Alignment: left/center/right/both (= justify).
            let v = value.trim().to_lowercase();
            let valid = match v.as_str() {
                "left" => Some("left"),
                "center" | "centre" => Some("center"),
                "right" => Some("right"),
                "both" | "justify" => Some("both"),
                "none" => None,
                _ => return Err(AppError(format!("Invalid align: {value}"))),
            };
            p.alignment = valid.map(|s| s.to_string());
        }
        "ilvl" => {
            // Đổi level list (1.1 -> 1.1.1 khi +1; 1.1.1 -> 1.1 khi -1).
            // Paragraph phải là list item — hoặc có num_id (inline numPr),
            // hoặc có para_style bắt đầu bằng "List".
            let is_list = p.num_id.is_some()
                || p.para_style
                    .as_deref()
                    .map(|s| s.starts_with("List"))
                    .unwrap_or(false);
            if !is_list {
                return Err(AppError(
                    "Paragraph is not a list item — use :DocxIndent instead".to_string(),
                ));
            }
            let trimmed = value.trim();
            let current: i32 = p.num_ilvl.unwrap_or(0) as i32;
            let new_level: i32 = if let Some(stripped) = trimmed.strip_prefix('+') {
                let delta: i32 = stripped
                    .parse()
                    .map_err(|_| AppError(format!("Invalid ilvl: {value}")))?;
                current + delta
            } else if trimmed.starts_with('-') {
                let delta: i32 = trimmed
                    .parse()
                    .map_err(|_| AppError(format!("Invalid ilvl: {value}")))?;
                current + delta
            } else {
                trimmed
                    .parse()
                    .map_err(|_| AppError(format!("Invalid ilvl: {value}")))?
            };
            let clamped = new_level.clamp(0, 8) as u32;
            p.num_ilvl = Some(clamped);
        }
        _ => return Err(AppError(format!("Unknown style attr: {attr}"))),
    }
    Ok(())
}

fn parse_color(s: &str) -> AppResult<Option<String>> {
    let lower = s.trim().to_lowercase();
    if lower == "none" || lower == "auto" {
        return Ok(None);
    }
    let hex = match lower.as_str() {
        "red" => "FF0000",
        "green" => "00B050",
        "blue" => "0070C0",
        "yellow" => "FFFF00",
        "orange" => "FFA500",
        "purple" => "800080",
        "gray" | "grey" => "808080",
        "white" => "FFFFFF",
        "black" => "000000",
        _ => {
            let stripped = lower.trim_start_matches('#');
            if stripped.len() == 6 && stripped.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(Some(stripped.to_uppercase()));
            }
            return Err(AppError(format!("Invalid color: {s}")));
        }
    };
    Ok(Some(hex.to_string()))
}
