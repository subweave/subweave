---
name: subweave-escalation
description: Use when a Subweave box escalates to you: settle the escalation, fix the weave so the next case does not escalate, and upstream generic fixes.
---

# Subweave escalation

Each command below runs on the box. From another machine, put the box's reach in
front of it: `ssh <user@host> sudo -n subweave …`. Over MCP, `subweave mcp` gives the
same tools.

## 1. What an escalation is

- A box runs services as their Weavefile (the weave) says. When a goal fails, the
  daemon climbs a ladder: code (learned steps, `RUN`), then the decider (a cheap
  model), then the coder (a cheap model).
- An escalation is the last step: the ladder did not settle the case, so it comes to
  you. Its number is `n`; it takes one of its answer words.
- Read what the ladder already tried before you act: `subweave log <service>`. Read
  the running weave and its ARGs: `subweave weave <service>`.

## 2. Settle it

- Answer with one of its words: `subweave answer <n> <word> --note "why"`. The note
  goes in the journal; write the reason in one sentence.
- Or act, and let the goal hold again: `subweave task <service> "<what to do>"` or
  `subweave do <service> <action>`. An escalation withdraws itself when its goal
  holds.
- Bring the person in through your own channel to them (in a Steery conversation: a
  Steery request) when the case needs a product or risk decision, can lose data,
  costs money, or needs a rule change.
- An escalation marked signed is not yours to answer: only the person's signed answer
  on Steery settles it.

## 3. Teach kinds of log lines

- A goal that judges log lines (a quoted GOAL naming a LINES fact) is judged one kind
  of line at a time. A kind is the line with its values masked: numbers, times, hex,
  IDs, IPs, quoted text. The escalation's `lines` are the kinds nobody knows normal.
- When its lines are routine or real trouble, say so with the answer:
  `subweave answer <n> <word> --known normal|trouble --note "why"` (MCP: `known`).
- Outside an escalation: `subweave known <service> <goal> normal|trouble "<line>"
  --why "<reason>"`; `subweave forget-known <service> <goal> "<line>"` takes it back.
- A `PART` release the model judged wrong: `subweave known <service> <part>
  security|routine <version> --why "<reason>"` (MCP: `known_release`).
- Never add lists of normal lines to a goal's sentence: teach the kinds. The box
  drops normal ones in code and fails on trouble ones without a model.
- `subweave learn <service>` drafts KNOWN lines from what the box knows; generic ones
  belong in the weave (section 4).

## 4. Fix the weave, not only the box

- Ask: could the cheap ladder have settled this with a better FACT, GOAL wording,
  ACTION or SKILL? If yes, change the local weave, so the next case does not
  escalate.
- Write the new Weavefile and propose it: `subweave weave <service> --change <file>
  --why "<reason>"`, or the MCP tool `weave_change`. Never write the Weavefile on the
  box in place.
- The box checks the change first: it must parse, and the request check must pass
  with the service's ARGs.
- FACT, GOAL, ACTION, SKILL and KNOWN changes apply at once. Every other change
  (NEVER, ASK, DRILL, ARG, SECRET, MODEL, NEEDS, FROM, ABOUT) is a rule change: it
  waits for the person's signed answer. Propose it; do not insist.
- Write the general rule, not the incident. Leave out hostnames, versions and
  one-off strings unless the weave's job needs them.

## 4a. Drill escalations

- A drill breaks the service on purpose. Its escalation (action `drill`) says
  "`<goal>` did not notice `<label>`" or "did not recover from `<label>`". The box
  is not the problem: the weave is. RESTORE has already run, when the drill has one.
- "Did not notice": fix the goal's check so it fails on what BREAK did (a fact it
  does not read, a condition too loose, an EVERY too slow for FAILS WITHIN).
- "Did not recover": fix or add the ACTION that repairs it, or its WHEN, so the
  ladder picks it.
- Propose the weave change (section 4), then answer `yes`: the drill runs again.
  Answer `no` only when the drill itself is wrong, and say why in the note.
- Never weaken a drill to pass it; never remove one without the person.

## 5. Upstream generic fixes

- `subweave weave <service>` shows where the weave came from. Library weaves live in
  `github.com/subweave/library`.
- When the local change worked and the fix is generic, open a pull request there:
  fork, branch, `gh pr create`. One pull request per fix. Never push to someone's
  default branch.
- Redact everything personal or revealing: hostnames, IP addresses, account, company
  and product names, paths that reveal them, keys, chain or customer data, internal
  URLs.
- Describe the evidence in general words: "a healthy mainnet node logs X every few
  minutes".

## 6. Never

- Never weaken a NEVER rule.
- Never read or print SECRET values.
- Never delete data the weave protects.
- Never answer a signed escalation.
