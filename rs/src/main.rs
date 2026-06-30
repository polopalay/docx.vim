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
        "listzip" => {
            // Liệt kê toàn bộ entry (đường dẫn trong zip) của file DOCX.
            // Dùng cho :DocxListZip — user xem cấu trúc nội bộ + chọn file
            // để mở qua :DocxOpenFile (completion dùng output này).
            let path = require_arg(args, 2, "docx_file")?;
            cmd_listzip(path)
        }
        "extractentry" => {
            // Extract 1 entry CỤ THỂ (theo đường dẫn trong zip, vd
            // "word/media/image1.png") ra /tmp/, in path ra stdout.
            // Dùng cho :DocxOpenFile (mở file bất kỳ trong zip, không chỉ
            // media gắn với paragraph).
            let path = require_arg(args, 2, "docx_file")?;
            let entry_name = require_arg(args, 3, "entry_name")?;
            cmd_extract_entry(path, entry_name)
        }
        "upload" => {
            // Thêm 1 file từ hệ thống (ảnh hoặc Excel) vào word/media/
            // trong zip DOCX, đăng ký Content_Types nếu extension mới.
            // KHÔNG tự động chèn vào document (user tự insert sau qua
            // Word hoặc thủ công) — chỉ đưa file vào package.
            // In ra entry_name vừa thêm (vd "word/media/image5.png").
            let path = require_arg(args, 2, "docx_file")?;
            let src_path = require_arg(args, 3, "src_path")?;
            cmd_upload(path, src_path)
        }
        "uploadmany" => {
            // Upload NHIỀU file trong 1 lần mở zip (nhanh hơn nhiều so với
            // gọi 'upload' từng file khi có hàng trăm ảnh). list_file = file
            // text chứa mỗi dòng 1 đường dẫn nguồn. In mỗi dòng kết quả:
            // "OK\t<entry>" hoặc "SKIP\t<path>" hoặc "ERR\t<path>\t<lý do>".
            let path = require_arg(args, 2, "docx_file")?;
            let list_file = require_arg(args, 3, "list_file")?;
            cmd_uploadmany(path, list_file)
        }
        "insertimage" => {
            // Chèn 1 ảnh CÓ SẴN trong word/media/ vào paragraph có id =
            // para_id, dưới dạng inline drawing. width_cm = chiều rộng
            // mong muốn (cm); chiều cao tự co theo tỉ lệ gốc của ảnh.
            // Tạo relationship mới + sinh <w:drawing>, chèn vào paragraph,
            // ghi lại document.xml + rels. Emit output 'open' sau khi xong.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            let media_entry = require_arg(args, 4, "media_entry")?;
            let width_cm = require_arg(args, 5, "width_cm")?;
            cmd_insertimage(path, para_id, media_entry, width_cm)
        }
        "insertfile" => {
            // Chèn 1 reference tới file đính kèm (CÓ SẴN trong package) vào
            // paragraph para_id dưới dạng MARKER TEXT: "[📎 tên_file]".
            // Không đụng cấu trúc OLE phức tạp -> an toàn tuyệt đối. File
            // thật vẫn nằm trong package; gx/:DocxOpen trên dòng marker sẽ
            // trích tên file rồi mở. Dùng cho mọi loại file không phải ảnh.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            let file_name = require_arg(args, 4, "file_name")?;
            cmd_insertfile(path, para_id, file_name)
        }
        "insertole" => {
            // Chèn 1 OLE embedded object (CHỈ docx/xlsx/xlsm có sẵn trong
            // word/embeddings/) vào paragraph para_id, hiển thị dạng icon.
            // Dùng relationship type "package" (không cần CFBF wrapper vì
            // file là OOXML). Tạo icon PNG nếu chưa có. Ghi document.xml +
            // rels. Emit output 'open' sau khi xong.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            let embed_entry = require_arg(args, 4, "embed_entry")?;
            cmd_insertole(path, para_id, embed_entry)
        }
        "resizeimage" => {
            // Chỉnh kích thước 1 ảnh inline ĐÃ CHÈN trong paragraph para_id.
            // mode: "fit" (to ngang vùng nội dung trang), "width" (đặt chiều
            // rộng cm, cao tự co), "height" (đặt chiều cao cm, rộng tự co).
            // value: cm (bỏ qua khi mode=fit). Sửa cx/cy trong <wp:extent>/
            // <a:ext> của drawing, giữ tỉ lệ gốc của ảnh.
            let path = require_arg(args, 2, "docx_file")?;
            let para_id = require_arg(args, 3, "para_id")?;
            let mode = require_arg(args, 4, "mode")?;
            // value optional cho fit; lấy "" nếu thiếu.
            let value = args.get(5).map(|s| s.as_str()).unwrap_or("");
            cmd_resizeimage(path, para_id, mode, value)
        }
        "replaceimage" => {
            // Thay 1 ảnh trong word/media/ bằng file ảnh mới từ disk —
            // GHI ĐÈ bytes của entry, GIỮ NGUYÊN tên + document.xml. Mọi
            // reference tới ảnh đó tự hiển thị ảnh mới. An toàn tuyệt đối
            // (không đụng XML). media_entry = tên ảnh đích (vd image2.png),
            // src_path = đường dẫn ảnh mới trên hệ thống.
            let path = require_arg(args, 2, "docx_file")?;
            let media_entry = require_arg(args, 3, "media_entry")?;
            let src_path = require_arg(args, 4, "src_path")?;
            cmd_replaceimage(path, media_entry, src_path)
        }
        "deletefile" => {
            // Xóa 1 file trong package — CHỈ khi file "chưa được đính kèm"
            // (không có relationship nào trỏ tới + không có marker [📎] nào
            // nhắc tên trong document.xml). Từ chối nếu file đang được dùng
            // để tránh làm hỏng doc. entry = đường dẫn entry trong zip.
            let path = require_arg(args, 2, "docx_file")?;
            let entry = require_arg(args, 3, "entry")?;
            cmd_deletefile(path, entry)
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
    let mut doc = parser::parse_document(&xml)?;
    resolve_media_names(&mut doc, &mut archive);
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
    let mut doc = parser::parse_document(&xml)?;
    // QUAN TRỌNG: resolve media_name TRƯỚC khi apply_buffer_to_document.
    // Hàm save tự render lại doc gốc để so sánh diff với buffer text user
    // gõ; nếu thiếu bước này, render nội bộ sẽ in "[IMAGE]" (không tên)
    // còn buffer thực tế user thấy là "[IMAGE: ten.png]" -> lệch text ->
    // mọi paragraph có ảnh bị coi là "đã sửa" một cách giả (false dirty).
    resolve_media_names(&mut doc, &mut archive);
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
    let mut doc = parser::parse_document(&xml)?;
    resolve_media_names(&mut doc, &mut archive);
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

/// Liệt kê toàn bộ entry trong zip DOCX, in mỗi dòng "size\tname". Dùng
/// cho :DocxListZip. Size để user biết file nào đáng nhìn (vd ảnh lớn).
fn cmd_listzip(path: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let entries = zip_io::list_entries(&mut archive);
    for (name, size) in entries {
        println!("{size}\t{name}");
    }
    Ok(())
}

/// Extract 1 entry cụ thể (đường dẫn đầy đủ trong zip, vd
/// "word/media/image1.png") ra /tmp/, in path ra stdout. Dùng cho
/// :DocxOpenFile khi user chọn 1 file bất kỳ trong zip (không chỉ ảnh
/// gắn với 1 paragraph cụ thể như :DocxOpen).
fn cmd_extract_entry(path: &str, entry_name: &str) -> AppResult<()> {
    let mut archive = zip_io::open_archive(path)?;
    let bytes = zip_io::read_entry(&mut archive, entry_name)?;
    let tmp_dir = std::env::temp_dir();
    // Sanitize entry path thành tên file an toàn để tránh collision giữa
    // các entry cùng filename khác thư mục (hiếm nhưng có thể xảy ra).
    let safe_prefix = entry_name.replace(['/', '\\'], "_");
    let out_path = tmp_dir.join(format!("docx_{safe_prefix}"));
    std::fs::write(&out_path, &bytes)?;
    println!("{}", out_path.display());
    Ok(())
}

/// Đoán Content-Type cho 1 extension file. Chỉ hỗ trợ ảnh + Excel theo
/// yêu cầu upload (xlsx/xlsm/xls không cần Default Extension riêng vì
/// chuẩn OOXML coi chúng là package riêng — nhưng khi nhúng như file đính
/// kèm thô (oleObject/embedding) thì Word vẫn cần 1 content type generic;
/// dùng application/octet-stream an toàn cho mọi trường hợp không phải ảnh).
fn guess_content_type(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        // Ảnh
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        // Bảng tính
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "xls" => "application/vnd.ms-excel",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        // Tài liệu
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "odt" => "application/vnd.oasis.opendocument.text",
        "rtf" => "application/rtf",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "yaml" | "yml" => "text/yaml",
        "log" | "ini" | "cfg" | "conf" | "toml" | "env" | "properties" => "text/plain",
        "py" | "rb" | "rs" | "go" | "c" | "h" | "cpp" | "cc" | "java" | "sh" | "lua"
            | "sql" | "vim" | "ts" | "tsx" | "jsx" | "php" | "pl" => "text/plain",
        // Âm thanh
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        // Video
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "wmv" => "video/x-ms-wmv",
        // Nén
        "zip" => "application/zip",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        _ => "application/octet-stream",
    }
}

