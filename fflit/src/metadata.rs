//! What fflit knows about a paper, and where it gets it.
//!
//! CrossRef covers the published literature. Everything else a researcher
//! collects — arXiv preprints above all — is registered with DataCite instead,
//! so a DOI that CrossRef does not know is not yet a dead end.

use crate::{crossref, datacite, doi_org, openlibrary};
use anyhow::bail;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone)]
pub struct WorkMetadata {
    /// empty when the work has no DOI — a book known only by its ISBN
    pub doi: String,
    pub isbn: Option<String>,
    pub title: String,
    pub authors: Vec<Author>,
    pub year: Option<u32>,
    pub entry_type: String,
    pub container_title: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub publisher: Option<String>,
    pub abstract_text: Option<String>,
}

#[derive(Clone)]
pub struct Author {
    pub family: Option<String>,
    pub given: Option<String>,
}


/// Resolve a DOI, trying the agency most likely to hold it and falling back to
/// content negotiation, which every registration agency answers.
pub fn fetch(doi: &str) -> anyhow::Result<WorkMetadata> {
    let mut tried: Vec<&str> = Vec::new();

    if !is_datacite_prefix(doi) {
        match crossref::fetch(doi) {
            Ok(meta) => return Ok(meta),
            Err(_) => tried.push("crossref"),
        }
    }
    match datacite::fetch(doi) {
        Ok(meta) => return Ok(meta),
        Err(_) => tried.push("datacite"),
    }
    match doi_org::fetch(doi) {
        Ok(meta) => return Ok(meta),
        Err(_) => tried.push("doi.org"),
    }

    // say whether the doi is unknown or merely unhelpful
    match doi_org::registration_agency(doi) {
        Some(ra) => bail!(
            "{doi} is registered with {ra}, but none of {} could describe it",
            tried.join(", ")
        ),
        None => bail!("{doi} is not a registered DOI"),
    }
}

/// "Family, Given" or "Given Family" — agencies disagree, and some send the
/// whole name in one field.
pub fn author_from_name(name: &str) -> Author {
    let name = name.trim();
    if let Some((family, given)) = name.split_once(',') {
        return Author {
            family: Some(family.trim().to_string()),
            given: Some(given.trim().to_string()).filter(|g| !g.is_empty()),
        };
    }
    match name.rsplit_once(' ') {
        Some((given, family)) => Author {
            family: Some(family.to_string()),
            given: Some(given.to_string()),
        },
        None => Author {
            family: Some(name.to_string()).filter(|n| !n.is_empty()),
            given: None,
        },
    }
}

/// Abstracts arrive as JATS or HTML.
pub fn strip_tags(s: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let re = TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    re.replace_all(s, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        // punctuation left behind where a title element used to be
        .trim_start_matches([';', ':', '.', ','])
        .trim()
        .to_string()
}

/// Resolve an ISBN. CrossRef has the academic presses and gives us a DOI as
/// well; everything else comes from OpenLibrary.
pub fn fetch_isbn(isbn13: &str) -> anyhow::Result<WorkMetadata> {
    match crossref::fetch_isbn(isbn13) {
        Ok(Some(meta)) => Ok(meta),
        // no record, or only the individual chapters of an edited volume, which
        // do not describe the book in hand
        Ok(None) => openlibrary::fetch(isbn13),
        Err(crossref_err) => openlibrary::fetch(isbn13).map_err(|ol_err| {
            anyhow::anyhow!("{crossref_err}; openlibrary also failed: {ol_err}")
        }),
    }
}

/// arXiv (10.48550) and Zenodo (10.5281) never appear in CrossRef.
fn is_datacite_prefix(doi: &str) -> bool {
    let doi = doi.to_lowercase();
    doi.starts_with("10.48550/") || doi.starts_with("10.5281/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_split_whichever_way_they_arrive() {
        let a = author_from_name("Vaswani, Ashish");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Vaswani"), Some("Ashish")));
        let a = author_from_name("Noor Hasan");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Hasan"), Some("Noor")));
        let a = author_from_name("Thomas S. Kuhn");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Kuhn"), Some("Thomas S.")));
        let a = author_from_name("Aristotle");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Aristotle"), None));
    }

    #[test]
    fn markup_does_not_belong_in_an_abstract() {
        assert_eq!(strip_tags("<jats:p>Hello   there</jats:p>"), "Hello there");
        // what is left when an empty title element is stripped out
        assert_eq!(strip_tags("<jats:title/>;The complexity"), "The complexity");
    }

    #[test]
    fn arxiv_and_zenodo_skip_crossref() {
        assert!(is_datacite_prefix("10.48550/arXiv.1706.03762"));
        assert!(is_datacite_prefix("10.5281/zenodo.1234567"));
        assert!(!is_datacite_prefix("10.1038/nature14539"));
    }
}
