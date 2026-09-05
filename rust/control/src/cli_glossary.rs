use super::*;

#[derive(Debug, Args)]
pub(super) struct GlossaryScopeArgs {
    #[arg(long, conflicts_with = "repository_id")]
    path: Option<PathBuf>,
    #[arg(long)]
    repository_id: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct GlossaryListArgs {
    #[command(flatten)]
    scope: GlossaryScopeArgs,
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    language: Option<String>,
    #[arg(long, value_parser = ["draft", "approved", "deprecated"])]
    status: Option<String>,
    #[arg(long, value_parser = ["shared", "local", "inherited", "specialized"])]
    origin: Option<String>,
    #[arg(long)]
    limit: Option<u32>,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    expected_revision: Option<u32>,
    #[arg(long)]
    revision: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub(super) enum GlossaryCommand {
    List(GlossaryListArgs),
    Resolve(GlossaryListArgs),
    Get {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        concept_id: String,
        #[arg(long)]
        revision: Option<u32>,
    },
    Save {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        #[arg(long)]
        expected_revision: u32,
        #[arg(long)]
        concept_id: Option<String>,
        #[arg(long)]
        file: PathBuf,
    },
    Configure {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        #[arg(long)]
        expected_revision: u32,
        #[arg(long)]
        file: PathBuf,
    },
    Inherit {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        concept_id: String,
        #[arg(long)]
        expected_revision: u32,
    },
    History {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        #[arg(long)]
        concept_id: Option<String>,
        #[arg(long)]
        before_revision: Option<u32>,
        #[arg(long)]
        limit: Option<u32>,
    },
    Check {
        #[command(flatten)]
        scope: GlossaryScopeArgs,
        #[arg(long)]
        expected_revision: Option<u32>,
        #[arg(long)]
        file: PathBuf,
    },
    Impact {
        #[arg(long)]
        offset: Option<u32>,
        #[arg(long)]
        limit: Option<u32>,
    },
}

impl GlossaryScopeArgs {
    fn params(self) -> Result<Map<String, Value>, CliValidationError> {
        let mut params = Map::new();
        if let Some(path) = self.path {
            params.insert("path".to_owned(), json!(absolute_path(path)?));
        }
        insert_option(&mut params, "repository_id", self.repository_id);
        Ok(params)
    }
}

fn glossary_file(path: &PathBuf) -> Result<Value, CliValidationError> {
    let bytes = std::fs::read(path)
        .map_err(|error| invalid(format!("Cannot read glossary input: {error}")))?;
    if bytes.len() > 65536 {
        return Err(invalid("Glossary input must not exceed 64 KiB"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("Invalid glossary JSON: {error}")))
}

impl GlossaryCommand {
    pub(super) fn into_invocation(self) -> Result<Invocation, CliValidationError> {
        let operation = match &self {
            Self::List(_) => "glossary.list",
            Self::Resolve(_) => "glossary.resolve",
            Self::Get { .. } => "glossary.get",
            Self::Save { .. } => "glossary.save",
            Self::Configure { .. } => "glossary.configure",
            Self::Inherit { .. } => "glossary.inherit",
            Self::History { .. } => "glossary.history",
            Self::Check { .. } => "glossary.check",
            Self::Impact { .. } => "glossary.impact",
        };
        let params = match self {
            Self::List(args) | Self::Resolve(args) => {
                let mut params = args.scope.params()?;
                insert_option(&mut params, "query", args.query);
                insert_option(&mut params, "language", args.language);
                insert_option(&mut params, "status", args.status);
                insert_option(&mut params, "origin", args.origin);
                insert_number_option(&mut params, "limit", args.limit);
                insert_number_option(&mut params, "offset", args.offset);
                insert_number_option(&mut params, "expected_revision", args.expected_revision);
                insert_number_option(&mut params, "revision", args.revision);
                params
            }
            Self::Get {
                scope,
                concept_id,
                revision,
            } => {
                let mut params = scope.params()?;
                params.insert("concept_id".to_owned(), json!(concept_id));
                insert_number_option(&mut params, "revision", revision);
                params
            }
            Self::Save {
                scope,
                expected_revision,
                concept_id,
                file,
            } => {
                let mut params = scope.params()?;
                params.insert("concept".to_owned(), glossary_file(&file)?);
                params.insert("expected_revision".to_owned(), json!(expected_revision));
                insert_option(&mut params, "concept_id", concept_id);
                params
            }
            Self::Configure {
                scope,
                expected_revision,
                file,
            } => {
                let input = glossary_file(&file)?;
                let input = input
                    .as_object()
                    .ok_or_else(|| invalid("Glossary settings must be a JSON object"))?;
                if input.keys().any(|key| {
                    !["baseline_revision", "languages", "guidelines"].contains(&key.as_str())
                }) {
                    return Err(invalid("Unknown glossary settings field"));
                }
                let mut params = scope.params()?;
                params.extend(input.clone());
                params.insert("expected_revision".to_owned(), json!(expected_revision));
                params
            }
            Self::Inherit {
                scope,
                concept_id,
                expected_revision,
            } => {
                let mut params = scope.params()?;
                params.insert("concept_id".to_owned(), json!(concept_id));
                params.insert("expected_revision".to_owned(), json!(expected_revision));
                params
            }
            Self::History {
                scope,
                concept_id,
                before_revision,
                limit,
            } => {
                let mut params = scope.params()?;
                insert_option(&mut params, "concept_id", concept_id);
                insert_number_option(&mut params, "before_revision", before_revision);
                insert_number_option(&mut params, "limit", limit);
                params
            }
            Self::Check {
                scope,
                expected_revision,
                file,
            } => {
                let mut params = scope.params()?;
                params.insert("usages".to_owned(), glossary_file(&file)?);
                insert_number_option(&mut params, "expected_revision", expected_revision);
                params
            }
            Self::Impact { offset, limit } => {
                let mut params = Map::new();
                insert_number_option(&mut params, "offset", offset);
                insert_number_option(&mut params, "limit", limit);
                params
            }
        };
        remote(operation, Value::Object(params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glossary_cli_routes_preserve_typed_scope_and_revision() {
        for command in ["list", "resolve"] {
            let cli = Cli::try_parse_from([
                "devcoordinator2",
                "glossary",
                command,
                "--repository-id",
                "r1111111111111111",
                "--language",
                "ru",
                "--expected-revision",
                "3",
            ])
            .unwrap();
            let Invocation::Remote { operation, params } = cli.into_invocation().unwrap() else {
                panic!("not a remote call")
            };
            assert_eq!(operation, format!("glossary.{command}"));
            assert_eq!(params["repository_id"], "r1111111111111111");
            assert_eq!(params["expected_revision"], 3);
        }
        assert!(
            Cli::try_parse_from([
                "devcoordinator2",
                "glossary",
                "list",
                "--path",
                "/tmp",
                "--repository-id",
                "r1111111111111111"
            ])
            .is_err()
        );
    }

    #[test]
    fn glossary_cli_does_not_let_a_settings_file_redirect_its_scope() {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join("settings.json");
        std::fs::write(
            &file,
            r#"{"repository_id":"another-project","languages":[],"guidelines":[]}"#,
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "devcoordinator2",
            "glossary",
            "configure",
            "--expected-revision",
            "0",
            "--file",
            file.to_str().unwrap(),
        ])
        .unwrap();
        assert!(cli.into_invocation().is_err());
    }
}