/// Phân loại file theo extension để quyết định thư mục đích trong zip.
/// Trả về (category, dest_dir) hoặc None nếu không hỗ trợ upload.
/// Quy ước thư mục (theo cách Word tổ chức embeddings):
///   - ảnh        -> word/media
///   - còn lại    -> word/embeddings (tài liệu, bảng tính, media, nén...)
fn classify_upload_ext(ext: &str) -> Option<&'static str> {
    let e = ext.to_ascii_lowercase();
    let is_image = matches!(
        e.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "svg"
    );
    let is_spreadsheet = matches!(e.as_str(), "xlsx" | "xlsm" | "xls" | "ods" | "csv" | "tsv");
    let is_document = matches!(
        e.as_str(),
        "docx" | "doc" | "odt" | "rtf" | "pdf" | "txt" | "md"
    );
    let is_audio = matches!(e.as_str(), "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac");
    let is_video = matches!(
        e.as_str(),
        "mp4" | "mov" | "avi" | "mkv" | "webm" | "wmv"
    );
    let is_archive = matches!(e.as_str(), "zip" | "rar" | "7z" | "gz" | "tar");
    // Text/code: rất rộng — mọi định dạng văn bản thuần hay mở trong Vim.
    let is_text = matches!(
        e.as_str(),
        "log" | "json" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf"
            | "xml" | "html" | "htm" | "css" | "scss" | "less"
            | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
            | "py" | "rb" | "rs" | "go" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh"
            | "java" | "kt" | "kts" | "swift" | "mm" | "php" | "pl" | "pm"
            | "sh" | "bash" | "zsh" | "fish" | "vim" | "lua" | "sql" | "r" | "jl"
            | "dart" | "scala" | "clj" | "ex" | "exs" | "erl" | "hs" | "ml" | "mli"
            | "fs" | "vb" | "cs" | "gradle" | "properties" | "env"
            | "tex" | "rst" | "adoc" | "org" | "bat" | "ps1" | "ipynb" | "diff" | "patch"
    );

    if is_image {
        Some("word/media")
    } else if is_spreadsheet || is_document || is_audio || is_video || is_archive || is_text {
        Some("word/embeddings")
    } else {
        None
    }
}

/// Upload 1 file từ hệ thống vào trong package DOCX. Hỗ trợ nhiều loại:
/// ảnh (-> word/media), còn lại như bảng tính/tài liệu/âm thanh/video/
/// nén (-> word/embeddings). Không sửa document.xml — file chỉ được "đưa
/// vào package" (dùng :DocxOpenFile để mở lại; ảnh thì :DocxInsertImage
/// để chèn hiển thị). Đặt tên entry không trùng, patch [Content_Types].xml
/// nếu extension chưa khai báo.
fn cmd_upload(path: &str, src_path: &str) -> AppResult<()> {
    let ext = std::path::Path::new(src_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let dest_dir = match classify_upload_ext(&ext) {
        Some(d) => d,
        None => {
            return Err(AppError(format!(
                "Unsupported file type for upload: .{ext} \
(supported: images, spreadsheets, documents, audio, video, archives)"
            )))
        }
    };

    let file_bytes = std::fs::read(src_path)
        .map_err(|e| AppError(format!("Cannot read source file {src_path}: {e}")))?;

    let mut archive = zip_io::open_archive(path)?;

    // Danh sách tên đã tồn tại để tránh trùng tên entry.
    let existing_names: std::collections::HashSet<String> = zip_io::list_entries(&mut archive)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    let original_stem = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("upload");
    let mut entry_name = format!("{dest_dir}/{original_stem}.{ext}");
    let mut counter = 1u32;
    while existing_names.contains(&entry_name) {
        entry_name = format!("{dest_dir}/{original_stem}_{counter}.{ext}");
        counter += 1;
    }

    // Đọc + patch [Content_Types].xml nếu extension chưa được khai báo.
    let content_types_xml = zip_io::read_entry(&mut archive, "[Content_Types].xml")?;
    let content_types_str = String::from_utf8(content_types_xml.clone())
        .map_err(|e| AppError(format!("Content_Types not valid UTF-8: {e}")))?;
    let needs_default = !content_types_str.contains(&format!("Extension=\"{ext}\""));
    let new_content_types = if needs_default {
        let content_type = guess_content_type(&ext);
        let insertion = format!(
            "<Default Extension=\"{ext}\" ContentType=\"{content_type}\"/>"
        );
        // Chèn ngay trước tag đóng </Types> (luôn tồn tại trong DOCX hợp lệ).
        if let Some(idx) = content_types_str.rfind("</Types>") {
            let mut s = content_types_str.clone();
            s.insert_str(idx, &insertion);
            Some(s.into_bytes())
        } else {
            None
        }
    } else {
        None
    };

    let mut replacements: Vec<(&str, Vec<u8>)> = Vec::new();
    replacements.push((entry_name.as_str(), file_bytes));
    if let Some(ct_bytes) = &new_content_types {
        replacements.push(("[Content_Types].xml", ct_bytes.clone()));
    }

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &replacements)?;
    zip_io::atomic_write(path, &new_bytes)?;

    println!("{entry_name}");
    Ok(())
}

