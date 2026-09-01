use crate::metadata::{Author, WorkMetadata};
use anyhow::Context;
use regex::Regex;
use std::sync::OnceLock;

static TAG_RE: OnceLock<Regex> = OnceLock::new();

fn tag_regex() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";

pub fn fetch(doi_str: &str) -> anyhow::Result<WorkMetadata> {
    let url = format!("https://api.crossref.org/works/{}", doi_str);
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .with_context(|| format!("HTTP request failed for DOI {doi_str}"))?
        .error_for_status()
        .with_context(|| format!("CrossRef returned error for DOI {doi_str}"))?
        .json()
        .context("failed to parse CrossRef JSON")?;

    Ok(parse_work(&response["message"]))
}

/// Free text bibliographic search: how a pdf that never states its DOI can
/// still be identified. Best matches first, as CrossRef ranks them.
pub fn search(query: &str, rows: usize) -> anyhow::Result<Vec<WorkMetadata>> {
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get("https://api.crossref.org/works")
        .header("User-Agent", USER_AGENT)
        .query(&[
            ("query.bibliographic", query),
            ("rows", &rows.to_string()),
        ])
        .send()
        .context("HTTP request failed for CrossRef search")?
        .error_for_status()
        .context("CrossRef returned an error for the search")?
        .json()
        .context("failed to parse CrossRef JSON")?;

    Ok(response["message"]["items"]
        .as_array()
        .map(|items| items.iter().map(parse_work).collect())
        .unwrap_or_default())
}

/// Look a book up by ISBN. Returns `None` when CrossRef knows the ISBN only
/// through the chapters of an edited volume: a chapter record would describe
/// the wrong thing for a whole-book pdf.
pub fn fetch_isbn(isbn13: &str) -> anyhow::Result<Option<WorkMetadata>> {
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get("https://api.crossref.org/works")
        .header("User-Agent", USER_AGENT)
        .query(&[
            ("filter", format!("isbn:{isbn13}").as_str()),
            ("rows", "20"),
        ])
        .send()
        .with_context(|| format!("HTTP request failed for ISBN {isbn13}"))?
        .error_for_status()
        .with_context(|| format!("CrossRef returned an error for ISBN {isbn13}"))?
        .json()
        .context("failed to parse CrossRef JSON")?;

    let Some(items) = response["message"]["items"].as_array() else {
        return Ok(None);
    };
    Ok(items
        .iter()
        .find(|it| is_whole_book(it["type"].as_str().unwrap_or("")))
        .map(parse_work))
}

fn is_whole_book(t: &str) -> bool {
    matches!(t, "book" | "monograph" | "edited-book" | "reference-book" | "book-set")
}

fn parse_work(msg: &serde_json::Value) -> WorkMetadata {
    let title = msg["title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown Title")
        .to_string();

    let authors = msg["author"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|a| Author {
                    family: a["family"].as_str().map(str::to_string),
                    given: a["given"].as_str().map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();

    let year = msg["issued"]["date-parts"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|y| y.as_u64())
        .map(|y| y as u32);

    let entry_type = map_type(msg["type"].as_str().unwrap_or("misc")).to_string();

    let container_title = msg["container-title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let volume = msg["volume"].as_str().map(str::to_string);
    let issue = msg["issue"].as_str().map(str::to_string);
    let pages = msg["page"].as_str().map(str::to_string);
    let publisher = msg["publisher"].as_str().map(str::to_string);
    let abstract_text = msg["abstract"]
        .as_str()
        .map(|s| tag_regex().replace_all(s, "").into_owned());

    WorkMetadata {
        doi: msg["DOI"].as_str().unwrap_or_default().to_string(),
        isbn: msg["ISBN"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .and_then(crate::isbn::normalize),
        title,
        authors,
        year,
        entry_type,
        container_title,
        volume,
        issue,
        pages,
        publisher,
        abstract_text,
    }
}

fn map_type(t: &str) -> &'static str {
    match t {
        "journal-article" => "article",
        "proceedings-article" => "inproceedings",
        "book-chapter" => "incollection",
        "book" | "monograph" | "edited-book" | "reference-book" | "book-set" => "book",
        "dissertation" | "thesis" => "phdthesis",
        "report" | "report-component" => "techreport",
        _ => "misc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chapter_is_not_the_book() {
        assert!(is_whole_book("monograph"));
        assert!(is_whole_book("book"));
        assert!(is_whole_book("edited-book"));
        assert!(!is_whole_book("book-chapter"));
        assert!(!is_whole_book("journal-article"));
    }
}
