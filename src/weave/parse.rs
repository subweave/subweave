//! Logical lines into a `Weave`. Problems are collected, never fatal: each line is
//! read as far as it goes, so one run reports every broken line.

use std::time::Duration;

use super::Problem;
use super::ast::{
    Action, Arg, ArgKind, ArgOption, Condition, Fact, FactSource, Goal, Level, ModelHint, Needs,
    Rule, Secret, Skill, Step, Weave,
};
use super::expr::{self, Expr};
use super::lex::{self, Line};

/// Instruction keywords: a line starts with one of these.
pub const INSTRUCTIONS: &[&str] = &[
    "ABOUT", "FROM", "NEEDS", "MODEL", "ARG", "SECRET", "FACT", "GOAL", "ACTION", "WHEN", "RUN",
    "DO", "ADD", "NEVER", "ASK", "SKILL",
];

/// Every capital word with a meaning; none of them can be a name.
pub const KEYWORDS: &[&str] = &[
    "ABOUT", "FROM", "NEEDS", "MODEL", "ARG", "SECRET", "FACT", "GOAL", "ACTION", "WHEN", "RUN",
    "DO", "ADD", "NEVER", "ASK", "SKILL", "AFTER", "EVERY", "TRIES", "PER", "FOR", "WITHIN", "IF",
    "AND", "OR", "NOT", "SINCE", "PREV", "FOLLOW", "MUST", "SHOULD", "MAY", "BEFORE", "DECIDE",
    "CODE", "THEN", "CPU", "GPU", "GB", "TB", "RAM", "DISK", "BECAUSE",
];

/// An ACTION's label is a button on a phone.
const LABEL_MAX: usize = 40;

pub fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w)
}

/// ARG, slot and FACT names: they appear in expressions and as `$name`.
pub fn is_value_name(w: &str) -> bool {
    let mut b = w.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == b'_')
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// GOAL, ACTION and SKILL names.
pub fn is_item_name(w: &str) -> bool {
    let mut b = w.bytes();
    b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}

fn is_secret_name(w: &str) -> bool {
    let mut b = w.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == b'_')
        && b.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

fn name(tok: &str, valid: fn(&str) -> bool, form: &str) -> Result<String, String> {
    if is_keyword(tok) {
        Err(format!("`{tok}` is a keyword, not a name"))
    } else if valid(tok) {
        Ok(tok.to_string())
    } else {
        Err(format!("`{tok}` is no valid name: {form}"))
    }
}

const VALUE_FORM: &str = "lowercase letters, digits and `_`";
const ITEM_FORM: &str = "lowercase letters, digits, `-` and `_`";
const SECRET_FORM: &str = "capital letters, digits and `_`";

fn dur(tok: &str) -> Result<Duration, String> {
    expr::duration(tok).ok_or_else(|| format!("`{tok}` is no duration: use 90s, 10m, 25h or 30d"))
}

/// A cursor over one logical line.
struct Cur<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Self {
        Cur { s, i: 0 }
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && self.s.as_bytes()[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn rest(&mut self) -> &'a str {
        self.ws();
        self.s[self.i..].trim_end()
    }

    fn at_end(&mut self) -> bool {
        self.rest().is_empty()
    }

    fn peek(&mut self) -> Option<&'a str> {
        self.ws();
        let r = &self.s[self.i..];
        let end = r.find(|c: char| c.is_ascii_whitespace()).unwrap_or(r.len());
        (end > 0).then(|| &r[..end])
    }

    fn word(&mut self) -> Option<&'a str> {
        let w = self.peek()?;
        self.i += w.len();
        Some(w)
    }

    fn eat(&mut self, kw: &str) -> bool {
        if self.peek() == Some(kw) {
            self.i += kw.len();
            true
        } else {
            false
        }
    }

    fn starts_quoted(&mut self) -> bool {
        self.rest().starts_with('"')
    }

    /// `"text"`, with `\"` and `\\` read as `"` and `\`.
    fn quoted(&mut self) -> Result<String, String> {
        self.ws();
        let r = &self.s[self.i..];
        if !r.starts_with('"') {
            return Err(match r.split_whitespace().next() {
                Some(w) => format!("expected quoted text, found `{w}`"),
                None => "expected quoted text".into(),
            });
        }
        let mut out = String::new();
        let mut chars = r.char_indices().skip(1);
        while let Some((k, c)) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some((_, n @ ('"' | '\\'))) => out.push(n),
                    Some((_, n)) => {
                        out.push('\\');
                        out.push(n);
                    }
                    None => out.push('\\'),
                },
                '"' => {
                    self.i += k + 1;
                    return Ok(out);
                }
                c => out.push(c),
            }
        }
        Err("quoted text has no closing `\"`".into())
    }

    /// `a, b,c` after AFTER or FOR.
    fn names(&mut self) -> Vec<&'a str> {
        let mut out = Vec::new();
        loop {
            self.ws();
            let r = &self.s[self.i..];
            let end = r
                .find(|c: char| c.is_ascii_whitespace() || c == ',')
                .unwrap_or(r.len());
            if end == 0 {
                break;
            }
            out.push(&r[..end]);
            self.i += end;
            self.ws();
            if self.s[self.i..].starts_with(',') {
                self.i += 1;
            } else {
                break;
            }
        }
        out
    }
}

