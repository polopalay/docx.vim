//! Mô hình dữ liệu trong RAM cho 1 file DOCX, thiết kế hybrid: flatten mọi
//! paragraph (kể cả trong table cell, sdt, nested) thành list phẳng theo
//! thứ tự xuất hiện trong document.xml.
//!
//! Mỗi paragraph có `context` để render biết phải in prefix gì (├ cho cell,
//! không prefix cho top-level). Số dòng buffer = số paragraph + 1 dòng
//! separator/row cho mỗi table boundary -> luôn khớp với XML, save không
//! bao giờ báo line count mismatch.
//!
//! Minimal-diff save: paragraph chưa sửa copy nguyên byte gốc qua
//! byte_range; paragraph dirty mới re-emit XML.

#[derive(Debug, Clone)]
pub struct Run {
    pub text: String,
    pub style: RunStyle,
    /// True nếu run chứa <w:drawing> hoặc <w:pict> (image/shape inline).
    /// Run đó sẽ được render thành placeholder [IMAGE] để user thấy có
    /// ảnh; khi save không dirty thì copy nguyên byte gốc giữ ảnh.
    pub is_drawing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunStyle {
    pub bold: bool,
    pub italic: bool,
    pub size_half_pt: Option<u32>,
    pub color_hex: Option<String>,
    /// Màu highlight (<w:highlight>) — value là tên màu chuẩn (yellow,
    /// green, ...) chứ không phải hex. None = không highlight.
    pub highlight: Option<String>,
    /// Tên font chữ (vd "Times New Roman", "Roboto", "Arial"). DOCX lưu
    /// trong <w:rFonts w:ascii="..." w:hAnsi="..."/>. Mình lấy ascii là
    /// chính (font cho ký tự latin), fallback hAnsi.
    pub font_name: Option<String>,
}

impl RunStyle {
    pub fn is_default(&self) -> bool {
        !self.bold
            && !self.italic
            && self.size_half_pt.is_none()
            && self.color_hex.is_none()
            && self.highlight.is_none()
            && self.font_name.is_none()
    }
}

/// Context structural của paragraph — render dùng để biết phải in prefix gì.
#[derive(Debug, Clone)]
pub enum ParaContext {
    Body,
    /// Paragraph nằm trong cell của table. `table_depth` để render prefix
    /// theo độ sâu nesting (hiện chưa dùng — dự trữ cho nested table sau).
    /// `is_first_in_row` true cho paragraph đầu tiên của row mới -> render
    /// thêm dòng separator phía trên (├──┤).
    TableCell {
        #[allow(dead_code)]
        table_depth: u32,
        is_first_in_row: bool,
        is_last_in_row: bool,
    },
    Sdt,
}

#[derive(Debug, Clone)]
pub struct Paragraph {
    pub id: String,
    pub runs: Vec<Run>,
    pub para_style: Option<String>,
    pub alignment: Option<String>,
    /// Indent của paragraph theo đơn vị TWIPS (1/20 point). DOCX lưu trong
    /// <w:ind w:left="720"/>. 720 twips = 0.5 inch = ~1 cấp indent.
    /// None = mặc định (0). Tăng/giảm qua :DocxIndent.
    pub indent_twips: Option<i32>,
    /// Numbering reference (numId) cho list paragraph. None = không phải
    /// list item. Khi user đổi prefix list, mình GIỮ NGUYÊN <w:numPr> gốc
    /// — Word sẽ tự dùng numbering.xml để render đúng prefix.
    pub num_id: Option<u32>,
    /// Indent level trong list (ilvl). 0 = level top (vd "1."), 1 = sub
    /// (vd "1.1"), 2 = sub-sub (vd "1.1.1"). Chỉ có ý nghĩa khi num_id
    /// is Some. Tăng Tab -> +1 (max 8 theo schema DOCX).
    pub num_ilvl: Option<u32>,
    pub byte_range: (usize, usize),
    pub dirty: bool,
    pub context: ParaContext,
}

#[derive(Debug, Default)]
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    /// Boundary của body — phần XML trước <w:body> mở và sau </w:body> đóng
    /// được giữ nguyên byte khi save.
    pub body_inner_range: (usize, usize),
    pub original_xml: Vec<u8>,
}
