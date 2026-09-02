//! The registry of last resort.
//!
//! Every DOI registration agency — CrossRef, DataCite, ISTIC, mEDRA, JaLC,
//! KISTI, Airiti — answers content negotiation at doi.org with CSL-JSON. One
//! fallback therefore covers the agencies fflit does not speak to directly,
//! without a new API per agency.

use crate::metadata::{author_from_name, strip_tags, Author, WorkMetadata};
use anyhow::Context;

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";
const CSL_JSON: &str = "application/vnd.citationstyles.csl+json";

pub fn fetch(doi_str: &str) -> anyhow::Result<WorkMetadata> {
    let url = format!("https://doi.org/{}", doi_str);
    let response = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", CSL_JSON)
        .send()
        .with_context(|| format!("HTTP request failed for DOI {doi_str}"))?
        .error_for_status()
        .with_context(|| format!("doi.org returned an error for DOI {doi_str}"))?;

    // a publisher that ignores the Accept header sends its landing page instead
    let body = response.text().context("reading the doi.org response")?;
    let csl: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("doi.org did not return metadata for {doi_str}"))?;

    Ok(parse(&csl, doi_str))
}

/// Which agency registered a DOI, for telling "nobody has metadata" apart from
/// "this DOI does not exist".
pub fn registration_agency(doi_str: &str) -> Option<String> {
    let url = format!("https://doi.org/ra/{}", doi_str);
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .ok()?
        .json()
        .ok()?;
    response
        .as_array()?
        .first()?
        .get("RA")?
        .as_str()
        .map(str::to_string)
}

fn parse(csl: &serde_json::Value, fallback_doi: &str) -> WorkMetadata {
    WorkMetadata {
        doi: csl["DOI"].as_str().unwrap_or(fallback_doi).to_string(),
        isbn: first_string(&csl["ISBN"]).as_deref().and_then(crate::isbn::normalize),
        title: first_string(&csl["title"]).unwrap_or_else(|| "Unknown Title".into()),
        authors: authors(&csl["author"]),
        year: year(csl),
        entry_type: map_type(csl["type"].as_str().unwrap_or("")).to_string(),
        container_title: first_string(&csl["container-title"]),
        volume: first_string(&csl["volume"]),
        issue: first_string(&csl["issue"]),
        pages: first_string(&csl["page"]),
        publisher: csl["publisher"].as_str().map(str::to_string),
        abstract_text: csl["abstract"].as_str().map(strip_tags),
    }
}

/// CSL fields are strings, but some agencies send single element arrays, and
/// numbers turn up where strings are expected.
fn first_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Array(a) => a.first().and_then(first_string),
        _ => None,
    }
}

fn authors(v: &serde_json::Value) -> Vec<Author> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .map(|a| match (a["family"].as_str(), a["given"].as_str()) {
                    (Some(f), g) => Author {
                        family: Some(f.to_string()),
                        given: g.map(str::to_string),
                    },
                    // "literal" for organisations, and agencies that put the
                    // whole name in "given"
                    (None, Some(g)) => author_from_name(g),
                    (None, None) => match a["literal"].as_str() {
                        Some(l) => author_from_name(l),
                        None => Author { family: None, given: None },
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `issued` first, falling back to whenever it was published in any form.
fn year(csl: &serde_json::Value) -> Option<u32> {
    for key in ["issued", "published-print", "published-online", "published", "created"] {
        let parts = &csl[key]["date-parts"];
        if let Some(y) = parts.as_array().and_then(|a| a.first()).and_then(|p| p.as_array()).and_then(|p| p.first()) {
            if let Some(y) = y.as_u64() {
                return Some(y as u32);
            }
        }
    }
    None
}

/// CSL type names are not CrossRef's.
fn map_type(t: &str) -> &'static str {
    match t {
        "article-journal" | "article-magazine" | "article-newspaper" => "article",
        "paper-conference" => "inproceedings",
        "chapter" => "incollection",
        "book" | "monograph" => "book",
        "thesis" => "phdthesis",
        "report" => "techreport",
        _ => "misc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_csl_record_becomes_an_entry() {
        // shaped after what ISTIC actually returns
        let csl = serde_json::json!({
            "DOI": "10.3978/j.issn.2218-676X.2015.01.02",
            "title": "The promise and challenge of ovarian cancer models",
            "type": "article-journal",
            "author": [{"given": "Noor Hasan"}],
            "container-title": "Translational Cancer Research",
            "volume": "4",
            "issue": "1",
            "page": "14",
            "issued": {"date-parts": [[2015, 1, 2]]}
        });
        let m = parse(&csl, "fallback");
        assert_eq!(m.doi, "10.3978/j.issn.2218-676X.2015.01.02");
        assert_eq!(m.entry_type, "article");
        assert_eq!(m.year, Some(2015));
        assert_eq!(m.volume.as_deref(), Some("4"));
        // the whole name arrived in "given"
        assert_eq!(m.authors[0].family.as_deref(), Some("Hasan"));
        assert_eq!(m.authors[0].given.as_deref(), Some("Noor"));
    }

    #[test]
    fn split_names_are_left_alone() {
        let csl = serde_json::json!({
            "author": [{"family": "Vaswani", "given": "Ashish"}, {"literal": "The LIGO Collaboration"}]
        });
        let a = authors(&csl["author"]);
        assert_eq!(a[0].family.as_deref(), Some("Vaswani"));
        assert_eq!(a[1].family.as_deref(), Some("Collaboration"));
    }

    #[test]
    fn a_year_is_found_wherever_the_agency_put_it() {
        assert_eq!(year(&serde_json::json!({"issued": {"date-parts": [[2015, 1]]}})), Some(2015));
        assert_eq!(year(&serde_json::json!({"published-print": {"date-parts": [[1998]]}})), Some(1998));
        assert_eq!(year(&serde_json::json!({"issued": {"date-parts": [[]]}})), None);
        assert_eq!(year(&serde_json::json!({})), None);
    }

    #[test]
    fn numbers_where_strings_were_expected() {
        assert_eq!(first_string(&serde_json::json!(4)).as_deref(), Some("4"));
        assert_eq!(first_string(&serde_json::json!(["Nature"])).as_deref(), Some("Nature"));
        assert_eq!(first_string(&serde_json::json!("Nature")).as_deref(), Some("Nature"));
        assert_eq!(first_string(&serde_json::json!(null)), None);
    }

    #[test]
    fn csl_types_map_to_bibtex_types() {
        assert_eq!(map_type("article-journal"), "article");
        assert_eq!(map_type("paper-conference"), "inproceedings");
        assert_eq!(map_type("chapter"), "incollection");
        assert_eq!(map_type("posted-content"), "misc");
    }
}