/// Splits a trailing `IF <expr>` outside quotes.
fn split_if(s: &str) -> (&str, Option<&str>) {
    split_kw(s, "IF")
}

/// Splits at the last `kw` word outside quotes: what comes before it and after it.
fn split_kw<'s>(s: &'s str, kw: &str) -> (&'s str, Option<&'s str>) {
    let b = s.as_bytes();
    let k = kw.as_bytes();
    let (mut dq, mut sq) = (false, false);
    let mut last = None;
    for i in 0..b.len() {
        match b[i] {
            b'"' if !sq => dq = !dq,
            b'\'' if !dq => sq = !sq,
            _ if !dq
                && !sq
                && i > 0
                && b[i - 1].is_ascii_whitespace()
                && b[i..].starts_with(k)
                && b.get(i + k.len()).is_none_or(|c| c.is_ascii_whitespace()) =>
            {
                last = Some(i)
            }
            _ => {}
        }
    }
    match last {
        Some(i) => (s[..i].trim_end(), Some(s[i + k.len()..].trim())),
        None => (s, None),
    }
}

/// A code rule's trailing `BECAUSE "<why>"`, split off.
fn split_because(s: &str) -> Result<(&str, Option<String>), String> {
    let (head, why) = split_kw(s, "BECAUSE");
    let Some(why) = why else {
        return Ok((head, None));
    };
    let mut c = Cur::new(why);
    let text = c.quoted()?;
    if !c.at_end() {
        return Err("BECAUSE \"<why>\" comes last, after IF".into());
    }
    Ok((head, Some(text)))
}

struct Parser {
    w: Weave,
    problems: Vec<Problem>,
    /// The action that WHEN/RUN/DO/ADD lines belong to.
    action: Option<usize>,
    /// An options ARG that option lines may follow.
    arg: Option<usize>,
    /// WHEN lines per action.
    whens: Vec<u32>,
}

pub fn parse(name: &str, text: &str) -> (Weave, Vec<Problem>) {
    let (lines, problems) = lex::lines(text);
    let mut p = Parser {
        w: Weave {
            name: name.to_string(),
            about: None,
            from: None,
            needs: Needs::default(),
            model: ModelHint::default(),
            args: Vec::new(),
            secrets: Vec::new(),
            facts: Vec::new(),
            goals: Vec::new(),
            actions: Vec::new(),
            rules: Vec::new(),
            skills: Vec::new(),
        },
        problems,
        action: None,
        arg: None,
        whens: Vec::new(),
    };
    for line in &lines {
        if let Err(msg) = p.line(line) {
            p.problems.push(Problem::new(line.no, msg));
        }
    }
    for (a, whens) in p.w.actions.iter().zip(&p.whens) {
        if *whens == 0 {
            p.problems.push(Problem::new(
                a.line,
                format!("ACTION {} has no WHEN", a.name),
            ));
        }
    }
    (p.w, p.problems)
}

impl Parser {
    fn expr(&mut self, line: u32, src: &str) -> Option<Expr> {
        match expr::parse(src) {
            Ok(e) => Some(e),
            Err(msg) => {
                self.problems.push(Problem::new(line, msg));
                None
            }
        }
    }

