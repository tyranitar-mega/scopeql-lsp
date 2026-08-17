" scopeql-lsp vim plugin.
"
" Brings Semantic highlighting to ScopeQL (.scopeql) files through the
" scopeql-lsp language server. Requires:
"   * coc.nvim (https://github.com/neoclide/coc.nvim), and
"   * the scopeql-lsp binary in $PATH (or g:scopeql_lsp_command).
"
" Setup (one time, in your project's .vim/coc-settings.json or the global
" coc-settings file):
"
"   {
"     "semanticTokens.enable": true,
"     "languageserver": {
"       "scopeql": {
"         "command": "scopeql-lsp",
"         "filetypes": ["scopeql"],
"         "rootPatterns": ["**/*.scopeql"]
"       }
"     }
"   }
"
" The highlight groups below map the LSP semantic token types back onto vim's
" standard highlight groups. Override any of them in your vimrc to customize
" the colors, for example:
"
"   highlight CocSemTypeFunction guifg=#ff8700

" Absolute path of the scopeql-lsp executable (used by coc config generation).
if !exists('g:scopeql_lsp_command')
  let g:scopeql_lsp_command = 'scopeql-lsp'
endif

" --- Semantic token highlight groups ---------------------------------------
"
" coc.nvim highlights semantic tokens via groups named
"   CocSemType<TokenType>          and
"   CocSemTypeMod<TokenType><Modifier>
" (see coc's handler/semanticTokens/buffer.ts). scopeql-lsp emits the types
" keyword, type, namespace, class, struct, property, variable, function,
" string, number, operator and comment, with the declaration modifier.

highlight default link CocSemTypeKeyword       Keyword
highlight default link CocSemTypeType          Type
highlight default link CocSemTypeNamespace     Identifier
highlight default link CocSemTypeClass         Type
highlight default link CocSemTypeStruct        Type
highlight default link CocSemTypeEnum          Type
highlight default link CocSemTypeProperty      Identifier
highlight default link CocSemTypeVariable      Identifier
highlight default link CocSemTypeParameter     Identifier
highlight default link CocSemTypeFunction      Function
highlight default link CocSemTypeMethod        Function
highlight default link CocSemTypeString        String
highlight default link CocSemTypeNumber        Number
highlight default link CocSemTypeOperator      Operator
highlight default link CocSemTypeComment       Comment

" Object-definition keywords (CREATE / DROP / ALTER ...) carry the
" `declaration` modifier from scopeql-lsp.
highlight default link CocSemTypeModKeywordDeclaration Keyword
highlight default link CocSemTypeModTypeDeclaration     Type
highlight default link CocSemTypeModPropertyDeclaration Identifier

" --- Commands ---------------------------------------------------------------

" Open the example coc.nvim configuration ready to paste.
command! -nargs=0 ScopeQLSetupCoc
      \ call s:show_coc_setup()

function! s:show_coc_setup() abort
  let l:cmd = g:scopeql_lsp_command
  let l:snippet = [
        \ '{',
        \ '  "semanticTokens.enable": true,',
        \ '  "languageserver": {',
        \ '    "scopeql": {',
        \ '      "command": "' . l:cmd . '",',
        \ '      "filetypes": ["scopeql"],',
        \ '      "rootPatterns": ["**/*.scopeql"]',
        \ '    }',
        \ '  }',
        \ '}',
        \ ]
  echom 'Add the following to your .vim/coc-settings.json (or run :CocConfig):'
  for l:line in l:snippet
    echom l:line
  endfor
  echom 'Then restart coc.nvim with :CocRestart.'
endfunction