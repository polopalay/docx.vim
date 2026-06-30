//! Zip I/O cho DOCX. DOCX = zip chứa nhiều file XML; mỗi lần save chỉ cần
//! ghi đè 1-2 file (word/document.xml + thỉnh thoảng word/styles.xml), giữ
//! nguyên các entry khác y nguyên byte (theme, fontTable, images...).

use crate::error::{AppError, AppResult};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub fn open_archive(path: &str) -> AppResult<ZipArchive<File>> {
    let f = File::open(path)?;
    Ok(ZipArchive::new(f)?)
}

pub fn read_entry<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> AppResult<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .map_err(|e| AppError(format!("entry {name}: {e}")))?;
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Liệt kê tên + kích thước (uncompressed bytes) của mọi entry trong zip,
/// theo thứ tự lưu trữ gốc. Dùng cho lệnh `listzip` — hiển thị cấu trúc
/// nội bộ DOCX (word/media/imageN.png, word/document.xml, ...) cho user.
pub fn list_entries<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<(String, u64)> {
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            out.push((entry.name().to_string(), entry.size()));
        }
    }
    out
}

/// Ghi 1 file DOCX mới bằng cách clone toàn bộ entries từ archive gốc, trừ
/// các entries có trong `replacements` thì dùng nội dung mới. Cách này giữ
/// nguyên byte mọi file XML/binary không liên quan đến phần user sửa —
/// đúng yêu cầu "minimal diff".
pub fn write_replacing_entries<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    replacements: &[(&str, Vec<u8>)],
) -> AppResult<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut out);
        let replacement_names: std::collections::HashSet<&str> =
            replacements.iter().map(|(n, _)| *n).collect();

        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i)?;
            let name = entry.name().to_string();
            if replacement_names.contains(name.as_str()) {
                continue; // sẽ ghi bản mới ở dưới
            }
            // raw_copy_file giữ nguyên compression method + byte content
            // của entry gốc -> không thay đổi gì.
            writer.raw_copy_file(entry)?;
        }

        for (name, bytes) in replacements {
            let opts = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o644);
            writer.start_file(*name, opts)?;
            writer.write_all(bytes)?;
        }
        writer.finish()?;
    }
    Ok(out.into_inner())
}

/// Atomic write: ghi vào file tạm rồi rename, tránh để file gốc bị corrupt
/// nếu process bị kill giữa chừng.
pub fn atomic_write(path: &str, bytes: &[u8]) -> AppResult<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Tổng quát hơn write_replacing_entries: ghi lại zip với cả replacements
/// (entry thay nội dung / thêm mới) lẫn removals (entry bị XÓA hẳn). Mọi
/// entry không nằm trong cả hai danh sách được copy nguyên byte.
/// - replacements: (name, bytes) -> ghi mới (đè nếu trùng tên).
/// - removals: tên entry cần loại bỏ hoàn toàn khỏi zip.
pub fn write_with_changes<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    replacements: &[(&str, Vec<u8>)],
    removals: &[&str],
) -> AppResult<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut out);
        let replacement_names: std::collections::HashSet<&str> =
            replacements.iter().map(|(n, _)| *n).collect();
        let removal_names: std::collections::HashSet<&str> = removals.iter().copied().collect();

        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i)?;
            let name = entry.name().to_string();
            if replacement_names.contains(name.as_str()) || removal_names.contains(name.as_str()) {
                continue;
            }
            writer.raw_copy_file(entry)?;
        }

        for (name, bytes) in replacements {
            let opts = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o644);
            writer.start_file(*name, opts)?;
            writer.write_all(bytes)?;
        }
        writer.finish()?;
    }
    Ok(out.into_inner())
}
