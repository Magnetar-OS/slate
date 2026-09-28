// SPDX-License-Identifier: GPL-3.0-only

//! Meeting-link detection: the URL worth a "Join" button, if an event has one.
//!
//! Looks through the fields real invitations put links in — location first,
//! then description — for the video-call services the roadmap names (Meet,
//! Zoom, Teams, Jitsi, `BigBlueButton`). Deliberately conservative: a link is
//! offered only when its host matches a known service, because a "Join"
//! button that opens someone's agenda document teaches people to ignore it.
//!
//! RFC 7986's `CONFERENCE` property is the exception: it exists to name the
//! way into the meeting, so its URI is trusted whatever the host
//! ([`conference_link`]).

/// Domains that identify a video-call link: the host is one of these, or a
/// subdomain of one (`us02web.zoom.us`).
const SERVICES: &[&str] = &[
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "meet.jit.si",
];

/// Host labels that identify a self-hosted video-call server
/// (`jitsi.example.org`, `bbb.uni.example`). Matched as whole labels, so
/// `notjitsi.com` is not one.
const SELF_HOSTED: &[&str] = &["jitsi", "bigbluebutton", "bbb"];

/// Whether `host` is a known video-call service.
///
/// Whole domains and whole labels only: a substring test let
/// `zoom.us.example.net` — anyone's host — earn a Join button.
fn is_meeting_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    // A port or credentials are not part of the name.
    let host = host.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();

    SERVICES
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        || host.split('.').any(|label| SELF_HOSTED.contains(&label))
}

/// The first video-call URL found in `location` or `description`.
#[must_use]
pub fn meeting_link(location: Option<&str>, description: Option<&str>) -> Option<String> {
    location
        .and_then(find_in)
        .or_else(|| description.and_then(find_in))
}

/// The first `https` URI among an event's `CONFERENCE` properties, given the
/// event's unmodelled property lines (`Event::other`).
///
/// The value is whatever follows the first colon outside a quoted parameter
/// — `LABEL="Join: dial-in"` must not end the name part early.
#[must_use]
pub fn conference_link(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let name_end = line.find([';', ':'])?;
        if !line[..name_end].eq_ignore_ascii_case("CONFERENCE") {
            return None;
        }
        let mut quoted = false;
        let colon = line.char_indices().find_map(|(index, c)| match c {
            '"' => {
                quoted = !quoted;
                None
            }
            ':' if !quoted => Some(index),
            _ => None,
        })?;
        let value = line[colon + 1..].trim();
        value.starts_with("https://").then(|| value.to_owned())
    })
}

fn find_in(text: &str) -> Option<String> {
    for (index, _) in text.match_indices("https://") {
        let candidate: String = text[index..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '>' | ')' | ']' | '"' | '\''))
            .collect();
        // Trailing punctuation is prose, not address.
        let candidate = candidate.trim_end_matches(['.', ',', ';']);

        let host = candidate
            .strip_prefix("https://")
            .unwrap_or(candidate)
            .split('/')
            .next()
            .unwrap_or_default();

        if is_meeting_host(host) {
            return Some(candidate.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meet_link_in_the_location_is_found() {
        assert_eq!(
            meeting_link(Some("https://meet.google.com/abc-defg-hij"), None),
            Some("https://meet.google.com/abc-defg-hij".to_owned())
        );
    }

    #[test]
    fn a_zoom_link_buried_in_a_description_is_found() {
        let description = "Agenda:\n1. Standup\nJoin: https://us02web.zoom.us/j/123?pwd=x. Thanks!";
        assert_eq!(
            meeting_link(None, Some(description)),
            Some("https://us02web.zoom.us/j/123?pwd=x".to_owned())
        );
    }

    #[test]
    fn the_location_wins_over_the_description() {
        assert_eq!(
            meeting_link(
                Some("https://meet.jit.si/room"),
                Some("https://zoom.us/j/999")
            ),
            Some("https://meet.jit.si/room".to_owned())
        );
    }

    #[test]
    fn an_ordinary_url_is_not_a_meeting() {
        assert_eq!(
            meeting_link(Some("https://example.com/agenda.pdf"), None),
            None
        );
        assert_eq!(meeting_link(Some("Kolonaki, Athens"), None), None);
    }

    #[test]
    fn a_conference_property_names_the_way_in() {
        let lines = vec![
            "X-MICROSOFT-CDO-BUSYSTATUS:BUSY".to_owned(),
            r#"CONFERENCE;VALUE=URI;FEATURE=VIDEO;LABEL="Join: video":https://video.example.org/r/42"#
                .to_owned(),
        ];
        assert_eq!(
            conference_link(&lines),
            Some("https://video.example.org/r/42".to_owned())
        );
    }

    #[test]
    fn a_dial_in_conference_is_not_a_link() {
        let lines = vec!["CONFERENCE;VALUE=URI;FEATURE=PHONE:tel:+1-555-0100".to_owned()];
        assert_eq!(conference_link(&lines), None);
    }

    #[test]
    fn a_lookalike_host_is_not_a_meeting() {
        for url in [
            "https://zoom.us.example.net/j/1",
            "https://notjitsi.com/room",
            "https://evil-meet.google.com.example/x",
            "https://bbbq.example.org/b/room",
        ] {
            assert_eq!(meeting_link(Some(url), None), None, "{url}");
        }
    }

    #[test]
    fn self_hosted_servers_and_subdomains_are_meetings() {
        for url in [
            "https://jitsi.example.org/standup",
            "https://bbb.uni.example/b/abc-123",
            "https://us02web.zoom.us/j/123",
            "https://ZOOM.US/j/5",
        ] {
            assert_eq!(meeting_link(Some(url), None).as_deref(), Some(url), "{url}");
        }
    }

    #[test]
    fn angle_brackets_and_quotes_do_not_travel_with_the_link() {
        assert_eq!(
            meeting_link(None, Some("<https://teams.microsoft.com/l/meetup/xyz>")),
            Some("https://teams.microsoft.com/l/meetup/xyz".to_owned())
        );
    }
}
