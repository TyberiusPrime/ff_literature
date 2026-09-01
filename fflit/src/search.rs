use anyhow::{bail, Context};
use colored::Colorize;
use tantivy::query::AllQuery;
use std::path::{Path, PathBuf};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, SchemaBuilder, Value, STORED, STRING, TEXT};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

pub struct SearchIndex {
    pub index: Index,
    pub schema: Schema,
}

fn build_schema() -> Schema {
    let mut b: SchemaBuilder = Schema::builder();
    b.add_text_field("bibkey", STRING | STORED);
    b.add_text_field("title", TEXT | STORED);
    b.add_text_field("authors", TEXT | STORED);
    b.add_text_field("keywords", TEXT | STORED);
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
    let index = if path.join("meta.json").exists() {
        Index::open_in_dir(&path).context("opening tantivy index")?
    } else {
        Index::create_in_dir(&path, build_schema()).context("creating tantivy index")?
    };
    // an index built by an older fflit has fewer fields than build_schema();
    // go by what is actually on disk and degrade until the next reindex
    let schema = index.schema();
    Ok(SearchIndex { index, schema })
}

/// Fields added after an index was first built are absent until `fflit reindex`.
fn optional_field(schema: &Schema, name: &str) -> Option<tantivy::schema::Field> {
    schema.get_field(name).ok()
}

pub fn add_document(
    idx: &SearchIndex,
    bibkey: &str,
    title: &str,
    authors: &str,
    pdf_path: &Path,
) -> anyhow::Result<()> {
    // a freshly filed paper has no keywords yet; they are hand written into
    // literature.bibtex and picked up by `fflit reindex --tags-only`
    let mut writer: IndexWriter = idx.index.writer(50_000_000)?;
    let doc = build_document(idx, bibkey, title, authors, "", pdf_path);
    writer.add_document(doc)?;
    writer.commit()?;
    Ok(())
}

/// Bring the indexed keywords in line with `literature.bibtex`, touching only
/// the documents whose tags actually changed. Everything else — full text above
/// all — is left alone, so this costs one `pdftotext` per edited entry rather
/// than one per paper.
pub fn reindex_tags() -> anyhow::Result<()> {
    let idx = open_or_create()?;
    let Some(keywords_f) = optional_field(&idx.schema, "keywords") else {
        bail!("this index predates keyword search — run `fflit reindex` once");
    };
    let bibkey_f = idx.schema.get_field("bibkey").unwrap();
    let title_f = idx.schema.get_field("title").unwrap();
    let authors_f = idx.schema.get_field("authors").unwrap();

    let db = crate::bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;

    let reader = idx.index.reader()?;
    let searcher = reader.searcher();
    let all = searcher.search(&AllQuery, &TopDocs::with_limit(searcher.num_docs().max(1) as usize))?;

    let mut stale: Vec<(String, String, String, String)> = Vec::new();
    let mut indexed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_score, addr) in all {
        let doc: TantivyDocument = searcher.doc(addr)?;
        let key = doc.get_first(bibkey_f).and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() {
            continue;
        }
        indexed.insert(key.to_string());
        let Some(entry) = db.get(key) else { continue };
        let wanted = entry.keywords();
        let have = crate::bibtex::split_keywords(
            doc.get_first(keywords_f).and_then(|v| v.as_str()).unwrap_or(""),
        );
        if same_tags(&wanted, &have) {
            continue;
        }
        stale.push((
            key.to_string(),
            doc.get_first(title_f).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            doc.get_first(authors_f).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            wanted.join(", "),
        ));
    }

    // an entry whose pdf is on disk but has no document is a gap a tags-only
    // pass cannot close; one without a pdf is simply not indexable
    let unindexed = db
        .entries
        .iter()
        .filter(|e| !indexed.contains(&e.key))
        .filter(|e| Path::new(&format!("./pdfs/{}.pdf", e.key)).exists())
        .count();

    if stale.is_empty() {
        eprintln!("{} documents, tags already up to date", indexed.len());
        report_unindexed(unindexed);
        return Ok(());
    }

    let mut writer: IndexWriter = idx.index.writer(50_000_000)?;
    let mut updated = 0usize;
    for (key, title, authors, keywords) in &stale {
        let pdf = PathBuf::from(format!("./pdfs/{key}.pdf"));
        if !pdf.exists() {
            eprintln!("  {}: no ./pdfs/{key}.pdf, skipped", "warning".yellow().bold());
            continue;
        }
        writer.delete_term(Term::from_field_text(bibkey_f, key));
        writer.add_document(build_document(&idx, key, title, authors, keywords, &pdf))?;
        eprintln!(
            "  {key} [{}]",
            match keywords.is_empty() {
                true => "no tags".to_string(),
                false => keywords.clone(),
            }
        );
        updated += 1;
    }
    writer.commit()?;

    eprintln!("{updated} of {} documents updated", indexed.len());
    report_unindexed(unindexed);
    Ok(())
}

