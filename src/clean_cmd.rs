use console::style;

use crate::cli::CleanArgs;
use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, cruise_home};

pub fn run(args: CleanArgs) -> Result<()> {
    let home = cruise_home().ok_or_else(|| CruiseError::Other("HOME not set".to_string()))?;
    let manager = SessionManager::new(home);

    let report = manager.cleanup_old(args.days)?;

    if report.deleted == 0 {
        eprintln!("No sessions to clean up.");
    } else {
        eprintln!(
            "{} Removed {} session(s) older than {} day(s).",
            style("✓").green().bold(),
            report.deleted,
            args.days
        );
    }

    Ok(())
}
