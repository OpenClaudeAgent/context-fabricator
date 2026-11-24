use uuid::Uuid;

pub const USAGE: &str = concat!(
    "Usage: ctxfab [--trace] <command>\n",
    "Commands: status | settings | diagnostics | retrieval | ",
    "search QUERY [--mode bm25|semantic|hybrid] [--repository-id ID --branch NAME] | ",
    "ask QUERY [--request-id UUID] [--repository-id ID --branch NAME ",
    "[--history-commit SHA | --actions-run-id ID --actions-run-attempt N]] | ",
    "cancel UUID | models | use-model PROFILE | model-downloads | install-model PROFILE | ",
    "pause-model PROFILE | resume-model PROFILE | discard-model-download PROFILE | ",
    "model-removal-plan PROFILE | delete-model PROFILE IDENTITY | ",
    "embedding-index | enable-embedding-index | disable-embedding-index | delete-embedding-index | ",
    "runtime-status | storage-plan | download-runtime | install-runtime ZIP | ",
    "setup-plan hosted|localArchive answers|hybrid [PROFILE] | ",
    "setup-start PLAN_JSON OPERATION_UUID [ZIP] | setup-status | ",
    "setup-archive OPERATION_UUID ZIP | setup-cancel-archive OPERATION_UUID | ",
    "github-actions REPOSITORY_ID | github-graph ID --branch NAME [--root PATH --symbol NAME]",
);

