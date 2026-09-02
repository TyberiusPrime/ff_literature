//! The publisher's own copy, for when the library has a subscription.
//!
//! Nothing here circumvents anything: it resolves the DOI, reads the
//! `citation_pdf_url` the publisher advertises for indexing, and asks for that
//! file. Off the subscribing network it gets a paywall page and gives up.

use regex::Regex;
use std::sync::OnceLock;
use std::time::Duration;

static PDF_META_RE: OnceLock<Regex> = OnceLock::new();

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";

/// `<meta name="citation_pdf_url" content="…">`, in either attribute order.
fn pdf_meta_regex() -> &'static Regex {
    PDF_META_RE.get_or_init(|| {
        Regex::new(
            r#"(?is)<meta[^>]*(?:name=["']citation_pdf_url["'][^>]*content=["']([^"']+)["']|content=["']([^"']+)["'][^>]*name=["']citation_pdf_url["'])"#,
        )
        .unwrap()
    })
}

/// What came of asking the publisher, including where a person should go when
/// a script cannot.
pub struct Probe {
    /// where the DOI actually landed, which is the page to open by hand
    pub landing_url: String,
    pub pdf_url: Option<String>,
    /// why there is no pdf url, in words, when there is none
    pub blocked: Option<&'static str>,
}

/// Resolve the DOI and read the pdf link the publisher advertises for indexing.
pub fn probe(doi: &str) -> anyhow::Result<Probe> {
    let response = reqwest::blocking::Client::new()
        .get(format!("https://doi.org/{doi}"))
        .header("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(60))
        .send()?;

    let status = response.status();
    // the url after every redirect: linkinghub for Elsevier, dl.acm.org for ACM
    let landing_url = clean_url(response.url().as_str());
    let html = response.text().unwrap_or_default();

    if let Some(url) = extract_pdf_url(&html) {
        return Ok(Probe { landing_url, pdf_url: Some(url), blocked: None });
    }
    Ok(Probe {
        landing_url,
        pdf_url: None,
        blocked: Some(diagnose(status, &html)),
    })
}

/// Why a landing page yielded nothing — worth saying, because the answer
/// decides whether opening it in a browser will work.
fn diagnose(status: reqwest::StatusCode, html: &str) -> &'static str {
    let head: String = html.chars().take(4000).collect::<String>().to_lowercase();
    if head.contains("just a moment") || head.contains("cf-browser-verification") || head.contains("challenge-platform") {
        return "bot challenge, opens fine in a browser";
    }
    if head.contains("window.location") || head.contains("<title>redirecting") {
        return "javascript redirect, opens fine in a browser";
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return "403 to a script, may open in a browser";
    }
    if status.is_success() {
        return "no pdf link advertised";
    }
    "publisher returned an error"
}

fn extract_pdf_url(html: &str) -> Option<String> {
    let c = pdf_meta_regex().captures(html)?;
    let raw = c.get(1).or_else(|| c.get(2))?.as_str();
    Some(decode_entities(raw))
}

/// Publishers hang session noise off the landing url — an error code from the
/// cookie check we did not do — which is stale by the time anyone clicks it.
fn clean_url(url: &str) -> String {
    const NOISE: &[&str] = &["error", "code", "cookieset", "cookies"];
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| {
            let name = p.split('=').next().unwrap_or("").to_lowercase();
            !NOISE.contains(&name.as_str())
        })
        .collect();
    match kept.is_empty() {
        true => base.to_string(),
        false => format!("{base}?{}", kept.join("&")),
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_is_read_in_either_attribute_order() {
        let plos = r#"<meta name="citation_pdf_url" content="https://journals.plos.org/x?id=10.1&type=printable">"#;
        assert_eq!(
            extract_pdf_url(plos).as_deref(),
            Some("https://journals.plos.org/x?id=10.1&type=printable")
        );
        let nature = r#"<meta content="https://www.nature.com/articles/x.pdf" name="citation_pdf_url"/>"#;
        assert_eq!(
            extract_pdf_url(nature).as_deref(),
            Some("https://www.nature.com/articles/x.pdf")
        );
    }

    #[test]
    fn escaped_ampersands_are_undone() {
        let html = r#"<meta name="citation_pdf_url" content="https://x.invalid/a?id=1&amp;type=pdf">"#;
        assert_eq!(extract_pdf_url(html).as_deref(), Some("https://x.invalid/a?id=1&type=pdf"));
    }

    #[test]
    fn session_noise_is_stripped_from_landing_urls() {
        assert_eq!(
            clean_url("https://www.nature.com/articles/nature14539?error=cookies_not_supported&code=aeafe650"),
            "https://www.nature.com/articles/nature14539"
        );
        // a query the page actually needs is left alone
        assert_eq!(
            clean_url("https://academic.oup.com/nar/article/49/D1/D480?login=true"),
            "https://academic.oup.com/nar/article/49/D1/D480?login=true"
        );
        assert_eq!(clean_url("https://dl.acm.org/doi/10.1145/x"), "https://dl.acm.org/doi/10.1145/x");
    }

    #[test]
    fn the_reason_is_named() {
        use reqwest::StatusCode;
        assert_eq!(
            diagnose(StatusCode::FORBIDDEN, "<title>Just a moment...</title>"),
            "bot challenge, opens fine in a browser"
        );
        assert_eq!(
            diagnose(StatusCode::OK, "<title>Redirecting</title><script>window.location=..."),
            "javascript redirect, opens fine in a browser"
        );
        assert_eq!(diagnose(StatusCode::OK, "<html>a paywall</html>"), "no pdf link advertised");
        assert_eq!(diagnose(StatusCode::FORBIDDEN, "nope"), "403 to a script, may open in a browser");
    }

    #[test]
    fn a_page_without_the_tag_offers_nothing() {
        assert_eq!(extract_pdf_url("<html><head><title>Paywall</title></head></html>"), None);
        // a different citation meta tag must not be mistaken for it
        assert_eq!(extract_pdf_url(r#"<meta name="citation_abstract_html_url" content="x">"#), None);
    }
}
