use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;

use super::flow;
use crate::model::{self, ModelRef, decider::Decider};
use crate::service::{self, Bundle, Command, Spec};
use crate::{plan, remote};

#[derive(clap::Args)]
pub struct Args {
    /// A weave directory, or a name like postgres or postgres:0.5
    pub weave: String,
    /// One sentence saying what you want, like "postgres 18, private network"
    pub request: Option<String>,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
    /// The service's name; the weave's name by default
    #[arg(long)]
    pub name: Option<String>,
    /// The decider model, <id>[@<provider>]
    #[arg(long, value_name = "MODEL")]
    pub decide: Option<String>,
    /// A coder model, <id>[@<provider>]; repeat for the THEN order
    #[arg(long, value_name = "MODEL")]
    pub code: Vec<String>,
    /// Go on past warnings without asking
    #[arg(long)]
    pub yes: bool,
    /// Hand the service over and return, without following it
    #[arg(short, long)]
    pub detach: bool,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let Some((w, text)) = super::load_text(&args.weave).await? else {
        return Ok(ExitCode::FAILURE);
    };
    let name = args.name.clone().unwrap_or_else(|| w.name.clone());
    check_name(&name)?;
    let decide = model::decide_model(args.decide.as_deref(), &w.model)?;
    let code = model::code_models(&args.code, &w.model)?;
    let decider = Decider::new(decide.clone())?;
    // The service already here: a new weave version, and the sentence, if any, is a change
    // to its ARGs, as with `tell`. Every ARG it does not say keeps its value.
    let before = service::names()?
        .contains(&name)
        .then(|| service::spec(&name))
        .transpose()?;
    let request = match (args.request, &before) {
        (Some(r), _) => r,
        (None, Some(s)) => s.request.clone(),
        (None, None) => String::new(),
    };
    let said = if before.is_some() && request == before.as_ref().map_or("", |s| s.request.as_str())
    {
        ""
    } else {
        request.as_str()
    };
    let plan = plan::plan(&w, said, &decider, before.as_ref().map(|s| &s.args)).await?;
    if !flow::settle(&plan, args.yes)? {
        return Ok(ExitCode::FAILURE);
    }
    let have = super::tell::stored_secrets(&name);
    let missing: Vec<String> = plan
        .secrets
        .iter()
        .filter(|s| !have.contains_key(*s))
        .cloned()
        .collect();
    let secrets = flow::secrets(&w, &missing, &plan.values)?;
    let bundle = Bundle {
        spec: Spec {
            name: name.clone(),
            weave: args.weave.clone(),
            request,
            args: plan.values,
            decide: decide.to_string(),
            code: code.iter().map(ModelRef::to_string).collect(),
            created: before.map_or_else(now, |s| s.created),
            stopped: false,
        },
        weavefile: text,
        readme: readme(&args.weave)?,
        secrets,
    };
    let journal = flow::journal_end(&name);
    let since = Timestamp::now();
    service::put(&Command::Run {
        bundle,
        by: super::by(),
    })?;
    println!("handed {name} to the daemon");
    flow::ensure_daemon()?;
    if args.detach {
        println!("follow it: subweave log -f {name}");
        return Ok(ExitCode::SUCCESS);
    }
    flow::follow(&name, since, journal).await?;
    Ok(ExitCode::SUCCESS)
}

/// `run --on`: the weave directory, the model keys and each SECRET set in this shell go
/// to the box, then the same command runs there; the box asks for the other SECRETs.
/// The box is remembered for the service.
pub async fn forward(args: Args, host: &str, mut argv: Vec<String>) -> Result<i32> {
    let Some(w) = super::load(&args.weave).await? else {
        return Ok(1);
    };
    let name = args.name.clone().unwrap_or_else(|| w.name.clone());
    check_name(&name)?;
    let mut models = vec![model::decide_model(args.decide.as_deref(), &w.model)?];
    models.extend(model::code_models(&args.code, &w.model)?);
    remote::send_keys(host, &models)?;
    let mut cleanup = None;
    let path = Path::new(&args.weave);
    if path.exists() {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().context("a Weavefile needs a directory")?
        };
        let (remote_dir, tmp) = remote::upload_dir(host, dir)?;
        if let Some(a) = argv.iter_mut().skip(1).find(|a| **a == args.weave) {
            *a = remote_dir;
        }
        cleanup = Some(tmp);
    }
    let mut line = remote::command_line(&argv);
    let secrets: Vec<(String, String)> = w
        .secrets
        .iter()
        .filter_map(|s| {
            let v = std::env::var(&s.name).ok().filter(|v| !v.is_empty())?;
            Some((s.name.clone(), v))
        })
        .collect();
    if !secrets.is_empty() {
        let env = remote::quote(&remote::send_env(host, &secrets)?);
        line = format!(". {env} && rm -f {env} && {line}");
    }
    if let Some(tmp) = cleanup {
        line = format!("{line}; s=$?; rm -rf {}; exit $s", remote::quote(&tmp));
    }
    let code = remote::run_line(host, &line)?;
    if code == 0 {
        remote::remember(&name, host)?;
    }
    Ok(code)
}

fn check_name(name: &str) -> Result<()> {
    let ok = name
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        bail!("a service name is lowercase letters, digits and -, not {name:?}");
    }
    Ok(())
}

/// The README beside a local Weavefile.
fn readme(spec: &str) -> Result<Option<String>> {
    let path = Path::new(spec);
    let dir = if path.is_dir() {
        path
    } else if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        return Ok(None);
    };
    let file = dir.join("README.md");
    match std::fs::read_to_string(&file) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", file.display())),
    }
}

/// RFC 3339, UTC, whole seconds.
pub(crate) fn now() -> String {
    Timestamp::from_second(Timestamp::now().as_second())
        .unwrap_or_else(|_| Timestamp::now())
        .to_string()
}