/// Upload NHIỀU file trong MỘT lần mở + ghi lại zip. Nhận đường dẫn file
/// text (list_file), mỗi dòng 1 path nguồn. Nhanh hơn rất nhiều so với gọi
/// 'upload' n lần (mỗi lần n đó phải đọc+ghi lại toàn bộ archive).
///
/// In ra stdout mỗi dòng:
///   "OK\t<entry_name>"        - upload thành công
///   "SKIP\t<src_path>"        - extension không hỗ trợ
///   "ERR\t<src_path>\t<msg>"  - lỗi đọc file
fn cmd_uploadmany(path: &str, list_file: &str) -> AppResult<()> {
    let list_content = std::fs::read_to_string(list_file)
        .map_err(|e| AppError(format!("Cannot read list file {list_file}: {e}")))?;
    let sources: Vec<&str> = list_content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if sources.is_empty() {
        return Err(AppError("Empty upload list".to_string()));
    }

    let mut archive = zip_io::open_archive(path)?;

    // Tên entry đã tồn tại — cập nhật dần trong vòng lặp để tránh trùng
    // GIỮA các file mới upload cùng lượt (vd 2 file cùng tên khác thư mục).
    let mut taken: std::collections::HashSet<String> = zip_io::list_entries(&mut archive)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    // [Content_Types].xml: đọc 1 lần, gom mọi extension mới rồi patch 1 lần.
    let ct_xml = zip_io::read_entry(&mut archive, "[Content_Types].xml")?;
    let mut ct_str =
        String::from_utf8(ct_xml).map_err(|e| AppError(format!("CT not UTF-8: {e}")))?;
    let mut ct_changed = false;

    // Phải giữ bytes sống tới lúc write -> lưu (entry_name, bytes) owned.
    let mut new_entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut report: Vec<String> = Vec::new();

    for src in &sources {
        let ext = std::path::Path::new(src)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let dest_dir = match classify_upload_ext(&ext) {
            Some(d) => d,
            None => {
                report.push(format!("SKIP\t{src}"));
                continue;
            }
        };
        let bytes = match std::fs::read(src) {
            Ok(b) => b,
            Err(e) => {
                report.push(format!("ERR\t{src}\t{e}"));
                continue;
            }
        };
        let stem = std::path::Path::new(src)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("upload");
        let mut entry_name = format!("{dest_dir}/{stem}.{ext}");
        let mut counter = 1u32;
        while taken.contains(&entry_name) {
            entry_name = format!("{dest_dir}/{stem}_{counter}.{ext}");
            counter += 1;
        }
        taken.insert(entry_name.clone());

        // Patch content type nếu extension chưa khai báo.
        if !ct_str.contains(&format!("Extension=\"{ext}\"")) {
            let ins = format!(
                "<Default Extension=\"{ext}\" ContentType=\"{}\"/>",
                guess_content_type(&ext)
            );
            if let Some(idx) = ct_str.rfind("</Types>") {
                ct_str.insert_str(idx, &ins);
                ct_changed = true;
            }
        }

        new_entries.push((entry_name.clone(), bytes));
        report.push(format!("OK\t{entry_name}"));
    }

    if new_entries.is_empty() {
        // Không có gì để ghi (toàn skip/err) — vẫn in report để Vim biết.
        for line in &report {
            println!("{line}");
        }
        return Ok(());
    }

    // Build replacements (ép kiểu &str từ owned String).
    let mut replacements: Vec<(&str, Vec<u8>)> = Vec::with_capacity(new_entries.len() + 1);
    for (name, bytes) in &new_entries {
        replacements.push((name.as_str(), bytes.clone()));
    }
    if ct_changed {
        replacements.push(("[Content_Types].xml", ct_str.into_bytes()));
    }

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &replacements)?;
    zip_io::atomic_write(path, &new_bytes)?;

    for line in &report {
        println!("{line}");
    }
    Ok(())
}

/// Đọc kích thước (width_px, height_px) của ảnh từ raw bytes. Hỗ trợ
/// PNG và JPEG (đủ cho các ảnh thường gặp trong DOCX). Parse header thủ
/// công, không cần thêm crate. Trả về None nếu không nhận dạng được.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // PNG: signature 8 byte, sau đó IHDR chunk chứa width/height (big-endian
    // u32) tại offset 16 và 20.
    const PNG_SIG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() >= 24 && bytes.starts_with(PNG_SIG) {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }
    // JPEG: bắt đầu FF D8. Duyệt các marker segment tới SOF0..SOF3/SOF5..
    // SOF15 (trừ SOF4/SOF8/SOF12 là không phải start-of-frame chuẩn) để
    // lấy height (2 byte) + width (2 byte) big-endian.
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            // Standalone markers không có length (RSTn, SOI, EOI, TEM).
            if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
                i += 2;
                continue;
            }
            let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            // SOF markers chứa kích thước. Loại trừ DHT(C4)/DAC(CC)/RSTn.
            let is_sof = matches!(marker,
                0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 |
                0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF);
            if is_sof && i + 9 < bytes.len() {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            if seg_len < 2 {
                break;
            }
            i += 2 + seg_len;
        }
    }
    None
}

/// Tìm số rId lớn nhất trong rels XML (dạng Id="rId12") + trả về rId tiếp
/// theo chưa dùng (vd "rId13"). Nếu rels rỗng/không có rId nào -> "rId1".
fn next_rel_id(rels_xml: &str) -> String {
    let mut max_n = 0u32;
    let mut search = rels_xml;
    while let Some(pos) = search.find("Id=\"rId") {
        let rest = &search[pos + 7..]; // sau 'Id="rId'
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<u32>() {
            if n > max_n {
                max_n = n;
            }
        }
        // Luôn advance qua phần đã quét (kể cả khi num rỗng) để tránh lặp
        // vô hạn trên chuỗi không có digit sau "rId".
        let advance = num.len().max(1);
        if advance >= rest.len() {
            break;
        }
        search = &rest[advance..];
    }
    format!("rId{}", max_n + 1)
}

