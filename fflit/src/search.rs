use anyhow::Context;
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

    let bibkey_f = idx.schema.get_field("bibkey").unwrap();
    let title_f = idx.schema.get_field("title").unwrap();
    let authors_f = idx.schema.get_field("authors").unwrap();
    let fulltext_f = idx.schema.get_field("fulltext").unwrap();

    let mut writer: IndexWriter = idx.index.writer(50_000_000)?;
    let mut doc = TantivyDocument::default();
    doc.add_text(bibkey_f, bibkey);
    doc.add_text(title_f, title);
    doc.add_text(authors_f, authors);
    doc.add_text(fulltext_f, &fulltext);
    writer.add_document(doc)?;
    writer.commit()?;
    Ok(())
}

pub fn search(query_str: &str) -> anyhow::Result<()> {
    let idx = open_or_create()?;
    let reader = idx.index.reader()?;
    let searcher = reader.searcher();

    let title_f = idx.schema.get_field("title").unwrap();
    let authors_f = idx.schema.get_field("authors").unwrap();
    let fulltext_f = idx.schema.get_field("fulltext").unwrap();
    let bibkey_f = idx.schema.get_field("bibkey").unwrap();

    let query_parser =
        QueryParser::for_index(&idx.index, vec![title_f, authors_f, fulltext_f]);
    let query = query_parser
        .parse_query(query_str)
        .context("invalid query syntax")?;
    let top_docs = searcher.search(&query, &TopDocs::with_limit(20))?;

    for (_score, addr) in top_docs {
        let doc: TantivyDocument = searcher.doc(addr)?;
        let key = doc
            .get_first(bibkey_f)
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!("./pdfs/{}.pdf", key);
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

    // Load bibtex for title/author metadata
    let db = crate::bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;
    let meta: std::collections::HashMap<&str, (&str, String)> = db
        .entries
        .iter()
        .map(|e| {
            let title = e
                .fields
                .iter()
                .find(|(k, _)| k == "title")
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            let authors = e
                .fields
                .iter()
                .find(|(k, _)| k == "author")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
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
        let bibkey = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let (title, authors) = meta
            .get(bibkey)
            .map(|(t, a)| (*t, a.as_str()))
            .unwrap_or(("", ""));

        let fulltext = extract_text(entry.path()).unwrap_or_default();

        let mut doc = TantivyDocument::default();
        doc.add_text(bibkey_f, bibkey);
        doc.add_text(title_f, title);
        doc.add_text(authors_f, authors);
        doc.add_text(fulltext_f, &fulltext);
        writer.add_document(doc)?;
        eprintln!("  indexed {}", bibkey);
        count += 1;
    }

    writer.commit()?;
    eprintln!("reindex complete: {} documents", count);
    Ok(())
}

fn extract_text(path: &Path) -> Option<String> {
    let output = std::process::Command::new("pdftotext")
        .args([path.to_str()?, "-"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}
