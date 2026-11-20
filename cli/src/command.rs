use uuid::Uuid;

pub const USAGE: &str = "Usage: ctxfab status | settings | diagnostics | retrieval | search QUERY [--mode bm25|semantic|hybrid] [--repository-id ID --branch NAME] | ask QUERY [--request-id UUID] [--repository-id ID --branch NAME] | cancel UUID | models | use-model PROFILE | model-downloads | install-model PROFILE";

pub enum Command<'a> {
    Read(&'static str),
    Search(&'a str, QueryOptions<'a>),
    Ask(&'a str, QueryOptions<'a>),
    Cancel(Uuid),
    Select(&'a str),
    Install(&'a str),
    Help,
}

#[derive(Default)]
pub struct QueryOptions<'a> {
    pub mode: Option<&'a str>,
    pub repository_id: Option<i64>,
    pub branch: Option<&'a str>,
    pub request_id: Option<Uuid>,
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
            ["use-model", profile] => Self::Select(profile),
            ["install-model", profile] => Self::Install(profile),
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
                _ => return Err(ParseError::Usage),
            }
        }
        if options.repository_id.is_some() != options.branch.is_some() {
            return Err(ParseError::Usage);
        }
        Ok(options)
    }
}
