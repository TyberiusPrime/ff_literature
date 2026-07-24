use anyhow::Context;
use colored::Colorize;
use std::path::{Path, PathBuf};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, SchemaBuilder, Value, STORED, STRING, TEXT};
use tantivy::{Index, IndexWriter, TantivyDocument};

pub struct SearchIndex {
    pub index: Index,
    pub schema: Schema,
}

fn build_schema() -> Schema {
    let mut b: SchemaBuilder = Schema::builder();
    b.add_text_field("bibkey", STRING | STORED);
    b.add_text_field("title", TEXT | STORED);
    b.add_text_field("authors", TEXT | STORED);
    b.add_text_field("fulltext", TEXT);
    // first ~5000 chars of extracted text, stored for context snippets
    b.add_text_field("preview", TEXT | STORED);
    b.build()
}

fn index_path() -> PathBuf {
    PathBuf::from("./search_index")
}

pub fn open_or_create() -> anyhow::Result<SearchIndex> {
    let path = index_path();
    std::fs::create_dir_all(&path)?;
    let schema = build_schema();
    let index = if path.join("meta.json").exists() {
        Index::open_in_dir(&path).context("opening tantivy index")?
    } else {
        Index::create_in_dir(&path, schema.clone()).context("creating tantivy index")?
    };
    Ok(SearchIndex { index, schema })
}

pub fn add_document(
    idx: &SearchIndex,
    bibkey: &str,
    title: &str,
    authors: &str,
    pdf_path: &Path,
) -> anyhow::Result<()> {
    let fulltext = extract_text(pdf_path).unwrap_or_default();
    let preview: String = fulltext.chars().take(5000).collect();

    let bibkey_f = idx.schema.get_field("bibkey").unwrap();
    let title_f = idx.schema.get_field("title").unwrap();
    let authors_f = idx.schema.get_field("authors").unwrap();
    let fulltext_f = idx.schema.get_field("fulltext").unwrap();
    let preview_f = idx.schema.get_field("preview").unwrap();

    let mut writer: IndexWriter = idx.index.writer(50_000_000)?;
    let mut doc = TantivyDocument::default();
    doc.add_text(bibkey_f, bibkey);
    doc.add_text(title_f, title);
    doc.add_text(authors_f, authors);
    doc.add_text(fulltext_f, &fulltext);
    doc.add_text(preview_f, &preview);
    writer.add_document(doc)?;
    writer.commit()?;
    Ok(())
}

pub fn search(query_str: &str, show_context: bool) -> anyhow::Result<()> {
    let idx = open_or_create()?;
    let reader = idx.index.reader()?;
    let searcher = reader.searcher();

    let title_f = idx.schema.get_field("title").unwrap();
    let authors_f = idx.schema.get_field("authors").unwrap();
    let fulltext_f = idx.schema.get_field("fulltext").unwrap();
    let bibkey_f = idx.schema.get_field("bibkey").unwrap();
    let preview_f = idx.schema.get_field("preview").unwrap();

    let query_parser =
        QueryParser::for_index(&idx.index, vec![title_f, authors_f, fulltext_f]);
    let query = query_parser
        .parse_query(query_str)
        .context("invalid query syntax")?;
    let top_docs = searcher.search(&query, &TopDocs::with_limit(20))?;

    let terms = simple_terms(query_str);
    let mut first = true;

    for (_score, addr) in top_docs {
        let doc: TantivyDocument = searcher.doc(addr)?;

        let key = doc.get_first(bibkey_f).and_then(|v| v.as_str()).unwrap_or("?");
        let title = doc.get_first(title_f).and_then(|v| v.as_str()).unwrap_or("");
        let authors = doc.get_first(authors_f).and_then(|v| v.as_str()).unwrap_or("");
        let first_family = first_family_name(authors);

        if !first {
            println!();
        }
        first = false;

        println!(
            "{}  {} — {}",
            format!("./pdfs/{key}.pdf").cyan(),
            title.bold(),
            first_family.yellow()
        );

        if show_context {
            let preview = doc.get_first(preview_f).and_then(|v| v.as_str()).unwrap_or("");
            for passage in context_passages(preview, &terms, 3, 80) {
                println!("    …{}…", highlight_terms(&passage, &terms));
            }
        }
    }
    Ok(())
}

