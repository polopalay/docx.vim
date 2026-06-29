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
mod numbering;
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
        "create" => {
            // Tạo file DOCX trống (template tối thiểu) tại path. Dùng
            // cho file mới hoàn toàn — Vim BufReadCmd phát hiện file
            // không tồn tại hoặc không phải zip valid → gọi create
            // trước khi open.
            let path = require_arg(args, 2, "docx_file")?;
            cmd_create(path)?;
            emit_open_output(path)
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
            // Chèn 1 paragraph rỗng SAU (mặc định) hoặc TRƯỚC paragraph
            // có id = para_id. Position: "after" (default) | "before".
            // Paragraph mới kế thừa numId/ilvl/pStyle/alignment + style
            // run đầu (bold/italic/font/color) của source. Dùng cho `o` /
            // `O` mappings trong Vim.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            let position = args.get(4).map(|s| s.as_str()).unwrap_or("after");
            cmd_listadd(path, para_id, position)
        }
        "listdel" => {
            // Xoá paragraph có id = para_id. Dùng cho `dd` trên dòng list
            // hoặc khi user xoá dòng nào đó.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_listdel(path, para_id)
        }
        "listexit" => {
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_listexit(path, para_id)
        }
        "extract" => {
            // Extract media (image/OLE object) của paragraph có id =
            // para_id ra /tmp/. Print path file đã extract qua stdout
            // để Vim biết mở app nào. Nếu paragraph có nhiều media,
            // extract tất cả và print mỗi path 1 dòng.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            cmd_extract(path, para_id)
        }
        _ => Err(AppError(format!("Unknown command: {cmd}"))),
    }
}

fn require_arg<'a>(args: &'a [String], idx: usize, name: &str) -> AppResult<&'a str> {
    args.get(idx)
        .map(|s| s.as_str())
        .ok_or_else(|| AppError(format!("Missing argument: {name}")))
}

