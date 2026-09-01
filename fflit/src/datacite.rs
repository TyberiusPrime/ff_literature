//! DataCite: arXiv preprints, Zenodo deposits, datasets — everything CrossRef
//! does not register.

use crate::metadata::{Author, WorkMetadata};
use anyhow::Context;

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";

pub fn fetch(doi_str: &str) -> anyhow::Result<WorkMetadata> {
    let url = format!("https://api.datacite.org/dois/{}", doi_str);
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .with_context(|| format!("HTTP request failed for DOI {doi_str}"))?
        .error_for_status()
        .with_context(|| format!("DataCite returned error for DOI {doi_str}"))?
        .json()
        .context("failed to parse DataCite JSON")?;

    Ok(parse(&response["data"]["attributes"], doi_str))
}

fn parse(attr: &serde_json::Value, fallback_doi: &str) -> WorkMetadata {
    let title = attr["titles"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t["title"].as_str())
        .unwrap_or("Unknown Title")
        .to_string();

    let authors = attr["creators"]
        .as_array()
        .map(|arr| arr.iter().map(parse_creator).collect())
        .unwrap_or_default();

    let year = attr["publicationYear"].as_u64().map(|y| y as u32);

    let abstract_text = attr["descriptions"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|d| d["descriptionType"].as_str() == Some("Abstract"))
                .or_else(|| arr.first())
        })
        .and_then(|d| d["description"].as_str())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "));

    WorkMetadata {
        doi: attr["doi"].as_str().unwrap_or(fallback_doi).to_string(),
        isbn: None,
        title,
        authors,
        year,
        entry_type: map_type(attr["types"]["resourceTypeGeneral"].as_str().unwrap_or("")).to_string(),
        container_title: attr["container"]["title"].as_str().map(str::to_string),
        volume: attr["container"]["volume"].as_str().map(str::to_string),
        issue: attr["container"]["issue"].as_str().map(str::to_string),
        pages: attr["container"]["firstPage"].as_str().map(str::to_string),
        publisher: publisher(attr),
        abstract_text,
    }
}

/// DataCite 4.5 made `publisher` an object; older records have a plain string.
fn publisher(attr: &serde_json::Value) -> Option<String> {
    attr["publisher"]
        .as_str()
        .or_else(|| attr["publisher"]["name"].as_str())
        .map(str::to_string)
}

/// Creators carry either split names or one "Family, Given" string.
fn parse_creator(c: &serde_json::Value) -> Author {
    if let Some(family) = c["familyName"].as_str() {
        return Author {
            family: Some(family.to_string()),
            given: c["givenName"].as_str().map(str::to_string),
        };
    }
    let Some(name) = c["name"].as_str() else {
        return Author { family: None, given: None };
    };
    match name.split_once(',') {
        Some((family, given)) => Author {
            family: Some(family.trim().to_string()),
            given: Some(given.trim().to_string()),
        },
        // "Ashish Vaswani" — assume the last word is the family name
        None => match name.rsplit_once(' ') {
            Some((given, family)) => Author {
                family: Some(family.to_string()),
                given: Some(given.to_string()),
            },
            None => Author { family: Some(name.to_string()), given: None },
        },
    }
}

fn map_type(t: &str) -> &'static str {
    match t {
        "JournalArticle" => "article",
        "ConferencePaper" | "ConferenceProceeding" => "inproceedings",
        "BookChapter" => "incollection",
        "Book" => "book",
        "Dissertation" => "phdthesis",
        "Report" => "techreport",
        // a preprint is not published; @misc is the honest entry type
        _ => "misc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creators_come_in_three_shapes() {
        let split = serde_json::json!({"familyName": "Vaswani", "givenName": "Ashish"});
        let a = parse_creator(&split);
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Vaswani"), Some("Ashish")));

        let comma = serde_json::json!({"name": "Vaswani, Ashish"});
        let a = parse_creator(&comma);
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Vaswani"), Some("Ashish")));

        let plain = serde_json::json!({"name": "Ashish Vaswani"});
        let a = parse_creator(&plain);
        assert_eq!((a.family.as_deref(), a.given.as_deref()), (Some("Vaswani"), Some("Ashish")));
    }

    #[test]
    fn publisher_survived_the_schema_change() {
        assert_eq!(publisher(&serde_json::json!({"publisher": "arXiv"})).as_deref(), Some("arXiv"));
        assert_eq!(
            publisher(&serde_json::json!({"publisher": {"name": "arXiv"}})).as_deref(),
            Some("arXiv")
        );
    }

    #[test]
    fn a_preprint_is_misc_not_article() {
        assert_eq!(map_type("Preprint"), "misc");
        assert_eq!(map_type("JournalArticle"), "article");
    }
}
