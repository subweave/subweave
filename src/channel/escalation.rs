//! What an escalation target reads: the ask as its card shows it, how to act on it from
//! where the target runs, and the escalation skill.

use std::fmt::Write as _;

use super::Message;

/// `skills/escalation/SKILL.md`: how an agent treats an escalation and the weave.
pub const SKILL: &str = include_str!("../../skills/escalation/SKILL.md");

/// Steery's limit on a throw's body.
const MAX: usize = 64 * 1024;

/// The escalation of `msg` as Markdown. `outpost`: the Steery outpost a target elsewhere
/// calls this box's tools through; `None` on the box itself, where `subweave` runs.
/// `skill`: the skill whole, else one line that points at it.
pub(crate) fn body(msg: &Message, outpost: Option<&str>, skill: bool) -> String {
    let n = msg.ask.as_ref().map_or(0, |a| a.n);
    let mut out = format!(
        "# Escalation #{n}: {}\n\n**{}** on **{}** escalates: code, the decider and the coder did not settle it.\n\n",
        msg.card.title.trim(),
        msg.service,
        msg.host
    );
    out.push_str(&super::details(msg, true));
    if msg.ask.is_some() {
        out.push_str("\n\nAnswers:");
        for b in &msg.card.buttons {
            let _ = write!(out, "\n- `{}`: {}", b.word, b.label);
        }
    }
    let service = &msg.service;
    match outpost {
        None => {
            let _ = write!(
                out,
                "\n\nCommands:\n- `subweave inbox`: the open escalations\n- `subweave answer {n} <word> --note \"…\"`: settle this one\n- `subweave log {service}`: what the ladder tried\n- `subweave weave {service}`: the running weave and its ARGs\n- MCP: `subweave mcp`\n\n"
            );
        }
        Some(o) => {
            let _ = write!(
                out,
                "\n\nTools: this box is the Steery outpost `{o}`. Read its tools' schemas with `steery outposts get {o}`, then call each with `steery invocations create --json '{{\"outpost\": \"{o}\", \"tool\": \"<tool>\", \"arguments\": {{…}}, \"wait_secs\": 60}}'`:\n- `inbox`: the open escalations\n- `answer` (`service: \"{service}\"`, `n: {n}`, `word`, `note`): settle this one\n- `log` (`service: \"{service}\"`): what the ladder tried\n- `weave` (`service: \"{service}\"`): the running weave and its ARGs\n- `task` and `weave_change` run only after the person's Go on Steery: the invocation waits for it; read it again with `steery invocations get <id> --wait-secs 60`.\n\n"
            );
        }
    }
    if skill {
        out.push_str(SKILL);
    } else if outpost.is_some() {
        out.push_str("Read the escalation skill first: the outpost's `skill` tool.\n");
    } else {
        out.push_str("Read the escalation skill first: `subweave skill`.\n");
    }
    if out.len() > MAX {
        let mut end = MAX - 16;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n…[cut]\n");
    }
    out
}
