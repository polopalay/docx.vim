" ============================================================================
" GUARD & KHAI BÁO BIẾN GỐC
" ============================================================================
" Ngăn plugin load lại nhiều lần
if exists('g:loaded_docxPlugin')
    finish
endif
let g:loaded_docxPlugin = 1
" Xác định đường dẫn gốc của plugin. Hỗ trợ 2 cách bố trí:
"   (a) Chuẩn Vim plugin: docxPlugin.vim nằm ở <plugin>/plugin/docxPlugin.vim
"       -> s:plugin_root lùi 1 cấp ra <plugin>/, rs/ là sibling.
"   (b) Flat: docxPlugin.vim nằm thẳng trong <plugin>/, rs/ cùng cấp.
" Cách dò: thử cả 2 vị trí Cargo.toml, lấy cái tồn tại; nếu không có nào,
" mặc định theo (a) để báo lỗi rõ ràng cho user khi build.
let s:script_dir = fnamemodify(expand('<sfile>:p:h'), ':p:h')
let s:parent_dir = fnamemodify(s:script_dir, ':h')
if filereadable(s:parent_dir . '/rs/Cargo.toml')
    " Trường hợp (a): nằm trong plugin/<file>.vim
    let s:rs_root = s:parent_dir . '/rs'
elseif filereadable(s:script_dir . '/rs/Cargo.toml')
    " Trường hợp (b): flat layout
    let s:rs_root = s:script_dir . '/rs'
else
    " Không tìm thấy — vẫn set theo (a) để báo lỗi rõ khi user gõ :DocxBuild
    let s:rs_root = s:parent_dir . '/rs'
endif
" Xác định đường dẫn tới binary docx đã build (release)
if has('win32') || has('win64')
    let s:docx_bin = s:rs_root . '/target/release/docx.exe'
else
    let s:docx_bin = s:rs_root . '/target/release/docx'
endif
" ============================================================================
" INIT - AUTOCMD MỞ FILE / FILETYPE / OVERRIDE ZIP
" ============================================================================
" Khi mở *.docx -> gọi DocxOpen() thay vì để Vim mở binary
augroup DocxViewer
    autocmd!
    autocmd BufReadCmd *.docx call DocxOpen()
    " File .docx MỚI (chưa tồn tại trên disk): Vim fire BufNewFile thay vì
    " BufReadCmd. Tạo template DOCX trống rồi mở như bình thường.
    autocmd BufNewFile *.docx call DocxNew()
augroup END
" Đảm bảo DocxOpen() được ưu tiên hơn cơ chế zip.vim của Vim (docx = zip)
function! s:EnsureDocxOverridesZip() abort
    if exists('#zip#BufReadCmd#*.docx')
        autocmd! zip BufReadCmd *.docx
    endif
    if exists('#zip#BufWriteCmd#*.docx')
        autocmd! zip BufWriteCmd *.docx
    endif
endfunction
augroup DocxViewerInit
    autocmd!
    autocmd VimEnter,SourcePost * call s:EnsureDocxOverridesZip()
augroup END
" Tự động gán filetype=docx cho mọi file .docx
augroup DocxFiletype
    autocmd!
    autocmd BufRead,BufNewFile *.docx setfiletype docx
augroup END
" ----------------------------------------------------------------------------
" Đảm bảo binary docx đã build, nếu chưa thì build
" ----------------------------------------------------------------------------
function! s:EnsureBuilt() abort
    if filereadable(s:docx_bin)
        return 1
    endif
    echo 'First build docx...'
    if executable('cargo') == 0
        echoerr 'cargo not found in PATH'
        return 0
    endif
    let l:cmd =
                \ 'cargo build --release --manifest-path '
                \ . shellescape(s:rs_root . '/Cargo.toml')
    let l:out = systemlist(l:cmd)
    if v:shell_error
        echoerr join(l:out, "\n")
        return 0
    endif
    return filereadable(s:docx_bin)
endfunction
" ============================================================================
" HIGHLIGHT - tô màu theo style metadata từ binary
" ============================================================================
" ----------------------------------------------------------------------------
" DocxSetupHighlight(): setup syntax highlight cho buffer DOCX
" ----------------------------------------------------------------------------
function! DocxSetupHighlight()
    silent! call clearmatches()
    " Định nghĩa highlight group cho border bảng và link
    highlight DocxBorder guifg=#9ca0b0 ctermfg=245
    highlight DocxLink   guifg=#0000ff ctermfg=12 gui=underline cterm=underline
    highlight DocxHeading gui=bold cterm=bold
    " Highlight group cho popup hover: nền tối hơn buffer, border subtle.
    highlight default DocxHoverPopup  guibg=#2d2d3a guifg=#e0e0e0 ctermbg=237 ctermfg=255
    highlight default DocxHoverBorder guifg=#7080a0 ctermfg=103
    " --- Viền bảng / structural markers: + | - và các box-drawing chars ---
    call matchadd('DocxBorder', '[+]', 5)
    call matchadd('DocxBorder', '[|]', 5)
    call matchadd('DocxBorder', '-\+', 5)
    " Box-drawing characters dùng cho TABLE_START / ROW_SEP / TABLE_END
    " và prefix '├ ' của cell paragraph trong render mới.
    call matchadd('DocxBorder', '[┌┐└┘├┤┬┴┼─│┊]', 5)
    " --- URL: http/https/ftp ---
    call matchadd(
            \ 'DocxLink',
            \ '\v%(https?|ftp)://[[:alnum:]/._~:?#[\]@!$&''()*+,;=%-]+',
            \ 30
            \ )
    " --- Heading prefix: dòng bắt đầu bằng '# ', '## ', ... ---
    call matchadd('DocxHeading', '^#\+ .*$', 10)
    " --- Bullet prefix ---
    highlight DocxBullet guifg=#0070C0 ctermfg=32
    call matchadd('DocxBullet', '^[•] ', 10)
    " --- Style thực từ DOCX (bold/italic/font-size/font-color) ---
    call s:DocxApplyCellStyles()
endfunction
" ----------------------------------------------------------------------------
" s:DocxHighlightGroupName(): sinh tên highlight group duy nhất, ổn định
" theo tổ hợp (bold, italic, size_pt, color_hex).
" ----------------------------------------------------------------------------
function! s:DocxHighlightGroupName(bold, italic, size_pt, color_hex, hl) abort
    let l:key = a:bold . a:italic . '_' . a:size_pt . '_' . a:color_hex . '_' . a:hl
    let l:key = substitute(l:key, '[^A-Za-z0-9_]', '_', 'g')
    return 'DocxRunStyle_' . l:key
