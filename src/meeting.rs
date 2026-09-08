// SPDX-License-Identifier: GPL-3.0-only

//! Meeting-link detection: the URL worth a "Join" button, if an event has one.
//!
//! Looks through the fields real invitations put links in — location first,
//! then description — for the video-call services the roadmap names (Meet,
//! Zoom, Teams, Jitsi, `BigBlueButton`). Deliberately conservative: a link is
//! offered only when its host matches a known service, because a "Join"
//! button that opens someone's agenda document teaches people to ignore it.

/// Hosts (or host substrings) that identify a video-call link.
const SERVICES: &[&str] = &[
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "meet.jit.si",
    "jitsi",
    "bigbluebutton",
    "bbb.",
];

/// The first video-call URL found in `location` or `description`.
#[must_use]
pub fn meeting_link(location: Option<&str>, description: Option<&str>) -> Option<String> {
    location
        .and_then(find_in)
        .or_else(|| description.and_then(find_in))
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

        if SERVICES.iter().any(|s| host.contains(s)) {
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
    fn angle_brackets_and_quotes_do_not_travel_with_the_link() {
        assert_eq!(
            meeting_link(None, Some("<https://teams.microsoft.com/l/meetup/xyz>")),
            Some("https://teams.microsoft.com/l/meetup/xyz".to_owned())
        );
    }
}