    fn heredoc<'l>(line: &'l Line, tok: Option<&str>) -> Option<&'l str> {
        let h = line.heredoc.as_ref()?;
        (tok.and_then(lex::heredoc_token) == Some(h.word.as_str())).then_some(h.body.as_str())
    }

    fn line(&mut self, line: &Line) -> Result<(), String> {
        let mut c = Cur::new(&line.text);
        let kw = c.word().unwrap_or_default();
        if !KEYWORDS.contains(&kw) {
            // A capital word is a misspelt keyword, even under an ARG's options.
            let capital = kw.len() > 1 && kw.bytes().all(|b| b.is_ascii_uppercase());
            if let (Some(i), false) = (self.arg, capital) {
                return self.option(i, line);
            }
            return Err(if capital {
                format!("unknown keyword `{kw}`")
            } else {
                format!("unknown keyword `{kw}`: an instruction starts with a capital keyword")
            });
        }
        if !INSTRUCTIONS.contains(&kw) {
            return Err(format!("`{kw}` does not start an instruction"));
        }
        self.arg = None;
        if !matches!(kw, "WHEN" | "RUN" | "DO" | "ADD") {
            self.action = None;
        }
        let no = line.no;
        match kw {
            "ABOUT" => {
                let text = c.quoted()?;
                if !c.at_end() {
                    return Err("ABOUT takes one quoted line".into());
                }
                if self.w.about.is_some() {
                    return Err("a second ABOUT".into());
                }
                self.w.about = Some(text);
            }
            "FROM" => {
                let spec = c.word().unwrap_or_default();
                let (os, version) = match spec.split_once(':') {
                    Some((os, v)) => (os, Some(v)),
                    None => (spec, None),
                };
                let os_ok = os.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                    && os.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
                let version_ok = version.is_none_or(|v| {
                    v.bytes().next().is_some_and(|b| b.is_ascii_digit())
                        && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                });
                if !(os_ok && version_ok && c.at_end()) {
                    return Err(
                        "FROM takes <os> or <os>:<version>, like ubuntu or ubuntu:24.04".into(),
                    );
                }
                if self.w.from.is_some() {
                    return Err("a second FROM".into());
                }
                self.w.from = Some(super::ast::From {
                    os: os.into(),
                    version: version.map(str::to_string),
                    line: no,
                });
            }
            "NEEDS" => self.needs(no, c.rest())?,
            "MODEL" => self.model(&mut c)?,
            "ARG" => self.arg_line(no, &mut c)?,
            "SECRET" => {
                let (head, cond) = split_if(c.rest());
                let mut h = Cur::new(head);
                let n = name(h.word().unwrap_or_default(), is_secret_name, SECRET_FORM)?;
                let what = h.quoted()?;
                if !h.at_end() {
                    return Err(format!("SECRET {n}: text after the quoted line"));
                }
                let cond = cond.and_then(|src| self.expr(no, src));
                self.w.secrets.push(Secret {
                    name: n,
                    what,
                    cond,
                    line: no,
                });
            }
            "FACT" => {
                let n = name(c.word().unwrap_or_default(), is_value_name, VALUE_FORM)?;
                let follow = c.eat("FOLLOW");
                let rest = c.rest();
                let source = if let Some(body) = Self::heredoc(line, c.peek()) {
                    FactSource::Command(body.to_string())
                } else if rest.starts_with('"') {
                    let q = c.quoted()?;
                    if !c.at_end() {
                        return Err(format!("FACT {n}: text after the quoted line"));
                    }
                    FactSource::Quoted(q)
                } else if rest.is_empty() {
                    return Err(format!("FACT {n} has no command and no quoted text"));
                } else {
                    FactSource::Command(rest.to_string())
                };
                self.w.facts.push(Fact {
                    name: n,
                    follow,
                    source,
                    line: no,
                });
            }
            "GOAL" => self.goal(no, c.rest())?,
            "ACTION" => self.action_line(no, c.rest())?,
            "WHEN" | "RUN" | "DO" | "ADD" => self.step(kw, line, &mut c)?,
            "NEVER" => {
                if c.starts_quoted() {
                    let text = c.quoted()?;
                    if !c.at_end() {
                        return Err("NEVER: text after the quoted line".into());
                    }
                    self.w.rules.push(Rule::Never { text, line: no });
                } else if c.eat("IF") {
                    let (src, because) = split_because(c.rest())?;
                    if let Some(expr) = self.expr(no, src) {
                        self.w.rules.push(Rule::NeverIf {
                            expr,
                            because,
                            line: no,
                        });
                    }
                } else {
                    return Err("NEVER takes quoted text or IF <expr>".into());
                }
            }
            "ASK" => {
                if c.starts_quoted() {
                    let text = c.quoted()?;
                    if !c.at_end() {
                        return Err("ASK: text after the quoted line".into());
                    }
                    self.w.rules.push(Rule::Ask { text, line: no });
                } else if c.eat("BEFORE") {
                    let (rest, because) = split_because(c.rest())?;
                    let (head, cond) = split_if(rest);
                    let mut h = Cur::new(head);
                    let action = name(h.word().unwrap_or_default(), is_item_name, ITEM_FORM)?;
                    if !h.at_end() {
                        return Err(format!(
                            "ASK BEFORE {action}: `{}` is not IF or BECAUSE",
                            h.rest()
                        ));
                    }
                    let cond = cond.and_then(|src| self.expr(no, src));
                    self.w.rules.push(Rule::AskBefore {
                        action,
                        cond,
                        because,
                        line: no,
                    });
                } else if c.eat("IF") {
                    let (src, because) = split_because(c.rest())?;
                    if let Some(expr) = self.expr(no, src) {
                        self.w.rules.push(Rule::AskIf {
                            expr,
                            because,
                            line: no,
                        });
                    }
                } else {
                    return Err("ASK takes quoted text, BEFORE <action> or IF <expr>".into());
                }
            }
            "SKILL" => {
                let n = name(c.word().unwrap_or_default(), is_item_name, ITEM_FORM)?;
                if !c.eat("WHEN") {
                    return Err(format!("SKILL {n} takes WHEN \"<text>\" <<EOF"));
                }
                let when = c.quoted()?;
                let Some(body) = Self::heredoc(line, c.word()) else {
                    return Err(format!("SKILL {n} takes its body as a heredoc: <<EOF"));
                };
                self.w.skills.push(Skill {
                    name: n,
                    when,
                    body: body.to_string(),
                    recipes: lex::recipes(body),
                    line: no,
                });
            }
            _ => unreachable!("every instruction keyword has a branch"),
        }
        Ok(())
    }

    fn needs(&mut self, no: u32, rest: &str) -> Result<(), String> {
        const FORM: &str = "NEEDS takes sizes like 2 CPU, 1 GPU, 4 GB RAM, 40 GB DISK";
        let mut n = Needs {
            line: no,
            ..Needs::default()
        };
        for part in rest.split(',') {
            let toks: Vec<&str> = part.split_whitespace().collect();
            let num: f64 = toks
                .first()
                .and_then(|t| t.parse().ok())
                .filter(|v: &f64| v.is_finite() && *v >= 0.0)
                .ok_or(FORM)?;
            match toks[1..] {
                ["CPU"] => n.cpu = Some(num as u32),
                ["GPU"] => n.gpu = Some(num as u32),
                ["GB", "RAM"] => n.ram_gb = Some(num),
                ["TB", "RAM"] => n.ram_gb = Some(num * 1000.0),
                ["GB", "DISK"] => n.disk_gb = Some(num),
                ["TB", "DISK"] => n.disk_gb = Some(num * 1000.0),
                _ => return Err(FORM.into()),
            }
        }
        if self.w.needs.line != 0 {
            return Err("a second NEEDS".into());
        }
        self.w.needs = n;
        Ok(())
    }

    fn model(&mut self, c: &mut Cur) -> Result<(), String> {
        const FORM: &str = "MODEL takes DECIDE <id> or CODE <id> [THEN <id>]";
        if c.eat("DECIDE") {
            let id = c.word().ok_or(FORM)?;
            if !c.at_end() {
                return Err(FORM.into());
            }
            if self.w.model.decide.is_some() {
                return Err("a second MODEL DECIDE".into());
            }
            self.w.model.decide = Some(id.to_string());
        } else if c.eat("CODE") {
            let mut ids = vec![c.word().ok_or(FORM)?.to_string()];
            while c.eat("THEN") {
                ids.push(c.word().ok_or(FORM)?.to_string());
            }
            if !c.at_end() {
                return Err(FORM.into());
            }
            if !self.w.model.code.is_empty() {
                return Err("a second MODEL CODE".into());
            }
            self.w.model.code = ids;
        } else {
            return Err(FORM.into());
        }
        Ok(())
    }

    fn arg_line(&mut self, no: u32, c: &mut Cur) -> Result<(), String> {
        let head = c.word().unwrap_or_default();
        let (n, default) = match head.split_once('=') {
            Some((n, d)) => (n, Some(d)),
            None => (head, None),
        };
        let n = name(n, is_value_name, VALUE_FORM)?;
        if default == Some("") {
            return Err(format!("ARG {n}: `=` with no default"));
        }
        let level = match c.word() {
            Some("MUST") => Level::Must,
            Some("SHOULD") => Level::Should,
            Some("MAY") => Level::May,
            _ => return Err(format!("ARG {n} needs a level: MUST, SHOULD or MAY")),
        };
        let kind = if c.at_end() {
            self.arg = Some(self.w.args.len());
            ArgKind::Options(Vec::new())
        } else {
            let what = c.quoted()?;
            if !c.at_end() {
                return Err(format!("ARG {n}: text after the quoted line"));
            }
            ArgKind::Value { what }
        };
        self.w.args.push(Arg {
            name: n,
            default: default.map(String::from),
            level,
            kind,
            line: no,
        });
        Ok(())
    }

    fn option(&mut self, i: usize, line: &Line) -> Result<(), String> {
        let Some(q) = line.text.find('"') else {
            return Err(format!(
                "option `{}` of ARG {} has no quoted meaning",
                line.text, self.w.args[i].name
            ));
        };
        let label = line.text[..q].trim().to_string();
        let mut c = Cur::new(&line.text[q..]);
        let meaning = c.quoted()?;
        if !c.at_end() {
            return Err(format!("option `{label}`: text after the quoted meaning"));
        }
        if label.is_empty() {
            return Err("an option needs a label before its meaning".into());
        }
        let mut slots = Vec::new();
        let mut rest = label.as_str();
        while let Some(open) = rest.find('<') {
            let Some(close) = rest[open..].find('>') else {
                return Err(format!("option `{label}`: `<` has no closing `>`"));
            };
            let slot = &rest[open + 1..open + close];
            slots.push(name(slot, is_value_name, VALUE_FORM)?);
            rest = &rest[open + close + 1..];
        }
        if let ArgKind::Options(opts) = &mut self.w.args[i].kind {
            opts.push(ArgOption {
                label,
                meaning,
                slots,
                line: line.no,
            });
        }
        self.arg = Some(i);
        Ok(())
    }

    fn goal(&mut self, no: u32, rest: &str) -> Result<(), String> {
        let (head, when_src) = split_if(rest);
        let mut c = Cur::new(head);
        let n = name(c.word().unwrap_or_default(), is_item_name, ITEM_FORM)?;
        let mut goal = Goal {
            name: n.clone(),
            plain: None,
            after: Vec::new(),
            every: None,
            tries: None,
            cond: Condition::Quoted(String::new()),
            when: None,
            line: no,
        };
        // Quoted text right after the name is the plain sentence when a condition follows
        // it, else the quoted condition itself.
        let mut first = None;
        let res = (|| {
            if c.starts_quoted() {
                first = Some(c.quoted()?);
            }
            if c.eat("AFTER") {
                goal.after = c.names().into_iter().map(String::from).collect();
                if goal.after.is_empty() {
                    return Err(format!("GOAL {n}: AFTER names no goal"));
                }
            }
            if c.eat("EVERY") {
                goal.every = Some(dur(c.word().unwrap_or_default())?);
            }
            if c.eat("TRIES") {
                let t = c.word().unwrap_or_default();
                let tries: u32 = t
                    .parse()
                    .map_err(|_| format!("GOAL {n}: TRIES takes a count, not `{t}`"))?;
                if !c.eat("PER") {
                    return Err(format!("GOAL {n}: TRIES {tries} PER <duration>"));
                }
                goal.tries = Some((tries, dur(c.word().unwrap_or_default())?));
            }
            Ok(())
        })();
        let rest = c.rest();
        let cond_src = match first {
            Some(text) if rest.is_empty() => {
                goal.cond = Condition::Quoted(text);
                None
            }
            Some(text) => {
                goal.plain = Some(text);
                Some(rest)
            }
            None => Some(rest),
        };
        if let (Ok(()), Some(cond_src)) = (&res, cond_src) {
            if cond_src.is_empty() {
                self.problems
                    .push(Problem::new(no, format!("GOAL {n} has no condition")));
            } else if cond_src.starts_with('"') {
                let mut q = Cur::new(cond_src);
                match q.quoted() {
                    Ok(text) if q.at_end() => goal.cond = Condition::Quoted(text),
                    Ok(_) => self.problems.push(Problem::new(
                        no,
                        format!("GOAL {n}: text after the quoted condition"),
                    )),
                    Err(msg) => self.problems.push(Problem::new(no, msg)),
                }
            } else if let Some(e) = self.expr(no, cond_src) {
                goal.cond = Condition::Code(e);
            }
        }
        goal.when = when_src.and_then(|src| self.expr(no, src));
        self.w.goals.push(goal);
        res
    }

    fn action_line(&mut self, no: u32, rest: &str) -> Result<(), String> {
        let (head, when_src) = split_if(rest);
        let mut c = Cur::new(head);
        let n = name(c.word().unwrap_or_default(), is_item_name, ITEM_FORM)?;
        if n == crate::service::TASK {
            return Err(format!(
                "ACTION {n}: the name belongs to a person's task (subweave task); pick another"
            ));
        }
        let mut action = Action {
            name: n.clone(),
            label: None,
            for_goals: Vec::new(),
            within: None,
            when_if: None,
            when: String::new(),
            steps: Vec::new(),
            line: no,
        };
        let res = (|| {
            if c.starts_quoted() {
                let l = c.quoted()?;
                if l.trim().is_empty() || l.chars().count() > LABEL_MAX {
                    return Err(format!(
                        "ACTION {n}: the label is a button: 1 to {LABEL_MAX} characters"
                    ));
                }
                action.label = Some(l);
            }
            if c.eat("FOR") {
                action.for_goals = c.names().into_iter().map(String::from).collect();
                if action.for_goals.is_empty() {
                    return Err(format!("ACTION {n}: FOR names no goal"));
                }
            }
            if c.eat("WITHIN") {
                action.within = Some(dur(c.word().unwrap_or_default())?);
            }
            if !c.at_end() {
                return Err(format!(
                    "ACTION {n}: `{}` is not FOR, WITHIN or IF",
                    c.rest()
                ));
            }
            Ok(())
        })();
        action.when_if = when_src.and_then(|src| self.expr(no, src));
        self.action = Some(self.w.actions.len());
        self.w.actions.push(action);
        self.whens.push(0);
        res
    }

    fn step(&mut self, kw: &str, line: &Line, c: &mut Cur) -> Result<(), String> {
        let Some(i) = self.action else {
            return Err(format!("{kw} outside an ACTION"));
        };
        let no = line.no;
        let action = &mut self.w.actions[i];
        match kw {
            "WHEN" => {
                let text = c.quoted()?;
                if !c.at_end() {
                    return Err("WHEN: text after the quoted line".into());
                }
                self.whens[i] += 1;
                if self.whens[i] > 1 {
                    return Err(format!("a second WHEN for ACTION {}", action.name));
                }
                action.when = text;
            }
            "RUN" => {
                let script = match Self::heredoc(line, c.peek()) {
                    Some(body) => body.to_string(),
                    None => c.rest().to_string(),
                };
                if script.trim().is_empty() {
                    return Err("RUN has no command".into());
                }
                action.steps.push(Step::Run { script, line: no });
            }
            "DO" => {
                let task = match Self::heredoc(line, c.peek()) {
                    Some(body) => body.to_string(),
                    None => {
                        let q = c.quoted()?;
                        if !c.at_end() {
                            return Err("DO: text after the quoted task".into());
                        }
                        q
                    }
                };
                action.steps.push(Step::Do { task, line: no });
            }
            "ADD" => {
                let Some(content) = Self::heredoc(line, c.word()) else {
                    return Err("ADD takes <<EOF <path>".into());
                };
                let path = c.rest();
                if path.is_empty() || path.contains(char::is_whitespace) {
                    return Err("ADD takes <<EOF <path>, one path".into());
                }
                action.steps.push(Step::Add {
                    path: path.to_string(),
                    content: content.to_string(),
                    line: no,
                });
            }
            _ => unreachable!("only step keywords reach step()"),
        }
        Ok(())
    }
}