endfunction
" ----------------------------------------------------------------------------
" Bộ màu Word highlight chuẩn -> hex RGB cho `guibg`. Dùng để vẽ background
" trong Vim cho run có <w:highlight>.
" ----------------------------------------------------------------------------
let s:docx_highlight_hex = {
            \ 'yellow':      'FFFF00',
            \ 'green':       '00FF00',
            \ 'cyan':        '00FFFF',
            \ 'magenta':     'FF00FF',
            \ 'blue':        '0000FF',
            \ 'red':         'FF0000',
            \ 'darkBlue':    '000080',
            \ 'darkCyan':    '008080',
            \ 'darkGreen':   '008000',
            \ 'darkMagenta': '800080',
            \ 'darkRed':     '800000',
            \ 'darkYellow':  '808000',
            \ 'darkGray':    '808080',
            \ 'lightGray':   'C0C0C0',
            \ 'black':       '000000',
            \ 'white':       'FFFFFF',
            \ }
function! s:DocxDefineHighlight(group, bold, italic, color_hex, hl) abort
    let l:attrs = []
    if a:bold
        call add(l:attrs, 'bold')
    endif
    if a:italic
        call add(l:attrs, 'italic')
    endif
    let l:gui_attr = empty(l:attrs) ? 'NONE' : join(l:attrs, ',')
    " term=, cterm=, gui= cùng giá trị để cover mọi loại terminal (tty,
    " xterm-256, gVim, Neovim GUI). Một số terminal chỉ honor term= cho
    " bold, không read cterm=.
    let l:cmd = 'highlight ' . a:group
                \ . ' term=' . l:gui_attr
                \ . ' cterm=' . l:gui_attr
                \ . ' gui=' . l:gui_attr
    if a:color_hex !=# '-'
        let l:cmd .= ' guifg=#' . a:color_hex
        let l:cmd .= ' ctermfg=' . s:DocxHexToCterm(a:color_hex)
    endif
    if a:hl !=# '-'
        let l:bg_hex = get(s:docx_highlight_hex, a:hl, '')
        if !empty(l:bg_hex)
            let l:cmd .= ' guibg=#' . l:bg_hex
            let l:cmd .= ' ctermbg=' . s:DocxHexToCterm(l:bg_hex)
        endif
    endif
    execute l:cmd
endfunction
" ----------------------------------------------------------------------------
" s:DocxHexToCterm(): quy đổi gần đúng "RRGGBB" -> mã màu xterm-256 cho
" terminal không hỗ trợ guifg trực tiếp.
" ----------------------------------------------------------------------------
function! s:DocxHexToCterm(hex) abort
    let l:r = str2nr(a:hex[0:1], 16)
    let l:g = str2nr(a:hex[2:3], 16)
    let l:b = str2nr(a:hex[4:5], 16)
    let l:r6 = (l:r * 5 + 127) / 255
    let l:g6 = (l:g * 5 + 127) / 255
    let l:b6 = (l:b * 5 + 127) / 255
    return 16 + (36 * l:r6) + (6 * l:g6) + l:b6
endfunction
" Cache các highlight group đã định nghĩa trong session, tránh gọi lại
" :highlight nhiều lần không cần thiết cho cùng 1 tổ hợp style.
let s:docx_hl_defined = {}
" ----------------------------------------------------------------------------
" s:DocxApplyCellStyles(): đọc b:docx_style_meta, tạo highlight group tương
" ứng và gọi matchaddpos() để tô đúng vùng buffer.
"
" b:docx_style_meta = list of [line, char_start, char_end, bold, italic,
" size_pt, color_hex]. char_start/char_end là 1-based char index (giống
" Excel plugin), cần convert qua byteidx() trước khi gọi matchaddpos().
" ----------------------------------------------------------------------------
function! s:DocxApplyCellStyles() abort
    if !exists('b:docx_style_meta') || empty(b:docx_style_meta)
        return
    endif
    for l:item in b:docx_style_meta
        let [l:line, l:char_start, l:char_end, l:bold, l:italic, l:size_pt, l:color_hex, l:font, l:hl] = l:item
        if l:line < 1 || l:line > line('$')
            continue
        endif
        let l:text = getline(l:line)
        let l:byte_start = byteidx(l:text, l:char_start - 1)
        let l:byte_end = byteidx(l:text, l:char_end)
        if l:byte_start < 0 || l:byte_end < 0
            continue
        endif
        let l:byte_len = l:byte_end - l:byte_start
        if l:byte_len <= 0
            continue
        endif
        let l:group = s:DocxHighlightGroupName(l:bold, l:italic, l:size_pt, l:color_hex, l:hl)
        if !get(s:docx_hl_defined, l:group, 0)
            call s:DocxDefineHighlight(l:group, l:bold, l:italic, l:color_hex, l:hl)
            let s:docx_hl_defined[l:group] = 1
        endif
        call matchaddpos(l:group, [[l:line, l:byte_start + 1, l:byte_len]], 50)
    endfor
endfunction
" ============================================================================
" PARAGRAPH LOOKUP - tìm paragraph_id theo cursor / visual selection
" ============================================================================
" ----------------------------------------------------------------------------
" s:DocxParaIdAtLine(line): trả về paragraph_id (vd "P3") của line, hoặc ''
" nếu line không phải dòng paragraph (dòng border bảng, dòng ngoài table).
"
" b:docx_para_map = list of [line, para_id]. DOCX dùng 1 paragraph = 1 line
" buffer (đơn vị duy nhất), nên không cần check char range như cell map.
" ----------------------------------------------------------------------------
function! s:DocxParaIdAtLine(line) abort
    if !exists('b:docx_para_map') || empty(b:docx_para_map)
        return ''
    endif
    " Exact match
    for l:item in b:docx_para_map
        if l:item[0] == a:line
            return l:item[1]
        endif
    endfor
    " Fallback: paragraph gần nhất phía TRÊN (dòng border / dòng marker
    " không có paramap entry → vẫn cho phép `o`/`O` trên đó, anchor tới
    " paragraph ngay trên).
    let l:best_line = -1
    let l:best_pid = ''
    for l:item in b:docx_para_map
        if l:item[0] < a:line && l:item[0] > l:best_line
            let l:best_line = l:item[0]
            let l:best_pid = l:item[1]
        endif
    endfor
    return l:best_pid
endfunction
" ----------------------------------------------------------------------------
" s:DocxParaIdAtCursor(): trả về paragraph_id tại dòng cursor hiện tại.
" ----------------------------------------------------------------------------
function! s:DocxParaIdAtCursor() abort
    return s:DocxParaIdAtLine(line('.'))
endfunction
" ----------------------------------------------------------------------------
" s:DocxParaIdsInRange(line1, line2): trả về list paragraph_id (không trùng)
" cho mọi line trong [line1, line2]. Bỏ qua các dòng không có paragraph
" (border của table).
" ----------------------------------------------------------------------------
function! s:DocxParaIdsInRange(line1, line2) abort
    let l:ids = []
    let l:seen = {}
    if !exists('b:docx_para_map')
        return l:ids
    endif
    for l:item in b:docx_para_map
        let [l:line, l:pid] = l:item
        if l:line < a:line1 || l:line > a:line2
            continue
        endif
        if !has_key(l:seen, l:pid)
            let l:seen[l:pid] = 1
            call add(l:ids, l:pid)
        endif
    endfor
    return l:ids
