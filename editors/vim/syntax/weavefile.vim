" Vim syntax for Subweave Weavefiles. The TextMate grammar in editors/ is the reference.
if exists("b:current_syntax")
  finish
endif

syn case match

syn match   weaveComment     "^\s*#.*$"
syn match   weaveInstruction "^\s*\zs\(ABOUT\|FROM\|NEEDS\|MODEL\|ARG\|SECRET\|FACT\|GOAL\|ACTION\|WHEN\|RUN\|DO\|ADD\|NEVER\|ASK\|SKILL\|KNOWN\)\>"
syn match   weaveName        "\(^\s*\(ARG\|SECRET\|FACT\|GOAL\|ACTION\|SKILL\)\s\+\)\@<=[A-Za-z_][A-Za-z0-9_-]*"
syn keyword weaveLevel       MUST SHOULD MAY
syn keyword weaveKeyword     AFTER EVERY TRIES PER FOR WITHIN IF BEFORE BECAUSE FOLLOW THEN DECIDE CODE NORMAL TROUBLE
syn keyword weaveLogic       AND OR NOT
syn keyword weaveUnit        CPU GPU GB TB RAM DISK
syn match   weaveFunction    "\<\(SINCE\|PREV\)\ze("
syn match   weaveDuration    "\<\d\+[smhd]\>"
syn match   weaveNumber      "\<\d\+\(\.\d\+\)\=\>"
syn match   weaveVariable    "\$\({\)\=[A-Za-z_][A-Za-z0-9_]*\(}\)\="
syn match   weaveSlot        "<[a-z_][a-z0-9_]*>"
syn match   weaveBacktick    "`[^`]*`" contained
syn region  weaveModel       start=+"+ skip=+\\.+ end=+"+ contains=weaveVariable,weaveBacktick
syn region  weaveCode        start=+'+ end=+'+ oneline

" Heredocs: RUN and FACT bodies are bash; DO and SKILL bodies go to a model.
syn include @weaveShell syntax/sh.vim
unlet! b:current_syntax
syn region  weaveShellDoc    matchgroup=weaveHeredoc start="^\s*\(RUN\|FACT\s\+\S\+\(\s\+FOLLOW\)\=\)\s\+<<\z([A-Z]\+\)\s*$" end="^\s*\z1\s*$" contains=@weaveShell
syn region  weaveModelDoc    matchgroup=weaveHeredoc start="^\s*\(DO\|SKILL\s.*\)<<\z([A-Z]\+\)\s*$" end="^\s*\z1\s*$" contains=weaveVariable,weaveBacktick,weaveRecipe
syn region  weaveRecipe      matchgroup=weaveHeredoc start="^\s*```recipe\s*$" end="^\s*```\s*$" contained contains=@weaveShell
syn region  weaveFileDoc     matchgroup=weaveHeredoc start="^\s*ADD\s\+<<\z([A-Z]\+\)\s\+\S\+\s*$" end="^\s*\z1\s*$"

hi def link weaveComment     Comment
hi def link weaveInstruction Statement
hi def link weaveName        Identifier
hi def link weaveLevel       StorageClass
hi def link weaveKeyword     Keyword
hi def link weaveLogic       Operator
hi def link weaveUnit        Type
hi def link weaveFunction    Function
hi def link weaveDuration    Number
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
