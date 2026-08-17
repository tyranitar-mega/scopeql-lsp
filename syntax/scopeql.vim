" ScopeQL syntax highlighting.
"
" This is the *fallback* lexical highlighting used when the scopeql-lsp
" server is not running. When the LSP is active, coc.nvim applies the
" semantic tokens on top (see plugin/scopeql.vim), which take priority and
" refine the identifier colours.

if exists('b:current_syntax')
  finish
endif

syn case ignore

" Reserved words / clause keywords.
syn keyword scopeqlKeyword SELECT FROM WHERE GROUP ORDER BY LIMIT OFFSET UNION
syn keyword scopeqlKeyword ALL DISTINCT AS ON JOIN INNER LEFT RIGHT FULL OUTER
syn keyword scopeqlKeyword INSERT INTO VALUES UPDATE DELETE SET WITH WINDOW
syn keyword scopeqlKeyword AGGREGATE SAMPLE PERCENT WITHIN BETWEEN AND OR XOR NOT
syn keyword scopeqlKeyword IS IN EXISTS ANY CASE WHEN THEN ELSE END CAST EXEC
syn keyword scopeqlKeyword EXPLAIN PLAN ANALYZE

" Data definition / administration keywords.
syn keyword scopeqlKeyword CREATE DROP ALTER RENAME REPLACE TABLE VIEW SCHEMA
syn keyword scopeqlKeyword DATABASE DATABASES SCHEMAS TABLES VIEWS INDEX
syn keyword scopeqlKeyword COLUMN KEY PARTITION CLUSTER COMMENT RETENTION DATA DAY
syn keyword scopeqlKeyword MATERIALIZED PATTERN POINT RANGE SEARCH
syn keyword scopeqlKeyword JOB JOBS NODEGROUP SCHEDULE RESUME SUSPEND
syn keyword scopeqlKeyword SHOW DESCRIBE OPTIMIZE VACUUM STATEMENTS
syn keyword scopeqlKeyword ADD EXCLUDE FIRST LAST TO NULLS ASC DESC BEGIN END
syn keyword scopeqlKeyword TRUE FALSE NULL

" Type names.
syn keyword scopeqlType INT UINT FLOAT STRING BOOLEAN TIMESTAMP INTERVAL
syn keyword scopeqlType BINARY OBJECT ARRAY ANY

syn case match

" String literals: single / double quotes and backticks.
syn region scopeqlString start=+'+ skip=+\\'+ end=+'+
syn region scopeqlString start=+"+ skip=+\\"+ end=+"+
syn region scopeqlString start=+`+ skip=+\\`+ end=+`+

" Numbers.
syn match scopeqlNumber "\<[0-9][0-9_]*\>"
syn match scopeqlNumber "\<0[xX][0-9a-fA-F]+\>"
syn match scopeqlNumber "\<[0-9][0-9_]*\.[0-9][0-9_]*\([eE][+-]\?[0-9]\+\)\?\>"
syn match scopeqlNumber "\<[0-9][0-9_]*[eE][+-]\?[0-9]\+\>"

" Comments.
syn region scopeqlLineComment start="--" end="$"
syn region scopeqlBlockComment start="/\*" end="\*/"

" Operators and punctuation.
syn match scopeqlOperator "[-+*/%<>=!|^:;,().\[\]{}]"

hi def link scopeqlKeyword  Keyword
hi def link scopeqlType     Type
hi def link scopeqlString   String
hi def link scopeqlNumber   Number
hi def link scopeqlComment  Comment
hi def link scopeqlOperator Operator

let b:current_syntax = 'scopeql'