endfunction
" ----------------------------------------------------------------------------
" s:DocxResolveTargetParas(line1, line2): trả về danh sách paragraph_id cần
" áp style. Tương tự s:ExcelResolveTargetCells trong excelPlugin.vim:
"   - Nếu line1 != line2: explicit range -> dùng range đó.
"   - Nếu line1 == line2 nhưng marks '<'/'>' định nghĩa 1 vùng VÀ cursor
"     vẫn nằm trong vùng đó (Visual mode mới vừa Esc): dùng marks.
"   - Còn lại: Normal mode tại cursor -> 1 paragraph.
" ----------------------------------------------------------------------------
function! s:DocxResolveTargetParas(line1, line2) abort
    " Explicit range: ưu tiên hơn marks (vd :5,10DocxBold).
    if a:line1 != a:line2
        let l:ids = s:DocxParaIdsInRange(a:line1, a:line2)
        if empty(l:ids)
            echoerr 'Selection does not cover any paragraph'
            return []
        endif
        return l:ids
    endif

    " line1 == line2: có thể Visual mode mà '<,'> bị mất khi gõ command, hoặc
    " thật sự Normal mode. Check marks:
    let l:lt_line = line("'<")
    let l:gt_line = line("'>")
    let l:has_marks = l:lt_line > 0 && l:gt_line > 0
    let l:marks_define_range = l:has_marks
                \ && (l:lt_line != l:gt_line || virtcol("'<") != virtcol("'>"))
    let l:marks_relevant = l:marks_define_range
                \ && l:lt_line <= a:line1 && a:line1 <= l:gt_line

    if l:marks_relevant
        let l:ids = s:DocxParaIdsInRange(l:lt_line, l:gt_line)
        if empty(l:ids)
            echoerr 'Selection does not cover any paragraph'
            return []
        endif
        return l:ids
    endif

    " Normal mode: cursor tại 1 paragraph.
    let l:pid = s:DocxParaIdAtCursor()
    if empty(l:pid)
        echoerr 'Cursor is not on a paragraph (border line or outside content?)'
        return []
    endif
    return [l:pid]
endfunction
" ============================================================================
" METADATA PARSING - tách @@STYLE@@ / @@PARAMAP@@ khỏi output binary
" ============================================================================
" ----------------------------------------------------------------------------
" s:DocxParseMeta(): tách khối @@STYLE@@...@@END@@ và @@PARAMAP@@...
" @@PARAMAPEND@@ khỏi output thô của binary docx.
" Trả về [content_lines, style_list, paramap_list].
" ----------------------------------------------------------------------------
function! s:DocxParseMeta(output) abort
    let l:marker_idx = index(a:output, '@@STYLE@@')
    if l:marker_idx == -1
        return [a:output, [], [], []]
    endif
    let l:content_lines = a:output[0 : l:marker_idx - 1]
    let l:after_marker = a:output[l:marker_idx + 1 :]
    let l:end_idx_rel = index(l:after_marker, '@@END@@')
    if l:end_idx_rel == -1
        let l:style_lines = l:after_marker
        let l:after_style_end = []
    else
        let l:style_lines = l:after_marker[0 : l:end_idx_rel - 1]
        let l:after_style_end = l:after_marker[l:end_idx_rel + 1 :]
    endif

    let l:styles = []
    for l:raw in l:style_lines
        if empty(l:raw)
            continue
        endif
        let l:parts = split(l:raw, "\t")
        " Format mới (9 trường): line, cs, ce, bold, italic, size_pt, color,
        " font, highlight. Hỗ trợ backward compat 7/8 trường.
        if len(l:parts) == 7
            let l:font = '-'
            let l:hl = '-'
        elseif len(l:parts) == 8
            let l:font = l:parts[7]
            let l:hl = '-'
        elseif len(l:parts) == 9
            let l:font = l:parts[7]
            let l:hl = l:parts[8]
        else
            continue
        endif
        let l:size_pt = l:parts[5] ==# '-' ? '-' : l:parts[5]
        call add(l:styles, [
                \ str2nr(l:parts[0]),
                \ str2nr(l:parts[1]),
                \ str2nr(l:parts[2]),
                \ str2nr(l:parts[3]),
                \ str2nr(l:parts[4]),
                \ l:size_pt,
                \ l:parts[6],
                \ l:font,
                \ l:hl,
                \ ])
    endfor

    " --- Parse khối @@PARAMAP@@ ---
    let l:paramap = []
    let l:listinfo = []
    let l:pm_idx = index(l:after_style_end, '@@PARAMAP@@')
    if l:pm_idx != -1
        let l:after_pm = l:after_style_end[l:pm_idx + 1 :]
        let l:pm_end_rel = index(l:after_pm, '@@PARAMAPEND@@')
        let l:pm_lines = l:pm_end_rel == -1
                    \ ? l:after_pm
                    \ : l:after_pm[0 : l:pm_end_rel - 1]
        for l:raw in l:pm_lines
            if empty(l:raw)
                continue
            endif
            let l:parts = split(l:raw, "\t")
            if len(l:parts) != 2
                continue
            endif
            call add(l:paramap, [str2nr(l:parts[0]), l:parts[1]])
        endfor

        " Parse @@LISTINFO@@ (xuất hiện sau @@PARAMAPEND@@).
        let l:after_pm_end = l:pm_end_rel == -1
                    \ ? []
                    \ : l:after_pm[l:pm_end_rel + 1 :]
        let l:li_idx = index(l:after_pm_end, '@@LISTINFO@@')
        if l:li_idx != -1
            let l:after_li = l:after_pm_end[l:li_idx + 1 :]
            let l:li_end_rel = index(l:after_li, '@@LISTINFOEND@@')
            let l:li_lines = l:li_end_rel == -1
                        \ ? l:after_li
                        \ : l:after_li[0 : l:li_end_rel - 1]
            for l:raw in l:li_lines
                if empty(l:raw)
                    continue
                endif
                let l:parts = split(l:raw, "\t")
                if len(l:parts) != 2
                    continue
                endif
                call add(l:listinfo, [str2nr(l:parts[0]), str2nr(l:parts[1])])
            endfor
        endif

        " @@CELLMAP@@ block: parse được nhưng Vim chưa dùng (save logic
        " xử lý bên Rust). Skip để không gây lỗi.
    endif

    " Bỏ dòng trống cuối content (Rust println! thêm 1 trailing newline)
    while !empty(l:content_lines) && l:content_lines[-1] ==# ''
        call remove(l:content_lines, -1)
    endwhile
    return [l:content_lines, l:styles, l:paramap, l:listinfo]
