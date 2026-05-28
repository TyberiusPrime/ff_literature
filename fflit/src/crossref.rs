use anyhow::Context;
use regex::Regex;
use std::sync::OnceLock;

static TAG_RE: OnceLock<Regex> = OnceLock::new();

fn tag_regex() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

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

pub struct Author {
    pub family: Option<String>,
    pub given: Option<String>,
}

pub fn fetch(doi_str: &str) -> anyhow::Result<WorkMetadata> {
    let url = format!("https://api.crossref.org/works/{}", doi_str);
    let client = reqwest::blocking::Client::new();
    let response: serde_json::Value = client
        .get(&url)
        .header(
            "User-Agent",
            "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)",
        )
        .send()
        .with_context(|| format!("HTTP request failed for DOI {doi_str}"))?
        .error_for_status()
        .with_context(|| format!("CrossRef returned error for DOI {doi_str}"))?
        .json()
        .context("failed to parse CrossRef JSON")?;

    let msg = &response["message"];

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

    Ok(WorkMetadata {
        doi: doi_str.to_string(),
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
    })
}

fn map_type(t: &str) -> &'static str {
    match t {
        "journal-article" => "article",
        "proceedings-article" => "inproceedings",
        "book-chapter" => "incollection",
        "book" => "book",
        "dissertation" | "thesis" => "phdthesis",
        "report" | "report-component" => "techreport",
        _ => "misc",
    }
}
