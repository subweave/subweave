//! The rules checked in code, by the id `rules.toml` gives them in `code` or `unless`.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Element, commands, exprs};
use crate::weave::expr::Expr;
use crate::weave::{ArgKind, Condition, FactSource, Rule, Step, Weave};

pub const CHECKS: &[&str] = &[
    "about",
    "needs",
    "option-slot",
    "one-option",
    "arg-used",
    "secret-name",
    "secret-used",
    "bash-syntax",
    "goal-has-action",
    "goal-since-action",
    "goal-names-facts",
    "action-steps",
    "within",
    "literal-arg",
    "catch-all",
    "asked-before",
    "no-default",
    "no-do",
];

/// Whether the element passes check `id`. `within` reads the decider's answer, so it
/// never comes here.
pub fn check(id: &str, w: &Weave, el: &Element) -> bool {
    let name = el.str("name");
    match id {
        "about" => w.about.is_some(),
        "needs" => w.needs.cpu.is_some() && w.needs.ram_gb.is_some() && w.needs.disk_gb.is_some(),
        "option-slot" => el
            .state
            .get("slots")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty),
        "one-option" => el
            .state
            .get("options")
            .and_then(Value::as_object)
            .is_none_or(|o| o.len() < 2),
        "arg-used" => {
            // An option ARG is used when one of its slots is: the slot carries its value.
            let slots: Vec<&str> = match w.arg(name).map(|a| &a.kind) {
                Some(ArgKind::Options(opts)) => opts
                    .iter()
                    .flat_map(|o| o.slots.iter().map(String::as_str))
                    .collect(),
                _ => Vec::new(),
            };
            let texts = texts(w);
            let exprs = exprs(w);
            std::iter::once(name).chain(slots).any(|n| {
                texts.iter().any(|t| dollar_ref(t, n)) || exprs.iter().any(|e| reads(e, n))
            })
        }
        "secret-name" => {
            let mut b = name.bytes();
            b.next().is_some_and(|c| c.is_ascii_uppercase())
                && b.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        }
        "secret-used" => texts(w).iter().any(|t| {
            t.match_indices(name).any(|(i, _)| {
                !t[..i].chars().next_back().is_some_and(is_word)
                    && !t[i + name.len()..].chars().next().is_some_and(is_word)
            })
        }),
        "bash-syntax" => bash_parses(el.str("command")),
        "goal-has-action" => w
            .actions
            .iter()
            .any(|a| a.for_goals.is_empty() || a.for_goals.iter().any(|g| g == name)),
        "goal-since-action" => {
            let Some(Condition::Code(e)) = w.goal(name).map(|g| &g.cond) else {
                return true;
            };
            let mut ok = true;
            e.walk(&mut |x| {
                if let Expr::Since(a) = x {
                    ok &= w.action(a).is_some_and(|a| {
                        a.for_goals.is_empty() || a.for_goals.iter().any(|g| g == name)
                    });
                }
            });
            ok
        }
        "goal-names-facts" => backticked(el.str("condition")),
        "action-steps" => w.action(name).is_some_and(|a| !a.steps.is_empty()),
        "literal-arg" => {
            // a value ARG whose default holds a digit (a version, a port) written out
            let cmd = el.str("command");
            !w.args.iter().any(|a| {
                let (ArgKind::Value { .. }, Some(d)) = (&a.kind, &a.default) else {
                    return false;
                };
                d.bytes().any(|b| b.is_ascii_digit())
                    && cmd.match_indices(d.as_str()).any(|(i, _)| {
                        let apart = |c: char| !(is_word(c) || c == '.');
                        cmd[..i].chars().next_back().is_none_or(apart)
                            && cmd[i + d.len()..].chars().next().is_none_or(apart)
                    })
            })
        }
        "catch-all" => w
            .actions
            .iter()
            .any(|a| a.for_goals.is_empty() && !w.undoes(&a.name)),
        "asked-before" => w.action(name).is_some_and(|a| {
            w.rules.iter().any(|r| match r {
                Rule::AskBefore { on, .. } | Rule::NeverBefore { on, .. } => on.gates(a),
                _ => false,
            })
        }),
        "no-default" => el.state.get("default").is_none_or(Value::is_null),
        "no-do" => !w
            .actions
            .iter()
            .flat_map(|a| &a.steps)
            .any(|s| matches!(s, Step::Do { .. })),
        _ => unreachable!("rules() checked every code id against CHECKS"),
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `$name` or `${name}` in `text`, the name ending at a word boundary.
pub fn dollar_ref(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let before = &text[..i];
        (before.ends_with('$') || before.ends_with("${"))
            && !text[i + name.len()..].chars().next().is_some_and(is_word)
    })
}

/// A backticked name: `` `name` ``.
fn backticked(text: &str) -> bool {
    text.match_indices('`').any(|(i, _)| {
        let rest = &text[i + 1..];
        let n = rest
            .find(|c: char| !(is_word(c) || c == '-'))
            .unwrap_or(rest.len());
        n > 0 && rest[n..].starts_with('`')
    })
}

/// Whether the expression reads `name`, as a name or through a window function.
fn reads(e: &Expr, name: &str) -> bool {
    let mut found = false;
    e.walk(&mut |x| {
        if let Expr::Name(n) | Expr::Window(_, n) = x {
            found |= n == name;
        }
    });
    found
}

fn bash_parses(script: &str) -> bool {
    let Ok(mut child) = Command::new("bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

/// Every text a name can be read in: commands, quoted text, task and skill bodies, and
/// the expressions as written.
fn texts(w: &Weave) -> Vec<String> {
    let mut t: Vec<String> = commands(w)
        .into_iter()
        .map(|(_, c, _)| c.to_string())
        .collect();
    for a in &w.actions {
        t.push(a.when.clone());
        t.extend(a.when_if.as_ref().map(Expr::to_string));
        for s in &a.steps {
            t.push(match s {
                Step::Run { script, .. } => script.clone(),
                Step::Do { task, .. } => task.clone(),
                Step::Add { content, .. } => content.clone(),
            });
        }
    }
    for g in &w.goals {
        t.push(match &g.cond {
            Condition::Code(e) => e.to_string(),
            Condition::Quoted(q) => q.clone(),
        });
        t.extend(g.when.as_ref().map(Expr::to_string));
    }
    for f in &w.facts {
        if let FactSource::Quoted(q) = &f.source {
            t.push(q.clone());
        }
    }
    for s in &w.secrets {
        t.push(s.what.clone());
        t.extend(s.cond.as_ref().map(Expr::to_string));
    }
    for r in &w.rules {
        match r {
            Rule::Never { text, .. } | Rule::Ask { text, .. } => t.push(text.clone()),
            Rule::NeverIf { expr, .. } | Rule::AskIf { expr, .. } => t.push(expr.to_string()),
            Rule::AskBefore { cond, .. } | Rule::NeverBefore { cond, .. } => {
                t.extend(cond.as_ref().map(Expr::to_string))
            }
        }
    }
    for s in &w.skills {
        t.push(s.body.clone());
        t.push(s.when.clone());
    }
    t
}
