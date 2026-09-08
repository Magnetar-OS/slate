// SPDX-License-Identifier: GPL-3.0-only

//! Slate's side of the suite's one cross-process contract: iMIP hand-off.
//!
//! Envelope finds a `text/calendar` part carrying a `METHOD` and calls
//! `DeliverInvitation` here; the invitation view, the conflict check, and the
//! PARTSTAT decision all happen in the app, asynchronously — `true` on the
//! call promises only that Slate has the payload. The reply goes back out
//! through Envelope's `SendSchedulingReply` when its name is owned, and
//! degrades to "reply from your mail client" when it is not. Neither
//! direction ever starts the other app: an invitation must not *launch* a
//! mail client, so Envelope's absence is checked with `NameHasOwner` rather
//! than discovered through an auto-starting method call.
//!
//! The iTIP semantics — the attendee gate, the SEQUENCE rule, the
//! RECURRENCE-ID-scoped CANCEL — live in `cosmic_pim_caldav::itip` and are
//! deliberately not reimplemented here. See cosmic-pim/ARCHITECTURE.md,
//! "iMIP hand-off".

use cosmic::iced::futures::channel::mpsc::UnboundedSender;

/// The object path both apps derive from their own well-known name.
pub const OBJECT_PATH: &str = "/com/magnetaros/Slate";
/// Envelope's side of the contract, for the reply direction.
pub const ENVELOPE_NAME: &str = "com.magnetaros.Envelope";
pub const ENVELOPE_PATH: &str = "/com/magnetaros/Envelope";
const INTERFACE: &str = "com.magnetaros.CosmicPim.Scheduling1";

/// An invitation as delivered: the verbatim payload and the suite account it
/// arrived on.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub ics: String,
    pub account_id: String,
}

/// The D-Bus interface Envelope calls. Forwards into the app's update loop.
pub struct Scheduling {
    tx: UnboundedSender<Delivery>,
}

impl Scheduling {
    #[must_use]
    pub fn new(tx: UnboundedSender<Delivery>) -> Self {
        Self { tx }
    }
}

#[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling1")]
impl Scheduling {
    /// Takes one `text/calendar` payload off a mailer's hands.
    fn deliver_invitation(&self, ics: String, account_id: String) -> bool {
        self.tx.unbounded_send(Delivery { ics, account_id }).is_ok()
    }
}

/// Hands a `METHOD:REPLY` to Envelope's outbox, if Envelope is running.
///
/// `Ok(true)`: queued — Envelope's outbox owns retries from here.
/// `Ok(false)`: Envelope's name is unowned, or it declined the payload; the
/// caller falls back to telling the user to reply from their mail client.
pub async fn send_reply(
    conn: &zbus::Connection,
    ics: &str,
    account_id: &str,
    to: &str,
) -> zbus::Result<bool> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let name = zbus::names::BusName::try_from(ENVELOPE_NAME)?;
    if !dbus.name_has_owner(name).await? {
        return Ok(false);
    }
    let proxy = zbus::Proxy::new(conn, ENVELOPE_NAME, ENVELOPE_PATH, INTERFACE).await?;
    proxy
        .call("SendSchedulingReply", &(ics, account_id, to))
        .await
}
