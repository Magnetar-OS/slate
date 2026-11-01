// SPDX-License-Identifier: GPL-3.0-only

//! Starting the calendar from the applet and the launcher plugin.

use cosmic_launch::CustomExecutable;

/// Start `exec` as the calendar, with `env` added to its environment.
///
/// The canonical launch path: libcosmic's launcher starts it as its own
/// `app-<id>@<random>.service` unit of the user's systemd session. A plain
/// `Command::spawn` would leave the window a child of whatever opened it — the
/// panel, or a pop-launcher plugin that pop-launcher reaps — so restarting or
/// closing that would take the calendar with it.
///
/// `exec` is an `Exec` line: split with shell rules, field codes expanded.
/// Failures are logged; whatever asked has no window left to report them in.
pub async fn spawn(app_id: &str, exec: String, env: Vec<(String, String)>) {
    let session = match zbus::Connection::session().await {
        Ok(session) => session,
        Err(why) => {
            tracing::error!(%why, exec, "no session bus to launch on");
            return;
        }
    };

    let executable = CustomExecutable::new(app_id.to_owned(), exec);
    if let Err(why) = cosmic_launch::spawn(
        &session,
        &executable,
        None,
        &[],
        None,
        env,
        &[] as &[String],
    )
    .await
    {
        tracing::error!(%why, "failed to launch the calendar");
    }
}
