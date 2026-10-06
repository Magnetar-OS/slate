// SPDX-License-Identifier: GPL-3.0-only

//! Handing off to another application of the desktop.
//!
//! Each is a program started by name, and none of them is a dependency of
//! Slate's package: it may not be installed. A start that fails is the
//! caller's to answer — it has something else to offer, or something to say.

use std::process::{Command, Stdio};

/// The desktop's Accounts window. See `magnetar-accounts`.
pub const ACCOUNTS_WINDOW: &str = "magnetar-accounts";

/// The command that opens the Accounts window on its add page, saying that
/// it is a calendar an account is wanted for.
#[must_use]
pub fn add_account(program: &str) -> Command {
    let mut command = Command::new(program);
    command.arg("--for=calendar");
    command
}

/// Starts `command` and leaves it running.
///
/// # Errors
///
/// The program could not be started: most often, it is not installed.
pub fn start(mut command: Command) -> std::io::Result<()> {
    let mut child = command.stdin(Stdio::null()).spawn()?;
    // Waited for, however long it runs: a child nobody waits for stays in the
    // process table after it exits, for as long as Slate does. How it ended
    // is its own business.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accounts_window_is_asked_for_its_add_page_for_a_calendar() {
        let command = add_account(ACCOUNTS_WINDOW);

        assert_eq!(command.get_program(), "magnetar-accounts");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--for=calendar"]);
    }

    #[test]
    fn a_program_that_is_not_installed_is_an_error_to_answer() {
        let missing = start(add_account("/nonexistent/magnetar-accounts"));

        assert_eq!(
            missing.expect_err("started").kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_program_that_is_installed_is_started() {
        // `true` takes any arguments and exits at once.
        start(add_account("true")).expect("started");
    }
}
