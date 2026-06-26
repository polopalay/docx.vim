# Vim DOCX Viewer

Edit Microsoft Word `.docx` documents directly inside Vim or Neovim.

Vim DOCX Viewer renders Word documents as editable plain text while preserving document structure and common paragraph formatting. Changes are written back to the original document using a fast Rust backend.

No Microsoft Word, LibreOffice, WPS Office, Python, Java, or OpenXML SDK is required.

---

## Features

### Document

* Open `.docx` files directly in Vim or Neovim
* Save changes back to the original document
* Automatic reload after saving
* Automatic first-time Rust build
* Pure Rust backend

### Paragraph Editing

* Edit document text using normal Vim commands
* Jump directly to any paragraph (`P0`, `P1`, ...)
* Statusline displays the active paragraph
* Supports Normal mode and Visual mode operations
* Add new paragraphs
* Delete paragraphs
* Smart paragraph insertion

### Formatting

* Preserve existing formatting
* Toggle **Bold**
* Toggle *Italic*
* Change font family
* Change font size
* Change font color
* Change text highlight
* Change paragraph alignment
* Increase or decrease paragraph indentation
* Supports named colors and custom `#RRGGBB` colors

### Lists

* Preserve Word lists
* Bullet lists
* Numbered lists
* Nested lists
* Increase list level
* Decrease list level
* Smart Enter behavior
* Add list items
* Delete list items

### Rendering

* Built-in syntax highlighting
* URLs
* Headings
* Bullets
* Table borders
* Real Word formatting (bold, italic, font color)

---

## Requirements

### Vim / Neovim

* Vim 8.2+
* Neovim 0.7+

### Rust

Rust and Cargo are required only for the initial build.

Verify installation:

```bash
cargo --version
```

Install Rust:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Installation

### vim-plug

```vim
Plug 'polopalay/docx.vim'
```

Then:

```vim
:PlugInstall
```

### lazy.nvim

```lua
{
    "polopalay/docx.vim",
}
```

---

## First Build

The Rust backend is built automatically the first time a DOCX file is opened.

To build manually:

```vim
:DocxBuild
```

Generated binary:

Linux / macOS

```text
rs/target/release/docx
```

Windows

```text
rs/target/release/docx.exe
```

---

## Opening Documents

```bash
vim report.docx
```

or

```bash
nvim report.docx
```

Example:

```text
# Project Report

This document was edited inside Vim.

• Item 1
• Item 2

+-----------------------+
| Table Content         |
+-----------------------+
```

Save normally:

```vim
:w
```

or

```vim
:DocxSave
```

---

# Paragraph Navigation

Jump directly to a paragraph.

```vim
:DocxGoto P15
```

Tab completion is supported.

The current paragraph is displayed automatically in the Vim statusline.

Example:

```text
P15
```

---

# Formatting Commands

## Font Color

```vim
:DocxColor red
```

Visual mode:

```vim
:'<,'>DocxColor blue
```

## Text Highlight

```vim
:DocxHighlight yellow
```

Visual mode:

```vim
:'<,'>DocxHighlight "#FFE599"
```

## Font

```vim
:DocxFont "Times New Roman"
```

## Font Size

```vim
:DocxSize 14
```

## Alignment

```vim
:DocxAlign center
```

## Indentation

```vim
:DocxIndent +1
```

## Bold

```vim
:DocxBold
```

Visual selection:

```vim
:'<,'>DocxBold
```

## Italic

```vim
:DocxItalic
```

Visual selection:

```vim
:'<,'>DocxItalic
```

---

# List Commands

Insert a new list item:

```vim
:DocxListAdd
```

Delete the current list item:

```vim
:DocxListDel
```

Increase list level:

```text
Tab
```

Decrease list level:

```text
Shift-Tab
```

---

# Supported Colors

Named colors:

```
red
green
blue
yellow
orange
purple
gray
white
black
none
```

Custom colors:

```
#RRGGBB
```

Example:

```vim
:DocxColor "#2E75B6"
```

---

# Commands

| Command          | Description                  |
| ---------------- | ---------------------------- |
| `:DocxBuild`     | Build Rust backend           |
| `:DocxSave`      | Save document                |
| `:DocxGoto`      | Jump to a paragraph          |
| `:DocxBold`      | Toggle bold                  |
| `:DocxItalic`    | Toggle italic                |
| `:DocxFont`      | Change font family           |
| `:DocxSize`      | Change font size             |
| `:DocxColor`     | Change font color            |
| `:DocxHighlight` | Change text highlight        |
| `:DocxAlign`     | Change paragraph alignment   |
| `:DocxIndent`    | Change paragraph indentation |
| `:DocxListAdd`   | Insert paragraph/list item   |
| `:DocxListDel`   | Delete paragraph/list item   |
| `:DocxInfo`      | Show formatting at cursor    |

---

# Syntax Highlighting

Built-in highlighting includes:

* Headings
* URLs
* Bullets
* Table borders
* Existing Word bold text
* Existing Word italic text
* Existing Word font colors

---

# Supported Features

| Feature                        | Status |
| ------------------------------ | ------ |
| Read DOCX                      | ✓      |
| Write DOCX                     | ✓      |
| Paragraph Navigation           | ✓      |
| Statusline Paragraph Indicator | ✓      |
| Bold                           | ✓      |
| Italic                         | ✓      |
| Font Family                    | ✓      |
| Font Size                      | ✓      |
| Font Color                     | ✓      |
| Text Highlight                 | ✓      |
| Paragraph Alignment            | ✓      |
| Paragraph Indentation          | ✓      |
| Bullet Lists                   | ✓      |
| Numbered Lists                 | ✓      |
| Nested Lists                   | ✓      |
| Smart List Editing             | ✓      |
| Preserve Tables                | ✓      |
| Preserve Formatting            | ✓      |
| DOC Format                     | ✗      |
| Images Editing                 | ✗      |
| Headers / Footers Editing      | ✗      |
| Track Changes                  | ✗      |
| Comments                       | ✗      |

---

# ZIP Plugin Compatibility

DOCX documents are ZIP containers internally.

The plugin automatically overrides Vim's built-in `zip.vim` handlers for `.docx` files, so no additional configuration is required.

---

# Limitations

* Only `.docx` files are supported
* Images are preserved but cannot be edited
* Headers and footers are preserved
* Comments are preserved but cannot be edited
* Track Changes is not supported
* Extremely complex Word layouts may not render identically to Microsoft Word

---

# License

MIT

---

# Credits

Built with

* Rust
* Vim
* Neovim
* ZIP
* quick-xml
