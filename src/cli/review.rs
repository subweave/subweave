use std::process::ExitCode;

use anyhow::Result;

use crate::model::{self, decider::Decider};
use crate::review;

#[derive(clap::Args)]
pub struct Args {
    /// A weave directory, or a name like postgres or postgres:0.5
    weave: String,
    /// Print JSON: the score, the areas and every check
    #[arg(long)]
    json: bool,
    /// The decider model, <id>[@<provider>]
    #[arg(long, value_name = "MODEL")]
    decide: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let Some(w) = super::load(&args.weave).await? else {
        return Ok(ExitCode::FAILURE);
    };
    let decider = Decider::new(model::decide_model(args.decide.as_deref(), &w.model)?)?;
    let report = review::review(&w, decider).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", review::render(&report));
    }
    if let Some(e) = &report.error {
        eprintln!(
            "{} of {} decider requests failed; their checks count as failing: {e}",
            report.failed_requests, report.requests
        );
    }
    Ok(ExitCode::SUCCESS)
}