/// Tìm docPr id lớn nhất đang dùng trong document.xml (wp:docPr id="N")
/// để cấp id mới không trùng. Word yêu cầu mỗi drawing có 1 docPr id duy
/// nhất. Fallback 1 nếu chưa có.
fn next_docpr_id(doc_xml: &str) -> u32 {
    let mut max_n = 0u32;
    let mut search = doc_xml;
    while let Some(pos) = search.find("wp:docPr id=\"") {
        let rest = &search[pos + 13..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<u32>() {
            if n > max_n {
                max_n = n;
            }
        }
        let advance = num.len().max(1);
        if advance >= rest.len() {
            break;
        }
        search = &rest[advance..];
    }
    max_n + 1
}

/// Sinh đoạn XML <w:r><w:drawing>...</w:r> inline image hoàn chỉnh.
/// - rel_id: relationship trỏ tới file media (vd "rId13").
/// - cx_emu/cy_emu: kích thước tính bằng EMU (1 cm = 360000 EMU).
/// - docpr_id: id duy nhất cho wp:docPr.
/// - name: tên ảnh hiển thị (vd "image2.png").
fn build_drawing_run(rel_id: &str, cx_emu: i64, cy_emu: i64, docpr_id: u32, name: &str) -> String {
    let name_esc = xml_emit::xml_escape_attr(name);
    format!(
        "<w:r><w:drawing>\
<wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<wp:extent cx=\"{cx_emu}\" cy=\"{cy_emu}\"/>\
<wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
<wp:docPr id=\"{docpr_id}\" name=\"Picture {docpr_id}\" descr=\"{name_esc}\"/>\
<wp:cNvGraphicFramePr>\
<a:graphicFrameLocks xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" noChangeAspect=\"1\"/>\
</wp:cNvGraphicFramePr>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:nvPicPr>\
<pic:cNvPr id=\"{docpr_id}\" name=\"{name_esc}\"/>\
<pic:cNvPicPr/>\
</pic:nvPicPr>\
<pic:blipFill>\
<a:blip r:embed=\"{rel_id}\"/>\
<a:stretch><a:fillRect/></a:stretch>\
</pic:blipFill>\
<pic:spPr>\
<a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx_emu}\" cy=\"{cy_emu}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
</pic:spPr>\
</pic:pic>\
</a:graphicData>\
</a:graphic>\
</wp:inline>\
</w:drawing></w:r>"
    )
}

/// Đọc word/_rels/document.xml.rels; nếu CHƯA TỒN TẠI (file DOCX tối giản
/// chưa từng có relationship nào), trả về 1 rels rỗng hợp lệ. Tránh fail
/// khi insert ảnh/OLE vào doc chưa có rels. Trả về (bytes, đã_tồn_tại).
fn read_or_init_document_rels<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> (Vec<u8>, bool) {
    match zip_io::read_entry(archive, "word/_rels/document.xml.rels") {
        Ok(b) => (b, true),
        Err(_) => {
            let empty = b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>"
                .to_vec();
            (empty, false)
        }
    }
}

/// Chèn 1 ảnh có sẵn trong word/media/ vào paragraph para_id dưới dạng
/// inline drawing. width_cm = chiều rộng mong muốn, chiều cao tự co theo
/// tỉ lệ gốc của ảnh.
fn cmd_insertimage(path: &str, para_id: &str, media_entry: &str, width_cm: &str) -> AppResult<()> {
    // 1. Validate width.
    let width_cm: f64 = width_cm
        .trim()
        .parse()
        .map_err(|_| AppError(format!("Invalid width (cm): {width_cm}")))?;
    if width_cm <= 0.0 || width_cm > 100.0 {
        return Err(AppError(format!("Width out of range (0-100 cm): {width_cm}")));
    }

    let mut archive = zip_io::open_archive(path)?;

    // 2. media_entry phải tồn tại trong zip. Chuẩn hoá: cho phép user
    //    truyền "image2.png" (ngắn) hoặc "word/media/image2.png" (đầy đủ).
    let full_entry = if media_entry.starts_with("word/") {
        media_entry.to_string()
    } else {
        format!("word/media/{media_entry}")
    };
    let img_bytes = zip_io::read_entry(&mut archive, &full_entry)
        .map_err(|_| AppError(format!("Image not found in DOCX: {full_entry}")))?;

    // 3. Tính kích thước EMU. 1 cm = 360000 EMU.
    let (px_w, px_h) = image_dimensions(&img_bytes).unwrap_or((0, 0));
    let cx_emu = (width_cm * 360000.0).round() as i64;
    let cy_emu = if px_w > 0 && px_h > 0 {
        ((cx_emu as f64) * (px_h as f64) / (px_w as f64)).round() as i64
    } else {
        // Không đọc được tỉ lệ -> giả định vuông (1:1).
        cx_emu
    };

    // 4. Target relative cho relationship = phần sau "word/".
    //    "word/media/image2.png" -> "media/image2.png".
    let rel_target = full_entry
        .strip_prefix("word/")
        .unwrap_or(full_entry.as_str())
        .to_string();

    // 5. Đọc rels hiện có (tạo rỗng nếu chưa tồn tại), kiểm tra đã có
    //    relationship trỏ tới target này chưa (tái dùng rId nếu có).
    let rels_path = "word/_rels/document.xml.rels";
    let (rels_xml_bytes, rels_existed) = read_or_init_document_rels(&mut archive);
    let rels_str = String::from_utf8(rels_xml_bytes.clone())
        .map_err(|e| AppError(format!("rels not UTF-8: {e}")))?;
    let existing_rel_id = parse_rels(&rels_xml_bytes)
        .into_iter()
        .find(|(_, t)| t == &rel_target || t.ends_with(rel_target.as_str()))
        .map(|(id, _)| id);

    // Tính rel_id + chuỗi rels cần ghi lại.
    // - Nếu đã có rel trỏ tới ảnh: tái dùng id đó. Vẫn phải ghi rels ra đĩa
    //   nếu file rels vừa được khởi tạo rỗng (rels_existed == false), nhưng
    //   trường hợp đó existing_rel_id luôn None nên không xảy ra.
    // - Nếu chưa có: cấp rId mới, chèn Relationship, ghi lại.
    let (rel_id, new_rels_str) = match existing_rel_id {
        Some(id) => (id, if rels_existed { None } else { Some(rels_str.clone()) }),
        None => {
            let new_id = next_rel_id(&rels_str);
            let insertion = format!(
                "<Relationship Id=\"{new_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{rel_target}\"/>"
            );
            let patched = match rels_str.rfind("</Relationships>") {
                Some(idx) => {
                    let mut s = rels_str.clone();
                    s.insert_str(idx, &insertion);
                    s
                }
                None => return Err(AppError("Malformed document.xml.rels".to_string())),
            };
            (new_id, Some(patched))
        }
    };

    // 6. Parse document, tìm paragraph, chèn drawing run.
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc_xml_str = String::from_utf8_lossy(&xml).into_owned();
    let docpr_id = next_docpr_id(&doc_xml_str);
    let drawing = build_drawing_run(&rel_id, cx_emu, cy_emu, docpr_id, &rel_target);

    let mut doc = parser::parse_document(&xml)?;
    let p = doc
        .paragraphs
        .iter_mut()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;
    p.runs.push(model::Run {
        text: String::new(),
        style: model::RunStyle::default(),
        is_drawing: true,
        rel_id: Some(rel_id.clone()),
        byte_range: None,
        media_name: None,
        drawing_xml: Some(drawing),
    });
    p.dirty = true;

    // 7. Rebuild document.xml + ghi kèm rels mới (nếu có).
    let new_xml = save::rebuild_document_xml(&doc)?;
    let mut replacements: Vec<(&str, Vec<u8>)> = vec![("word/document.xml", new_xml)];
    if let Some(ref patched) = new_rels_str {
        replacements.push((rels_path, patched.clone().into_bytes()));
    }
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &replacements)?;
    zip_io::atomic_write(path, &new_bytes)?;

    emit_open_output(path)
}

/// Chèn 1 marker đính kèm file vào paragraph para_id. Marker là text thuần
/// dạng "[📎 tên_file]" — KHÔNG đụng cấu trúc OLE/drawing phức tạp nên an
/// toàn tuyệt đối, không thể làm hỏng file. File thật vẫn nằm trong package
/// (đã upload trước); gx/:DocxOpen trên dòng marker sẽ trích tên file rồi
/// mở. file_name = tên file (vd "baocao.pdf"), chỉ phần basename.
fn cmd_insertfile(path: &str, para_id: &str, file_name: &str) -> AppResult<()> {
    // Chỉ lấy basename để marker gọn + để gx parse lại dễ. Loại bỏ mọi
    // thành phần thư mục nếu user lỡ truyền đường dẫn đầy đủ.
    let base = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_name)
        .trim();
    if base.is_empty() {
        return Err(AppError("Empty file name".to_string()));
    }

    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let mut doc = parser::parse_document(&xml)?;

    let p = doc
        .paragraphs
        .iter_mut()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;

    // Chèn marker text. Dùng U+1F4CE (📎) + tên file trong ngoặc vuông.
    // Đây là run text thường (không drawing), emit_run sẽ tạo <w:t>.
    let marker = format!("[\u{1F4CE} {base}]");
    p.runs.push(model::Run {
        text: marker,
        style: model::RunStyle::default(),
        is_drawing: false,
        rel_id: None,
        byte_range: None,
        media_name: None,
        drawing_xml: None,
    });
    p.dirty = true;

    let new_xml = save::rebuild_document_xml(&doc)?;
    let new_bytes = zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;

    emit_open_output(path)
}