endfunction
" ============================================================================
" BINARY CALLER
" ============================================================================
" ----------------------------------------------------------------------------
" s:DocxCmd(): gọi binary docx với 1 subcommand + args, trả về output list.
" ----------------------------------------------------------------------------
function! s:DocxCmd(mode, ...) abort
    let l:cmd =
                \ shellescape(s:docx_bin)
                \ . ' '
                \ . a:mode
                \ . ' '
                \ . shellescape(b:docx_file)
    for arg in a:000
        let l:cmd .= ' ' . shellescape(arg)
    endfor
    return systemlist(l:cmd)
endfunction
" ----------------------------------------------------------------------------
" s:DocxLoadIntoBuffer(): gọi binary 'open', parse metadata, nạp vào buffer
" và lưu lại b:docx_style_meta / b:docx_para_map cho highlight + para lookup.
" ----------------------------------------------------------------------------
function! s:DocxLoadIntoBuffer(raw_output) abort
    " Lưu cursor + viewport TRƯỚC khi reload để tránh nháy/mất vị trí.
    let l:save_pos = exists('b:docx_buffer') ? getpos('.') : [0, 1, 1, 0]
    let l:save_topline = exists('b:docx_buffer') ? line('w0') : 1
    let [l:content_lines, l:styles, l:paramap, l:listinfo] = s:DocxParseMeta(a:raw_output)
    let b:docx_style_meta = l:styles
    let b:docx_para_map = l:paramap
    let b:docx_list_info = l:listinfo
    setlocal modifiable
    silent %delete _
    call setline(1, l:content_lines)
    call DocxSetupHighlight()
    " Restore cursor + viewport (clamp về số dòng buffer mới nếu cần)
    let l:max_line = line('$')
    if l:save_pos[1] > l:max_line
        let l:save_pos[1] = l:max_line
    endif
    call setpos('.', l:save_pos)
    " Restore topline để viewport không nhảy
    if l:save_topline > 0 && l:save_topline <= l:max_line
        execute 'normal! ' . l:save_topline . 'zt'
        " Cursor có thể bị move sau zt, set lại
        call setpos('.', l:save_pos)
    endif
    call s:DocxUpdateStatusPara()
endfunction
" ----------------------------------------------------------------------------
" s:DocxUpdateStatusPara(): cập nhật b:docx_status_para với paragraph_id tại
" cursor, dùng cho statusline. Cache để statusline không tính lại mỗi redraw.
" ----------------------------------------------------------------------------
function! s:DocxUpdateStatusPara() abort
    let b:docx_status_para = s:DocxParaIdAtCursor()
endfunction
" ----------------------------------------------------------------------------
" DocxStatusLine(): hàm gọi từ 'statusline' để hiển thị paragraph_id tại
" cursor. Format: "P3 — file.docx". Thông tin KHÔNG nằm trong buffer text,
" an toàn cho yank/copy.
" ----------------------------------------------------------------------------
function! DocxStatusLine() abort
    if !exists('b:docx_file')
        return ''
    endif
    let l:pid = get(b:, 'docx_status_para', '')
    return empty(l:pid) ? '(border/outside)' : l:pid
endfunction

" ----------------------------------------------------------------------------
" Kiểm tra đã có Excel plugin hay chưa
" ----------------------------------------------------------------------------
function! s:DocxHasXlsxPlugin() abort
    return exists('*ExcelOpen') ||
          \ exists(':ExcelOpen') == 2 ||
          \ exists('g:loaded_excelPlugin')
endfunction

" ----------------------------------------------------------------------------
" Kiểm tra file Excel
" ----------------------------------------------------------------------------
function! s:DocxIsExcelFile(path) abort
    return a:path =~? '\.\(xlsx\|xlsm\|xls\)$'
endfunction
" ----------------------------------------------------------------------------
" DocxGoto(ref): nhảy con trỏ đến paragraph có id `ref` (vd "P3").
" ----------------------------------------------------------------------------
function! DocxGoto(ref) abort
    if !exists('b:docx_para_map')
        echoerr 'This buffer is not a DOCX file'
        return
    endif
    let l:target = toupper(trim(a:ref))
    for l:item in b:docx_para_map
        if l:item[1] ==# l:target
            call cursor(l:item[0], 1)
            return
        endif
    endfor
    echoerr 'Paragraph not found: ' . a:ref
endfunction
" ----------------------------------------------------------------------------
" DocxGotoComplete(): Tab completion cho :DocxGoto, liệt kê toàn bộ paragraph id khả dụng (P0, P1, ...).
" ----------------------------------------------------------------------------
function! DocxGotoComplete(A, L, P) abort
    if !exists('b:docx_para_map')
        return []
    endif
    let l:ids = []
    let l:seen = {}
    for l:item in b:docx_para_map
        let l:pid = l:item[1]
        if !has_key(l:seen, l:pid)
            let l:seen[l:pid] = 1
            call add(l:ids, l:pid)
        endif
    endfor
    return filter(l:ids, 'v:val =~? "^" . a:A')
endfunction
" ============================================================================
" CORE FUNCTIONS - OPEN / SAVE
" ============================================================================
" ----------------------------------------------------------------------------
" DocxBuild(): build binary docx
" ----------------------------------------------------------------------------
function! DocxBuild() abort
    if executable('cargo') == 0
        echoerr 'cargo not found in PATH'
        return
    endif
    echo 'Building rs...'
    let l:cmd =
                \ 'cargo build --release --manifest-path '
                \ . shellescape(s:rs_root . '/Cargo.toml')
    let l:out = systemlist(l:cmd)
    if v:shell_error
        echoerr join(l:out, "\n")
        return
    endif
    echo 'Build success: ' . s:docx_bin
endfunction
" ----------------------------------------------------------------------------
" DocxNew(): tạo file .docx MỚI (chưa tồn tại) — gọi binary `create` để
" sinh template DOCX trống, rồi mở như bình thường. Dùng cho BufNewFile.
" ----------------------------------------------------------------------------
function! DocxNew() abort
    if exists('b:docx_buffer')
        return
    endif
    if !s:EnsureBuilt()
        echoerr 'Build failed'
        return
    endif
    let l:file = expand('<amatch>')
    if empty(l:file)
        let l:file = expand('%:p')
    endif
    let b:docx_file = fnamemodify(l:file, ':p')
    " Tạo template DOCX trống tại path qua binary `create`. Binary cũng
    " emit luôn output 'open' (text + metadata) để load thẳng vào buffer.
    let l:output = s:DocxCmd('create')
    if v:shell_error
        echoerr join(l:output, "\n")
        return
    endif
    let b:docx_buffer = 1
    call s:DocxLoadIntoBuffer(l:output)
    call s:DocxSetupBufferOptions()
    set nomodified
