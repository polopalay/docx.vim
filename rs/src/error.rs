//! Kiểu lỗi đơn giản dùng chung cho toàn bộ binary docx — chỉ chứa 1
//! message string, không phân biệt source (io/xml/zip), vì với CLI tool
//! người dùng chỉ cần biết "có lỗi gì" qua stderr là đủ.

use std::fmt;

pub struct AppError(pub String);

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError(format!("io error: {e}"))
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(e: zip::result::ZipError) -> Self {
        AppError(format!("zip error: {e}"))
    }
}

impl From<quick_xml::Error> for AppError {
    fn from(e: quick_xml::Error) -> Self {
        AppError(format!("xml error: {e}"))
    }
}

impl From<std::str::Utf8Error> for AppError {
    fn from(e: std::str::Utf8Error) -> Self {
        AppError(format!("utf-8 error: {e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
