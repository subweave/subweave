//! Physical lines to logical lines: comments and blank lines dropped, `\` joins,
//! heredocs read into their instruction.

use super::Problem;

pub struct Line {
    /// The first physical line, counted from 1.
    pub no: u32,
    pub text: String,
    pub heredoc: Option<Heredoc>,
}

pub struct Heredoc {
    pub word: String,
    pub body: String,
}

fn is_heredoc_word(w: &str) -> bool {
    let mut b = w.bytes();
    b.next().is_some_and(|c| c.is_ascii_uppercase())
        && b.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

/// `<<EOF` as a whole token gives `EOF`.
pub fn heredoc_token(tok: &str) -> Option<&str> {
    tok.strip_prefix("<<").filter(|w| is_heredoc_word(w))
}

/// The heredoc word when `<<WORD` stands in place of the instruction's text.
/// A bash heredoc inside a one-line `RUN` or `FACT` command is bash's own.
fn heredoc_start(line: &str) -> Option<String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let at = match *toks.first()? {
        "RUN" | "DO" | "ADD" => 1,
        "FACT" => {
            if toks.get(2) == Some(&"FOLLOW") {
                3
            } else {
                2
            }
        }
        "SKILL" => toks.len() - 1,
        _ => return None,
    };
    let tok = toks.get(at)?;
    let word = heredoc_token(tok)?;
    // A FACT heredoc is the whole text; a RUN heredoc too, or bash would see it.
    if matches!(toks[0], "FACT" | "RUN" | "DO") && at + 1 != toks.len() {
        return None;
    }
    Some(word.to_string())
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start_matches([' ', '\t']).len()
}

/// Splits a Weavefile into logical lines. An unclosed heredoc is a problem and
/// ends the file.
pub fn lines(text: &str) -> (Vec<Line>, Vec<Problem>) {
    let raw: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let no = i as u32 + 1;
        let first = raw[i];
        i += 1;
        let trimmed = first.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut line = first.trim_end().to_string();
        while line.ends_with('\\') && i < raw.len() {
            line.pop();
            line.truncate(line.trim_end().len());
            line.push(' ');
            line.push_str(raw[i].trim());
            line.truncate(line.trim_end().len());
            i += 1;
        }
        let line = line.trim().to_string();
        let heredoc = match heredoc_start(&line) {
            None => None,
            Some(word) => {
                let start = i;
                while i < raw.len() && raw[i].trim() != word {
                    i += 1;
                }
                if i >= raw.len() {
                    problems.push(Problem::new(
                        no,
                        format!("heredoc <<{word} has no closing {word} line"),
                    ));
                    break;
                }
                let body_lines = &raw[start..i];
                i += 1;
                let ind = body_lines
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| indent(l))
                    .min()
                    .unwrap_or(0);
                let body = body_lines
                    .iter()
                    .map(|l| if l.trim().is_empty() { "" } else { &l[ind..] })
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(Heredoc { word, body })
            }
        };
        out.push(Line {
            no,
            text: line,
            heredoc,
        });
    }
    (out, problems)
}

/// The commands of a skill body's ```` ```recipe ```` blocks, one per entry. A line
/// ending in `\` continues its command; blank lines and `#` comments are dropped.
pub fn recipes(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut cmd = String::new();
    for l in body.lines() {
        let t = l.trim();
        if !inside {
            inside = t == "```recipe";
            continue;
        }
        if t.starts_with("```") {
            inside = false;
            if !cmd.is_empty() {
                out.push(std::mem::take(&mut cmd));
            }
            continue;
        }
        if cmd.is_empty() && (t.is_empty() || t.starts_with('#')) {
            continue;
        }
        if !cmd.is_empty() {
            cmd.push('\n');
        }
        cmd.push_str(if cmd.is_empty() { t } else { l.trim_end() });
        if !t.ends_with('\\') {
            out.push(std::mem::take(&mut cmd));
        }
    }
    if !cmd.is_empty() {
        out.push(cmd);
    }
    out
}
