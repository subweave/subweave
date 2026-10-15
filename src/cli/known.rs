//! `subweave known` and `subweave forget-known`: a person or an agent tells the box what a
//! kind of log line is for a goal, or whether a PART's release is a security fix; or
//! makes it forget a kind of line.

use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::daemon::{journal, memory};
use crate::service::{self, Command};
use crate::weave::Class;

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// A quoted goal over a LINES fact, or a PART
    name: String,
    /// normal or trouble for a goal's line; security or routine for a PART's release
    #[arg(value_parser = said)]
    said: Said,
    /// One line of the kind, as the log printed it; or the release's version
    line: String,
    /// Why, in a sentence
    #[arg(long)]
    why: Option<String>,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// What `known` says: a kind of line's class, or whether a release is a security fix.
#[derive(Clone, Copy)]
enum Said {
    Line(Class),
    Release { security: bool },
}

fn said(word: &str) -> Result<Said, String> {
    match word {
        "security" => Ok(Said::Release { security: true }),
        "routine" => Ok(Said::Release { security: false }),
        w => Class::parse(w)
            .map(Said::Line)
            .ok_or_else(|| format!("{w:?} is not normal, trouble, security or routine")),
    }
}

#[derive(clap::Args)]
pub struct ForgetArgs {
    pub service: String,
    goal: String,
    /// One line of the kind, as the log printed it
    line: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// `normal` or `trouble`, for clap.
pub(crate) fn class(word: &str) -> Result<Class, String> {
    Class::parse(word).ok_or_else(|| format!("{word:?} is not normal or trouble"))
}

pub fn run(args: Args) -> Result<ExitCode> {
    let why = args.why.map(|w| w.trim().to_string()).filter(|w| !w.is_empty());
    match args.said {
        Said::Line(class) => {
            let template = send(&args.service, &args.name, &args.line, Some(class), why, super::by())?;
            println!(
                "{} {}: lines like `{template}` are known {}",
                args.service,
                args.name,
                class.as_str()
            );
        }
        Said::Release { security } => {
            let version = send_release(&args.service, &args.name, &args.line, security, why, super::by())?;
            println!(
                "{} {} {version}: known {}",
                args.service,
                args.name,
                if security { "security" } else { "routine" }
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Hands the daemon whether release `version` of PART `part` is a security fix, after
/// checking the part exists and the version parses. The version as kept, without `v`.
pub(crate) fn send_release(
    service: &str,
    part: &str,
    version: &str,
    security: bool,
    why: Option<String>,
    by: String,
) -> Result<String> {
    let w = super::service_weave(service)?;
    if w.part(part).is_none() {
        bail!("{service} has no PART {part}");
    }
    let Some(v) = crate::weave::Version::parse(version) else {
        bail!("{version:?} is no version");
    };
    let version = v.bare().to_string();
    service::put(&Command::Release {
        service: service.to_string(),
        part: part.to_string(),
        version: version.clone(),
        security,
        why,
        by,
    })?;
    Ok(version)
}

pub fn forget(args: ForgetArgs) -> Result<ExitCode> {
    let template = send(&args.service, &args.goal, &args.line, None, None, super::by())?;
    println!(
        "asked the daemon to forget lines like `{template}` for {} in {}",
        args.goal, args.service
    );
    Ok(ExitCode::SUCCESS)
}

/// Hands the daemon what lines like `line` are for `goal` (`None`: forget them), after
/// checking the goal judges lines and, to forget, that the box knows something of them.
/// The lines' template.
pub(crate) fn send(
    service: &str,
    goal: &str,
    line: &str,
    class: Option<Class>,
    why: Option<String>,
    by: String,
) -> Result<String> {
    let w = super::service_weave(service)?;
    let Some(g) = w.goal(goal) else {
        bail!("{service} has no goal {goal}");
    };
    if w.judged_lines(g).next().is_none() {
        bail!("{goal} judges no log lines: it is no quoted goal over a LINES fact");
    }
    let line = line.trim();
    if line.is_empty() {
        bail!("the line is empty");
    }
    let template = memory::template(line);
    if class.is_none() {
        let st = journal::read(service);
        let known = st.memory.goals.get(goal).is_some_and(|k| k.contains_key(&template));
        if !known {
            bail!(
                "{service} knows nothing of lines like `{template}` for {goal}; a KNOWN line in \
                 the weave changes only with the weave"
            );
        }
    }
    service::put(&Command::Known {
        service: service.to_string(),
        goal: goal.to_string(),
        line: line.to_string(),
        class,
        why,
        by,
    })?;
    Ok(template)
}