fn report_unindexed(n: usize) {
    if n == 0 {
        return;
    }
    eprintln!(
        "{}: {} pdf(s) are not in the index at all — run a full `fflit reindex`",
        "note".yellow(),
        n
    );
}

/// Order and case of the keywords field are not meaningful.
fn same_tags(a: &[String], b: &[String]) -> bool {
    let norm = |v: &[String]| {
        let mut v: Vec<String> = v.iter().map(|t| t.to_lowercase()).collect();
        v.sort();
        v
    };
    norm(a) == norm(b)
}

fn build_document(
    idx: &SearchIndex,
    bibkey: &str,
    title: &str,
    authors: &str,
    keywords: &str,
    pdf_path: &Path,
) -> TantivyDocument {
    let fulltext = extract_text(pdf_path).unwrap_or_default();
    let preview: String = fulltext.chars().take(5000).collect();

    let mut doc = TantivyDocument::default();
    doc.add_text(idx.schema.get_field("bibkey").unwrap(), bibkey);
    doc.add_text(idx.schema.get_field("title").unwrap(), title);
    doc.add_text(idx.schema.get_field("authors").unwrap(), authors);
    doc.add_text(idx.schema.get_field("fulltext").unwrap(), &fulltext);
    doc.add_text(idx.schema.get_field("preview").unwrap(), &preview);
    if let Some(f) = optional_field(&idx.schema, "keywords") {
        doc.add_text(f, keywords);
    }
    doc
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

    let keywords_f = optional_field(&idx.schema, "keywords");
    let mut query_fields = vec![title_f, authors_f, fulltext_f];
    query_fields.extend(keywords_f);
    let query_parser = QueryParser::for_index(&idx.index, query_fields);
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

        let tags = keywords_f
            .and_then(|f| doc.get_first(f))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        println!(
            "{}  {} — {}{}",
            format!("./pdfs/{key}.pdf").cyan(),
            title.bold(),
            first_family.yellow(),
            match tags.is_empty() {
                true => String::new(),
                false => format!("  [{}]", tags.green()),
            }
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
    let keywords_f = schema.get_field("keywords").unwrap();
    let fulltext_f = schema.get_field("fulltext").unwrap();
    let preview_f = schema.get_field("preview").unwrap();

    let db = crate::bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;
    let meta: std::collections::HashMap<&str, (&str, &str, &str)> = db
        .entries
        .iter()
        .map(|e| {
            let title = e.field("title").unwrap_or("");
            let authors = e.field("author").unwrap_or("");
            let keywords = e.field("keywords").unwrap_or("");
            (e.key.as_str(), (title, authors, keywords))
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
        let (title, authors, keywords) = meta.get(bibkey).copied().unwrap_or(("", "", ""));
        let fulltext = extract_text(entry.path()).unwrap_or_default();
        let preview: String = fulltext.chars().take(5000).collect();

        let mut doc = TantivyDocument::default();
        doc.add_text(bibkey_f, bibkey);
        doc.add_text(title_f, title);
        doc.add_text(authors_f, authors);
        doc.add_text(keywords_f, keywords);
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