endfunction
" ----------------------------------------------------------------------------
" s:DocxSetupBufferOptions(): thiết lập options + autocmd + mapping cho
" buffer DOCX. Tách riêng để DocxOpen() và DocxNew() cùng dùng.
" ----------------------------------------------------------------------------
function! s:DocxSetupBufferOptions() abort
    " Thiết lập buffer giống Excel plugin:
    " - buftype=acwrite: ghi qua BufWriteCmd
    " - bufhidden=hide: ẩn khi đóng tab
    " - noswapfile: không tạo swap (buffer "ảo")
    " - filetype=docx: cho highlight/ftplugin riêng
    setlocal buftype=acwrite
    setlocal bufhidden=hide
    setlocal noswapfile
    setlocal filetype=docx
    " Statusline: hiển thị paragraph_id tại cursor + tên file.
    " Thông tin KHÔNG nằm trong buffer text -> yank/copy an toàn.
    setlocal statusline=%{DocxStatusLine()}\ —\ %f%=%l,%c\ \ %P
    augroup DocxBuffer
        autocmd! * <buffer>
        autocmd BufWriteCmd <buffer> call DocxSave()
        autocmd CursorMoved,CursorMovedI <buffer> call s:DocxUpdateStatusPara()
        " Khi cursor di chuyển: đóng popup hover (nếu đang mở). Mở lại
        " sau khi idle (CursorHold).
        autocmd CursorMoved,CursorMovedI,BufLeave,WinLeave <buffer> call s:DocxClosePopup()
        " CursorHold: sau khi cursor idle 'updatetime' ms, hiển thị popup
        " với thông tin font/size/bold/italic/color/highlight.
        autocmd CursorHold <buffer> call DocxHoverInfo()
    augroup END
    " Mapping Tab / Shift-Tab cho indent (local-to-buffer):
    nnoremap <silent> <buffer> <Tab>   :call DocxSmartTab('+1', line('.'), line('.'))<CR>
    nnoremap <silent> <buffer> <S-Tab> :call DocxSmartTab('-1', line('.'), line('.'))<CR>
    xnoremap <silent> <buffer> <Tab>   :<C-u>call DocxSmartTab('+1', line("'<"), line("'>"))<CR>
    xnoremap <silent> <buffer> <S-Tab> :<C-u>call DocxSmartTab('-1', line("'<"), line("'>"))<CR>
    " `o` Normal mode: chèn paragraph mới ngay SAU dòng hiện tại + vào
    " Insert mode. Kế thừa pStyle/numPr/indent/alignment + style run đầu
    " (bold/italic/font/color) của paragraph nguồn.
    nnoremap <silent> <buffer> o :call DocxListAdd()<CR>
    " `O` (Shift-O) Normal mode: chèn paragraph mới ngay TRƯỚC dòng hiện
    " tại. Kế thừa cùng cách như `o`.
    nnoremap <silent> <buffer> O :call DocxListAddBefore()<CR>
    " `gx` Normal mode: nếu cursor trên paragraph chứa [IMAGE] / OLE
    " object → extract media ra /tmp/ và mở bằng app mặc định OS.
    nnoremap <silent> <buffer> gx :call DocxOpenMedia()<CR>
endfunction
" ----------------------------------------------------------------------------
" DocxOpen(): đọc file .docx qua binary, hiển thị nội dung text trong buffer
" ----------------------------------------------------------------------------
function! DocxOpen()
    if exists('b:docx_buffer')
        return
    endif
    if !s:EnsureBuilt()
        echoerr 'Build failed'
        return
    endif
    let b:docx_buffer = 1
    let l:file = expand('<amatch>')
    if empty(l:file)
        let l:file = expand('%:p')
    endif
    let b:docx_file = fnamemodify(l:file, ':p')
    if !executable(s:docx_bin) && !filereadable(s:docx_bin)
        echoerr 'docx binary not found: ' . s:docx_bin
        unlet! b:docx_buffer
        return
    endif
    let l:output = s:DocxCmd('open')
    if v:shell_error
        " File tồn tại nhưng KHÔNG phải DOCX zip hợp lệ (vd file rỗng do
        " `vim newfile.docx` tạo, hoặc plain text với đuôi .docx). Thử
        " tạo lại template DOCX trống rồi mở.
        let l:joined = join(l:output, ' ')
        if l:joined =~? 'zip\|central directory\|not a zip\|invalid'
            let l:recreate = s:DocxCmd('create')
            if v:shell_error
                echoerr join(l:recreate, "\n")
                unlet! b:docx_buffer
                return
            endif
            call s:DocxLoadIntoBuffer(l:recreate)
            call s:DocxSetupBufferOptions()
            set nomodified
            echo 'Created new blank DOCX (file was not a valid .docx)'
            return
        endif
        echoerr l:joined
        unlet! b:docx_buffer
        return
    endif
    call s:DocxLoadIntoBuffer(l:output)
    call s:DocxSetupBufferOptions()
    set nomodified
endfunction
" ----------------------------------------------------------------------------
" DocxSave(): lưu buffer text trở lại file .docx qua binary, sau đó reload
" để hiển thị nội dung đã format chuẩn.
" ----------------------------------------------------------------------------
function! DocxSave()
    let l:tmp = tempname()
    call writefile(getline(1, '$'), l:tmp)
    " Binary save giờ emit luôn output 'open' sau khi save -> không cần
    " gọi binary lần 2.
    let l:result = s:DocxCmd('save', l:tmp)
    call delete(l:tmp)
    if v:shell_error
        echoerr join(l:result, "\n")
        return
    endif
    call s:DocxLoadIntoBuffer(l:result)
    set nomodified
    echo 'DOCX saved & reformatted'
endfunction
" ============================================================================
" STYLE COMMANDS
" ============================================================================
" ----------------------------------------------------------------------------
" Bộ màu chuẩn (khớp với Rust) cho completion :DocxColor.
" ----------------------------------------------------------------------------
let s:docx_color_names = ['red', 'green', 'blue', 'yellow', 'orange', 'purple', 'gray', 'white', 'black', 'none']
" ----------------------------------------------------------------------------
" s:DocxRunStyleCmd(): gọi binary setstyle cho tất cả paragraph trong
" para_ids (comma-joined), rồi reload buffer. Tương tự s:ExcelRunStyleCmd
" trong excelPlugin.vim. Batch 1 lần spawn process cho cả vùng.
" ----------------------------------------------------------------------------
function! s:DocxRunStyleCmd(attr, para_ids, value) abort
    if empty(a:para_ids)
        return 0
    endif
    let l:joined = join(a:para_ids, ',')
    " Binary setstyle giờ emit luôn output 'open' sau khi save -> không
    " cần gọi binary lần 2. Tiết kiệm ~50% thời gian / style command.
    let l:result = s:DocxCmd('setstyle', l:joined, a:attr, a:value)
    if v:shell_error
        echoerr join(l:result, "\n")
        return 0
    endif
    call s:DocxLoadIntoBuffer(l:result)
    set nomodified
    return 1