pub enum Command<'a> {
    Read(&'static str),
    Search(&'a str, QueryOptions<'a>),
    Ask(&'a str, QueryOptions<'a>),
    Cancel(Uuid),
    Select(&'a str),
    Install(&'a str),
    Runtime(RuntimeCommand<'a>),
    Setup(SetupCommand<'a>),
    Maintenance(Maintenance<'a>),
    ControlDownload(&'a str, &'static str),
    DiscardDownload(&'a str),
    GitHubActions(i64),
    GitHubGraph {
        id: i64,
        branch: &'a str,
        lookup: Option<(&'a str, &'a str)>,
    },
    Help,
}

pub enum RuntimeCommand<'a> {
    Download,
    Install(&'a str),
}

pub enum SetupCommand<'a> {
    Plan {
        source: &'a str,
        embedding: bool,
        profile: Option<&'a str>,
    },
    Start {
        plan: &'a str,
        id: Uuid,
        archive: Option<&'a str>,
    },
    Archive {
        id: Uuid,
        path: &'a str,
    },
    CancelArchive(Uuid),
    Status,
}

pub enum Maintenance<'a> {
    SetIndex(bool),
    DeleteIndex,
    ModelPlan(&'a str),
    DeleteModel(&'a str, &'a str),
}

#[derive(Default)]
pub struct QueryOptions<'a> {
    pub mode: Option<&'a str>,
    pub repository_id: Option<i64>,
    pub branch: Option<&'a str>,
    pub request_id: Option<Uuid>,
    pub history_commit: Option<&'a str>,
    pub actions_run_id: Option<i64>,
    pub actions_run_attempt: Option<u32>,
}

pub enum ParseError {
    Usage,
    RequestId,
}

impl<'a> Command<'a> {
    pub fn parse(args: &[&'a str]) -> Result<Self, ParseError> {
        Ok(match args {
            [] | ["--help" | "-h"] => Self::Help,
            ["status"] => Self::Read("/v1/status"),
            ["settings"] => Self::Read("/v1/settings"),
            ["diagnostics"] => Self::Read("/v1/diagnostics"),
            ["retrieval"] => Self::Read("/v1/retrieval"),
            ["models"] => Self::Read("/v1/generation/models"),
            ["model-downloads"] => Self::Read("/v1/model-downloads"),
            ["runtime-status"] => Self::Read("/v1/runtime"),
            ["storage-plan"] => Self::Read("/v1/inference/storage"),
            [
                verb @ ("setup-status"
                | "setup-plan"
                | "setup-start"
                | "setup-archive"
                | "setup-cancel-archive"),
                args @ ..,
            ] => Self::Setup(SetupCommand::parse(verb, args)?),
            ["embedding-index"] => Self::Read("/v1/retrieval/index"),
            ["enable-embedding-index"] => Self::Maintenance(Maintenance::SetIndex(true)),
            ["disable-embedding-index"] => Self::Maintenance(Maintenance::SetIndex(false)),
            ["delete-embedding-index"] => Self::Maintenance(Maintenance::DeleteIndex),
            ["model-removal-plan", profile] if valid_profile(profile) => {
                Self::Maintenance(Maintenance::ModelPlan(profile))
            }
            ["delete-model", profile, identity]
                if valid_profile(profile) && !identity.is_empty() =>
            {
                Self::Maintenance(Maintenance::DeleteModel(profile, identity))
            }
            ["use-model", profile] => Self::Select(profile),
            ["install-model", profile] => Self::Install(profile),
            ["install-runtime", path] if !path.is_empty() => {
                Self::Runtime(RuntimeCommand::Install(path))
            }
            ["download-runtime"] => Self::Runtime(RuntimeCommand::Download),
            ["discard-model-download", profile] if valid_profile(profile) => {
                Self::DiscardDownload(profile)
            }
            [action @ ("pause-model" | "resume-model"), profile] if valid_profile(profile) => {
                Self::ControlDownload(
                    profile,
                    if *action == "pause-model" {
                        "pause"
                    } else {
                        "resume"
                    },
                )
            }
            ["github-graph", id, "--branch", branch, rest @ ..] if !branch.is_empty() => {
                let id = id
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or(ParseError::Usage)?;
                let lookup = match rest {
                    [] => None,
                    ["--root", root, "--symbol", symbol] | ["--symbol", symbol, "--root", root]
                        if !root.is_empty() && !symbol.is_empty() =>
                    {
                        Some((*root, *symbol))
                    }
                    _ => return Err(ParseError::Usage),
                };
                Self::GitHubGraph { id, branch, lookup }
            }
            ["github-actions", id] => Self::GitHubActions(
                id.parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or(ParseError::Usage)?,
            ),
            ["cancel", id] => Self::Cancel(Uuid::parse_str(id).map_err(|_| ParseError::RequestId)?),
            [verb @ ("search" | "ask"), query, args @ ..] => {
                let search = *verb == "search";
                let options = QueryOptions::parse(args, search)?;
                if search {
                    Self::Search(query, options)
                } else {
                    Self::Ask(query, options)
                }
            }
            _ => return Err(ParseError::Usage),
        })
    }
}

impl<'a> QueryOptions<'a> {
    fn parse(args: &[&'a str], search: bool) -> Result<Self, ParseError> {
        let (pairs, remainder) = args.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(ParseError::Usage);
        }
        let mut options = Self::default();
        for &[flag, value] in pairs {
            match flag {
                "--mode"
                    if search
                        && options.mode.is_none()
                        && matches!(value, "bm25" | "semantic" | "hybrid") =>
                {
                    options.mode = Some(value);
                }
                "--request-id" if !search && options.request_id.is_none() => {
                    options.request_id =
                        Some(Uuid::parse_str(value).map_err(|_| ParseError::RequestId)?);
                }
                "--repository-id" if options.repository_id.is_none() => {
                    options.repository_id = Some(
                        value
                            .parse::<i64>()
                            .ok()
                            .filter(|id| *id > 0)
                            .ok_or(ParseError::Usage)?,
                    );
                }
                "--branch" if options.branch.is_none() && !value.is_empty() => {
                    options.branch = Some(value);
                }
                "--history-commit"
                    if !search && options.history_commit.is_none() && valid_commit(value) =>
                {
                    options.history_commit = Some(value);
                }
                "--actions-run-id" if !search && options.actions_run_id.is_none() => {
                    options.actions_run_id = Some(
                        value
                            .parse::<i64>()
                            .ok()
                            .filter(|id| *id > 0)
                            .ok_or(ParseError::Usage)?,
                    );
                }
                "--actions-run-attempt" if !search && options.actions_run_attempt.is_none() => {
                    options.actions_run_attempt = Some(
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|attempt| *attempt > 0)
                            .ok_or(ParseError::Usage)?,
                    );
                }
                _ => return Err(ParseError::Usage),
            }
        }
        if options.repository_id.is_some() != options.branch.is_some()
            || (options.history_commit.is_some() && options.repository_id.is_none())
            || options.actions_run_id.is_some() != options.actions_run_attempt.is_some()
            || (options.actions_run_id.is_some()
                && (options.repository_id.is_none() || options.history_commit.is_some()))
        {
            return Err(ParseError::Usage);
        }
        Ok(options)
    }
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_profile(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl<'a> SetupCommand<'a> {
    fn parse(verb: &str, args: &[&'a str]) -> Result<Self, ParseError> {
        let id = |value: &str| Uuid::parse_str(value).map_err(|_| ParseError::Usage);
        match (verb, args) {
            ("setup-status", []) => Ok(Self::Status),
            (
                "setup-plan",
                [
                    source @ ("hosted" | "localArchive"),
                    mode @ ("answers" | "hybrid"),
                    args @ ..,
                ],
            ) => Ok(Self::Plan {
                source,
                embedding: *mode == "hybrid",
                profile: optional_argument(args)?,
            }),
            ("setup-start", [plan, argument, args @ ..]) if !plan.is_empty() => Ok(Self::Start {
                plan,
                id: id(argument)?,
                archive: optional_argument(args)?,
            }),
            ("setup-archive", [argument, path]) if !path.is_empty() => Ok(Self::Archive {
                id: id(argument)?,
                path,
            }),
            ("setup-cancel-archive", [argument]) => Ok(Self::CancelArchive(id(argument)?)),
            _ => Err(ParseError::Usage),
        }
    }
}

fn optional_argument<'a>(args: &[&'a str]) -> Result<Option<&'a str>, ParseError> {
    match args {
        [] => Ok(None),
        [value] if !value.is_empty() => Ok(Some(value)),
        _ => Err(ParseError::Usage),
    }
}
