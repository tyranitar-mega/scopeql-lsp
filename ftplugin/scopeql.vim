" ScopeQL filetype plugin: editing options for .scopeql files.

if exists('b:did_ftplugin')
  finish
endif
let b:did_ftplugin = 1

" Comments use the SQL-lite `--` style.
setlocal commentstring=--\ %s
setlocal comments=:--

" Long analytical queries benefit from soft wrapping at the default width.
setlocal formatoptions+=t

let b:undo_ftplugin = 'setlocal commentstring< comments< formatoptions<'