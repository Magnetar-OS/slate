// SPDX-License-Identifier: GPL-3.0-only

//! Slate's side of the suite's one cross-process contract: iMIP hand-off.
//!
//! Envelope finds a `text/calendar` part carrying a `METHOD` and calls
//! `DeliverInvitation2` here; the invitation view, the conflict check, and the
//! PARTSTAT decision all happen in the app, asynchronously — `true` on the
//! call promises only that Slate has the payload. The reply goes back out
//! through Envelope's `SendSchedulingReply` when its name is owned, and
//! degrades to "reply from your mail client" when it is not. Neither
//! direction ever starts the other app: an invitation must not *launch* a
//! mail client, so Envelope's absence is checked with `NameHasOwner` rather
//! than discovered through an auto-starting method call.
//!
//! The contract, as this side serves and calls it:
//!
//! ```text
//! Slate exports, on com.magnetaros.CosmicPim.Scheduling1:
//!   DeliverInvitation2(ics: s, account_id: s, sender: s) → (accepted: b)
//!   DeliverInvitation(ics: s, account_id: s)             → (accepted: b)
//!
//! Slate calls, on Envelope:
//!   com.magnetaros.CosmicPim.Scheduling2
//!     SendSchedulingReply(ics: s, account_id: s, to: s, from: s) → (queued: b)
//!   com.magnetaros.CosmicPim.Scheduling1
//!     SendSchedulingReply(ics: s, account_id: s, to: s)          → (queued: b)
//! ```
//!
//! `sender` is the mail's `From` address — the address alone, no display
//! name. It is what lets the organizer check refuse a REQUEST or CANCEL that
//! names one person as ORGANIZER and was mailed by another. The two-argument
//! `DeliverInvitation` is what a mailer from before that knows; its payloads
//! are checked without a sender, which still refuses one with no organizer.
//!
//! `from` is the ATTENDEE address the invitation was sent to, so an
//! invitation that reached an alias is answered from that alias. An Envelope
//! from before `Scheduling2` answers `UnknownInterface`, and the reply is
//! handed to its `Scheduling1` method instead, which sends from the
//! account's primary address.
//!
//! The iTIP semantics — the attendee gate, the organizer check, the SEQUENCE
//! rule, the RECURRENCE-ID-scoped CANCEL — live in `cosmic_pim_caldav::itip`
//! and are deliberately not reimplemented here. See
//! cosmic-pim/ARCHITECTURE.md, "iMIP hand-off".

use cosmic::iced::futures::channel::mpsc::UnboundedSender;

/// The object path both apps derive from their own well-known name.
pub const OBJECT_PATH: &str = "/com/magnetaros/Slate";
/// Envelope's side of the contract, for the reply direction.
pub const ENVELOPE_NAME: &str = "com.magnetaros.Envelope";
pub const ENVELOPE_PATH: &str = "/com/magnetaros/Envelope";
const INTERFACE: &str = "com.magnetaros.CosmicPim.Scheduling1";
/// Where Envelope takes a reply together with the address to send it from.
const REPLY_INTERFACE: &str = "com.magnetaros.CosmicPim.Scheduling2";

/// An invitation as delivered: the verbatim payload, the suite account it
/// arrived on, and who mailed it.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub ics: String,
    pub account_id: String,
    /// The mail's `From` address, which the organizer check compares with
    /// the payload's ORGANIZER. `None` from a mailer that only knows
    /// `DeliverInvitation` and so cannot say.
    pub sender: Option<String>,
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
    /// Takes one `text/calendar` payload off a mailer's hands, without
    /// learning who mailed it. What a mailer from before
    /// `DeliverInvitation2` calls.
    fn deliver_invitation(&self, ics: String, account_id: String) -> bool {
        self.tx
            .unbounded_send(Delivery {
                ics,
                account_id,
                sender: None,
            })
            .is_ok()
    }

    /// Takes one `text/calendar` payload off a mailer's hands, with the
    /// `From` address of the mail that carried it.
    #[zbus(name = "DeliverInvitation2")]
    fn deliver_invitation2(&self, ics: String, account_id: String, sender: String) -> bool {
        self.tx
            .unbounded_send(Delivery {
                ics,
                account_id,
                sender: Some(sender),
            })
            .is_ok()
    }
}

