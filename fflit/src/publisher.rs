//! The publisher's own copy, for when the library has a subscription.
//!
//! Nothing here circumvents anything: it resolves the DOI, then asks for the
//! pdf the way the "PDF" button on that page would — the `citation_pdf_url`
//! publishers advertise for indexing where there is one, and otherwise the url
//! the publisher's own scheme says the file has. Off the subscribing network
//! every one of those gets a paywall page and gives up.

use crate::http::{self, host_of};
use regex::Regex;
use std::sync::OnceLock;
use std::time::Duration;

static PDF_META_RE: OnceLock<Regex> = OnceLock::new();
static PII_RE: OnceLock<Regex> = OnceLock::new();

/// More than this many guesses at one publisher is hammering, not trying.
const MAX_CANDIDATES: usize = 3;

/// `<meta name="citation_pdf_url" content="…">`, in either attribute order.
fn pdf_meta_regex() -> &'static Regex {
    PDF_META_RE.get_or_init(|| {
        Regex::new(
            r#"(?is)<meta[^>]*(?:name=["']citation_pdf_url["'][^>]*content=["']([^"']+)["']|content=["']([^"']+)["'][^>]*name=["']citation_pdf_url["'])"#,
        )
        .unwrap()
    })
}

/// Elsevier's own id for an article, which is what its pdf url is built from.
fn pii_regex() -> &'static Regex {
    PII_RE.get_or_init(|| Regex::new(r#"(?i)(?:/pii/|["']pii["']\s*:\s*["'])([A-Z0-9]{10,})"#).unwrap())
}

/// What came of asking the publisher, including where a person should go when
/// a script cannot.
pub struct Probe {
    /// where the DOI actually landed, which is the page to open by hand
    pub landing_url: String,
    /// every url worth asking for, best first
    pub candidates: Vec<String>,
    /// why there is nothing to ask for, in words, when there is nothing
    pub blocked: Option<&'static str>,
}

/// Resolve the DOI and work out where its pdf lives.
pub fn probe(doi: &str) -> anyhow::Result<Probe> {
    let response = http::client()
        .get(format!("https://doi.org/{doi}"))
        // a landing page, not a file: ask for it as a browser would, since
        // some publishers serve their pdf-less mobile page otherwise
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .timeout(Duration::from_secs(60))
        .send()?;

    let status = response.status();
    // the url after every redirect: linkinghub for Elsevier, dl.acm.org for ACM
    let final_url = response.url().to_string();
    let landing_url = clean_url(&final_url);
    let html = response.text().unwrap_or_default();

    let candidates = candidates(&final_url, doi, &html);
    let blocked = candidates.is_empty().then(|| diagnose(status, &html));
    Ok(Probe { landing_url, candidates, blocked })
}

/// Ask for one candidate, as the click it stands in for. The landing page is
/// the referer because that is where a person would have clicked.
pub fn download(url: &str, referer: &str) -> anyhow::Result<Vec<u8>> {
    http::get_pdf(url, Some(referer))
}

/// Every url worth asking for, best first: what the page advertises, then what
/// the publisher's url scheme says the file is called. The second is the point
/// — plenty of publishers never advertise the tag, and their pdf is still one
/// predictable url away from the page you are already on.
fn candidates(landing: &str, doi: &str, html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(url) = extract_pdf_url(html) {
        out.push(absolute(landing, &url));
    }
    out.extend(by_url_scheme(landing, doi, html));

    let mut seen: Vec<String> = Vec::new();
    out.retain(|u| match seen.contains(u) {
        true => false,
        false => {
            seen.push(u.clone());
            true
        }
    });
    out.truncate(MAX_CANDIDATES);
    out
}

/// Where this publisher keeps its pdfs, given the page the DOI landed on.
fn by_url_scheme(landing: &str, doi: &str, html: &str) -> Vec<String> {
    let base = landing.split(['?', '#']).next().unwrap_or(landing).trim_end_matches('/');
    let host = host_of(base);
    let mut out = Vec::new();

    match host {
        h if h.ends_with("nature.com") => out.push(format!("{base}.pdf")),
        h if h.ends_with("link.springer.com") => {
            out.push(format!("https://link.springer.com/content/pdf/{doi}.pdf"));
        }
        // linkinghub is the redirect Elsevier sends scripts to; either way the
        // pdf is addressed by the PII rather than the doi
        h if h.ends_with("sciencedirect.com") || h.ends_with("elsevier.com") => {
            if let Some(pii) = pii(base, html) {
                out.push(format!(
                    "https://www.sciencedirect.com/science/article/pii/{pii}/pdfft?isDTMRedir=true&download=true"
                ));
            }
        }
        h if h.ends_with("mdpi.com") => out.push(format!("{base}/pdf")),
        h if h.ends_with("biorxiv.org") || h.ends_with("medrxiv.org") => {
            out.push(format!("{base}.full.pdf"));
        }
        h if h.ends_with("cambridge.org") => out.push(format!("{base}/pdf")),
        _ => {}
    }

    // the /doi/… shape, which Wiley, Taylor & Francis, Sage, ACS, Science, ACM
    // and a long tail of Atypon sites all share
    if let Some(url) = doi_path_pdf(base, doi) {
        out.push(url);
    }
    out
}

/// `…/doi/full/10.1/x` → `…/doi/pdf/10.1/x`, which is the link behind the PDF
/// button on every site built this way.
fn doi_path_pdf(base: &str, doi: &str) -> Option<String> {
    let (root, rest) = base.split_once("/doi/")?;
    // already the pdf url, so there is nothing to derive
    if rest.starts_with("pdf/") || rest.starts_with("pdfdirect/") {
        return None;
    }
    // Wiley's plain /doi/pdf/ is a javascript viewer; pdfdirect is the file
    let verb = match root.contains("wiley.com") {
        true => "pdfdirect",
        false => "pdf",
    };
    Some(format!("{root}/doi/{verb}/{doi}"))
}

fn pii(url: &str, html: &str) -> Option<String> {
    let found = pii_regex().captures(url).or_else(|| pii_regex().captures(html))?;
    Some(found.get(1)?.as_str().to_uppercase())
}

/// Publishers advertise the tag as a path as often as a url.
fn absolute(landing: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    let scheme = match landing.starts_with("http://") {
        true => "http",
        false => "https",
    };
    let host = host_of(landing);
    match url.starts_with('/') {
        true => format!("{scheme}://{host}{url}"),
        false => format!("{scheme}://{host}/{url}"),
    }
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

    #[test]
    fn a_page_that_advertises_nothing_still_offers_its_publishers_scheme() {
        // the tag is missing, but the pdf is one predictable url away
        assert_eq!(
            candidates("https://www.nature.com/articles/nature14539", "10.1038/nature14539", "<html>paywall</html>"),
            vec!["https://www.nature.com/articles/nature14539.pdf"]
        );
        assert_eq!(
            candidates("https://link.springer.com/article/10.1007/s00285-021-01606-1", "10.1007/s00285-021-01606-1", ""),
            vec!["https://link.springer.com/content/pdf/10.1007/s00285-021-01606-1.pdf"]
        );
        assert_eq!(
            candidates("https://www.mdpi.com/2072-6643/13/1/123", "10.3390/nu13010123", ""),
            vec!["https://www.mdpi.com/2072-6643/13/1/123/pdf"]
        );
    }

    #[test]
    fn the_advertised_tag_is_still_tried_first() {
        let html = r#"<meta name="citation_pdf_url" content="https://www.nature.com/articles/x.pdf">"#;
        let c = candidates("https://www.nature.com/articles/nature14539", "10.1038/nature14539", html);
        assert_eq!(c[0], "https://www.nature.com/articles/x.pdf");
        // and the scheme guess comes after it rather than instead of it
        assert_eq!(c[1], "https://www.nature.com/articles/nature14539.pdf");
    }

    #[test]
    fn the_doi_path_shape_covers_the_atypon_publishers() {
        assert_eq!(
            doi_path_pdf("https://www.tandfonline.com/doi/full/10.1080/1234", "10.1080/1234").as_deref(),
            Some("https://www.tandfonline.com/doi/pdf/10.1080/1234")
        );
        assert_eq!(
            doi_path_pdf("https://dl.acm.org/doi/10.1145/3292500", "10.1145/3292500").as_deref(),
            Some("https://dl.acm.org/doi/pdf/10.1145/3292500")
        );
        // Wiley's /doi/pdf/ is a viewer page; the file itself is pdfdirect
        assert_eq!(
            doi_path_pdf("https://onlinelibrary.wiley.com/doi/abs/10.1111/x", "10.1111/x").as_deref(),
            Some("https://onlinelibrary.wiley.com/doi/pdfdirect/10.1111/x")
        );
        // a url that is already the pdf has nothing to derive
        assert_eq!(doi_path_pdf("https://dl.acm.org/doi/pdf/10.1145/3292500", "10.1145/3292500"), None);
        assert_eq!(doi_path_pdf("https://www.nature.com/articles/nature14539", "10.1038/nature14539"), None);
    }

    #[test]
    fn elsevier_is_addressed_by_its_own_id() {
        // the redirect a script gets carries the PII in the url
        assert_eq!(
            candidates("https://linkinghub.elsevier.com/retrieve/pii/S0092867420301021", "10.1016/j.cell.2020.01.021", ""),
            vec!["https://www.sciencedirect.com/science/article/pii/S0092867420301021/pdfft?isDTMRedir=true&download=true"]
        );
        // and a rendered page carries it in the json it ships with
        assert_eq!(
            candidates("https://www.sciencedirect.com/science/article/abs/pii/x", "10.1016/j.cell.2020.01.021", r#"{"pii":"S0092867420301021","x":1}"#),
            vec!["https://www.sciencedirect.com/science/article/pii/S0092867420301021/pdfft?isDTMRedir=true&download=true"]
        );
    }

    #[test]
    fn a_relative_tag_is_made_absolute() {
        let html = r#"<meta name="citation_pdf_url" content="/content/pdf/10.1/x.pdf">"#;
        assert_eq!(
            candidates("https://x.invalid/article/10.1/x", "10.1/x", html)[0],
            "https://x.invalid/content/pdf/10.1/x.pdf"
        );
    }

    #[test]
    fn a_publisher_is_never_asked_more_than_three_times() {
        let html = r#"<meta name="citation_pdf_url" content="https://onlinelibrary.wiley.com/doi/epdf/10.1111/x">"#;
        let c = candidates("https://onlinelibrary.wiley.com/doi/full/10.1111/x", "10.1111/x", html);
        assert!(c.len() <= MAX_CANDIDATES, "{c:?}");
    }
}
