use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::{Result, bail};
use jiff::Timestamp;

use super::flow;
use crate::model::{ModelRef, decider::Decider};
use crate::plan::{self, Plan};
use crate::service::{self, Command};
use crate::weave::Weave;
use crate::{paths, remote};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// One sentence saying what to change, like "keep 30 backups"
    pub request: Option<String>,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
    /// A new decider model, <id>[@<provider>]
    #[arg(long, value_name = "MODEL")]
    pub decide: Option<String>,
    /// New coder models, <id>[@<provider>]; repeat for the THEN order
    #[arg(long, value_name = "MODEL")]
    pub code: Vec<String>,
    /// Go on past warnings without asking
    #[arg(long)]
    pub yes: bool,
    /// Hand the change over and return, without following it
    #[arg(short, long)]
    pub detach: bool,
}

/// The request check of `tell`, for the CLI and the dashboard alike.
pub(crate) struct Check {
    pub weave: Weave,
    /// The ARGs now.
    pub before: BTreeMap<String, String>,
    pub plan: Plan,
    /// SECRETs the new ARGs turn on that the daemon does not hold yet.
    pub missing: Vec<String>,
}

/// Checks a request against a running service's weave and ARGs, with `decide` or the
/// service's own decider.
pub(crate) async fn check(service: &str, request: &str, decide: Option<&ModelRef>) -> Result<Check> {
    let weave = super::service_weave(service)?;
    let spec = service::spec(service)?;
    let model = match decide {
        Some(m) => m.clone(),
        None => ModelRef::parse(&spec.decide)?,
    };
    let plan = plan::plan(&weave, request, &Decider::new(model)?, Some(&spec.args)).await?;
    let have = stored_secrets(service);
    let missing = plan
        .secrets
        .iter()
        .filter(|s| !have.contains_key(*s))
        .cloned()
        .collect();
    Ok(Check {
        weave,
        before: spec.args,
        plan,
        missing,
    })
}

pub async fn run(args: Args) -> Result<ExitCode> {
    if args.request.is_none() && args.decide.is_none() && args.code.is_empty() {
        bail!("say what to change: a request, --decide or --code");
    }
    super::known(&args.service)?;
    let spec = service::spec(&args.service)?;
    let decide = args.decide.as_deref().map(ModelRef::parse).transpose()?;
    let code = args
        .code
        .iter()
        .map(|c| ModelRef::parse(c))
        .collect::<Result<Vec<_>>>()?;
    let (request, values, secrets) = match &args.request {
        Some(request) => {
            let c = check(&args.service, request, decide.as_ref()).await?;
            if !flow::settle(&c.plan, args.yes)? {
                return Ok(ExitCode::FAILURE);
            }
            let secrets = flow::secrets(&c.weave, &c.missing, &c.plan.values)?;
            (request.clone(), c.plan.values, secrets)
        }
        None => (spec.request.clone(), spec.args.clone(), BTreeMap::new()),
    };
    let journal = flow::journal_end(&args.service);
    let since = Timestamp::now();
    service::put(&Command::Tell {
        service: args.service.clone(),
        request,
        args: values,
        secrets,
        decide: decide.map(|m| m.to_string()),
        code: (!code.is_empty()).then(|| code.iter().map(ModelRef::to_string).collect()),
        by: super::by(),
    })?;
    println!("handed the change to {} to the daemon", args.service);
    if args.detach {
        println!("follow it: subweave log -f {}", args.service);
        return Ok(ExitCode::SUCCESS);
    }
    flow::follow(&args.service, since, journal).await?;
    Ok(ExitCode::SUCCESS)
}

/// `tell --on`: the keys of the models it names go to the box first.
pub fn send_keys(args: &Args, host: &str) -> Result<()> {
    let mut models = Vec::new();
    if let Some(d) = &args.decide {
        models.push(ModelRef::parse(d)?);
    }
    for c in &args.code {
        models.push(ModelRef::parse(c)?);
    }
    remote::send_keys(host, &models)
}

/// The SECRETs the daemon holds; none when this user cannot read them.
pub(crate) fn stored_secrets(service: &str) -> BTreeMap<String, serde_json::Value> {
    std::fs::read(paths::secrets(service))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
