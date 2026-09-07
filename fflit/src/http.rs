//! One HTTP client for the whole run, and the one place fflit says who it is.
//!
//! A publisher hands a pdf to a browser and a paywall page to a script, and on
//! a subscribing network the difference is usually not the IP: it is a session
//! cookie set by the landing page and a `Referer` saying you arrived from it. A
//! client that keeps its cookies and says where it came from gets the file the
//! subscription already entitles you to; one that throws them away does not.

use std::sync::OnceLock;
use std::time::Duration;

/// Where to complain if fflit's traffic is a nuisance. Crossref, Unpaywall and
/// NCBI all ask for an address and give politer service to requests that carry
/// one, so it goes in the user agent and in the query parameters they define
/// for it.
pub const CONTACT_EMAIL: &str = "john@coonabibba.de";
pub const HOMEPAGE: &str = "https://github.com/ff_literature";

/// APIs answer quickly or not at all; the ones that need longer say so at the
/// call.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// A pdf is a file rather than a json document, and some of them are large.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// Honest about what it is — this is not Chrome and does not claim to be — but
/// shaped like a user agent string, because filters that never read past the
/// first token refuse a bare tool name. The version comes from `Cargo.toml`
/// rather than a literal, which is one fewer thing to forget.
pub fn user_agent() -> String {
    format!(
        "Mozilla/5.0 (compatible; fflit/{}; +{HOMEPAGE}; mailto:{CONTACT_EMAIL})",
        env!("CARGO_PKG_VERSION")
    )
}

/// Shared, and with a cookie jar, so a landing page and the pdf request that
/// follows it are one visit rather than two strangers. Everything fflit sends
/// goes through here, so the user agent is set once and cannot drift.
pub fn client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(user_agent())
            .cookie_store(true)
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("building the http client")
    })
}

/// Ask for a file the way the click it stands in for would.
pub fn get_pdf(url: &str, referer: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let mut request = client()
        .get(url)
        .header("Accept", "application/pdf,*/*;q=0.8")
        .timeout(DOWNLOAD_TIMEOUT);
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

    #[test]
    fn fflit_says_who_it_is_and_how_to_be_rid_of_it() {
        let ua = user_agent();
        assert!(ua.contains(CONTACT_EMAIL), "{ua}");
        assert!(ua.contains(HOMEPAGE), "{ua}");
        // the version tracks Cargo.toml rather than a literal that goes stale
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "{ua}");
    }
}
