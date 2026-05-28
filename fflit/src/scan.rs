use crate::{bibtex, crossref, pdf, search};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn scan() -> anyhow::Result<()> {
    ensure_dirs()?;
    let mut db = bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;
    let idx = search::open_or_create()?;

    let incoming = PathBuf::from("./incoming");
    let pdfs: Vec<PathBuf> = std::fs::read_dir(&incoming)
        .with_context(|| "reading ./incoming")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("pdf"))
        .collect();

    for path in pdfs {
        eprintln!("processing: {}", path.display());
        match process_pdf(&path, None, &mut db, &idx) {
            Ok(()) => {}
            Err(e) => eprintln!("  error: {e}"),
        }
    }

    db.write(Path::new("./literature.bibtex"))?;
    Ok(())
}

pub fn add_with_doi(path: &Path, doi: &str) -> anyhow::Result<()> {
    ensure_dirs()?;
    let mut db = bibtex::BibDatabase::load(Path::new("./literature.bibtex"))?;
    let idx = search::open_or_create()?;
    process_pdf(path, Some(doi), &mut db, &idx)?;
    db.write(Path::new("./literature.bibtex"))?;
    Ok(())
}

fn process_pdf(
    path: &Path,
    doi_override: Option<&str>,
    db: &mut bibtex::BibDatabase,
    idx: &search::SearchIndex,
) -> anyhow::Result<()> {
    // 1. SHA-256 duplicate check
    let sha = sha256_file(path)?;
    if db.contains_sha256(&sha) {
        eprintln!("  duplicate (sha256) — moving to ./duplicates/");
        move_file(path, "./duplicates")?;
        return Ok(());
    }

    // 2. DOI
    let doi_str = match doi_override {
        Some(d) => normalize_doi(d),
        None => match pdf::extract_doi(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  no DOI found ({e}) — moving to ./failed_pdfs/");
                move_file(path, "./failed_pdfs")?;
                return Ok(());
            }
        },
    };

    if db.contains_doi(&doi_str) {
        eprintln!("  duplicate DOI {doi_str} — moving to ./duplicates/");
        move_file(path, "./duplicates")?;
        return Ok(());
    }

    // 3. CrossRef metadata
    let meta = match crossref::fetch(&doi_str) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("  metadata fetch failed ({e}) — moving to ./failed_pdfs/");
            move_file(path, "./failed_pdfs")?;
            return Ok(());
        }
    };

    // 4. BibTeX key
    let first_family = meta
        .authors
        .first()
        .and_then(|a| a.family.as_deref())
        .unwrap_or("Unknown");
    let year = meta.year.unwrap_or(0);
    let key = db.generate_key(first_family, year, &meta.title);

    // 5. Build fields in canonical order
    let mut fields: Vec<(String, String)> = Vec::new();
    fields.push(("author".into(), format_authors(&meta.authors)));
    fields.push(("title".into(), meta.title.clone()));
    if let Some(y) = meta.year {
        fields.push(("year".into(), y.to_string()));
    }
    fields.push(("doi".into(), doi_str.clone()));

    let ct_key = match meta.entry_type.as_str() {
        "inproceedings" | "incollection" => "booktitle",
        _ => "journal",
    };
    if let Some(ct) = &meta.container_title {
        fields.push((ct_key.into(), ct.clone()));
    }
    if let Some(v) = &meta.volume {
        fields.push(("volume".into(), v.clone()));
    }
    if let Some(n) = &meta.issue {
        fields.push(("number".into(), n.clone()));
    }
    if let Some(p) = &meta.pages {
        fields.push(("pages".into(), p.clone()));
    }
    if let Some(pub_) = &meta.publisher {
        fields.push(("publisher".into(), pub_.clone()));
    }
    if let Some(abs) = &meta.abstract_text {
        fields.push(("abstract".into(), abs.clone()));
    }
    fields.push(("sha256".into(), sha));

    // 6. Move PDF into place
    let dest = PathBuf::from(format!("./pdfs/{}.pdf", key));
    std::fs::rename(path, &dest)
        .with_context(|| format!("moving {} to {}", path.display(), dest.display()))?;

    // 7. Add to database + index
    db.add(bibtex::BibEntry {
        entry_type: meta.entry_type.clone(),
        key: key.clone(),
        fields,
    });

    let authors_str = meta
        .authors
        .iter()
        .filter_map(|a| a.family.as_deref())
        .collect::<Vec<_>>()
        .join(", ");
    search::add_document(idx, &key, &meta.title, &authors_str, &dest)?;

    eprintln!("  added {doi_str} → ./pdfs/{key}.pdf");
    Ok(())
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn move_file(path: &Path, dest_dir: &str) -> anyhow::Result<()> {
    let fname = path
        .file_name()
        .with_context(|| "path has no filename")?;
    let dest = Path::new(dest_dir).join(fname);
    std::fs::rename(path, &dest)
        .with_context(|| format!("moving {} to {}", path.display(), dest.display()))?;
    Ok(())
}

fn ensure_dirs() -> anyhow::Result<()> {
    for dir in &["./incoming", "./pdfs", "./failed_pdfs", "./duplicates"] {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

pub fn format_authors(authors: &[crossref::Author]) -> String {
    authors
        .iter()
        .map(|a| match (&a.family, &a.given) {
            (Some(f), Some(g)) => format!("{f}, {g}"),
            (Some(f), None) => f.clone(),
            (None, Some(g)) => g.clone(),
            (None, None) => "Unknown".into(),
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

fn normalize_doi(s: &str) -> String {
    s.trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:")
        .to_string()
}
