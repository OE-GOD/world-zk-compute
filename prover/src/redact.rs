//! Cutting URLs to their origin, for log lines and error texts.
//!
//! The path or query of an RPC URL often holds the provider's API key, the
//! path of a webhook URL its token, and a database URL the password.
//!
//! The same code as `crates/watcher/src/redact.rs` (this crate is a workspace
//! of its own).

use url::Url;

/// `text` with every URL in it cut to its origin (see [`url_origin`]). For the
/// text of a failed RPC call, which names the URL the request went to.
pub fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(separator) = rest.find("://") {
        // The scheme is the run of scheme characters right before "://".
        let scheme_start = rest[..separator]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .last()
            .map_or(separator, |(i, _)| i);
        // The URL goes on to the next space or double quote. Closing
        // punctuation at its end belongs to the sentence around it.
        let url_end = rest[separator..]
            .find(|c: char| c.is_whitespace() || c == '"')
            .map_or(rest.len(), |i| separator + i);
        let quoted = &rest[scheme_start..url_end];
        let url = quoted.trim_end_matches([')', '\'', ',', '.', ';', ':']);
        out.push_str(&rest[..scheme_start]);
        if scheme_start + url.len() == separator + "://".len() {
            // A scheme that is named, with nothing behind it.
            out.push_str(quoted);
        } else {
            out.push_str(&url_origin(url));
            out.push_str(&quoted[url.len()..]);
        }
        rest = &rest[url_end..];
    }
    out.push_str(rest);
    out
}

/// Scheme, host and port of a URL. The path, the query and any credentials
/// are left out. What is not a complete URL, or has a '@' behind the host, is
/// not shown at all.
pub fn url_origin(url: &str) -> String {
    origin(url).unwrap_or_else(|| "<URL not shown>".to_string())
}

fn origin(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, after_scheme) = url.split_once("://")?;
    // The parser is lenient: it takes "http:host" for a URL, skips slashes in
    // front of the host, and drops tabs and line breaks. What it then takes
    // for the host can be a part of the credentials or the path.
    let is_scheme = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.');
    if !scheme.chars().all(is_scheme)
        || after_scheme.starts_with(['/', '\\'])
        || url.contains(|c: char| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    // A '@' behind the host can mean that a '/', '?' or '#' in the credentials
    // was not encoded, and that what the parser takes for the host is a part
    // of them. (Looked for in the text as written: the parser takes '..' out
    // of a path.)
    let host_end = after_scheme.find(['/', '\\', '?', '#']);
    if host_end.is_some_and(|end| after_scheme[end..].contains('@')) {
        return None;
    }
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{}://{}:{}", parsed.scheme(), host, port),
        None => format!("{}://{}", parsed.scheme(), host),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_urls_cuts_every_url_to_its_origin() {
        for (text, redacted) in [
            (
                "error sending request for url (http://127.0.0.1:8545/v2/SECRETKEY)",
                "error sending request for url (http://127.0.0.1:8545)",
            ),
            (
                "https://user:pw@rpc.example.com/v2/SECRETKEY: timed out",
                "https://rpc.example.com: timed out",
            ),
            (
                r#"{"url":"wss://rpc.example.com/ws/SECRETKEY","code":1}"#,
                r#"{"url":"wss://rpc.example.com","code":1}"#,
            ),
            (
                "first http://a.example.com/SECRETKEY, then http://b.example.com?k=SECRETKEY.",
                "first http://a.example.com, then http://b.example.com.",
            ),
            ("://SECRETKEY", "<URL not shown>"),
            (
                "http:///SECRETKEY, ws://\\\\SECRETKEY",
                "<URL not shown>, <URL not shown>",
            ),
            (
                "postgres://indexer:12#SECRET@db:5432/tee refused",
                "<URL not shown> refused",
            ),
            ("http://host/SECRETKEY://", "http://host"),
            (
                "HTTPS://RPC.example.com/SECRETKEY",
                "https://rpc.example.com",
            ),
            (
                "must start with http://, https://, ws://, or wss://.",
                "must start with http://, https://, ws://, or wss://.",
            ),
            ("missing '://' separator", "missing '://' separator"),
            ("no URL here", "no URL here"),
            ("", ""),
        ] {
            assert_eq!(redact_urls(text), redacted);
        }
    }

    #[test]
    fn test_url_origin_drops_path_query_and_credentials() {
        assert_eq!(
            url_origin("https://user:pw@arb-mainnet.example.com/v2/SECRETKEY?x=1"),
            "https://arb-mainnet.example.com"
        );
        assert_eq!(url_origin("http://127.0.0.1:8545"), "http://127.0.0.1:8545");
        assert_eq!(
            url_origin("postgres://indexer:SECRETPW@db.example.com:5432/tee"),
            "postgres://db.example.com:5432"
        );
        assert_eq!(
            url_origin("wss://rpc.example.com:8546/ws/SECRETKEY"),
            "wss://rpc.example.com:8546"
        );
        assert_eq!(url_origin("SECRETKEY, not a URL"), "<URL not shown>");
        assert_eq!(url_origin("file:///etc/SECRETKEY"), "<URL not shown>");
        // Spaces around it do not count.
        assert_eq!(
            url_origin(" https://rpc.example.com/SECRETKEY\n"),
            "https://rpc.example.com"
        );
        // Credentials with a character that should have been encoded, and
        // texts that the parser reads as a URL although they are not one.
        for url in [
            "https://user:p@ss#SECRET@host/",
            "https://user:1/SECRET@host/",
            "https://user:1?SECRET@host/",
            "https://SECRET:1/x@host/..",
            "https://SECRET:1/x@host/%2e%2e/a",
            "http:SECRET",
            "http:SECRET?u=http://z",
            "http:SECRET:1/x@h/?u=http://z",
            "https:\\\\SECRET:1/x@host/://",
            "https://\t/SECRET/x",
            "https://h\nSECRET/x",
        ] {
            assert_eq!(url_origin(url), "<URL not shown>", "{url}");
        }
    }
}
