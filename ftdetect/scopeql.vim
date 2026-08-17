" ScopeQL filetype detection.
"
" Installed by the scopeql-lsp plugin: any *.scopeql file is recognized as
" filetype `scopeql`, which drives both the fallback syntax highlighting and
" the coc.nvim language server registration.

au BufRead,BufNewFile *.scopeql setfiletype scopeql