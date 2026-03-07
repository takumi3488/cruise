use std::cell::RefCell;
use std::path::Path;

use console::style;
use inquire::InquireError;

use crate::cli::RunArgs;
use crate::config::{WorkflowConfig, validate_groups};
use crate::engine::{execute_steps, print_dry_run};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::session::{SessionManager, SessionPhase, current_iso8601, get_cruise_home};
use crate::variable::VariableStore;
use crate::worktree;

/// Variable name that maps to the plan file.
const PLAN_VAR: &str = "plan";
const PR_NUMBER_VAR: &str = "pr.number";
const PR_URL_VAR: &str = "pr.url";

pub async fn run(args: RunArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);

    // Determine which session to run.
    let session_id = match args.session {
        Some(id) => id,
        None => select_pending_session(&manager)?,
    };

    let mut session = manager.load(&session_id)?;

    // Load config from session dir.
    let config_path = manager.sessions_dir().join(&session_id).join("config.yaml");
    let yaml = std::fs::read_to_string(&config_path).map_err(|e| {
        CruiseError::Other(format!(
            "failed to read session config {}: {}",
            config_path.display(),
            e
        ))
    })?;
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    validate_groups(&config)?;

    if args.dry_run {
        eprintln!("{}", style(format!("Session: {}", session_id)).dim());
        return print_dry_run(&config, session.current_step.as_deref());
    }

    ensure_gh_available()?;

    // Determine start step.
    let start_step = session.current_step.clone().map(Ok).unwrap_or_else(|| {
        config
            .steps
            .keys()
            .next()
            .ok_or_else(|| CruiseError::Other("config has no steps".to_string()))
            .cloned()
    })?;

    // Show resume message if restarting an interrupted session.
    if let Some(ref step) = session.current_step {
        match &session.phase {
            SessionPhase::Running => {
                eprintln!("{} Resuming from step: {}", style("↺").cyan(), step);
            }
            SessionPhase::Failed(_) => {
                eprintln!(
                    "{} Retrying from failed step: {}",
                    style("↺").yellow(),
                    step
                );
            }
            _ => {}
        }
    }

    // chdir to base_dir.
    let base_dir = session.base_dir.clone();
    std::env::set_current_dir(&base_dir)?;

    // Create or reuse worktree at ~/.cruise/worktrees/{session_id}/.
    let worktrees_dir = manager.worktrees_dir();
    let (ctx, reused) = worktree::setup_session_worktree(
        &base_dir,
        &session_id,
        &session.input,
        &worktrees_dir,
        session.worktree_branch.as_deref(),
    )?;
    let suffix = if reused { " (reused)" } else { "" };
    eprintln!(
        "{} worktree: {}{}",
        style("→").cyan(),
        ctx.path.display(),
        suffix
    );

    session.worktree_path = Some(ctx.path.clone());
    session.worktree_branch = Some(ctx.branch.clone());
    session.phase = SessionPhase::Running;
    manager.save(&session)?;

    // chdir to worktree.
    std::env::set_current_dir(&ctx.path)?;

    // Set up variables.
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path);

    let mut tracker = FileTracker::with_root(ctx.path.clone());

    // Use RefCell for interior mutability in the callback.
    let session_cell = RefCell::new(&mut session);

    let exec_result = execute_steps(
        &config,
        &mut vars,
        &mut tracker,
        &start_step,
        args.max_retries,
        args.rate_limit_retries,
        &|step| {
            let mut s = session_cell.borrow_mut();
            s.current_step = Some(step.to_string());
            manager.save(&s)
        },
    )
    .await;

    let session = session_cell.into_inner();

    let overall_result = match exec_result {
        Ok(_) => match attempt_pr_creation(&ctx, &session.input) {
            Ok(pr_attempt) => {
                pr_attempt.report();
                match pr_attempt {
                    PrAttemptOutcome::Created { url, .. } => {
                        eprintln!("{} PR created: {}", style("✓").green().bold(), url);
                        if let Some(number) = extract_last_path_segment(&url) {
                            vars.set_named_value(PR_NUMBER_VAR, number);
                        }
                        vars.set_named_value(PR_URL_VAR, url.clone());
                        session.pr_url = Some(url);

                        // Run after_pr steps if any.
                        if let Some(first_step) = config.after_pr.keys().next() {
                            let mut after_config = config.clone();
                            after_config.steps = std::mem::take(&mut after_config.after_pr);
                            if let Err(e) = execute_steps(
                                &after_config,
                                &mut vars,
                                &mut tracker,
                                first_step,
                                args.max_retries,
                                args.rate_limit_retries,
                                &|_| Ok(()),
                            )
                            .await
                            {
                                eprintln!("warning: after-pr steps failed: {}", e);
                            }
                        }
                        Ok(())
                    }
                    PrAttemptOutcome::SkippedNoCommits => Err(CruiseError::Other(format!(
                        "cannot create PR for {}: branch has no commits beyond its base; make changes and rerun `cruise run`",
                        ctx.branch
                    ))),
                    PrAttemptOutcome::CreateFailed { error, .. } => {
                        eprintln!("warning: PR creation failed: {}", error);
                        Ok(())
                    }
                }
            }
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };

    match &overall_result {
        Ok(()) => {
            session.phase = SessionPhase::Completed;
            session.completed_at = Some(current_iso8601());
        }
        Err(e) => {
            session.phase = SessionPhase::Failed(e.to_string());
            session.completed_at = Some(current_iso8601());
        }
    }
    manager.save(session)?;

    overall_result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitOutcome {
    Created,
    NoChanges,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PrAttemptOutcome {
    Created {
        url: String,
        commit_outcome: CommitOutcome,
    },
    SkippedNoCommits,
    CreateFailed {
        error: String,
        commit_outcome: CommitOutcome,
    },
}

impl PrAttemptOutcome {
    fn report(&self) {
        match self {
            Self::Created { commit_outcome, .. } | Self::CreateFailed { commit_outcome, .. } => {
                report_commit_outcome(*commit_outcome);
            }
            Self::SkippedNoCommits => {}
        }
    }
}

fn report_commit_outcome(commit_outcome: CommitOutcome) {
    match commit_outcome {
        CommitOutcome::Created => {
            eprintln!("{} Changes committed", style("✓").green().bold());
        }
        CommitOutcome::NoChanges => {
            eprintln!(
                "{} No new changes to commit; using existing branch commits",
                style("→").cyan()
            );
        }
    }
}

fn attempt_pr_creation(ctx: &worktree::WorktreeContext, message: &str) -> Result<PrAttemptOutcome> {
    let commit_outcome = commit_changes(&ctx.path, message)?;
    if branch_commit_count(ctx)? == 0 {
        return Ok(PrAttemptOutcome::SkippedNoCommits);
    }

    match create_pr(&ctx.path, &ctx.branch) {
        Ok(url) => Ok(PrAttemptOutcome::Created {
            url,
            commit_outcome,
        }),
        Err(e) => Ok(PrAttemptOutcome::CreateFailed {
            error: e.to_string(),
            commit_outcome,
        }),
    }
}

fn branch_commit_count(ctx: &worktree::WorktreeContext) -> Result<usize> {
    let base_head = git_stdout(
        &ctx.original_dir,
        &["rev-parse", "HEAD"],
        "git rev-parse HEAD failed",
    )?;
    let merge_base = git_stdout(
        &ctx.path,
        &["merge-base", "HEAD", &base_head],
        "git merge-base failed",
    )?;
    let count = git_stdout(
        &ctx.path,
        &["rev-list", "--count", &format!("{merge_base}..HEAD")],
        "git rev-list --count failed",
    )?;
    count.parse::<usize>().map_err(|e| {
        CruiseError::Other(format!(
            "failed to parse branch commit count from `{count}`: {e}"
        ))
    })
}

fn git_stdout(current_dir: &Path, args: &[&str], context: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(current_dir)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git {}: {}", args.join(" "), e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!("{context}: {}", stderr.trim())));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        Err(CruiseError::Other(format!(
            "{context}: command produced no stdout"
        )))
    } else {
        Ok(stdout)
    }
}