/// Hands a `METHOD:REPLY` to Envelope's outbox, if Envelope is running, to
/// go out `from` the address the invitation was sent to.
///
/// `Ok(true)`: queued — Envelope's outbox owns retries from here.
/// `Ok(false)`: Envelope's name is unowned, or it declined the payload; the
/// caller falls back to telling the user to reply from their mail client.
pub async fn send_reply(
    conn: &zbus::Connection,
    ics: &str,
    account_id: &str,
    to: &str,
    from: &str,
) -> zbus::Result<bool> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let name = zbus::names::BusName::try_from(ENVELOPE_NAME)?;
    if !dbus.name_has_owner(name).await? {
        return Ok(false);
    }
    let sent = conn
        .call_method(
            Some(ENVELOPE_NAME),
            ENVELOPE_PATH,
            Some(REPLY_INTERFACE),
            "SendSchedulingReply",
            &(ics, account_id, to, from),
        )
        .await;
    let reply = match sent {
        // An Envelope from before `Scheduling2`. It cannot be told which
        // address to send from, and sends from the account's primary one.
        Err(zbus::Error::MethodError(error, ..))
            if error.as_str() == "org.freedesktop.DBus.Error.UnknownInterface" =>
        {
            conn.call_method(
                Some(ENVELOPE_NAME),
                ENVELOPE_PATH,
                Some(INTERFACE),
                "SendSchedulingReply",
                &(ics, account_id, to),
            )
            .await?
        }
        sent => sent?,
    };
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testbus::PrivateBus;
    use cosmic::iced::futures::{StreamExt, channel::mpsc};
    use cosmic_pim_caldav::itip::{self, Outcome};
    use std::sync::{Arc, Mutex};

    const ME: &str = "me@example.com";

    /// boss@ invites me@: the payload a genuine mail and a spoofed one carry
    /// alike, which is why the payload alone cannot tell them apart.
    const REQUEST: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n\
         UID:review@x\r\nSEQUENCE:0\r\nSUMMARY:Review\r\nDTSTART:20260804T090000Z\r\n\
         ORGANIZER:mailto:boss@example.com\r\nATTENDEE:mailto:me@example.com\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n";

    /// Slate serving the contract on a private bus, and a mailer's
    /// connection to it.
    async fn slate_on_private_bus() -> (
        PrivateBus,
        zbus::Connection,
        zbus::Connection,
        mpsc::UnboundedReceiver<Delivery>,
    ) {
        let bus = PrivateBus::start();
        let (tx, rx) = mpsc::unbounded();
        let slate = bus.connect().await;
        slate
            .object_server()
            .at(OBJECT_PATH, Scheduling::new(tx))
            .await
            .unwrap();
        slate.request_name("com.magnetaros.Slate").await.unwrap();
        let mailer = bus.connect().await;
        (bus, slate, mailer, rx)
    }

    /// Calls Slate the way Envelope does, and returns what reached the app.
    async fn deliver(
        mailer: &zbus::Connection,
        rx: &mut mpsc::UnboundedReceiver<Delivery>,
        sender: Option<&str>,
    ) -> Delivery {
        let destination = Some("com.magnetaros.Slate");
        let reply = match sender {
            Some(sender) => {
                mailer
                    .call_method(
                        destination,
                        OBJECT_PATH,
                        Some(INTERFACE),
                        "DeliverInvitation2",
                        &(REQUEST, "work", sender),
                    )
                    .await
            }
            None => {
                mailer
                    .call_method(
                        destination,
                        OBJECT_PATH,
                        Some(INTERFACE),
                        "DeliverInvitation",
                        &(REQUEST, "work"),
                    )
                    .await
            }
        }
        .unwrap();
        assert!(reply.body().deserialize::<bool>().unwrap());
        rx.next().await.unwrap()
    }

    #[tokio::test]
    async fn an_invitation_mailed_by_someone_else_is_refused_and_said_so() {
        let (_bus, _slate, mailer, mut rx) = slate_on_private_bus().await;
        let collection = tempfile::tempdir().unwrap();

        let delivery = deliver(&mailer, &mut rx, Some("mallory@example.net")).await;
        assert_eq!(delivery.sender.as_deref(), Some("mallory@example.net"));

        // What the app does with it: no question for the user, and a
        // message saying why.
        let parsed = itip::parse(&delivery.ics).unwrap();
        let refusal = crate::app::review_request(delivery.clone(), parsed, Some(ME)).unwrap_err();
        assert_eq!(refusal, crate::fl!("invitation-not-from-organizer"));

        // And the library refuses to store it, whatever the app asked.
        assert_eq!(
            itip::apply(
                collection.path(),
                &delivery.ics,
                ME,
                delivery.sender.as_deref()
            )
            .unwrap(),
            Outcome::NotFromOrganizer
        );
        assert_eq!(std::fs::read_dir(collection.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_invitation_mailed_by_its_organizer_is_asked_about_and_stored() {
        let (_bus, _slate, mailer, mut rx) = slate_on_private_bus().await;
        let collection = tempfile::tempdir().unwrap();

        // Addresses compare whatever their case.
        let delivery = deliver(&mailer, &mut rx, Some("Boss@Example.com")).await;
        let parsed = itip::parse(&delivery.ics).unwrap();
        let invitation = crate::app::review_request(delivery, parsed, Some(ME)).unwrap();
        assert_eq!(invitation.sender.as_deref(), Some("Boss@Example.com"));

        // Accepting it applies the payload with the sender it came with.
        assert!(matches!(
            itip::apply(
                collection.path(),
                &invitation.ics,
                ME,
                invitation.sender.as_deref()
            )
            .unwrap(),
            Outcome::Created { .. }
        ));
    }

    #[tokio::test]
    async fn a_mailer_that_cannot_name_the_sender_is_still_served() {
        let (_bus, _slate, mailer, mut rx) = slate_on_private_bus().await;

        let delivery = deliver(&mailer, &mut rx, None).await;
        assert_eq!(delivery.sender, None);
        let parsed = itip::parse(&delivery.ics).unwrap();
        assert!(crate::app::review_request(delivery, parsed, Some(ME)).is_ok());
    }

    /// The calls a stand-in Envelope received, each as its arguments.
    type Seen = Arc<Mutex<Vec<Vec<String>>>>;

    /// Envelope's `Scheduling1`: a reply, and no say in where it goes from.
    struct EnvelopeV1(Seen);

    #[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling1")]
    impl EnvelopeV1 {
        fn send_scheduling_reply(&self, ics: String, account_id: String, to: String) -> bool {
            self.0.lock().unwrap().push(vec![ics, account_id, to]);
            true
        }
    }

    /// Envelope's `Scheduling2`: the same, with the address to send from.
    struct EnvelopeV2(Seen);

    #[zbus::interface(name = "com.magnetaros.CosmicPim.Scheduling2")]
    impl EnvelopeV2 {
        fn send_scheduling_reply(
            &self,
            ics: String,
            account_id: String,
            to: String,
            from: String,
        ) -> bool {
            self.0.lock().unwrap().push(vec![ics, account_id, to, from]);
            true
        }
    }

    /// A stand-in Envelope on a private bus, serving `Scheduling1` and, when
    /// `current`, `Scheduling2` beside it.
    async fn envelope_on_private_bus(
        current: bool,
    ) -> (PrivateBus, zbus::Connection, zbus::Connection, Seen) {
        let bus = PrivateBus::start();
        let seen = Seen::default();
        let envelope = bus.connect().await;
        let server = envelope.object_server();
        server
            .at(ENVELOPE_PATH, EnvelopeV1(seen.clone()))
            .await
            .unwrap();
        if current {
            server
                .at(ENVELOPE_PATH, EnvelopeV2(seen.clone()))
                .await
                .unwrap();
        }
        envelope.request_name(ENVELOPE_NAME).await.unwrap();
        let slate = bus.connect().await;
        (bus, envelope, slate, seen)
    }

    const REPLY: &str = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n";

    #[tokio::test]
    async fn the_reply_names_the_address_it_answers_as() {
        let (_bus, _envelope, slate, seen) = envelope_on_private_bus(true).await;

        let queued = send_reply(
            &slate,
            REPLY,
            "work",
            "boss@example.com",
            "alias@example.com",
        )
        .await
        .unwrap();

        assert!(queued);
        assert_eq!(
            *seen.lock().unwrap(),
            [[REPLY, "work", "boss@example.com", "alias@example.com"]]
        );
    }

    #[tokio::test]
    async fn an_envelope_from_before_scheduling2_still_gets_the_reply() {
        let (_bus, _envelope, slate, seen) = envelope_on_private_bus(false).await;

        let queued = send_reply(
            &slate,
            REPLY,
            "work",
            "boss@example.com",
            "alias@example.com",
        )
        .await
        .unwrap();

        assert!(queued);
        assert_eq!(*seen.lock().unwrap(), [[REPLY, "work", "boss@example.com"]]);
    }

    #[tokio::test]
    async fn without_envelope_the_reply_is_left_to_the_user() {
        let bus = PrivateBus::start();
        let slate = bus.connect().await;

        let queued = send_reply(&slate, "", "work", "boss@example.com", ME)
            .await
            .unwrap();

        assert!(!queued);
    }
}