endfunction
" ----------------------------------------------------------------------------
" DocxBold([range]): đảo bold cho paragraph tại cursor (Normal mode) hoặc
" toàn bộ paragraph trong Visual selection.
" ----------------------------------------------------------------------------
function! DocxBold(line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('togglebold', l:ids, '')
        echo 'Bold toggled (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ')
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxItalic([range]): đảo italic, tương tự DocxBold.
" ----------------------------------------------------------------------------
function! DocxItalic(line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('toggleitalic', l:ids, '')
        echo 'Italic toggled (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ')
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxSize(pt, [range]): đặt font-size (đơn vị points, vd 14 hoặc 11.5).
" ----------------------------------------------------------------------------
function! DocxSize(pt, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('size', l:ids, a:pt)
        echo 'Font size = ' . a:pt . 'pt (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ')
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxColor(color, [range]): đặt màu chữ. color = tên (red/green/...) hoặc
" "#RRGGBB" hoặc "none" để xoá.
" ----------------------------------------------------------------------------
function! DocxColor(color, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('color', l:ids, a:color)
        echo 'Font color (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ') . ' -> ' . a:color
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxColorComplete(): gợi ý tên màu chuẩn khi gõ Tab ở :DocxColor.
" ----------------------------------------------------------------------------
function! DocxColorComplete(A, L, P) abort
    return filter(copy(s:docx_color_names), 'v:val =~? "^" . a:A')
endfunction
" ----------------------------------------------------------------------------
" Bộ font phổ biến (chỉ là gợi ý cho Tab completion; thực ra user gõ tên gì
" cũng được — DOCX nhận tên free-form).
" ----------------------------------------------------------------------------
let s:docx_font_names = [
            \ 'Times New Roman', 'Arial', 'Calibri', 'Cambria', 'Verdana',
            \ 'Tahoma', 'Georgia', 'Courier New', 'Roboto', 'Open Sans',
            \ 'Helvetica', 'Comic Sans MS', '.VnTime', 'none',
            \ ]
" ----------------------------------------------------------------------------
" DocxFontComplete(): gợi ý tên font khi gõ Tab ở :DocxFont.
" ----------------------------------------------------------------------------
function! DocxFontComplete(A, L, P) abort
    return filter(copy(s:docx_font_names), 'v:val =~? "^" . a:A')
endfunction
" ----------------------------------------------------------------------------
" DocxFont(name, [range]): đặt font cho paragraph tại cursor / vùng chọn.
" name = tên font tự do, vd "Times New Roman", "Roboto"; "none" để xoá.
" ----------------------------------------------------------------------------
function! DocxFont(name, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('font', l:ids, a:name)
        echo 'Font (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ') . ' -> ' . a:name
    endif
endfunction
" ----------------------------------------------------------------------------
" Bộ alignment hợp lệ cho :DocxAlign.
" ----------------------------------------------------------------------------
let s:docx_align_names = ['left', 'center', 'right', 'both', 'justify', 'none']
function! DocxAlignComplete(A, L, P) abort
    return filter(copy(s:docx_align_names), 'v:val =~? "^" . a:A')
