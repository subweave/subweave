//! What an escalation target reads: the ask as its card shows it, the commands that act
//! on it from where the target runs, and the escalation skill.

use std::fmt::Write as _;

use super::Message;

/// `skills/escalation/SKILL.md`: how an agent treats an escalation and the weave.
pub const SKILL: &str = include_str!("../../skills/escalation/SKILL.md");

/// Steery's limit on a throw's body.
const MAX: usize = 64 * 1024;

/// The escalation of `msg` as Markdown. `reach`: how a target elsewhere runs `subweave`
/// on this box (`ssh <reach> sudo -n subweave …`); `None` on the box itself. `skill`:
/// the skill whole, else one line that points at `subweave skill`.
pub(crate) fn body(msg: &Message, reach: Option<&str>, skill: bool) -> String {
    let sw = match reach {
        Some(r) => format!("ssh {r} sudo -n subweave"),
        None => "subweave".to_owned(),
    };
    let n = msg.ask.as_ref().map_or(0, |a| a.n);
    let mut out = format!(
        "# Escalation #{n}: {}\n\n**{}** on **{}** escalates: code, the decider and the coder did not settle it.\n\n",
        msg.title.trim(),
        msg.service,
        msg.host
    );
    out.push_str(&super::details(msg, true));
    if let Some(ask) = &msg.ask {
        out.push_str("\n\nAnswers:");
        for w in &ask.answers {
            let _ = write!(out, "\n- `{w}`: {}", ask.label(w));
        }
    }
    let service = &msg.service;
    let _ = write!(
        out,
        "\n\nCommands:\n- `{sw} inbox`: the open escalations\n- `{sw} answer {n} <word> --note \"…\"`: settle this one\n- `{sw} log {service}`: what the ladder tried\n- `{sw} weave {service}`: the running weave and its ARGs\n- MCP: `{sw} mcp`\n\n"
    );
    if skill {
        out.push_str(SKILL);
    } else {
        let _ = writeln!(out, "Read the escalation skill first: `{sw} skill`.");
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