// ===================== PNG ICON GENERATOR =====================
// Sinh PNG đặc 1 màu thuần Rust (không cần crate ảnh). Dùng cho icon
// preview của OLE object. Tự cài CRC32 + Adler32 + deflate "stored".

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Bọc 1 PNG chunk: length(4) + type(4) + data + crc(4).
fn png_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(ctype);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Sinh PNG RGB đặc 1 màu, kích thước w×h. Dùng deflate "stored" (block
/// không nén) để không cần thư viện nén.
/// Đóng gói raw scanlines (đã gồm filter byte mỗi dòng) thành file PNG.
/// color_type: 2 = RGB (3 byte/px), 6 = RGBA (4 byte/px). bit depth 8.
fn encode_png(w: u32, h: u32, color_type: u8, raw: &[u8]) -> Vec<u8> {
    // zlib stream: header (0x78 0x01) + deflate stored blocks + adler32.
    let mut zlib = Vec::new();
    zlib.push(0x78);
    zlib.push(0x01);
    let mut offset = 0usize;
    while offset < raw.len() {
        let chunk_len = std::cmp::min(65535, raw.len() - offset);
        let is_final = offset + chunk_len >= raw.len();
        zlib.push(if is_final { 1 } else { 0 });
        let len = chunk_len as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + chunk_len]);
        offset += chunk_len;
    }
    zlib.extend_from_slice(&adler32(raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(color_type);
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

#[allow(dead_code)]
fn make_solid_png(w: u32, h: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
    let mut raw = Vec::with_capacity((h * (1 + w * 3)) as usize);
    for _ in 0..h {
        raw.push(0u8); // filter type None
        for _ in 0..w {
            raw.push(rgb.0);
            raw.push(rgb.1);
            raw.push(rgb.2);
        }
    }
    encode_png(w, h, 2, &raw)
}

/// Vẽ 1 icon hình tờ giấy (document) RGBA trong suốt, kích thước w×h:
/// thân giấy trắng, viền xám, góc trên-phải gập, vài dòng kẻ xám giả text.
/// Dùng làm preview icon cho OLE object (đẹp hơn ô đặc màu).
fn make_file_icon_png(w: u32, h: u32) -> Vec<u8> {
    // Buffer RGBA, mặc định trong suốt (0,0,0,0).
    let mut px = vec![0u8; (w * h * 4) as usize];
    let set = |px: &mut Vec<u8>, x: u32, y: u32, c: (u8, u8, u8, u8)| {
        if x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            px[i] = c.0;
            px[i + 1] = c.1;
            px[i + 2] = c.2;
            px[i + 3] = c.3;
        }
    };

    // Hình học tờ giấy: chừa margin trái/phải, fold ở góc phải-trên.
    let mx = w / 8; // margin ngang
    let my = h / 12; // margin dọc
    let left = mx;
    let right = w - mx;
    let top = my;
    let bottom = h - my;
    let fold = (right - left) / 3; // kích thước góc gập

    let border = (0x6B, 0x72, 0x80, 0xFF); // xám đậm viền
    let paper = (0xFF, 0xFF, 0xFF, 0xFF); // trắng thân
    let fold_col = (0xD0, 0xD5, 0xDD, 0xFF); // xám nhạt mặt gập
    let text_line = (0xB4, 0xBA, 0xC4, 0xFF); // xám nhạt dòng text

    for y in top..bottom {
        for x in left..right {
            // Vùng tam giác góc gập (trên-phải): x >= right-fold và
            // y <= top+fold, với điều kiện chéo (x-(right-fold)) > (y-top)
            // -> phần bị "cắt" khỏi thân (để lộ mặt gập).
            let in_fold_box = x >= right - fold && y < top + fold;
            let dx = x as i64 - (right - fold) as i64;
            let dy = y as i64 - top as i64;
            let is_corner_cut = in_fold_box && dx >= dy; // nửa trên-phải đường chéo

            // Viền ngoài (khung giấy) dày 1px.
            let on_border = x == left || x == right - 1 || y == top || y == bottom - 1;

            if is_corner_cut {
                // Mặt gập: tô màu xám nhạt + viền chéo.
                if dx == dy {
                    set(&mut px, x, y, border); // đường chéo gập
                } else {
                    set(&mut px, x, y, fold_col);
                }
                continue;
            }
            if on_border {
                set(&mut px, x, y, border);
            } else {
                set(&mut px, x, y, paper);
            }
        }
    }
    // Viền của cạnh gập (đoạn dọc bên phải phần dưới fold + cạnh trên trái fold)
    // đã xấp xỉ đủ qua on_border. Thêm vài dòng kẻ "text" giả.
    let text_left = left + (right - left) / 6;
    let text_right = right - (right - left) / 6;
    let mut ly = top + fold + (h / 14);
    let line_gap = h / 9;
    while ly < bottom - (h / 14) {
        for x in text_left..text_right {
            // Dòng đầu (trong vùng có thể chồng fold) -> chừa nếu nằm vùng cắt.
            set(&mut px, x, ly, text_line);
        }
        ly += line_gap;
    }

    // Build raw scanlines RGBA (filter byte 0 mỗi dòng).
    let mut raw = Vec::with_capacity((h * (1 + w * 4)) as usize);
    for y in 0..h {
        raw.push(0u8);
        let row = ((y * w) * 4) as usize;
        raw.extend_from_slice(&px[row..row + (w * 4) as usize]);
    }
    encode_png(w, h, 6, &raw)
}

// ===================== INSERT OLE (docx/xlsx) =====================

/// ProgID OLE theo extension OOXML.
fn ole_progid(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "xlsx" => Some("Excel.Sheet.12"),
        "xlsm" => Some("Excel.SheetMacroEnabled.12"),
        "docx" => Some("Word.Document.12"),
        _ => None,
    }
}

/// Đảm bảo có 1 icon PNG dùng chung trong word/media/. Trả về tên entry.
/// Nếu chưa có thì caller tự thêm vào replacements (hàm này chỉ tạo bytes).
const OLE_ICON_ENTRY: &str = "word/media/ole_icon.png";

