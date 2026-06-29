# Vim DOCX Viewer

Edit Microsoft Word `.docx` documents directly inside Vim or Neovim.

Vim DOCX Viewer renders Word documents as editable plain text while preserving document structure, paragraph formatting, tables, lists, and embedded objects. Changes are written back to the original document using a fast Rust backend.

No Microsoft Word, LibreOffice, WPS Office, Python, Java, or OpenXML SDK is required.

---

## Features

### Document

* Open `.docx` files directly inside Vim or Neovim
* Save changes back to the original document
* Automatic reload after every save
* Automatic first-time Rust build
* Pure Rust backend
* Automatic override of Vim's built-in `zip.vim` support for DOCX files
* No temporary Office conversion
* Minimal document rewriting

### Editing

* Edit Word documents using normal Vim commands
* Insert new paragraphs above or below the current paragraph
* Delete paragraphs
* Automatic paragraph insertion
* Preserve paragraph formatting when inserting new content
* Automatic document reformatting after every save

### Paragraph Navigation

* Jump directly to any paragraph (`P0`, `P1`, ...)
* Tab completion for paragraph IDs
* Paragraph-aware editing commands
* Statusline displays the active paragraph automatically
* Supports both Normal mode and Visual mode operations

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
* Automatic list continuation
* Automatic list exit on empty items
* Add list items
* Delete list items

### Tables

* Preserve Word tables
* Render tables using ASCII borders
* Preserve merged cells
* Preserve table formatting
* Edit table content directly inside Vim

### Media

* Preserve embedded images
* Preserve embedded files
* Preserve OLE objects
* Open embedded images directly from Vim
* Extract embedded media automatically
* Open embedded Excel workbooks inside Vim (when the Excel plugin is installed)
* Open attachments using the operating system's default application

### Terminal Image Preview

* Inline image preview in Kitty
* Inline image preview in Ghostty
* Inline image preview in WezTerm
* Sixel terminal support
* Automatic fallback to `chafa`
* Automatic fallback to the system image viewer

### Hover Information

* Display formatting information at the cursor
* Font family
* Font size
* Bold / Italic state
* Font color
* Highlight color
* Paragraph ID
* Automatic popup on `CursorHold`

### Rendering

* Built-in syntax highlighting
* URLs
* Headings
* Bullets
* Table borders
* Existing Word formatting
* Bold rendering
* Italic rendering
* Font colors
* Highlight colors

### Performance

* Pure Rust parser and writer
* Fast incremental style updates
* Automatic binary build
* No Python required
* No Java required
* No Microsoft Word required
* No LibreOffice required
* No WPS Office required

### Compatibility

* Vim 8.2+
* Neovim 0.7+
* Linux
* macOS
* Windows

---

## Supported Features

| Feature                      | Status |
| ---------------------------- | ------ |
| Read DOCX                    | ✓      |
| Write DOCX                   | ✓      |
| Automatic Build              | ✓      |
| Automatic Reload             | ✓      |
| Paragraph Navigation         | ✓      |
| Paragraph Statusline         | ✓      |
| Hover Formatting Information | ✓      |
| Paragraph Insert             | ✓      |
| Paragraph Delete             | ✓      |
| Bold                         | ✓      |
| Italic                       | ✓      |
| Font Family                  | ✓      |
| Font Size                    | ✓      |
| Font Color                   | ✓      |
| Text Highlight               | ✓      |
| Paragraph Alignment          | ✓      |
| Paragraph Indentation        | ✓      |
| Bullet Lists                 | ✓      |
| Numbered Lists               | ✓      |
| Nested Lists                 | ✓      |
| Smart List Editing           | ✓      |
| Smart Enter                  | ✓      |
| Preserve Tables              | ✓      |
| Preserve Merged Cells        | ✓      |
| Preserve Images              | ✓      |
| Preserve OLE Objects         | ✓      |
| Open Embedded Images         | ✓      |
| Open Embedded Attachments    | ✓      |
| Embedded Excel Support       | ✓      |
| Terminal Image Preview       | ✓      |
| Preserve Headers             | ✓      |
| Preserve Footers             | ✓      |
| Preserve Comments            | ✓      |
| Preserve Formatting          | ✓      |
| DOC Format                   | ✗      |
| Image Editing                | ✗      |
| Header Editing               | ✗      |
| Footer Editing               | ✗      |
| Comment Editing              | ✗      |
| Track Changes                | ✗      |

---

## Key Bindings

| Key          | Action                                 |
| ------------ | -------------------------------------- |
| `Tab`        | Increase paragraph or list indentation |
| `Shift-Tab`  | Decrease paragraph or list indentation |
| `o`          | Insert a new paragraph below           |
| `O`          | Insert a new paragraph above           |
| `gx`         | Open embedded image or attachment      |
| `CursorHold` | Show formatting information popup      |

---

## Commands

| Command              | Description                  |
| -------------------- | ---------------------------- |
| `:DocxBuild`         | Build the Rust backend       |
| `:DocxSave`          | Save the current document    |
| `:DocxGoto`          | Jump to a paragraph          |
| `:DocxBold`          | Toggle bold                  |
| `:DocxItalic`        | Toggle italic                |
| `:DocxFont`          | Change font family           |
| `:DocxSize`          | Change font size             |
| `:DocxColor`         | Change font color            |
| `:DocxHighlight`     | Change text highlight        |
| `:DocxAlign`         | Change paragraph alignment   |
| `:DocxIndent`        | Change paragraph indentation |
| `:DocxListAdd`       | Insert a paragraph below     |
| `:DocxListAddBefore` | Insert a paragraph above     |
| `:DocxListDel`       | Delete the current paragraph |
| `:DocxListEnter`     | Smart Enter for lists        |
| `:DocxOpen`          | Open embedded media          |
| `:DocxInfo`          | Show formatting information  |
| `:DocxGoto`          | Jump to a paragraph          |
| `:DocxDebug`         | Debug paragraph mapping      |

---

## Limitations

* Only `.docx` files are supported.
* Images and embedded objects are preserved but cannot be edited.
* Headers and footers are preserved but cannot be edited.
* Comments are preserved but cannot be edited.
* Track Changes is not supported.
* Extremely complex Word layouts may not render identically to Microsoft Word.

---

## ZIP Plugin Compatibility

DOCX files are ZIP containers internally.

The plugin automatically overrides Vim's built-in `zip.vim` handlers for `.docx` files, allowing documents to open directly without additional configuration.

---

## License

MIT

---

## Credits

Built with

* Rust
* Vim
* Neovim
* ZIP
* quick-xml
