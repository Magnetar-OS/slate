// SPDX-License-Identifier: GPL-3.0-only

//! The Accounts context page: add a CalDAV account, see its state, sync now.
//!
//! Deliberately spare. A calendar account is a URL, a username, and a password,
//! and every field beyond those is one more thing standing between a user and a
//! working calendar. Discovery handles the rest: `CaldavClient::discover`
//! accepts a server root, a principal URL, or a calendar home and works out
//! which it was given, so the user can paste whatever their provider's help
//! page told them to.

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget;

use crate::app::{AccountForm, ConflictRow, Message, SubscriptionForm};
use crate::fl;
use crate::model::CalendarMeta;

/// One row per account, plus the add form or the button that opens it —
/// followed by ICS subscriptions and any sync conflicts awaiting an answer.
pub fn view<'a>(
    accounts: &'a [cosmic_pim_accounts::Account],
    form: Option<&'a AccountForm>,
    syncing: bool,
    status: Option<&'a str>,
    feeds: Vec<CalendarMeta>,
    sub_form: Option<&'a SubscriptionForm>,
    conflicts: &'a [ConflictRow],
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(8).spacing(spacing.space_s);

    if accounts.is_empty() && form.is_none() {
        column = column.push(
            widget::text::body(fl!("no-accounts-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    if !accounts.is_empty() {
        let mut list = widget::settings::section();
        for account in accounts {
            list = list.add(
                widget::settings::item::builder(account.display_name.clone())
                    .description(format!("{} · {}", account.username, account.url))
                    .control(
                        widget::button::text(fl!("remove"))
                            .class(cosmic::theme::Button::Destructive)
                            .on_press(Message::AccountRemove(account.id.clone())),
                    ),
            );
        }
        column = column.push(list);
    }

    column = match form {
        Some(form) => column.push(add_form(form)),
        None => column.push(
            widget::button::text(fl!("add-account"))
                .class(cosmic::theme::Button::Suggested)
                .on_press(Message::AccountAddStart),
        ),
    };

    if !accounts.is_empty() {
        let label = if syncing {
            fl!("syncing")
        } else {
            fl!("sync-now")
        };
        let button = widget::button::text(label);
        // No `on_press` while a pass is in flight: a second concurrent pass
        // would race the first one on the same sidecar files.
        column = column.push(if syncing {
            button
        } else {
            button.on_press(Message::SyncNow)
        });
    }

    if let Some(status) = status {
        column = column.push(
            widget::text::caption(status.to_owned())
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    column = column.push(subscriptions_section(feeds, sub_form));

    if !conflicts.is_empty() {
        column = column.push(conflicts_section(conflicts));
    }

    column.into()
}

/// ICS feed subscriptions: one row per feed with a Remove, and the add form.
fn subscriptions_section<'a>(
    feeds: Vec<CalendarMeta>,
    sub_form: Option<&'a SubscriptionForm>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(4)
        .spacing(spacing.space_s)
        .push(widget::text::heading(fl!("subscriptions")));

    if feeds.is_empty() && sub_form.is_none() {
        column = column.push(
            widget::text::caption(fl!("no-subscriptions-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    if !feeds.is_empty() {
        let mut list = widget::settings::section();
        for feed in feeds {
            list = list.add(
                widget::settings::item::builder(feed.name)
                    .description(fl!("subscription-read-only"))
                    .control(
                        widget::button::text(fl!("remove"))
                            .class(cosmic::theme::Button::Destructive)
                            .on_press(Message::SubRemoveRequest(feed.id)),
                    ),
            );
        }
        column = column.push(list);
    }

    match sub_form {
        Some(form) => column.push(subscription_form(form)).into(),
        None => column
            .push(widget::button::text(fl!("add-subscription")).on_press(Message::SubAddStart))
            .into(),
    }
}

fn subscription_form(form: &SubscriptionForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let section = widget::settings::section()
        .add(
            widget::settings::item::builder(fl!("subscription-name")).control(
                widget::text_input(fl!("subscription-name"), &form.name)
                    .on_input(Message::SubNameChanged)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("subscription-url")).control(
                widget::text_input("https://…", &form.url)
                    .on_input(Message::SubUrlChanged)
                    .width(Length::Fixed(220.0)),
            ),
        );

    let mut column = widget::column::with_capacity(3)
        .spacing(spacing.space_s)
        .push(section);

    if let Some(error) = &form.error {
        column = column.push(
            widget::text::body(error.clone())
                .class(cosmic::theme::Text::Custom(|theme| {
                    cosmic::iced::widget::text::Style {
                        color: Some(theme.cosmic().destructive_color().into()),
                        ..Default::default()
                    }
                }))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    let can_submit = !form.url.trim().is_empty();
    column
        .push(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xs)
                .push(widget::button::text(fl!("cancel")).on_press(Message::SubAddCancel))
                .push({
                    let add =
                        widget::button::text(fl!("add")).class(cosmic::theme::Button::Suggested);
                    if can_submit {
                        add.on_press(Message::SubAddConfirm)
                    } else {
                        add
                    }
                }),
        )
        .into()
}

/// Unresolved sync conflicts: what each side says, and the ways out.
/// Nothing resolves itself with time — the alternative to asking is guessing.
fn conflicts_section(conflicts: &[ConflictRow]) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(2 + conflicts.len())
        .spacing(spacing.space_s)
        .push(widget::text::heading(fl!("conflicts")))
        .push(
            widget::text::caption(fl!("conflicts-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );

    for (index, row) in conflicts.iter().enumerate() {
        column = column.push(conflict_card(index, row));
    }

    column.into()
}

/// One conflict: the wholesale answers, plus per-unit choices when the sync
/// pass kept the revision both sides diverged from.
fn conflict_card(index: usize, row: &ConflictRow) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let mut section = widget::settings::section().add(
        widget::settings::item::builder(fl!(
            "conflict-versions",
            yours = row.yours.clone(),
            theirs = row.theirs.clone()
        ))
        .description(row.collection.clone())
        .control(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xxs)
                .push(
                    widget::button::text(fl!("conflict-keep-mine"))
                        .on_press(Message::ConflictKeepLocal(index)),
                )
                .push(
                    widget::button::text(fl!("conflict-take-theirs"))
                        .on_press(Message::ConflictTakeRemote(index)),
                ),
        ),
    );

    let Some(disputes) = &row.disputes else {
        return section.into();
    };

    if disputes.units.is_empty() {
        // The two edits touch different units; nothing needs choosing.
        section = section.add(
            widget::settings::item::builder(fl!("conflict-merges-cleanly")).control(
                widget::button::suggested(fl!("conflict-merge-both"))
                    .on_press(Message::ConflictApplyMerge(index)),
            ),
        );
        return section.into();
    }

    let mut units = widget::column::with_capacity(disputes.units.len() + 2)
        .spacing(spacing.space_xs)
        .push(
            widget::text::caption(fl!("conflict-choose-description"))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );

    for (unit_index, overlap) in disputes.units.iter().enumerate() {
        units = units.push(dispute_unit(
            index,
            unit_index,
            overlap,
            disputes.choices.get(&overlap.unit).copied(),
        ));
    }

    let apply = widget::button::suggested(fl!("conflict-apply-merge"));
    units = units.push(if disputes.decided() {
        apply.on_press(Message::ConflictApplyMerge(index))
    } else {
        apply
    });

    section.add(units).into()
}

/// One disputed unit: the label, what it said before either edit, and the two
/// versions to pick between — the chosen one drawn as the suggested button.
fn dispute_unit<'a>(
    row: usize,
    unit: usize,
    overlap: &'a cosmic_pim_core::merge::Overlap,
    chosen: Option<cosmic_pim_core::merge::Side>,
) -> Element<'a, Message> {
    use cosmic_pim_core::merge::Side;

    let spacing = cosmic::theme::spacing();

    let side_button = |label: String, lines: Option<&'a Vec<String>>, side: Side| {
        let text = lines.map_or_else(|| fl!("conflict-absent"), |l| l.join("\n"));
        let body = widget::column::with_capacity(2)
            .spacing(spacing.space_xxxs)
            .push(widget::text::caption_heading(label))
            .push(widget::text::caption(text).wrapping(cosmic::iced::core::text::Wrapping::Word));
        widget::button::custom(body)
            .class(if chosen == Some(side) {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Standard
            })
            .padding(spacing.space_xxs)
            .width(Length::Fill)
            .on_press(Message::ConflictChooseSide(row, unit, side))
    };

    let mut column = widget::column::with_capacity(3)
        .spacing(spacing.space_xxxs)
        .push(widget::text::caption_heading(overlap.unit.clone()));

    if let Some(base) = &overlap.base {
        column = column.push(
            widget::text::caption(fl!("conflict-was", lines = base.join(" ")))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    column
        .push(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xxs)
                .push(side_button(
                    fl!("conflict-mine"),
                    overlap.local.as_ref(),
                    Side::Local,
                ))
                .push(side_button(
                    fl!("conflict-theirs-label"),
                    overlap.remote.as_ref(),
                    Side::Remote,
                )),
        )
        .into()
}

fn add_form(form: &AccountForm) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();

    let section = widget::settings::section()
        .add(
            widget::settings::item::builder(fl!("account-name")).control(
                widget::text_input(fl!("account-name"), &form.display_name)
                    .on_input(Message::AccountNameChanged)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("server-url")).control(
                widget::text_input("https://…", &form.url)
                    .on_input(Message::AccountUrlChanged)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("username")).control(
                widget::text_input(fl!("username"), &form.username)
                    .on_input(Message::AccountUsernameChanged)
                    .width(Length::Fixed(220.0)),
            ),
        )
        .add(
            widget::settings::item::builder(fl!("password"))
                .description(fl!("app-password-hint"))
                .control(
                    widget::secure_input(fl!("password"), &form.password, None, true)
                        .on_input(Message::AccountPasswordChanged)
                        .width(Length::Fixed(220.0)),
                ),
        );

    let mut column = widget::column::with_capacity(3)
        .spacing(spacing.space_s)
        .push(section);

    if let Some(error) = &form.error {
        column = column.push(
            widget::text::body(error.clone())
                .class(cosmic::theme::Text::Custom(|theme| {
                    cosmic::iced::widget::text::Style {
                        color: Some(theme.cosmic().destructive_color().into()),
                        ..Default::default()
                    }
                }))
                .wrapping(cosmic::iced::core::text::Wrapping::Word),
        );
    }

    // A URL and a username are the minimum that could possibly work; an empty
    // password is left submittable because some servers genuinely use none.
    let can_submit = !form.url.trim().is_empty() && !form.username.trim().is_empty();

    column
        .push(
            widget::row::with_capacity(2)
                .spacing(spacing.space_xs)
                .push(widget::button::text(fl!("cancel")).on_press(Message::AccountAddCancel))
                .push({
                    let add =
                        widget::button::text(fl!("add")).class(cosmic::theme::Button::Suggested);
                    if can_submit {
                        add.on_press(Message::AccountAddConfirm)
                    } else {
                        add
                    }
                }),
        )
        .into()
}