/// Stage all changes and commit them.
fn commit_changes(worktree_path: &Path, message: &str) -> Result<CommitOutcome> {
    // git add -A
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git add: {}", e)))?;
    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr);
        return Err(CruiseError::Other(format!(
            "git add -A failed: {}",
            stderr.trim()
        )));
    }

    // Check if there are staged changes
    let diff = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git diff: {}", e)))?;
    if diff.status.success() {
        // No changes to commit
        return Ok(CommitOutcome::NoChanges);
    }

    // git commit
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git commit: {}", e)))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        return Err(CruiseError::Other(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    Ok(CommitOutcome::Created)
}

/// Create a PR using `gh pr create --fill`. Falls back to `gh pr view` if a PR already exists.
fn create_pr(worktree_path: &Path, branch: &str) -> Result<String> {
    let output = std::process::Command::new("gh")
        .args(["pr", "create", "--fill", "--head", branch])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr create: {}", e)))?;

    if output.status.success()
        && let Some(url) = gh_output_line(&output.stdout)
    {
        return Ok(url);
    }

    // PR may already exist — try to fetch the URL.
    let fallback = std::process::Command::new("gh")
        .args(["pr", "view", branch, "--json", "url", "--jq", ".url"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr view: {}", e)))?;

    if fallback.status.success()
        && let Some(url) = gh_output_line(&fallback.stdout)
    {
        return Ok(url);
    }

    let create_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let view_stderr = String::from_utf8_lossy(&fallback.stderr).trim().to_string();
    Err(CruiseError::Other(format!(
        "gh pr create failed: {}; gh pr view also failed: {}",
        create_stderr, view_stderr
    )))
}

/// Trim and return a non-empty line from `gh` stdout bytes, or `None`.
fn gh_output_line(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Extracts the last path segment from a URL, stripping any query string or fragment.
/// Returns `None` if the URL has no non-empty trailing path segment.
fn extract_last_path_segment(url: &str) -> Option<String> {
    url.rsplit('/')
        .next()
        .map(|s| s.split_once(['?', '#']).map_or(s, |(prefix, _)| prefix))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Verify that `gh` CLI is available in PATH.
fn ensure_gh_available() -> Result<()> {
    let ok = std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok {
        Ok(())
    } else {
        Err(CruiseError::Other(
            "gh CLI is not installed. Install it from https://cli.github.com/".to_string(),
        ))
    }
}

/// Select a pending session interactively (or automatically if only one).
fn select_pending_session(manager: &SessionManager) -> Result<String> {
    let pending = manager.pending()?;

    if pending.is_empty() {
        return Err(CruiseError::Other(
            "No pending sessions. Run `cruise plan` first.".to_string(),
        ));
    }

    if pending.len() == 1 {
        let s = &pending[0];
        eprintln!(
            "{} Selected session: {} — {}",
            style("→").cyan(),
            s.id,
            crate::display::truncate(&s.input, 60)
        );
        return Ok(s.id.clone());
    }

    // Multiple pending sessions: let the user choose.
    let labels: Vec<String> = pending
        .iter()
        .map(|s| {
            format!(
                "{} | {} | {}",
                s.id,
                s.phase.label(),
                crate::display::truncate(&s.input, 60)
            )
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();

    let selected = match inquire::Select::new("Select a session to run:", label_refs).prompt() {
        Ok(s) => s,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Err(CruiseError::Other(
                "session selection cancelled".to_string(),
            ));
        }
        Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
    };

    let idx = labels.iter().position(|l| l.as_str() == selected).unwrap();
    Ok(pending[idx].id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    static GLOBAL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct PathEnvGuard {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl PathEnvGuard {
        fn prepend(dir: &Path) -> Self {
            let lock = GLOBAL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("PATH");
            let mut paths = vec![dir.to_path_buf()];
            if let Some(ref existing) = prev {
                paths.extend(std::env::split_paths(existing));
            }
            let joined = std::env::join_paths(paths).expect("failed to join PATH");
            // SAFETY: the test holds GLOBAL_ENV_LOCK, so no other test mutates PATH concurrently.
            unsafe { std::env::set_var("PATH", &joined) };
            Self { prev, _lock: lock }
        }
    }

    impl Drop for PathEnvGuard {
        fn drop(&mut self) {
            // SAFETY: the test holds GLOBAL_ENV_LOCK for the lifetime of the guard.
            unsafe {
                if let Some(ref prev) = self.prev {
                    std::env::set_var("PATH", prev);
                } else {
                    std::env::remove_var("PATH");
                }
            }
        }
    }

    fn run_git_ok(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command failed to start");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    fn git_stdout_ok(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command failed to start");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_git_repo(dir: &Path) {
        run_git_ok(dir, &["init"]);
        run_git_ok(dir, &["config", "user.email", "test@example.com"]);
        run_git_ok(dir, &["config", "user.name", "Test"]);
        fs::write(dir.join("README.md"), "init").unwrap();
        run_git_ok(dir, &["add", "."]);
        run_git_ok(dir, &["commit", "-m", "init"]);
        run_git_ok(dir, &["branch", "-M", "main"]);
    }

    fn create_worktree(tmp: &TempDir, session_id: &str) -> (PathBuf, worktree::WorktreeContext) {
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);

        let worktrees_dir = tmp.path().join("worktrees");
        let (ctx, reused) =
            worktree::setup_session_worktree(&repo, session_id, "test task", &worktrees_dir, None)
                .unwrap();
        assert!(!reused, "test worktree should be created fresh");
        (repo, ctx)
    }

    fn install_fake_gh(bin_dir: &Path, log_path: &Path, head_path: &Path, url: &str) {
        fs::create_dir_all(bin_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let script_path = bin_dir.join("gh");
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ngit rev-parse HEAD > \"{}\"\nprintf '%s\\n' \"{}\"\n",
                log_path.display(),
                head_path.display(),
                url
            );
            fs::write(&script_path, script).unwrap();
            let mut perms = fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap();
        }
        #[cfg(windows)]
        {
            let script_path = bin_dir.join("gh.cmd");
            let script = format!(
                "@echo off\r\necho %*>>\"{}\"\r\ngit rev-parse HEAD > \"{}\"\r\necho {}\r\n",
                log_path.display(),
                head_path.display(),
                url
            );
            fs::write(&script_path, script).unwrap();
        }
    }

    #[test]
    fn test_extract_last_path_segment_github_pr_url() {
        // Given: a standard GitHub PR URL
        let url = "https://github.com/owner/repo/pull/42";
        // When: extracting the last segment
        let result = extract_last_path_segment(url);
        // Then: last segment is returned
        assert_eq!(result, Some("42".to_string()));
    }

    #[test]
    fn test_attempt_pr_creation_skips_gh_when_branch_has_no_commits() {
        let tmp = TempDir::new().unwrap();
        let (_repo, ctx) = create_worktree(&tmp, "20260307225900");
        let bin_dir = tmp.path().join("bin");
        let log_path = tmp.path().join("gh.log");
        let head_path = tmp.path().join("gh-head.txt");
        install_fake_gh(
            &bin_dir,
            &log_path,
            &head_path,
            "https://github.com/owner/repo/pull/1",
        );
        let _path_guard = PathEnvGuard::prepend(&bin_dir);

        let result = attempt_pr_creation(&ctx, "test task").unwrap();

        assert_eq!(result, PrAttemptOutcome::SkippedNoCommits);
        assert!(
            !log_path.exists(),
            "gh should not be called when no commit exists"
        );
        assert!(
            !head_path.exists(),
            "gh should not observe HEAD when skipped"
        );
        worktree::cleanup_worktree(&ctx).unwrap();
    }

    #[test]
    fn test_attempt_pr_creation_commits_changes_before_calling_gh() {
        let tmp = TempDir::new().unwrap();
        let (repo, ctx) = create_worktree(&tmp, "20260307225901");
        let base_head = git_stdout_ok(&repo, &["rev-parse", "HEAD"]);
        fs::write(ctx.path.join("feature.txt"), "hello").unwrap();

        let bin_dir = tmp.path().join("bin");
        let log_path = tmp.path().join("gh.log");
        let head_path = tmp.path().join("gh-head.txt");
        let url = "https://github.com/owner/repo/pull/2";
        install_fake_gh(&bin_dir, &log_path, &head_path, url);
        let _path_guard = PathEnvGuard::prepend(&bin_dir);

        let result = attempt_pr_creation(&ctx, "add feature").unwrap();

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: url.to_string(),
                commit_outcome: CommitOutcome::Created,
            }
        );
        assert_eq!(
            git_stdout_ok(&ctx.path, &["log", "-1", "--pretty=%s"]),
            "add feature"
        );
        let worktree_head = git_stdout_ok(&ctx.path, &["rev-parse", "HEAD"]);
        assert_ne!(
            worktree_head, base_head,
            "helper should create a new commit"
        );
        assert_eq!(
            fs::read_to_string(&head_path).unwrap().trim(),
            worktree_head
        );
        assert!(
            fs::read_to_string(&log_path)
                .unwrap()
                .contains("pr create --fill --head"),
            "fake gh should receive a pr create invocation"
        );
        worktree::cleanup_worktree(&ctx).unwrap();
    }

    #[test]
    fn test_attempt_pr_creation_reuses_existing_branch_commits() {
        let tmp = TempDir::new().unwrap();
        let (repo, ctx) = create_worktree(&tmp, "20260307225902");
        let base_head = git_stdout_ok(&repo, &["rev-parse", "HEAD"]);
        fs::write(ctx.path.join("feature.txt"), "hello").unwrap();
        run_git_ok(&ctx.path, &["add", "."]);
        run_git_ok(&ctx.path, &["commit", "-m", "existing commit"]);

        let existing_head = git_stdout_ok(&ctx.path, &["rev-parse", "HEAD"]);
        assert_ne!(existing_head, base_head);

        let bin_dir = tmp.path().join("bin");
        let log_path = tmp.path().join("gh.log");
        let head_path = tmp.path().join("gh-head.txt");
        let url = "https://github.com/owner/repo/pull/3";
        install_fake_gh(&bin_dir, &log_path, &head_path, url);
        let _path_guard = PathEnvGuard::prepend(&bin_dir);

        let result = attempt_pr_creation(&ctx, "rerun without changes").unwrap();

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: url.to_string(),
                commit_outcome: CommitOutcome::NoChanges,
            }
        );
        assert_eq!(
            git_stdout_ok(&ctx.path, &["rev-parse", "HEAD"]),
            existing_head
        );
        assert_eq!(
            fs::read_to_string(&head_path).unwrap().trim(),
            existing_head
        );
        worktree::cleanup_worktree(&ctx).unwrap();
    }
}
