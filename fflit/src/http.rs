//! One HTTP client for the whole run.
//!
//! A publisher hands a pdf to a browser and a paywall page to a script, and on
//! a subscribing network the difference is usually not the IP: it is a session
//! cookie set by the landing page and a `Referer` saying you arrived from it. A
//! client that keeps its cookies and says where it came from gets the file the
//! subscription already entitles you to; one that throws them away does not.

use std::sync::OnceLock;
use std::time::Duration;

/// Honest about what it is — this is not Chrome and does not claim to be — but
/// shaped like a user agent string, because filters that never read past the
/// first token refuse a bare tool name.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; fflit/0.1; +https://github.com/fflit; mailto:john@coonabibba.de)";

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// Shared, and with a cookie jar, so a landing page and the pdf request that
/// follows it are one visit rather than two strangers.
pub fn client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .timeout(Duration::from_secs(120))
            .build()
            .expect("building the http client")
    })
}

/// Ask for a file the way the click it stands in for would.
pub fn get_pdf(url: &str, referer: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let mut request = client().get(url).header("Accept", "application/pdf,*/*;q=0.8");
    if let Some(referer) = referer {
        request = request.header("Referer", referer);
    }
    let bytes = request.send()?.error_for_status()?.bytes()?;
    Ok(bytes.to_vec())
}

/// The host part of a url, which is what a publisher is throttled by.
pub fn host_of(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest).split('/').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_what_is_between_the_scheme_and_the_path() {
        assert_eq!(host_of("https://www.nature.com/articles/x.pdf"), "www.nature.com");
        assert_eq!(host_of("https://dl.acm.org"), "dl.acm.org");
        assert_eq!(host_of("http://x.invalid/a/b"), "x.invalid");
    }
}
