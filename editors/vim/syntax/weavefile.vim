" Vim syntax for Subweave Weavefiles. The TextMate grammar in editors/ is the reference.
if exists("b:current_syntax")
  finish
endif

syn case match

syn match   weaveComment     "^\s*#.*$"
syn match   weaveInstruction "^\s*\zs\(ABOUT\|FROM\|NEEDS\|MODEL\|ARG\|SECRET\|FACT\|PART\|GOAL\|ACTION\|WHEN\|RUN\|DO\|ADD\|NEVER\|ASK\|SKILL\|KNOWN\|DRILL\|RESTORE\)\>"
syn match   weaveName        "\(^\s*\(ARG\|SECRET\|FACT\|PART\|GOAL\|ACTION\|SKILL\|DRILL\)\s\+\)\@<=[A-Za-z_][A-Za-z0-9_-]*"
syn keyword weaveLevel       MUST SHOULD MAY
syn keyword weaveKeyword     AFTER EVERY OVER TRIES PER FOR WITHIN CHANGES UNDO IF BEFORE BECAUSE THEN DECIDE CODE NORMAL TROUBLE BREAK EXPECT FAILS HOLDS
syn match   weaveEffect      "\(CHANGES\s\+\([a-z]\+,\s*\)*\)\@<=\(restart\|config\|version\|data\|network\|nothing\)\>"
syn keyword weaveType        BOOL NUMBER TEXT VERSION LINES
syn keyword weaveLogic       AND OR NOT
syn keyword weaveUnit        CPU GPU GB TB RAM DISK
syn match   weaveFunction    "\<\(SINCE\|MIN\|MAX\|AVG\|DELTA\|COUNT\)\ze("
syn match   weaveNumber      "\<\d\+\(\.\d\+\)\=\>"
syn match   weaveQuantity    "\<\d\+\(\.\d\+\)\=\(%\|\(ms\|s\|m\|h\|d\|B\|KB\|MB\|GB\|TB\)\>\)"
syn match   weaveVariable    "\$\({\)\=[A-Za-z_][A-Za-z0-9_]*\(}\)\="
syn match   weaveSlot        "<[a-z_][a-z0-9_]*>"
syn match   weaveBacktick    "`[^`]*`" contained
syn region  weaveModel       start=+"+ skip=+\\.+ end=+"+ contains=weaveVariable,weaveBacktick
syn region  weaveCode        start=+'+ end=+'+ oneline

" Heredocs: RUN and FACT bodies are bash; DO and SKILL bodies go to a model.
syn include @weaveShell syntax/sh.vim
unlet! b:current_syntax
syn region  weaveShellDoc    matchgroup=weaveHeredoc start="^\s*\(RUN\|FACT\s\+\S\+\s\+\(BOOL\|NUMBER\(\s\+\S\+\)\=\|TEXT\|VERSION\|LINES\)\)\s\+<<\z([A-Z]\+\)\s*$" end="^\s*\z1\s*$" contains=@weaveShell
syn region  weaveModelDoc    matchgroup=weaveHeredoc start="^\s*\(DO\|SKILL\s.*\)<<\z([A-Z]\+\)\s*$" end="^\s*\z1\s*$" contains=weaveVariable,weaveBacktick,weaveRecipe
syn region  weaveRecipe      matchgroup=weaveHeredoc start="^\s*```recipe\s*$" end="^\s*```\s*$" contained contains=@weaveShell
syn region  weaveFileDoc     matchgroup=weaveHeredoc start="^\s*ADD\s\+<<\z([A-Z]\+\)\s\+\S\+\s*$" end="^\s*\z1\s*$"

hi def link weaveComment     Comment
hi def link weaveInstruction Statement
hi def link weaveName        Identifier
hi def link weaveLevel       StorageClass
hi def link weaveKeyword     Keyword
hi def link weaveLogic       Operator
hi def link weaveEffect      Constant
hi def link weaveUnit        Type
hi def link weaveFunction    Function
hi def link weaveType        Type
hi def link weaveQuantity    Number
hi def link weaveNumber      Number
hi def link weaveVariable    PreProc
hi def link weaveSlot        Special
hi def link weaveBacktick    Special
hi def link weaveModel       String
hi def link weaveModelDoc    String
hi def link weaveCode        Character
hi def link weaveHeredoc     Delimiter
hi def link weaveFileDoc     Constant

let b:current_syntax = "weavefile"
