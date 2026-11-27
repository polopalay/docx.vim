//! cargo test --release : mở -> sửa buffer -> lưu, kiểm tra không mất dữ liệu.
use std::io::{Read, Write};
use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_docx")).args(args).output().unwrap();
    let mut s = String::from_utf8(o.stdout).unwrap();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.success(), s)
}

struct Doc(String);

impl Doc {
    fn new(name: &str, body: &str) -> Doc {
        let path = format!("{}/{name}.docx", env!("CARGO_TARGET_TMPDIR"));
        let xml = format!("<w:document xmlns:w=\"w\" xmlns:r=\"r\"><w:body>{body}<w:sectPr/></w:body></w:document>");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        z.start_file("word/document.xml", zip::write::FileOptions::default()).unwrap();
        z.write_all(xml.as_bytes()).unwrap();
        z.finish().unwrap();
        Doc(path)
    }
    fn xml(&self) -> String {
        let mut z = zip::ZipArchive::new(std::fs::File::open(&self.0).unwrap()).unwrap();
        let mut s = String::new();
        z.by_name("word/document.xml").unwrap().read_to_string(&mut s).unwrap();
        s
    }
    fn open(&self) -> String {
        run(&["open", &self.0]).1
    }
    fn buffer(&self) -> String {
        self.open().split("\n@@STYLE@@").next().unwrap().to_string()
    }
    fn save(&self, buf: &str) -> (bool, String) {
        let tmp = format!("{}.txt", self.0);
        std::fs::write(&tmp, format!("{buf}\n")).unwrap();
        run(&["save", &self.0, &tmp])
    }
    fn edit(&self, from: &str, to: &str) {
        let b = self.buffer();
        assert!(b.contains(from), "{from:?} not in {b:?}");
        let (ok, out) = self.save(&b.replace(from, to));
        assert!(ok, "{out}");
    }
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

const RICH: &str = "<w:p><w:pPr><w:spacing w:after=\"0\"/></w:pPr><w:r><w:rPr><w:b/><w:u w:val=\"single\"/></w:rPr><w:t>Title here</w:t></w:r></w:p>\
<w:p><w:r><w:t>before</w:t><w:br w:type=\"page\"/><w:t>after</w:t></w:r></w:p>\
<w:p><w:hyperlink r:id=\"r1\"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:footnoteReference w:id=\"1\"/></w:r><w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\"> tail</w:t><w:tab/><w:t>end</w:t></w:r></w:p>";

#[test]
fn no_edit_is_byte_identical() {
    let d = Doc::new("noedit", RICH);
    let before = d.xml();
    assert!(d.save(&d.buffer()).0);
    assert_eq!(before, d.xml());
}

#[test]
fn edit_keeps_everything_else() {
    let d = Doc::new("edit", RICH);
    let before = d.xml();
    d.edit("after", "AFTER"); // thay thế ngay sau <w:br>: không dời qua element khác
    assert_eq!(before.replace(">after<", ">AFTER<"), d.xml());
    let before = d.xml();
    d.edit("Title here", "Title now here");
    assert_eq!(before.replace(">Title here<", ">Title now here<"), d.xml());
    let before = d.xml();
    d.edit("link tail\tend", "link tail\tEND");
    assert_eq!(before.replace(">end<", ">END<"), d.xml());
}

#[test]
fn style_command_keeps_other_properties() {
    let d = Doc::new("style", RICH);
    assert!(run(&["setstyle", &d.0, "P0", "toggleitalic", ""]).0);
    assert!(run(&["setstyle", &d.0, "P0", "align", "center"]).0);
    let x = d.xml();
    assert!(x.contains("<w:rPr><w:b/><w:i/><w:u w:val=\"single\"/></w:rPr>"), "{x}");
    assert!(x.contains("<w:spacing w:after=\"0\"/><w:jc w:val=\"center\"/>"), "{x}");
}

#[test]
fn image_paragraph_text_is_editable() {
    let d = Doc::new("image", "<w:p><w:r><w:t>see </w:t></w:r><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>boxed</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r></w:p><w:p><w:r><w:t>next</w:t></w:r></w:p>");
    d.edit("see ", "look ");
    let x = d.xml();
    assert!(x.contains("look ") && x.contains("<w:t>boxed</w:t></w:r></w:p></w:txbxContent></w:drawing>"), "{x}");
    assert!(!d.save(&d.buffer().replace("[IMAGE]", "")).0);
}

#[test]
fn replaced_and_new_lines() {
    let d = Doc::new("lines", &[p("alpha"), p("beta"), p("gamma")].concat());
    let (ok, o) = d.save("alpha\ncompletely different\nbrand new\ngamma");
    assert!(ok, "{o}");
    assert_eq!(d.buffer(), "alpha\ncompletely different\nbrand new\ngamma");
}

#[test]
fn table_edits_and_unsupported_lines() {
    let cell = |t: &str| format!("<w:tc>{}</w:tc>", p(t));
    let body = format!("{}<w:tbl><w:tr>{}{}</w:tr></w:tbl>{}", p("top"), cell("left"), cell("right"), p("bottom"));
    let d = Doc::new("table", &body);
    d.edit("right", "RIGHT");
    assert!(d.xml().contains(">RIGHT<") && d.xml().contains(">left<"));
    // dòng mới ngay sau bảng không được biến mất
    let b = d.buffer().replace("┘", "┘\nafter table");
    let (ok, o) = d.save(&b);
    assert!(ok, "{o}");
    assert!(d.xml().contains("after table"));
    // dòng chỉ có số trong bảng cũng không được mất âm thầm
    let before = d.xml();
    assert!(!d.save(&d.buffer().replace("\n└", "\n│ 150.000 │ 2024 │\n│ 7 │ 8 │\n└")).0);
    assert_eq!(before, d.xml());
    // gõ thêm dòng trong bảng: từ chối thay vì mất chữ
    let before = d.xml();
    let b = d.buffer().replace("\n└", "\n│ zzz  │      │\n│ qqq  │      │\n└");
    assert!(!d.save(&b).0);
    assert_eq!(before, d.xml());
}

#[test]
fn replacing_first_word_of_a_run_keeps_its_style() {
    let d = Doc::new("runs", "<w:p><w:r><w:t xml:space=\"preserve\">a </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r></w:p>");
    d.edit("a bold", "a BOLD");
    assert!(d.xml().contains("<w:b/></w:rPr><w:t>BOLD</w:t>"), "{}", d.xml());
}

#[test]
fn page_breaks_reported() {
    let d = Doc::new("pages", &format!("{}<w:p><w:pPr><w:pageBreakBefore/></w:pPr><w:r><w:t>two</w:t></w:r></w:p>", p("one")));
    assert!(d.open().contains("@@PAGEBREAK@@\n2\t2\t0\n@@PAGEBREAKEND@@"), "{}", d.open());
    let long = Doc::new("long", &(0..200).map(|i| p(&format!("line {i}"))).collect::<String>());
    assert!(long.open().split("@@PAGEBREAK@@").nth(1).unwrap().contains("\t2\t1"));
}

#[test]
fn deleting_lettered_list_item_saves() {
    let item = |t: &str| format!("<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>");
    let d = Doc::new("letters", &[item("one"), item("two"), item("three")].concat());
    let numbering = r#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1)"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&d.0).unwrap();
    let mut z = zip::ZipWriter::new_append(f).unwrap();
    z.start_file("word/numbering.xml", zip::write::FileOptions::default()).unwrap();
    z.write_all(numbering.as_bytes()).unwrap();
    z.finish().unwrap();
    assert_eq!(d.buffer(), "a) one\nb) two\nc) three");
    let (ok, o) = d.save("a) one\nc) three");
    assert!(ok, "{o}");
    assert_eq!(d.buffer(), "a) one\nb) three");
}

#[test]
fn refuses_empty_buffer_and_row_removal() {
    let cell = |t: &str| format!("<w:tc>{}</w:tc>", p(t));
    let row = |a: &str, b: &str| format!("<w:tr>{}{}</w:tr>", cell(a), cell(b));
    let body = format!("{}<w:tbl>{}{}{}</w:tbl>", p("top"), row("a1", "b1"), row("a2", "b2"), row("a3", "b3"));
    let d = Doc::new("rows", &body);
    let before = d.xml();
    let lines: Vec<String> = d.buffer().lines().map(str::to_string).collect();
    let at = lines.iter().position(|l| l.contains("a2")).unwrap();
    for remove in [1, 2] {
        let mut l = lines.clone();
        l.drain(at..at + remove);
        assert!(!d.save(&l.join("\n")).0);
    }
    assert!(!d.save("").0);
    assert!(!d.save("top").0); // xoá cả bảng
    assert_eq!(before, d.xml());
    d.edit("a2", "A2");
    assert!(d.xml().contains(">A2<") && d.xml().contains(">a3<"));
}

#[test]
fn numbered_item_containing_its_number() {
    let d = Doc::new("num", "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>1</w:t></w:r></w:p>");
    let b = d.buffer();
    assert!(d.save(&format!("{b}0")).0);
    assert!(d.xml().contains("<w:t>10</w:t>"), "{}", d.xml());
}
