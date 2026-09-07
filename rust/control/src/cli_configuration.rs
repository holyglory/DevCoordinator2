use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{CliValidationError, DeploymentSelector, Invocation, remote};

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    Show,
    Authorize(AuthorizationArgs),
    Revoke(AuthorizationArgs),
    Reload {
        #[arg(long)]
        expected_revision: String,
    },
}

#[derive(Debug, Args)]
pub(super) struct AuthorizationArgs {
    #[command(flatten)]
    selector: DeploymentSelector,
    #[arg(long)]
    file: String,
    #[arg(long)]
    expected_revision: String,
}

impl ConfigCommand {
    pub(super) fn into_invocation(self) -> Result<Invocation, CliValidationError> {
        match self {
            Self::Show => remote("config.get", json!({})),
            Self::Reload { expected_revision } => remote(
                "config.reload",
                json!({"expected_revision":expected_revision}),
            ),
            Self::Authorize(args) => args.invocation(true),
            Self::Revoke(args) => args.invocation(false),
        }
    }
}

impl AuthorizationArgs {
    fn invocation(self, authorized: bool) -> Result<Invocation, CliValidationError> {
        let mut params = self.selector.params()?;
        params.insert("file".to_owned(), self.file.into());
        params.insert(
            "expected_revision".to_owned(),
            self.expected_revision.into(),
        );
        params.insert("authorized".to_owned(), authorized.into());
        remote("config.env.set", Value::Object(params))
    }
}
