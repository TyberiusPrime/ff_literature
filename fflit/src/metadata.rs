//! What fflit knows about a paper, and where it gets it.
//!
//! CrossRef covers the published literature. Everything else a researcher
//! collects — arXiv preprints above all — is registered with DataCite instead,
//! so a DOI that CrossRef does not know is not yet a dead end.

use crate::{crossref, datacite};

#[derive(Clone)]
pub struct WorkMetadata {
    pub doi: String,
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


/// Resolve a DOI to metadata, asking whichever registry is likely to have it.
pub fn fetch(doi: &str) -> anyhow::Result<WorkMetadata> {
    if is_datacite_prefix(doi) {
        return datacite::fetch(doi);
    }
    match crossref::fetch(doi) {
        Ok(meta) => Ok(meta),
        Err(crossref_err) => datacite::fetch(doi).map_err(|datacite_err| {
            anyhow::anyhow!("{crossref_err}; datacite also failed: {datacite_err}")
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
    fn arxiv_and_zenodo_skip_crossref() {
        assert!(is_datacite_prefix("10.48550/arXiv.1706.03762"));
        assert!(is_datacite_prefix("10.5281/zenodo.1234567"));
        assert!(!is_datacite_prefix("10.1038/nature14539"));
    }
}