endfunction
" ----------------------------------------------------------------------------
" DocxAlign(align, [range]): đặt căn lề. align = left/center/right/both/justify/none.
" ----------------------------------------------------------------------------
function! DocxAlign(align, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('align', l:ids, a:align)
        echo 'Align (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ') . ' -> ' . a:align
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxIndent(delta, line1, line2): tăng/giảm indent. delta = "+1", "-1",
" hoặc số nguyên (set thẳng). Dùng cho :DocxIndent +1 hoặc :DocxIndent -1
" hoặc :DocxIndent 0 (reset). Cũng được dùng làm callback cho mapping Tab.
" ----------------------------------------------------------------------------
function! DocxIndent(delta, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('indent', l:ids, a:delta)
        echo 'Indent ' . a:delta . ' (' . len(l:ids) . ' paragraphs)'
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxHighlight(color, line1, line2): đặt màu nền (highlight).
" ----------------------------------------------------------------------------
let s:docx_highlight_names = [
            \ 'yellow', 'green', 'cyan', 'magenta', 'blue', 'red',
            \ 'darkBlue', 'darkCyan', 'darkGreen', 'darkMagenta', 'darkRed',
            \ 'darkYellow', 'darkGray', 'lightGray', 'black', 'white', 'none',
            \ ]
function! DocxHighlightComplete(A, L, P) abort
    return filter(copy(s:docx_highlight_names), 'v:val =~? "^" . a:A')
endfunction
function! DocxHighlight(color, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    if s:DocxRunStyleCmd('highlight', l:ids, a:color)
        echo 'Highlight (' . len(l:ids) . ' paragraphs): ' . join(l:ids, ', ') . ' -> ' . a:color
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxHoverInfo(): hiển thị thông tin style tại con trỏ trong 1 popup nhỏ
" cạnh cursor (nếu Vim/Neovim hỗ trợ), fallback về echo nếu không.
" ----------------------------------------------------------------------------
let s:docx_popup_id = 0
function! s:DocxClosePopup() abort
    if has('nvim')
        if s:docx_popup_id != 0
            silent! call nvim_win_close(s:docx_popup_id, 1)
            let s:docx_popup_id = 0
        endif
    else
        if s:docx_popup_id != 0
            silent! call popup_close(s:docx_popup_id)
            let s:docx_popup_id = 0
        endif
    endif
endfunction
" ----------------------------------------------------------------------------
" s:DocxBuildHoverText(): build danh sách dòng text mô tả style tại cursor.
" Trả về [] nếu không có gì để hiện.
" ----------------------------------------------------------------------------
function! s:DocxBuildHoverText() abort
    if !exists('b:docx_style_meta')
        return []
    endif
    let l:line = line('.')
    let l:vcol = virtcol('.')
    let l:found = []
    for l:item in b:docx_style_meta
        let [l:ln, l:cs, l:ce, l:bold, l:italic, l:size, l:color, l:font, l:hl] = l:item
        if l:ln == l:line && l:vcol >= l:cs && l:vcol <= l:ce
            let l:found = l:item
            break
        endif
    endfor
    let l:pid = s:DocxParaIdAtCursor()
    let l:lines = []
    if !empty(l:pid)
        call add(l:lines, '◆ ' . l:pid)
    endif
    if empty(l:found)
        call add(l:lines, '(default style)')
        return l:lines
    endif
    if l:found[7] !=# '-'
        call add(l:lines, 'Font:  ' . l:found[7])
    endif
    if l:found[5] !=# '-'
        call add(l:lines, 'Size:  ' . l:found[5] . ' pt')
    endif
    let l:attrs = []
    if l:found[3]
        call add(l:attrs, 'Bold')
    endif
    if l:found[4]
        call add(l:attrs, 'Italic')
    endif
    if !empty(l:attrs)
        call add(l:lines, 'Style: ' . join(l:attrs, ', '))
    endif
    if l:found[6] !=# '-'
        call add(l:lines, 'Color: #' . l:found[6])
    endif
    if l:found[8] !=# '-'
        call add(l:lines, 'BG:    ' . l:found[8])
    endif
    return l:lines
endfunction
" ----------------------------------------------------------------------------
" DocxHoverInfo(): main entry — gọi từ CursorHold autocmd hoặc :DocxInfo.
" ----------------------------------------------------------------------------
function! DocxHoverInfo() abort
    call s:DocxClosePopup()
    let l:lines = s:DocxBuildHoverText()
    if empty(l:lines)
        return
    endif

    if has('nvim')
        let l:buf = nvim_create_buf(v:false, v:true)
        call nvim_buf_set_lines(l:buf, 0, -1, v:true, l:lines)
        let l:width = 10
        for l:l in l:lines
            if strdisplaywidth(l:l) > l:width
                let l:width = strdisplaywidth(l:l)
            endif
        endfor
        let l:opts = {
                    \ 'relative': 'cursor',
                    \ 'row': 1,
                    \ 'col': 1,
                    \ 'width': l:width + 2,
                    \ 'height': len(l:lines),
                    \ 'style': 'minimal',
                    \ 'border': 'rounded',
                    \ 'focusable': v:false,
                    \ }
        let s:docx_popup_id = nvim_open_win(l:buf, v:false, l:opts)
        call nvim_win_set_option(s:docx_popup_id, 'winhl', 'Normal:DocxHoverPopup,FloatBorder:DocxHoverBorder')
    elseif exists('*popup_create')
        let l:opts = {
                    \ 'pos': 'topleft',
                    \ 'line': 'cursor+1',
                    \ 'col': 'cursor+1',
                    \ 'border': [1, 1, 1, 1],
                    \ 'padding': [0, 1, 0, 1],
                    \ 'highlight': 'DocxHoverPopup',
                    \ 'borderhighlight': ['DocxHoverBorder'],
                    \ 'moved': 'any',
                    \ 'close': 'click',
                    \ }
        let s:docx_popup_id = popup_create(l:lines, l:opts)
    else
        echo join(l:lines, ' | ')
    endif
endfunction
" ----------------------------------------------------------------------------
" MEDIA / ATTACHMENT - mở image, OLE object bằng app mặc định OS
" ----------------------------------------------------------------------------
function! s:DocxSystemOpenCmd() abort
    if has('mac') || has('macunix')
        return 'open'
    elseif has('win32') || has('win64')
        return 'cmd /c start ""'
    else
        return 'xdg-open'
    endif
endfunction
function! s:DocxIsImageFile(path) abort
    return a:path =~? '\.\(png\|jpg\|jpeg\|gif\|webp\|bmp\|tiff\|tif\)$'
endfunction
function! s:DocxTermImageCmd(path) abort
    let l:p = shellescape(a:path)
    let l:term_prog = $TERM_PROGRAM
    let l:term = $TERM
    if l:term_prog ==# 'iTerm.app' && executable('imgcat')
        return 'imgcat ' . l:p
    endif
    if l:term_prog ==# 'WezTerm' && executable('wezterm')
        return 'wezterm imgcat ' . l:p
    endif
    if (l:term =~? 'kitty' || l:term_prog ==# 'ghostty' || l:term_prog ==# 'WezTerm')
                \ && executable('kitten')
        return 'kitten icat ' . l:p
    endif
    if l:term =~? 'kitty' && executable('kitty')
        return 'kitty +kitten icat ' . l:p
    endif
    if executable('img2sixel') && (l:term =~? 'sixel\|foot\|mlterm\|wezterm\|xterm-direct')
        return 'img2sixel ' . l:p
    endif
    if executable('chafa')
        return 'chafa --format=symbols ' . l:p
    endif
    return ''
endfunction
function! s:DocxRenderImageInTerm(path) abort
    let l:cmd = s:DocxTermImageCmd(a:path)
    if empty(l:cmd)
        return 0
    endif
    execute '!' . l:cmd
    return 1
endfunction
if !exists('g:docx_image_open_mode')
    let g:docx_image_open_mode = 'auto'
endif
function! DocxOpenMedia() abort
    let l:pid = s:DocxParaIdAtCursor()
    if empty(l:pid)
        echoerr 'Cursor not on a paragraph'
        return
    endif
    let l:result = s:DocxCmd('extract', l:pid)
    if v:shell_error
        let l:msg = join(l:result, ' ')
        if l:msg =~? 'no extractable media'
            echo 'No image/object on this paragraph'
        else
            echoerr l:msg
        endif
        return
    endif
    let l:opener = s:DocxSystemOpenCmd()
    let l:has_xlsx = s:DocxHasXlsxPlugin()
    for l:path in l:result
        if empty(l:path)
            continue
        endif
        if l:has_xlsx && s:DocxIsExcelFile(l:path)
            execute 'tabnew ' . fnameescape(l:path)
            echo 'Opened in Vim tab: ' . l:path
            continue
        endif
        if s:DocxIsImageFile(l:path)
            if g:docx_image_open_mode ==# 'term'
                if !s:DocxRenderImageInTerm(l:path)
                    echoerr 'Terminal does not support inline images. Install chafa or use g:docx_image_open_mode = "os"'
                endif
                continue
            elseif g:docx_image_open_mode ==# 'auto'
                if s:DocxRenderImageInTerm(l:path)
                    continue
                endif
            endif
        endif
        if has('win32') || has('win64')
            silent! call system(l:opener . ' ' . shellescape(l:path))
        else
            call system(l:opener . ' ' . shellescape(l:path) . ' &')
        endif
        echo 'Opened: ' . l:path
    endfor
endfunction
" ----------------------------------------------------------------------------
" LIST OPERATIONS
" ----------------------------------------------------------------------------
function! s:DocxListInfoAtLine(line) abort
    if !exists('b:docx_list_info')
        return [0, 0]
    endif
    for l:item in b:docx_list_info
        if l:item[0] == a:line
            return [1, l:item[1]]
        endif
    endfor
    return [0, 0]
endfunction
function! s:DocxIsLineEmpty(line) abort
    let l:text = getline(a:line)
    let l:stripped = substitute(l:text, '^\s*', '', '')
    let l:stripped = substitute(l:stripped, '^[├┊]\s*', '', '')
    let l:stripped = substitute(l:stripped, '^#\+\s*', '', '')
    let l:stripped = substitute(l:stripped, '^•\s*', '', '')
    let l:stripped = substitute(l:stripped, '^\d\+\.\s*', '', '')
    return empty(l:stripped)
endfunction
" ----------------------------------------------------------------------------
" DocxListAdd(): chèn 1 dòng list mới ngay SAU paragraph tại cursor.
" ----------------------------------------------------------------------------
function! DocxListAdd() abort
    call s:DocxListAddImpl('after')
endfunction
" ----------------------------------------------------------------------------
" DocxListAddBefore(): chèn paragraph mới NGAY TRƯỚC paragraph tại cursor.
" Dùng cho mapping `O` (Shift-O).
" ----------------------------------------------------------------------------
function! DocxListAddBefore() abort
    call s:DocxListAddImpl('before')
endfunction
function! s:DocxListAddImpl(position) abort
    let l:pid = s:DocxParaIdAtCursor()
    if empty(l:pid)
        echoerr 'Cursor not on a paragraph'
        return
    endif
    let l:current_line = line('.')
    let l:result = s:DocxCmd('listadd', l:pid, a:position)
    if v:shell_error
        echoerr join(l:result, "\n")
        return
    endif
    call s:DocxLoadIntoBuffer(l:result)
    set nomodified
    " Cursor vị trí dòng mới:
    " - after  -> dòng kế tiếp (current+1)
    " - before -> dòng hiện tại (paragraph mới chèn vào VỊ TRÍ này, paragraph
    "             cũ bị đẩy xuống current+1)
    let l:new_line = a:position ==# 'before' ? l:current_line : l:current_line + 1
    if l:new_line > line('$')
        let l:new_line = line('$')
    endif
    call cursor(l:new_line, 1)
    " Vị trí cursor trong Insert mode:
    " - Nếu dòng mới là dòng table (bắt đầu bằng `│`): đặt cursor SAU
    "   `│ ` đầu — TRONG cell 0, tránh chèn text sau border `│` cuối làm
    "   vỡ bảng.
    " - Dòng thường: đi cuối dòng (text input bình thường).
    let l:line_str = getline(l:new_line)
    if l:line_str =~# '^│'
        " `│` = 3 bytes UTF-8, space = 1 byte. byteidx(line, 2) = byte
        " offset sau "│ " (2 ký tự đầu). +1 để cursor đứng ngay đầu cell text.
        call cursor(l:new_line, byteidx(l:line_str, 2) + 1)
        startinsert
    else
        normal! $
        startinsert!
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxListDel(): xoá paragraph tại cursor + reload buffer.
" ----------------------------------------------------------------------------
function! DocxListDel() abort
    let l:pid = s:DocxParaIdAtCursor()
    if empty(l:pid)
        echoerr 'Cursor not on a paragraph'
        return
    endif
    let l:result = s:DocxCmd('listdel', l:pid)
    if v:shell_error
        echoerr join(l:result, "\n")
        return
    endif
    call s:DocxLoadIntoBuffer(l:result)
    set nomodified
endfunction
" ----------------------------------------------------------------------------
" DocxSmartTab(delta, line1, line2): Tab/Shift-Tab logic thông minh.
" ----------------------------------------------------------------------------
function! DocxSmartTab(delta, line1, line2) abort
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    if empty(l:ids)
        return
    endif
    let l:list_pids = {}
    if exists('b:docx_list_info') && exists('b:docx_para_map')
        for l:li in b:docx_list_info
            for l:pm in b:docx_para_map
                if l:pm[0] == l:li[0]
                    let l:list_pids[l:pm[1]] = 1
                    break
                endif
            endfor
        endfor
    endif
    let l:list_ids = []
    let l:nonlist_ids = []
    for l:pid in l:ids
        if get(l:list_pids, l:pid, 0)
            call add(l:list_ids, l:pid)
        else
            call add(l:nonlist_ids, l:pid)
        endif
    endfor
    if !empty(l:nonlist_ids)
        call s:DocxRunStyleCmd('indent', l:nonlist_ids, a:delta)
    endif
    if !empty(l:list_ids)
        call s:DocxRunStyleCmd('ilvl', l:list_ids, a:delta)
    endif
    echo 'Tab applied: list=' . len(l:list_ids) . ' non-list=' . len(l:nonlist_ids)
endfunction
" ----------------------------------------------------------------------------
" DocxListSmartEnter(): logic Enter trong list item.
" ----------------------------------------------------------------------------
function! DocxListSmartEnter() abort
    let l:line = line('.')
    let [l:is_list, l:ilvl] = s:DocxListInfoAtLine(l:line)
    if !l:is_list
        call DocxListAdd()
        return
    endif
    if s:DocxIsLineEmpty(l:line)
        let l:pid = s:DocxParaIdAtCursor()
        if l:ilvl > 0
            call s:DocxRunStyleCmd('ilvl', [l:pid], '-1')
        else
            let l:result = s:DocxCmd('listexit', l:pid)
            if v:shell_error
                echoerr join(l:result, "\n")
                return
            endif
            call s:DocxLoadIntoBuffer(l:result)
            set nomodified
        endif
        call cursor(l:line, 1)
        startinsert!
    else
        call DocxListAdd()
    endif
endfunction
" ----------------------------------------------------------------------------
" DocxDebug(line1, line2): debug helper.
" ----------------------------------------------------------------------------
function! DocxDebug(line1, line2) abort
    echo '--- DocxDebug ---'
    echo 'line1=' . a:line1 . ' line2=' . a:line2
    echo "mark '< : " . string(getpos("'<"))
    echo "mark '> : " . string(getpos("'>"))
    echo 'visualmode(): ' . string(visualmode())
    if exists('b:docx_para_map')
        echo 'paramap size: ' . len(b:docx_para_map)
        echo 'first 3 entries: ' . string(b:docx_para_map[0:2])
    else
        echo 'paramap: NOT SET (buffer not loaded?)'
    endif
    let l:ids = s:DocxResolveTargetParas(a:line1, a:line2)
    echo 'resolved paragraphs: ' . string(l:ids)
endfunction
" ============================================================================
" USER-FACING
" ============================================================================
command! DocxBuild call DocxBuild()
command! DocxSave call DocxSave()
command! -range DocxBold call DocxBold(<line1>, <line2>)
command! -range DocxItalic call DocxItalic(<line1>, <line2>)
command! -range -nargs=1 DocxSize call DocxSize(<q-args>, <line1>, <line2>)
command! -range -nargs=1 -complete=customlist,DocxColorComplete DocxColor
            \ call DocxColor(<q-args>, <line1>, <line2>)
command! -range -nargs=1 -complete=customlist,DocxHighlightComplete DocxHighlight
            \ call DocxHighlight(<q-args>, <line1>, <line2>)
command! -range -nargs=1 -complete=customlist,DocxFontComplete DocxFont
            \ call DocxFont(<q-args>, <line1>, <line2>)
command! -range -nargs=1 -complete=customlist,DocxAlignComplete DocxAlign
            \ call DocxAlign(<q-args>, <line1>, <line2>)
command! -range -nargs=1 DocxIndent call DocxIndent(<q-args>, <line1>, <line2>)
command! DocxListAdd call DocxListAdd()
command! DocxListAddBefore call DocxListAddBefore()
command! DocxListDel call DocxListDel()
command! DocxListEnter call DocxListSmartEnter()
command! DocxOpen call DocxOpenMedia()
command! DocxInfo call DocxHoverInfo()
command! -nargs=1 -complete=customlist,DocxGotoComplete DocxGoto call DocxGoto(<q-args>)
command! -range DocxDebug call DocxDebug(<line1>, <line2>)