/// Helper: load numbering.xml từ zip và parse -> map (numId, ilvl) ->
/// LvlTemplate. Trả về empty map nếu file không có numbering.xml hoặc
/// parse fail (DOCX không có list cũng OK).
fn load_numbering<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> std::collections::HashMap<(u32, u32), numbering::LvlTemplate> {
    match zip_io::read_entry(archive, "word/numbering.xml") {
        Ok(bytes) => numbering::parse_numbering(&bytes).unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// Tạo file DOCX trống tại path. Template minimal — đủ để Word/LibreOffice
/// mở được và plugin có thể parse/edit. Gọi khi user mở file .docx chưa
/// tồn tại hoặc file không phải DOCX hợp lệ (vd plain text với extension
/// .docx).
fn cmd_create(path: &str) -> AppResult<()> {
    use std::io::Write;
    use zip::write::FileOptions;
    use zip::CompressionMethod;

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p/>
<w:sectPr/>
</w:body>
</w:document>"#;

    let file = std::fs::File::create(path).map_err(|e| AppError(format!("Create file: {e}")))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = FileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", opts).map_err(|e| AppError(format!("Zip: {e}")))?;
    zip.write_all(content_types.as_bytes()).map_err(|e| AppError(format!("Write: {e}")))?;
    zip.start_file("_rels/.rels", opts).map_err(|e| AppError(format!("Zip: {e}")))?;
    zip.write_all(rels.as_bytes()).map_err(|e| AppError(format!("Write: {e}")))?;
    zip.start_file("word/document.xml", opts).map_err(|e| AppError(format!("Zip: {e}")))?;
    zip.write_all(document.as_bytes()).map_err(|e| AppError(format!("Write: {e}")))?;
    zip.finish().map_err(|e| AppError(format!("Zip finish: {e}")))?;
    Ok(())
}

fn cmd_open(path: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;
    let numbering = load_numbering(&mut archive);
    let r = render::render(&doc, &numbering);
    println!("{}", r.text);
    println!("@@STYLE@@");
    for (line, cs, ce, bold, italic, size_pt, color, font, highlight) in &r.style_meta {
        let size_str = size_pt.map(|s| format!("{s}")).unwrap_or_else(|| "-".to_string());
        let color_str = color.as_deref().unwrap_or("-");
        let font_str = font.as_deref().unwrap_or("-");
        let hl_str = highlight.as_deref().unwrap_or("-");
        println!(
            "{line}\t{cs}\t{ce}\t{b}\t{i}\t{size_str}\t{color_str}\t{font_str}\t{hl_str}",
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
    println!("@@CELLMAP@@");
    for (line, col, cs, ce, pid) in &r.cell_map {
        println!("{line}\t{col}\t{cs}\t{ce}\t{pid}");
    }
    println!("@@CELLMAPEND@@");
    Ok(())
}

fn cmd_save(path: &str, tmp_path: &str) -> AppResult<()> {
    let new_text = std::fs::read_to_string(tmp_path)?;
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;
    let numbering = load_numbering(&mut archive);
    let new_xml = save::apply_buffer_to_document(&doc, &new_text, &numbering)?;
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    emit_open_output(path)
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
    // Emit luôn output 'open' để Vim không phải spawn binary lần 2.
    // Để tránh re-parse XML mới, mình render từ `doc` (đã có sẵn trong RAM).
    // Tuy nhiên byte_range của doc cũ vẫn map sang XML CŨ — vì doc đã được
    // mutate. Bypass: re-open file mới để có byte_range mới (đảm bảo
    // paramap chính xác).
    emit_open_output(path)
}

/// Helper: re-open file đã save, render và emit metadata blocks tương tự
/// cmd_open. Tránh duplicate code cho mọi command có "save-then-open".
fn emit_open_output(path: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;
    let numbering = load_numbering(&mut archive);
    let r = render::render(&doc, &numbering);
    println!("{}", r.text);
    println!("@@STYLE@@");
    for (line, cs, ce, bold, italic, size_pt, color, font, highlight) in &r.style_meta {
        let size_str = size_pt.map(|s| format!("{s}")).unwrap_or_else(|| "-".to_string());
        let color_str = color.as_deref().unwrap_or("-");
        let font_str = font.as_deref().unwrap_or("-");
        let hl_str = highlight.as_deref().unwrap_or("-");
        println!(
            "{line}\t{cs}\t{ce}\t{b}\t{i}\t{size_str}\t{color_str}\t{font_str}\t{hl_str}",
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
    println!("@@CELLMAP@@");
    for (line, col, cs, ce, pid) in &r.cell_map {
        println!("{line}\t{col}\t{cs}\t{ce}\t{pid}");
    }
    println!("@@CELLMAPEND@@");
    Ok(())
}

// ---------- STRUCTURE EDIT COMMANDS ----------
//
// 3 lệnh này thay đổi SỐ paragraph trong document — khác setstyle/save
// vốn chỉ sửa nội dung paragraph hiện có. Cách thực hiện: chèn/xoá byte
// thẳng trong document.xml gốc, KHÔNG re-emit toàn bộ — vẫn giữ
// minimal-diff cho mọi paragraph khác.

fn cmd_listadd(path: &str, para_id: &str, position: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;

    // Tìm paragraph source
    let src = doc
        .paragraphs
        .iter()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Build pPr inner cho paragraph mới — kế thừa MỌI thuộc tính paragraph
    // từ source (numPr, indent, alignment, pStyle nếu là list style):
    let mut ppr_inner = String::new();
    // pStyle: kế thừa cho list style HOẶC heading (để dòng mới giữ cùng style)
    if let Some(style) = &src.para_style {
        if style.starts_with("List") || style.starts_with("Heading") || style == "Title" {
            ppr_inner.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
        }
    }
    // numPr: kế thừa cho list item (inline numbering)
    if let Some(num_id) = src.num_id {
        let ilvl = src.num_ilvl.unwrap_or(0);
        ppr_inner.push_str(&format!(
            "<w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>"
        ));
    } else if let Some(ilvl) = src.num_ilvl {
        ppr_inner.push_str(&format!("<w:numPr><w:ilvl w:val=\"{ilvl}\"/></w:numPr>"));
    }
    // indent: kế thừa nếu source có
    if let Some(ind) = src.indent_twips {
        if ind != 0 {
            ppr_inner.push_str(&format!("<w:ind w:left=\"{ind}\"/>"));
        }
    }
    // alignment: kế thừa căn lề
    if let Some(jc) = &src.alignment {
        ppr_inner.push_str(&format!("<w:jc w:val=\"{jc}\"/>"));
    }

    // Kế thừa style của FIRST RUN (bold/italic/font/color/size/highlight)
    // cho paragraph mới — nếu dòng cũ in đậm thì dòng mới cũng đậm.
    let rpr_inner = build_first_run_rpr(src);
    let run_xml = if rpr_inner.is_empty() {
        String::new()
    } else {
        format!("<w:r><w:rPr>{rpr_inner}</w:rPr></w:r>")
    };

    let new_para_xml = if ppr_inner.is_empty() && run_xml.is_empty() {
        "<w:p/>".to_string()
    } else if run_xml.is_empty() {
        format!("<w:p><w:pPr>{ppr_inner}</w:pPr></w:p>")
    } else if ppr_inner.is_empty() {
        format!("<w:p>{run_xml}</w:p>")
    } else {
        format!("<w:p><w:pPr>{ppr_inner}</w:pPr>{run_xml}</w:p>")
    };

    // Chèn vào XML gốc: TRƯỚC <w:p> của source (position=before) hoặc
    // SAU </w:p> của source (position=after, mặc định).
    let insert_pos = match position {
        "before" => src.byte_range.0,
        _ => src.byte_range.1,
    };
    let mut new_xml: Vec<u8> = Vec::with_capacity(xml.len() + new_para_xml.len());
    new_xml.extend_from_slice(&xml[..insert_pos]);
    new_xml.extend_from_slice(new_para_xml.as_bytes());
    new_xml.extend_from_slice(&xml[insert_pos..]);

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    emit_open_output(path)
}

/// Build rPr nội dung XML từ style của run đầu tiên của paragraph nguồn.
/// Dùng cho listadd để dòng mới kế thừa font/bold/italic/color.
fn build_first_run_rpr(src: &model::Paragraph) -> String {
    let first = src.runs.iter().find(|r| !r.is_drawing);
    let first = match first {
        Some(r) if !r.style.is_default() => r,
        _ => return String::new(),
    };
    let mut s = String::new();
    if let Some(font) = &first.style.font_name {
        s.push_str(&format!(
            "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\"/>",
            f = font
        ));
    }
    if first.style.bold {
        s.push_str("<w:b/>");
    }
    if first.style.italic {
        s.push_str("<w:i/>");
    }
    if let Some(sz) = first.style.size_half_pt {
        s.push_str(&format!("<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>"));
    }
    if let Some(color) = &first.style.color_hex {
        s.push_str(&format!("<w:color w:val=\"{color}\"/>"));
    }
    if let Some(hl) = &first.style.highlight {
        s.push_str(&format!("<w:highlight w:val=\"{hl}\"/>"));
    }
    s
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
    emit_open_output(path)
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
    emit_open_output(path)
}

fn cmd_extract(path: &str, para_id: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc = parser::parse_document(&xml)?;

    let p = doc
        .paragraphs
        .iter()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Đọc document.xml.rels để map rel_id -> target file
    let rels_xml = zip_io::read_entry(&mut archive, "word/_rels/document.xml.rels")?;
    let rel_map = parse_rels(&rels_xml);

    let mut found_any = false;
    for run in &p.runs {
        if !run.is_drawing {
            continue;
        }
        let rel_id = match &run.rel_id {
            Some(r) => r,
            None => continue,
        };
        let target = match rel_map.get(rel_id.as_str()) {
            Some(t) => t.clone(),
            None => continue,
        };
        // Target có dạng "media/image1.png" — đường dẫn relative tới
        // word/. Trong zip thì entry là "word/media/image1.png".
        let entry_path = if target.starts_with('/') {
            // Absolute trong zip
            target.trim_start_matches('/').to_string()
        } else {
            format!("word/{target}")
        };
        // Extract bytes
        let bytes = match zip_io::read_entry(&mut archive, &entry_path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Lấy filename cuối + write ra /tmp/docx_<para_id>_<filename>
        let filename = entry_path
            .rsplit('/')
            .next()
            .unwrap_or("attachment.bin");
        let tmp_dir = std::env::temp_dir();
        let out_path = tmp_dir.join(format!("docx_{para_id}_{filename}"));
        std::fs::write(&out_path, &bytes)?;
        // Print path để Vim đọc
        println!("{}", out_path.display());
        found_any = true;
    }

    if !found_any {
        return Err(AppError(format!(
            "Paragraph {para_id} has no extractable media (image/object)"
        )));
    }
    Ok(())
}

/// Parse document.xml.rels — XML đơn giản dạng:
///   <Relationships xmlns="...">
///     <Relationship Id="rId4" Type="..." Target="media/image1.png"/>
///     ...
///   </Relationships>
/// Trả về HashMap rel_id -> target.
fn parse_rels(xml: &[u8]) -> std::collections::HashMap<String, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut map = std::collections::HashMap::new();
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                // local name = "Relationship"
                let name = e.name();
                let local = match name.as_ref().iter().position(|&b| b == b':') {
                    Some(i) => &name.as_ref()[i + 1..],
                    None => name.as_ref(),
                };
                if local == b"Relationship" {
                    let mut id = None;
                    let mut target = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"Id" => {
                                id = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                            b"Target" => {
                                target = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                            _ => {}
                        }
                    }
                    if let (Some(i), Some(t)) = (id, target) {
                        map.insert(i, t);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    map
}

fn apply_style_attr(p: &mut model::Paragraph, attr: &str, value: &str) -> AppResult<()> {
    match attr {
        "togglebold" => {
            if p.runs.is_empty() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle { bold: true, ..Default::default() },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                let any_not_bold = p.runs.iter().any(|r| !r.style.bold);
                for r in p.runs.iter_mut() {
                    r.style.bold = any_not_bold;
                }
            }
        }
        "toggleitalic" => {
            if p.runs.is_empty() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle { italic: true, ..Default::default() },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                let any_not_italic = p.runs.iter().any(|r| !r.style.italic);
                for r in p.runs.iter_mut() {
                    r.style.italic = any_not_italic;
                }
            }
        }
        "size" => {
            let pt: f32 = value
                .parse()
                .map_err(|_| AppError(format!("Invalid size: {value}")))?;
            let half = (pt * 2.0).round() as u32;
            if p.runs.is_empty() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle { size_half_pt: Some(half), ..Default::default() },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                for r in p.runs.iter_mut() {
                    r.style.size_half_pt = Some(half);
                }
            }
        }
        "color" => {
            let v = parse_color(value)?;
            if p.runs.is_empty() && v.is_some() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle { color_hex: v.clone(), ..Default::default() },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                for r in p.runs.iter_mut() {
                    r.style.color_hex = v.clone();
                }
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
            if p.runs.is_empty() && v.is_some() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle { highlight: v.clone(), ..Default::default() },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                for r in p.runs.iter_mut() {
                    r.style.highlight = v.clone();
                }
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
            // Nếu paragraph KHÔNG có run nào (empty paragraph, vd dòng
            // mới insert chưa gõ text), tạo placeholder run với font
            // mới — để emit_paragraph có chỗ gắn rFonts. Đảm bảo
            // Word render với font đúng kể cả khi paragraph empty.
            if p.runs.is_empty() && opt.is_some() {
                p.runs.push(model::Run {
                    text: String::new(),
                    style: model::RunStyle {
                        font_name: opt.clone(),
                        ..Default::default()
                    },
                    is_drawing: false,
                    rel_id: None,
                    byte_range: None,
                });
            } else {
                for r in p.runs.iter_mut() {
                    r.style.font_name = opt.clone();
                }
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
