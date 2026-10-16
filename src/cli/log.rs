use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::daemon::journal::Tail;
use crate::daemon::state::{Event, Kind};
use crate::paths;
use crate::service;

/// Lines `log -f` shows before it follows.
const BACKLOG: usize = 20;

#[derive(clap::Args)]
pub struct Args {
    /// The service; may be left out when the box keeps one
    pub service: Option<String>,
    /// Keep printing new lines
    #[arg(short, long)]
    follow: bool,
    /// Print the journal's JSON lines as they are
    #[arg(long)]
    json: bool,
    /// Every service, by time
    #[arg(long, conflicts_with = "service")]
    all: bool,
    /// Run on this box
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let services = match (&args.service, args.all) {
        (Some(s), _) => {
            super::known(s)?;
            vec![s.clone()]
        }
        (None, true) => service::names()?,
        (None, false) => {
            let names = service::names()?;
            match names.len() {
                1 => names,
                0 => bail!("no services here"),
                _ => bail!("name a service, or use --all: {}", names.join(", ")),
            }
        }
    };
    let tag = args.all;
    let mut lines = Vec::new();
    let mut tails = Vec::new();
    for s in &services {
        let mut tail = Tail::start(paths::journal(s));
        lines.extend(tail.lines().into_iter().map(|l| Entry::new(s, l)));
        tails.push((s.clone(), tail));
    }
    lines.sort_by_key(|e| e.t);
    let skip = if args.follow {
        lines.len().saturating_sub(BACKLOG)
    } else {
        0
    };
    for e in &lines[skip..] {
        e.print(tag, args.json);
    }
    if !args.follow {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if args.all {
            for s in service::names()? {
                if tails.iter().all(|(t, _)| *t != s) {
                    tails.push((s.clone(), Tail::start(paths::journal(&s))));
                }
            }
        }
        let mut new = Vec::new();
        for (s, tail) in &mut tails {
            new.extend(tail.lines().into_iter().map(|l| Entry::new(s, l)));
        }
        new.sort_by_key(|e| e.t);
        for e in &new {
            e.print(tag, args.json);
        }
    }
}

/// One raw journal line of one service.
struct Entry<'a> {
    service: &'a str,
    raw: String,
    event: Option<Event>,
    t: Option<Timestamp>,
}

impl<'a> Entry<'a> {
    fn new(service: &'a str, raw: String) -> Entry<'a> {
        let event: Option<Event> = serde_json::from_str(&raw).ok();
        let t = event.as_ref().and_then(|e| e.t.parse().ok());
        Entry {
            service,
            raw,
            event,
            t,
        }
    }

    fn print(&self, tag: bool, json: bool) {
        if json {
            println!("{}", tagged_json(tag.then_some(self.service), &self.raw));
        } else if let Some(e) = &self.event {
            println!("{}", human(tag.then_some(self.service), e));
        }
    }
}

/// The raw line, with `"service"` first when tagged.
fn tagged_json(service: Option<&str>, raw: &str) -> String {
    let Some(service) = service else {
        return raw.to_string();
    };
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw) {
        Ok(fields) => {
            let mut out = serde_json::Map::with_capacity(fields.len() + 1);
            out.insert("service".into(), service.into());
            out.extend(fields);
            serde_json::Value::Object(out).to_string()
        }
        Err(_) => raw.to_string(),
    }
}

/// `HH:MM:SS` in this computer's time zone; the text as given when it is no time.
pub(crate) fn clock(t: &str) -> String {
    match t.parse::<Timestamp>() {
        Ok(ts) => ts
            .to_zoned(TimeZone::system())
            .strftime("%H:%M:%S")
            .to_string(),
        Err(_) => t.to_string(),
    }
}

/// `HH:MM:SS [service] goal  kind  ok  text  facts/p/cost`, one line.
pub(crate) fn human(service: Option<&str>, e: &Event) -> String {
    let mut out = clock(&e.t);
    if let Some(s) = service {
        let _ = write!(out, " {s:12}");
    }
    let ok = match e.ok() {
        Some(true) => "ok",
        Some(false) => "fail",
        None => "-",
    };
    let _ = write!(
        out,
        " {:12} {:7} {:4}",
        e.goal().unwrap_or("-"),
        e.kind.name(),
        ok
    );
    let mut text = String::new();
    if let Some(n) = e.n() {
        let _ = write!(text, "#{n} ");
    }
    if let Some(a) = e.action() {
        text.push_str(a);
        text.push(' ');
    }
    match &e.kind {
        // What was refused, then why.
        Kind::Refused { why, refused } => {
            let _ = write!(
                text,
                "{} {}: {}",
                refused.name(),
                one_line(refused.text().unwrap_or_default(), 40),
                one_line(why, 100)
            );
        }
        _ => {
            if let Some(t) = e.text() {
                text.push_str(&one_line(t, 100));
            }
        }
    }
    if let Kind::Ask { card, .. } = &e.kind {
        let _ = write!(text, " [{}]", card.words().join(" "));
    }
    let text = text.trim();
    if !text.is_empty() {
        out.push(' ');
        out.push_str(text);
    }
    let mut extra = Vec::new();
    if let Some(f) = e.kind.facts() {
        let facts: Vec<String> = f
            .iter()
            .map(|(k, v)| format!("{k}={}", one_line(v, 24)))
            .collect();
        if !facts.is_empty() {
            extra.push(facts.join(" "));
        }
    }
    if let Kind::Check { lines, p, .. } = &e.kind {
        for (k, ls) in lines {
            let n = ls.len();
            let last = ls.last().map(|l| one_line(&l.text, 60)).unwrap_or_default();
            extra.push(format!("{k}: {n} new line{}, last: {last}", if n == 1 { "" } else { "s" }));
        }
        if let Some(p) = p {
            extra.push(format!("p {p:.2}"));
        }
    }
    if let Kind::Decide { p, .. } | Kind::Gate { p, .. } = &e.kind {
        if !p.is_empty() {
            let mut p: Vec<(&String, &f64)> = p.iter().collect();
            p.sort_by(|a, b| b.1.total_cmp(a.1));
            let p: Vec<String> = p.iter().map(|(k, v)| format!("{k} {v:.2}")).collect();
            extra.push(p.join(" "));
        }
    }
    if let Some(c) = e.cost_usd {
        extra.push(format!("${c:.5}"));
    }
    for x in extra {
        out.push_str("  ");
        out.push_str(&x);
    }
    out.trim_end().to_string()
}

/// Whitespace runs as one space, cut at `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 3));
    for (i, w) in s.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.chars().count() > max {
        let cut: String = out.chars().take(max - 1).collect();
        return cut + "…";
    }
    out
}
