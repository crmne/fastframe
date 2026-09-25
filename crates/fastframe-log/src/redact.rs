//! Keep private words out of log lines.
//!
//! Error messages from HTTP clients and protocol libraries quote what they
//! were handling: download links with their access tokens, account ids,
//! addresses. Before such a message is logged at a level that ships, pass it
//! through one of these.
//!
//! ```
//! use fastframe_log::redact;
//!
//! let error = "HTTP 410 for https://cdn.example/v/t62/file?oh=token";
//! assert_eq!(redact::links(error), "HTTP 410 for <link>");
//!
//! // An app adds the shapes it knows are private.
//! let cdn = |word: &str| redact::is_link(word) || word.contains("oh=");
//! assert_eq!(redact::words("fetch /v/x?oh=1 failed", cdn), "fetch <link> failed");
//! ```
//!
//! To summarise a whole target's messages instead, see
//! [`Logging::redact`](crate::Logging::redact).

/// What a redacted word is replaced with.
pub const LINK: &str = "<link>";

/// Whether `word` looks like a link: it has a scheme (`https://`, `file://`).
pub fn is_link(word: &str) -> bool {
    word.contains("://")
}

/// `text` with every link replaced by [`LINK`].
///
/// Words are split on whitespace and joined with single spaces.
pub fn links(text: &str) -> String {
    words(text, is_link)
}

/// `text` with every whitespace-separated word for which `private` answers
/// `true` replaced by [`LINK`].
///
/// Words are split on whitespace and joined with single spaces.
pub fn words(text: &str, private: impl Fn(&str) -> bool) -> String {
    text.split_whitespace()
        .map(|word| if private(word) { LINK } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_lose_their_paths_and_tokens() {
        let redacted = links("HTTP 410 for https://mmg.whatsapp.net/v/t62/x?oh=1 (retry)");
        assert_eq!(redacted, "HTTP 410 for <link> (retry)");
        assert!(!redacted.contains("oh=1"));
    }

    #[test]
    fn plain_text_is_kept() {
        assert_eq!(
            links("connection reset by peer"),
            "connection reset by peer"
        );
    }

    #[test]
    fn an_app_can_add_its_own_private_shapes() {
        let cdn = |word: &str| is_link(word) || word.contains("/v/") || word.contains("oh=");
        assert_eq!(
            words("download /v/t62/x?oh=1 failed: 403", cdn),
            "download <link> failed: 403"
        );
    }

    #[test]
    fn whitespace_collapses_to_single_spaces() {
        assert_eq!(links("a\n\tb   https://x.example/c"), "a b <link>");
    }
}