pub fn reindex() -> anyhow::Result<()> {
    let path = index_path();
    if path.exists() {
        std::fs::remove_dir_all(&path)?;
    }
    std::fs::create_dir_all(&path)?;

    let schema = build_schema();
    let index = Index::create_in_dir(&path, schema.clone())?;
    let mut writer: IndexWriter = index.writer(50_000_000)?;

    let bibkey_f = schema.get_field("bibkey").unwrap();
    let title_f = schema.get_field("title").unwrap();
    let authors_f = schema.get_field("authors").unwrap();
    let fulltext_f = schema.get_field("fulltext").unwrap();
    let preview_f = schema.get_field("preview").unwrap();

    let db = crate::bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;
    let meta: std::collections::HashMap<&str, (&str, &str)> = db
        .entries
        .iter()
        .map(|e| {
            let title = e.fields.iter().find(|(k, _)| k == "title").map(|(_, v)| v.as_str()).unwrap_or("");
            let authors = e.fields.iter().find(|(k, _)| k == "author").map(|(_, v)| v.as_str()).unwrap_or("");
            (e.key.as_str(), (title, authors))
        })
        .collect();

    let pdfs_dir = Path::new("./pdfs");
    if !pdfs_dir.exists() {
        eprintln!("./pdfs does not exist, nothing to index");
        return Ok(());
    }

    let mut count = 0;
    for entry in walkdir::WalkDir::new(pdfs_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("pdf"))
    {
        let bibkey = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
        let (title, authors) = meta.get(bibkey).copied().unwrap_or(("", ""));
        let fulltext = extract_text(entry.path()).unwrap_or_default();
        let preview: String = fulltext.chars().take(5000).collect();

        let mut doc = TantivyDocument::default();
        doc.add_text(bibkey_f, bibkey);
        doc.add_text(title_f, title);
        doc.add_text(authors_f, authors);
        doc.add_text(fulltext_f, &fulltext);
        doc.add_text(preview_f, &preview);
        writer.add_document(doc)?;
        eprintln!("  indexed {bibkey}");
        count += 1;
    }

    writer.commit()?;
    eprintln!("reindex complete: {count} documents");
    Ok(())
}

fn extract_text(path: &Path) -> Option<String> {
    let output = std::process::Command::new(
        option_env!("NIX_PDF_TO_TEXT").unwrap_or("pdftotext")
    )
        .args([path.to_str()?, "-"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

// Extract up to `max` non-overlapping passages of ±`window` chars around any query term,
// snapping to word boundaries.
fn context_passages(text: &str, terms: &[String], max: usize, window: usize) -> Vec<String> {
    if terms.is_empty() || text.is_empty() {
        return vec![];
    }
    let lower = text.to_lowercase();

    let mut positions: Vec<usize> = terms
        .iter()
        .flat_map(|t| {
            let mut hits = Vec::new();
            let mut start = 0;
            while let Some(rel) = lower[start..].find(t.as_str()) {
                hits.push(start + rel);
                start += rel + t.len().max(1);
            }
            hits
        })
        .collect();

    positions.sort_unstable();

    let mut passages = Vec::new();
    let mut last_end: usize = 0;

    for pos in positions {
        if passages.len() >= max {
            break;
        }
        let start = word_boundary_back(text, pos.saturating_sub(window));
        let end = word_boundary_forward(text, (pos + window).min(text.len()));
        if start < last_end {
            continue;
        }
        let snippet = text[start..end].split_whitespace().collect::<Vec<_>>().join(" ");
        if !snippet.is_empty() {
            passages.push(snippet);
            last_end = end;
        }
    }
    passages
}

fn word_boundary_back(text: &str, from: usize) -> usize {
    // walk forward from `from` past any partial UTF-8 continuation bytes, then to next whitespace
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    // step forward to start of next word
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

fn word_boundary_forward(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = from.min(bytes.len());
    while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    // step back to end of previous word
    while i > 0 && i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

// Split query into lowercase alpha tokens for context matching.
fn simple_terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .collect()
}

fn highlight_terms(text: &str, terms: &[String]) -> String {
    if terms.is_empty() {
        return text.to_string();
    }
    let lower = text.to_lowercase();

    // collect (start, end) byte ranges for every term hit
    let mut ranges: Vec<(usize, usize)> = terms
        .iter()
        .flat_map(|t| {
            let mut hits = Vec::new();
            let mut start = 0;
            while let Some(rel) = lower[start..].find(t.as_str()) {
                let s = start + rel;
                hits.push((s, s + t.len()));
                start += rel + t.len().max(1);
            }
            hits
        })
        .collect();

    ranges.sort_unstable_by_key(|r| r.0);

    // merge overlapping/adjacent ranges
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }

    let mut out = String::with_capacity(text.len() + merged.len() * 16);
    let mut pos = 0;
    for (s, e) in merged {
        if pos < s {
            out.push_str(&text[pos..s]);
        }
        out.push_str(&text[s..e].bold().yellow().to_string());
        pos = e;
    }
    if pos < text.len() {
        out.push_str(&text[pos..]);
    }
    out
}

fn first_family_name(authors: &str) -> &str {
    // "Smith, John and Doe, Jane" → "Smith"
    authors
        .splitn(2, ',')
        .next()
        .map(str::trim)
        .unwrap_or(authors)
}
