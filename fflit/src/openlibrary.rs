//! OpenLibrary: the books no DOI registry knows about — technical, trade, and
//! most of what was printed before publishers started minting DOIs.

use crate::metadata::{Author, WorkMetadata};
use anyhow::{bail, Context};
use regex::Regex;
use std::sync::OnceLock;

static YEAR_RE: OnceLock<Regex> = OnceLock::new();

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";

pub fn fetch(isbn13: &str) -> anyhow::Result<WorkMetadata> {
    let key = format!("ISBN:{isbn13}");
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get("https://openlibrary.org/api/books")
        .header("User-Agent", USER_AGENT)
        .query(&[("bibkeys", key.as_str()), ("format", "json"), ("jscmd", "data")])
        .send()
        .with_context(|| format!("HTTP request failed for ISBN {isbn13}"))?
        .error_for_status()
        .with_context(|| format!("OpenLibrary returned an error for ISBN {isbn13}"))?
        .json()
        .context("failed to parse OpenLibrary JSON")?;

    let book = &response[&key];
    if book.is_null() {
        bail!("OpenLibrary has no record for ISBN {isbn13}");
    }
    Ok(parse(book, isbn13))
}

fn parse(book: &serde_json::Value, isbn13: &str) -> WorkMetadata {
    let mut title = book["title"].as_str().unwrap_or("Unknown Title").to_string();
    if let Some(sub) = book["subtitle"].as_str() {
        title = format!("{title}: {sub}");
    }

    let authors = book["authors"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|a| a["name"].as_str()).map(split_name).collect())
        .unwrap_or_default();

    WorkMetadata {
        doi: String::new(),
        isbn: Some(isbn13.to_string()),
        title,
        authors,
        year: book["publish_date"].as_str().and_then(year_in),
        entry_type: "book".into(),
        container_title: None,
        volume: None,
        issue: None,
        pages: None,
        publisher: book["publishers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|p| p["name"].as_str())
            .map(str::to_string),
        abstract_text: book["notes"].as_str().map(str::to_string),
    }
}

/// OpenLibrary writes names out in full: "Vince Buffalo".
fn split_name(name: &str) -> Author {
    match name.rsplit_once(' ') {
        Some((given, family)) => Author {
            family: Some(family.to_string()),
            given: Some(given.to_string()),
        },
        None => Author { family: Some(name.to_string()), given: None },
    }
}

/// `publish_date` is free text: "2015", "August 2015", "May 3, 2015".
fn year_in(date: &str) -> Option<u32> {
    YEAR_RE
        .get_or_init(|| Regex::new(r"\b(1[5-9]\d{2}|20\d{2})\b").unwrap())
        .captures(date)
        .and_then(|c| c[1].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_year_is_found_in_whatever_the_date_looks_like() {
        assert_eq!(year_in("2015"), Some(2015));
        assert_eq!(year_in("August 2015"), Some(2015));
        assert_eq!(year_in("May 3, 2015"), Some(2015));
        assert_eq!(year_in("1962"), Some(1962));
        assert_eq!(year_in("no date"), None);
    }

    #[test]
    fn full_names_are_split_at_the_last_space() {
        let a = split_name("Vince Buffalo");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Buffalo"), Some("Vince")));
        let a = split_name("Thomas S. Kuhn");
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Kuhn"), Some("Thomas S.")));
    }

    #[test]
    fn a_subtitle_joins_the_title() {
        let book = serde_json::json!({
            "title": "Bioinformatics Data Skills",
            "subtitle": "Reproducible and Robust Research",
            "publish_date": "2015",
            "publishers": [{"name": "O'Reilly Media"}],
            "authors": [{"name": "Vince Buffalo"}]
        });
        let m = parse(&book, "9781449367374");
        assert_eq!(m.title, "Bioinformatics Data Skills: Reproducible and Robust Research");
        assert_eq!(m.entry_type, "book");
        assert_eq!(m.isbn.as_deref(), Some("9781449367374"));
        assert!(m.doi.is_empty());
    }
}