/// Sinh đoạn XML <w:r><w:object>...</w:r> cho OLE embedded object hiển thị
/// dạng icon. icon_rel = rId trỏ tới ảnh icon; pkg_rel = rId trỏ tới file
/// embedded (relationship type package); prog_id = ProgID; shape_id/obj_id
/// duy nhất. Kích thước icon mặc định ~48x60pt (dxa: 1pt = 20 twips/dxa).
fn build_ole_run(icon_rel: &str, pkg_rel: &str, prog_id: &str, shape_id: u32, obj_id: u32) -> String {
    // width/height hiển thị (points). docPr/extent dùng EMU không cần ở
    // dạng object cổ điển; dùng VML style pt.
    let w_pt = 48;
    let h_pt = 58;
    format!(
        "<w:r><w:object w:dxaOrig=\"960\" w:dyaOrig=\"1160\">\
<v:shapetype id=\"_x0000_t75\" coordsize=\"21600,21600\" o:spt=\"75\" o:preferrelative=\"t\" \
path=\"m@4@5l@4@11@9@11@9@5xe\" filled=\"f\" stroked=\"f\" \
xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\">\
<v:stroke joinstyle=\"miter\"/>\
<v:formulas>\
<v:f eqn=\"if lineDrawn pixelLineWidth 0\"/>\
<v:f eqn=\"sum @0 1 0\"/>\
<v:f eqn=\"sum 0 0 @1\"/>\
<v:f eqn=\"prod @2 1 2\"/>\
<v:f eqn=\"prod @3 21600 pixelWidth\"/>\
<v:f eqn=\"prod @3 21600 pixelHeight\"/>\
<v:f eqn=\"sum @0 0 1\"/>\
<v:f eqn=\"prod @6 1 2\"/>\
<v:f eqn=\"prod @7 21600 pixelWidth\"/>\
<v:f eqn=\"sum @8 21600 0\"/>\
<v:f eqn=\"prod @7 21600 pixelHeight\"/>\
<v:f eqn=\"sum @10 21600 0\"/>\
</v:formulas>\
<v:path o:extrusionok=\"f\" gradientshapeok=\"t\" o:connecttype=\"rect\"/>\
<o:lock v:ext=\"edit\" aspectratio=\"t\"/>\
</v:shapetype>\
<v:shape id=\"_x0000_i{shape_id}\" type=\"#_x0000_t75\" style=\"width:{w_pt}pt;height:{h_pt}pt\" o:ole=\"\" \
xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<v:imagedata r:id=\"{icon_rel}\" o:title=\"\"/>\
</v:shape>\
<o:OLEObject Type=\"Embed\" ProgID=\"{prog_id}\" ShapeID=\"_x0000_i{shape_id}\" DrawAspect=\"Icon\" ObjectID=\"_{obj_id}\" r:id=\"{pkg_rel}\" \
xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"/>\
</w:object></w:r>"
    )
}

fn cmd_insertole(path: &str, para_id: &str, embed_entry: &str) -> AppResult<()> {
    // Chuẩn hoá entry: cho phép "file.xlsx" hoặc "word/embeddings/file.xlsx".
    let full_entry = if embed_entry.starts_with("word/") {
        embed_entry.to_string()
    } else {
        format!("word/embeddings/{embed_entry}")
    };
    let ext = std::path::Path::new(&full_entry)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let prog_id = ole_progid(&ext).ok_or_else(|| {
        AppError(format!(
            "OLE embed chỉ hỗ trợ docx/xlsx/xlsm (file: .{ext})"
        ))
    })?;

    let mut archive = zip_io::open_archive(path)?;
    // Xác nhận file embedded tồn tại trong package.
    if zip_io::read_entry(&mut archive, &full_entry).is_err() {
        return Err(AppError(format!(
            "Embedded file not found in package: {full_entry} (đã :DocxUpload chưa?)"
        )));
    }

    // Đảm bảo icon PNG tồn tại (tạo nếu chưa). Màu xám nhạt 48x58 px.
    let existing: std::collections::HashSet<String> = zip_io::list_entries(&mut archive)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let icon_bytes: Option<Vec<u8>> = if existing.contains(OLE_ICON_ENTRY) {
        None
    } else {
        Some(make_file_icon_png(48, 58))
    };

    // Đọc rels (tạo rỗng nếu chưa có), tạo 2 relationship: package + image.
    let rels_path = "word/_rels/document.xml.rels";
    let (rels_bytes, _rels_existed) = read_or_init_document_rels(&mut archive);
    let rels_str = String::from_utf8(rels_bytes.clone())
        .map_err(|e| AppError(format!("rels not UTF-8: {e}")))?;

    // rId cho package: tạo mới. rId cho icon: tái dùng nếu icon đã có rel.
    let pkg_target = full_entry.strip_prefix("word/").unwrap_or(&full_entry).to_string();
    let icon_target = OLE_ICON_ENTRY.strip_prefix("word/").unwrap_or(OLE_ICON_ENTRY).to_string();

    let rel_map = parse_rels(&rels_bytes);
    let existing_icon_rel = rel_map
        .iter()
        .find(|(_, t)| t.as_str() == icon_target || t.ends_with(&icon_target))
        .map(|(id, _)| id.clone());

    // Cấp rId mới: package luôn mới; icon tái dùng nếu có.
    let pkg_rel = next_rel_id(&rels_str);
    // next_rel_id chỉ tính max hiện có; để cấp 2 id khác nhau, icon = max+2.
    let icon_rel = match &existing_icon_rel {
        Some(id) => id.clone(),
        None => {
            // pkg_rel = "rIdN" -> icon = "rId(N+1)".
            let n: u32 = pkg_rel.trim_start_matches("rId").parse().unwrap_or(1);
            format!("rId{}", n + 1)
        }
    };

    // Patch rels: thêm package rel (+ icon rel nếu chưa có).
    let mut additions = String::new();
    additions.push_str(&format!(
        "<Relationship Id=\"{pkg_rel}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/package\" Target=\"{pkg_target}\"/>"
    ));
    if existing_icon_rel.is_none() {
        additions.push_str(&format!(
            "<Relationship Id=\"{icon_rel}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{icon_target}\"/>"
        ));
    }
    let new_rels = match rels_str.rfind("</Relationships>") {
        Some(idx) => {
            let mut s = rels_str.clone();
            s.insert_str(idx, &additions);
            s
        }
        None => return Err(AppError("Malformed document.xml.rels".to_string())),
    };

    // Cấp shape_id/obj_id duy nhất từ document.xml.
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc_xml_str = String::from_utf8_lossy(&xml).into_owned();
    let shape_id = next_docpr_id(&doc_xml_str) + 1000; // tránh trùng docPr
    let obj_id: u32 = 1_000_000_000 + shape_id;
    let ole_xml = build_ole_run(&icon_rel, &pkg_rel, prog_id, shape_id, obj_id);

    let mut doc = parser::parse_document(&xml)?;
    let p = doc
        .paragraphs
        .iter_mut()
        .find(|p| p.id == para_id)
        .ok_or_else(|| AppError(format!("Paragraph not found: {para_id}")))?;
    p.runs.push(model::Run {
        text: String::new(),
        style: model::RunStyle::default(),
        is_drawing: true,
        rel_id: Some(pkg_rel.clone()),
        byte_range: None,
        media_name: None,
        drawing_xml: Some(ole_xml),
    });
    p.dirty = true;

    let new_doc_xml = save::rebuild_document_xml(&doc)?;
    let mut replacements: Vec<(&str, Vec<u8>)> = vec![
        ("word/document.xml", new_doc_xml),
        (rels_path, new_rels.into_bytes()),
    ];
    if let Some(ref ib) = icon_bytes {
        replacements.push((OLE_ICON_ENTRY, ib.clone()));
    }
    // Patch [Content_Types].xml: đảm bảo png + ext của file embedded khai báo.
    let ct_bytes = zip_io::read_entry(&mut archive, "[Content_Types].xml")?;
    let ct_str = String::from_utf8(ct_bytes).map_err(|e| AppError(format!("CT not UTF-8: {e}")))?;
    let mut ct_new = ct_str.clone();
    for need_ext in ["png", ext.as_str()] {
        if !ct_new.contains(&format!("Extension=\"{need_ext}\"")) {
            let ins = format!(
                "<Default Extension=\"{need_ext}\" ContentType=\"{}\"/>",
                guess_content_type(need_ext)
            );
            if let Some(idx) = ct_new.rfind("</Types>") {
                ct_new.insert_str(idx, &ins);
            }
        }
    }
    let ct_changed = ct_new != ct_str;
    if ct_changed {
        replacements.push(("[Content_Types].xml", ct_new.into_bytes()));
    }

    let new_bytes = zip_io::write_replacing_entries(&mut archive, &replacements)?;
    zip_io::atomic_write(path, &new_bytes)?;
    emit_open_output(path)
}

