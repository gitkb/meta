use anyhow::Result;
use meta_plugin::{Plan, Request, Response, Scope, Step};
use std::path::Path;

use crate::worktree::helpers::*;

use super::cli_types::ExecArgs;
use crate::git_env;

/// `exec NAME -- CMD`: a plan of `CMD` in each checkout of the set meta's
/// `--include`/`--exclude`/`--tag` select (the host resolved them), named by
/// its path in the set.
pub(crate) fn handle_exec(args: ExecArgs, request: &Request) -> Result<Response> {
    let (_, repos) = discover_and_validate_worktree(&args.name, request)?;
    let git_env: std::collections::BTreeMap<String, String> =
        git_env::git_env().into_iter().collect();
    let step = |project: &str, dir: &Path| {
        let mut step = Step::new(Some(project), dir, args.command.clone());
        step.env = git_env.clone();
        step
    };
    // The checkouts the host selected in the set it named; a set it didn't
    // name (outside a workspace, found from META_WORKTREES or .worktrees)
    // takes no selection, so every checkout
    let named = request.scope == Scope::NamedSet
        && request
            .set
            .as_ref()
            .is_some_and(|set| set.name == args.name);
    let steps = match named {
        true => request
            .projects
            .iter()
            .map(|p| step(&p.path, &p.abs))
            .collect(),
        false => repos.iter().map(|r| step(&r.alias, &r.path)).collect(),
    };
    // one checkout after another, at once only with meta's --parallel
    // typed (a config's default is for `meta exec`)
    Ok(Response::plan(Plan {
        steps,
        parallel: Some(request.meta_options.typed_has("--parallel")),
        ..Plan::default()
    }))
}
