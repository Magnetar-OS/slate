// SPDX-License-Identifier: GPL-3.0-only

//! A private session bus for tests, so D-Bus behaviour is exercised against a
//! real `dbus-daemon` without touching the user's session.

use std::io::BufRead;

/// A `dbus-daemon` of our own, torn down on drop.
pub struct PrivateBus {
    child: std::process::Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    /// Starts a bus in a fresh temporary directory.
    ///
    /// # Panics
    ///
    /// When `dbus-daemon` is not installed or does not start: a test that
    /// needs a bus must fail loudly rather than pass without one.
    #[must_use]
    pub fn start() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory for the bus");
        let config = dir.path().join("bus.conf");
        std::fs::write(
            &config,
            // `/tmp` rather than the temporary directory: a socket path has
            // a 108-byte limit, and `TMPDIR` can be longer than that.
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        )
        .expect("writing the bus configuration");

        let mut child = std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--print-address=1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("dbus-daemon must be installed to run the D-Bus tests");

        let stdout = child.stdout.take().expect("the bus's stdout");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("the bus prints its address");

        Self {
            child,
            address: address.trim().to_owned(),
            _dir: dir,
        }
    }

    /// A new connection to this bus.
    pub async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("a valid bus address")
            .build()
            .await
            .expect("connecting to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
