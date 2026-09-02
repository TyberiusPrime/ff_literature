//! Unpaywall: where a legally free copy of a paper lives, if one does.
//!
//! It knows about publisher copies and repository deposits alike, and returns
//! every location it has, which matters because the first one is not always the
//! one that will actually hand over a file.

use anyhow::Context;

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";
const EMAIL: &str = "john@coonabibba.de";

pub struct OaCopy {
    pub url: String,
    pub version: String,
    pub host: String,
    /// a link Unpaywall says is the pdf itself, rather than a page about it
    pub direct: bool,
}

impl OaCopy {
    pub fn describe(&self) -> String {
        match self.direct {
            true => format!("{} {}", self.host, friendly_version(&self.version)),
            false => format!("{} landing page", self.host),
        }
    }
}

/// Every downloadable copy Unpaywall knows of, best first. Empty when the paper
/// is closed access — which is not an error, just an answer.
pub fn pdf_locations(doi: &str) -> anyhow::Result<Vec<OaCopy>> {
    let url = format!("https://api.unpaywall.org/v2/{doi}");
    let response = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .query(&[("email", EMAIL)])
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .with_context(|| format!("HTTP request failed for DOI {doi}"))?;

    // a DOI Unpaywall has never heard of is a 404, not a failure of ours
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }
    let json: serde_json::Value = response
        .error_for_status()
        .with_context(|| format!("Unpaywall returned an error for DOI {doi}"))?
        .json()
        .context("failed to parse Unpaywall JSON")?;

    let mut copies: Vec<OaCopy> = Vec::new();
    for l in json["oa_locations"].as_array().unwrap_or(&vec![]) {
        let version = l["version"].as_str().unwrap_or("").to_string();
        let host = l["host_type"].as_str().unwrap_or("unknown").to_string();
        if let Some(pdf) = l["url_for_pdf"].as_str() {
            copies.push(OaCopy { url: pdf.to_string(), version: version.clone(), host: host.clone(), direct: true });
        }
        // some deposits list only a landing page; it is sometimes the file
        // itself, and the download is checked for being a pdf either way
        if let Some(url) = l["url"].as_str() {
            if l["url_for_pdf"].as_str() != Some(url) {
                copies.push(OaCopy { url: url.to_string(), version, host, direct: false });
            }
        }
    }

    copies.sort_by_key(|c| (!c.direct, version_rank(&c.version), host_rank(&c.host)));
    copies.dedup_by(|a, b| a.url == b.url);
    Ok(copies)
}

/// The published version is the one worth having; a preprint is better than
/// nothing.
fn version_rank(version: &str) -> u8 {
    match version {
        "publishedVersion" => 0,
        "acceptedVersion" => 1,
        "submittedVersion" => 2,
        _ => 3,
    }
}

/// Repositories hand over files to a script; publishers often will not.
fn host_rank(host: &str) -> u8 {
    match host {
        "repository" => 0,
        "publisher" => 1,
        _ => 2,
    }
}

fn friendly_version(version: &str) -> &str {
    match version {
        "publishedVersion" => "published version",
        "acceptedVersion" => "accepted manuscript",
        "submittedVersion" => "preprint",
        _ => "unknown version",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_version_comes_first() {
        let mut copies = vec![
            OaCopy { url: "c".into(), version: "submittedVersion".into(), host: "repository".into(), direct: true },
            OaCopy { url: "a".into(), version: "publishedVersion".into(), host: "publisher".into(), direct: true },
            OaCopy { url: "b".into(), version: "acceptedVersion".into(), host: "repository".into(), direct: true },
        ];
        copies.sort_by_key(|c| (!c.direct, version_rank(&c.version), host_rank(&c.host)));
        assert_eq!(copies.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_repository_is_tried_before_a_publisher_of_the_same_version() {
        let mut copies = vec![
            OaCopy { url: "publisher".into(), version: "publishedVersion".into(), host: "publisher".into(), direct: true },
            OaCopy { url: "repo".into(), version: "publishedVersion".into(), host: "repository".into(), direct: true },
        ];
        copies.sort_by_key(|c| (!c.direct, version_rank(&c.version), host_rank(&c.host)));
        assert_eq!(copies[0].url, "repo");
    }

    #[test]
    fn a_direct_pdf_link_beats_any_landing_page() {
        let mut copies = vec![
            OaCopy { url: "landing".into(), version: "publishedVersion".into(), host: "publisher".into(), direct: false },
            OaCopy { url: "pdf".into(), version: "submittedVersion".into(), host: "repository".into(), direct: true },
        ];
        copies.sort_by_key(|c| (!c.direct, version_rank(&c.version), host_rank(&c.host)));
        assert_eq!(copies[0].url, "pdf");
    }

    #[test]
    fn versions_are_described_in_words() {
        let c = OaCopy { url: "u".into(), version: "acceptedVersion".into(), host: "repository".into(), direct: true };
        assert_eq!(c.describe(), "repository accepted manuscript");
        let c = OaCopy { url: "u".into(), version: "publishedVersion".into(), host: "publisher".into(), direct: false };
        assert_eq!(c.describe(), "publisher landing page");
    }
}