// ===================== RESIZE IMAGE =====================

/// Tìm giá trị số nguyên của attribute đầu tiên `attr="N"` trong xml.
fn find_attr_u64(xml: &str, attr: &str) -> Option<i64> {
    let needle = format!("{attr}=\"");
    let pos = xml.find(&needle)?;
    let rest = &xml[pos + needle.len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse::<i64>().ok()
}

/// Thay MỌI occurrence của `attr="<số>"` trong xml bằng `attr="<new>"`.
/// Dùng cho cx/cy (chỉ <wp:extent>/<a:ext> dùng 2 attr này; effectExtent
/// dùng l/t/r/b, docPr không có cx/cy -> thay toàn bộ là an toàn).
fn replace_attr_all(xml: &str, attr: &str, new_val: i64) -> String {
    let needle = format!("{attr}=\"");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(pos) = rest.find(&needle) {
        out.push_str(&rest[..pos]);
        out.push_str(&needle);
        let after = &rest[pos + needle.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            // Không phải dạng số (vd cx trong context khác) -> giữ nguyên,
            // tiến qua needle để tránh lặp.
            rest = after;
            continue;
        }
        out.push_str(&new_val.to_string());
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    out
}

/// Đọc chiều rộng vùng nội dung (content width) của trang từ sectPr trong
/// document.xml: pgSz.w - pgMar.left - pgMar.right (đơn vị twips). Trả về
/// EMU (1 twip = 635 EMU). Fallback A4 dọc lề 1 inch (~15.9cm) nếu không
/// parse được.
fn page_content_width_emu(doc_xml: &str) -> i64 {
    let twip_to_emu = 635i64;
    let pg_w = doc_xml
        .find("<w:pgSz")
        .and_then(|p| find_attr_u64(&doc_xml[p..], "w:w"));
    let (ml, mr) = match doc_xml.find("<w:pgMar") {
        Some(p) => {
            let seg = &doc_xml[p..];
            (find_attr_u64(seg, "w:left"), find_attr_u64(seg, "w:right"))
        }
        None => (None, None),
    };
    if let (Some(w), Some(l), Some(r)) = (pg_w, ml, mr) {
        let content = (w - l - r).max(1);
        return content * twip_to_emu;
    }
    // Fallback: A4 (11906 twips) - 2*1440 lề = 9026 twips.
    9026 * twip_to_emu
}

/// Resize 1 drawing ảnh: nhận XML hiện tại + mode/value + content width +
/// hàm đọc tỉ lệ. Trả về XML mới (đã đổi cx/cy) hoặc None nếu không phải
/// ảnh inline resize được.
fn resize_one_drawing(
    cur_xml: &str,
    mode: &str,
    value_cm: f64,
    content_w_emu: i64,
    ratio_override: Option<f64>,
) -> Option<String> {
    if cur_xml.contains("OLEObject") || cur_xml.contains("w:object") {
        return None;
    }
    if !(cur_xml.contains("wp:extent") && (cur_xml.contains("blip") || cur_xml.contains("a:ext"))) {
        return None;
    }
    let cur_cx = find_attr_u64(cur_xml, "cx").unwrap_or(0);
    let cur_cy = find_attr_u64(cur_xml, "cy").unwrap_or(0);
    let ratio_hw = ratio_override.unwrap_or_else(|| {
        if cur_cx > 0 && cur_cy > 0 {
            cur_cy as f64 / cur_cx as f64
        } else {
            1.0
        }
    });
    let (new_cx, new_cy) = match mode {
        "fit" => {
            let cx = content_w_emu;
            (cx, (cx as f64 * ratio_hw).round() as i64)
        }
        "width" => {
            let cx = (value_cm * 360000.0).round() as i64;
            (cx, (cx as f64 * ratio_hw).round() as i64)
        }
        "height" => {
            let cy = (value_cm * 360000.0).round() as i64;
            let cx = if ratio_hw > 0.0 {
                (cy as f64 / ratio_hw).round() as i64
            } else {
                cy
            };
            (cx, cy)
        }
        _ => return None,
    };
    let out = replace_attr_all(cur_xml, "cx", new_cx);
    let out = replace_attr_all(&out, "cy", new_cy);
    Some(out)
}

/// Resize ảnh inline. para_ids = danh sách para_id phân tách bằng dấu phẩy
/// (1 hoặc nhiều). Resize MỌI ảnh inline trong mỗi paragraph đó.
fn cmd_resizeimage(path: &str, para_ids: &str, mode: &str, value: &str) -> AppResult<()> {
    let mode = mode.to_ascii_lowercase();
    if !matches!(mode.as_str(), "fit" | "width" | "height") {
        return Err(AppError(format!(
            "Invalid mode '{mode}' (dùng: fit | width | height)"
        )));
    }
    let value_cm: f64 = if mode == "fit" {
        0.0
    } else {
        value
            .trim()
            .parse()
            .map_err(|_| AppError(format!("Invalid size (cm): {value}")))?
    };
    if mode != "fit" && (value_cm <= 0.0 || value_cm > 100.0) {
        return Err(AppError(format!("Size out of range (0-100 cm): {value_cm}")));
    }

    let id_set: std::collections::HashSet<&str> =
        para_ids.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if id_set.is_empty() {
        return Err(AppError("No paragraph id given".to_string()));
    }

    let mut archive = zip_io::open_archive(path)?;
    let xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc_xml_str = String::from_utf8_lossy(&xml).into_owned();
    let content_w_emu = page_content_width_emu(&doc_xml_str);

    // Đọc rels 1 lần để tra tỉ lệ ảnh thật theo rel_id.
    let rel_map = match zip_io::read_entry(&mut archive, "word/_rels/document.xml.rels") {
        Ok(b) => parse_rels(&b),
        Err(_) => std::collections::HashMap::new(),
    };
    // Cache đọc kích thước media theo entry để tránh đọc lại nhiều lần.
    let mut dim_cache: std::collections::HashMap<String, Option<(u32, u32)>> =
        std::collections::HashMap::new();

    let mut doc = parser::parse_document(&xml)?;
    let original_xml = doc.original_xml.clone();

    let mut resized = 0usize;
    // Duyệt mọi paragraph khớp id_set; với mỗi run drawing ảnh -> resize.
    for p in doc.paragraphs.iter_mut() {
        if !id_set.contains(p.id.as_str()) {
            continue;
        }
        let mut para_changed = false;
        for run in p.runs.iter_mut() {
            if !run.is_drawing {
                continue;
            }
            let cur_xml = if let Some(dx) = &run.drawing_xml {
                dx.clone()
            } else if let Some((s, e)) = run.byte_range {
                if e <= original_xml.len() && s <= e {
                    String::from_utf8_lossy(&original_xml[s..e]).into_owned()
                } else {
                    continue;
                }
            } else {
                continue;
            };
            // Tỉ lệ thật từ ảnh (nếu đọc được qua rel_id -> media).
            let mut ratio_override: Option<f64> = None;
            if let Some(rid) = run.rel_id.clone() {
                if let Some(target) = rel_map.get(&rid) {
                    let entry = format!("word/{target}");
                    let dim = if let Some(cached) = dim_cache.get(&entry) {
                        *cached
                    } else {
                        let d = zip_io::read_entry(&mut archive, &entry)
                            .ok()
                            .and_then(|b| image_dimensions(&b));
                        dim_cache.insert(entry.clone(), d);
                        d
                    };
                    if let Some((pw, ph)) = dim {
                        if pw > 0 {
                            ratio_override = Some(ph as f64 / pw as f64);
                        }
                    }
                }
            }
            if let Some(new_xml) =
                resize_one_drawing(&cur_xml, &mode, value_cm, content_w_emu, ratio_override)
            {
                run.drawing_xml = Some(new_xml);
                run.byte_range = None;
                para_changed = true;
                resized += 1;
            }
        }
        if para_changed {
            p.dirty = true;
        }
    }

    if resized == 0 {
        return Err(AppError(
            "Không tìm thấy ảnh inline nào để resize trong vùng đã chọn".to_string(),
        ));
    }

    let new_doc_xml = save::rebuild_document_xml(&doc)?;
    let new_bytes =
        zip_io::write_replacing_entries(&mut archive, &[("word/document.xml", new_doc_xml)])?;
    zip_io::atomic_write(path, &new_bytes)?;
    emit_open_output(path)
}

// ===================== REPLACE IMAGE =====================

fn cmd_replaceimage(path: &str, media_entry: &str, src_path: &str) -> AppResult<()> {
    let full_entry = if media_entry.starts_with("word/") {
        media_entry.to_string()
    } else {
        format!("word/media/{media_entry}")
    };
    let mut archive = zip_io::open_archive(path)?;
    // Entry đích phải tồn tại (chỉ THAY, không tạo mới qua lệnh này).
    if zip_io::read_entry(&mut archive, &full_entry).is_err() {
        return Err(AppError(format!(
            "Image not found in package: {full_entry} (xem :DocxListZip)"
        )));
    }
    let new_bytes_img = std::fs::read(src_path)
        .map_err(|e| AppError(format!("Cannot read source image {src_path}: {e}")))?;
    // Ghi đè bytes vào ĐÚNG tên entry cũ -> mọi reference giữ nguyên, ảnh
    // mới hiển thị. Không đụng document.xml.
    let new_zip = zip_io::write_replacing_entries(
        &mut archive,
        &[(full_entry.as_str(), new_bytes_img)],
    )?;
    zip_io::atomic_write(path, &new_zip)?;
    emit_open_output(path)
}

// ===================== DELETE FILE (orphan only) =====================

fn cmd_deletefile(path: &str, entry: &str) -> AppResult<()> {
    let full_entry = if entry.starts_with("word/") {
        entry.to_string()
    } else {
        // Thử media trước, rồi embeddings.
        let m = format!("word/media/{entry}");
        let e = format!("word/embeddings/{entry}");
        let mut a = zip_io::open_archive(path)?;
        if zip_io::read_entry(&mut a, &m).is_ok() {
            m
        } else if zip_io::read_entry(&mut a, &e).is_ok() {
            e
        } else {
            return Err(AppError(format!("File not found in package: {entry}")));
        }
    };

    let mut archive = zip_io::open_archive(path)?;
    if zip_io::read_entry(&mut archive, &full_entry).is_err() {
        return Err(AppError(format!("File not found in package: {full_entry}")));
    }

    // Bảo vệ: chỉ cho xóa file trong media/ hoặc embeddings/ (không xóa
    // document.xml, rels, styles...).
    if !(full_entry.starts_with("word/media/") || full_entry.starts_with("word/embeddings/")) {
        return Err(AppError(format!(
            "Refusing to delete non-media file: {full_entry}"
        )));
    }

    let base = full_entry.rsplit('/').next().unwrap_or(&full_entry).to_string();
    let target_rel = full_entry.strip_prefix("word/").unwrap_or(&full_entry).to_string();

    // 1. Kiểm tra relationship: có rel nào trỏ tới file này không?
    let rels_bytes = zip_io::read_entry(&mut archive, "word/_rels/document.xml.rels")
        .unwrap_or_default();
    let referenced_by_rel = if rels_bytes.is_empty() {
        false
    } else {
        parse_rels(&rels_bytes)
            .values()
            .any(|t| t == &target_rel || t.ends_with(&target_rel) || t.ends_with(&base))
    };

    // 2. Kiểm tra marker [📎 base] trong document.xml (text đính kèm do
    //    :DocxInsertFile chèn). Chỉ match dạng marker có emoji 📎 + tên để
    //    tránh false-positive khi tên file tình cờ là chuỗi con ở chỗ khác.
    let doc_xml = zip_io::read_entry(&mut archive, "word/document.xml")?;
    let doc_str = String::from_utf8_lossy(&doc_xml);
    let referenced_by_marker = doc_str.contains(&format!("\u{1F4CE} {base}"));

    if referenced_by_rel || referenced_by_marker {
        return Err(AppError(format!(
            "Cannot delete '{base}': file is still attached/referenced in the document. \
Remove the image/attachment from the text first."
        )));
    }

    // An toàn để xóa.
    let new_zip = zip_io::write_with_changes(&mut archive, &[], &[full_entry.as_str()])?;
    zip_io::atomic_write(path, &new_zip)?;
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

/// Resolve `media_name` cho mọi run is_drawing=true trong doc, dùng
/// document.xml.rels để map rel_id -> filename thực (vd "image1.png").
/// Gọi sau parse_document, trước render — không có tác dụng gì nếu
/// document không có rels (lỗi đọc rels -> bỏ qua, giữ media_name=None,
/// render fallback về "[IMAGE]" không tên).
fn resolve_media_names<R: std::io::Read + std::io::Seek>(
    doc: &mut model::Document,
    archive: &mut zip::ZipArchive<R>,
) {
    let rels_xml = match zip_io::read_entry(archive, "word/_rels/document.xml.rels") {
        Ok(x) => x,
        Err(_) => return,
    };
    let rel_map = parse_rels(&rels_xml);
    for p in doc.paragraphs.iter_mut() {
        for run in p.runs.iter_mut() {
            if !run.is_drawing {
                continue;
            }
            let rel_id = match &run.rel_id {
                Some(r) => r,
                None => continue,
            };
            if let Some(target) = rel_map.get(rel_id.as_str()) {
                let filename = target.rsplit('/').next().unwrap_or(target.as_str());
                run.media_name = Some(filename.to_string());
            }
        }
    }
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
            media_name: None,
                drawing_xml: None,
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
            media_name: None,
                drawing_xml: None,
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
            media_name: None,
                drawing_xml: None,
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
            media_name: None,
                drawing_xml: None,
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
            media_name: None,
                drawing_xml: None,
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
            media_name: None,
                drawing_xml: None,
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
