use crate::{bibtex, discover, metadata, pdf, search};
use anyhow::Context;
use colored::Colorize;
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

    // 2. What paper is this?
    let meta = match identify(path, doi_override, db) {
        Identified::Work { meta, how } => {
            eprintln!("  {how}");
            meta
        }
        Identified::Duplicate { doi } => {
            eprintln!("  duplicate DOI {doi} — moving to ./duplicates/");
            move_file(path, "./duplicates")?;
            return Ok(());
        }
        Identified::Unknown { why, guess } => {
            eprintln!("  {why} — moving to ./failed_pdfs/");
            if let Some(g) = guess {
                eprintln!("  {g}");
            }
            move_file(path, "./failed_pdfs")?;
            return Ok(());
        }
    };
    let doi_str = meta.doi.clone();

    // 3. BibTeX key
    let first_family = meta
        .authors
        .first()
        .and_then(|a| a.family.as_deref())
        .unwrap_or("Unknown");
    let year = meta.year.unwrap_or(0);
    let key = db.generate_key(first_family, year, &meta.title);

    // 4. Build fields in canonical order
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

    // 5. Move PDF into place
    let dest = PathBuf::from(format!("./pdfs/{}.pdf", key));
    std::fs::rename(path, &dest)
        .with_context(|| format!("moving {} to {}", path.display(), dest.display()))?;

    // 6. Add to database + index
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

/// The outcome of working out what a pdf is.
enum Identified {
    Work { meta: metadata::WorkMetadata, how: String },
    Duplicate { doi: String },
    Unknown { why: String, guess: Option<String> },
}

/// DOI printed in the file → DOI found in its text → asking CrossRef what the
/// title page is. Anything the file did not state about itself gets checked
/// against the title page before it is believed.
fn identify(
    path: &Path,
    doi_override: Option<&str>,
    db: &bibtex::BibDatabase,
) -> Identified {
    if let Some(doi) = doi_override {
        let doi = normalize_doi(doi);
        if db.contains_doi(&doi) {
            return Identified::Duplicate { doi };
        }
        return match metadata::fetch(&doi) {
            Ok(meta) => Identified::Work { meta, how: format!("doi {doi} (given)") },
            Err(e) => Identified::Unknown { why: format!("metadata fetch failed ({e})"), guess: None },
        };
    }

    // the title page is both a source of DOIs and the yardstick for everything
    // that is not printed in the file itself
    let front = pdf::page_text(path, 1, 1).unwrap_or_default();

    let mut rejected: Option<String> = None;
    match pdf::extract_doi(path) {
        Ok(hit) => {
            if db.contains_doi(&hit.doi) {
                return Identified::Duplicate { doi: hit.doi };
            }
            match metadata::fetch(&hit.doi) {
                Ok(meta) => {
                    if hit.trusted || discover::confirms(&meta, &front) {
                        let how = format!("doi {} ({})", hit.doi, hit.source);
                        return Identified::Work { meta, how };
                    }
                    // found in the body and it describes someone else's paper
                    rejected = Some(format!(
                        "{}: {} from {} looks like a citation, not this paper",
                        "note".yellow(),
                        hit.doi,
                        hit.source
                    ));
                }
                Err(e) => {
                    rejected = Some(format!("{}: {} did not resolve ({e})", "note".yellow(), hit.doi));
                }
            }
        }
        Err(_) => {}
    }
    if let Some(msg) = &rejected {
        eprintln!("  {msg}");
    }

    // 3. no usable DOI in the file: ask what this title page is
    let candidate = match discover::by_title_page(&front, pdf::info_title(path).as_deref()) {
        Ok(c) => c,
        Err(e) => {
            return Identified::Unknown { why: format!("no DOI in the pdf, and the search failed ({e})"), guess: None };
        }
    };

    let Some(candidate) = candidate else {
        return Identified::Unknown { why: "no DOI found, and no match on the title page".into(), guess: None };
    };

    if db.contains_doi(&candidate.meta.doi) {
        return Identified::Duplicate { doi: candidate.meta.doi };
    }

    if discover::accepted(&candidate) {
        let how = format!(
            "doi {} (title page, {:.0}% match to \"{}\")",
            candidate.meta.doi,
            candidate.score * 100.0,
            truncate(&candidate.meta.title, 60)
        );
        return Identified::Work { meta: candidate.meta, how };
    }

    // close enough to be worth a human glance, not close enough to file
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file.pdf");
    Identified::Unknown {
        why: "no DOI found, and the best title match is not convincing".into(),
        guess: Some(format!(
            "{}: {:.0}% {} \"{}\" — fflit add ./failed_pdfs/{} --doi {}",
            "guess".yellow(),
            candidate.score * 100.0,
            candidate.meta.doi.cyan(),
            truncate(&candidate.meta.title, 60),
            name,
            candidate.meta.doi
        )),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.chars().count() <= max {
        true => flat,
        false => flat.chars().take(max - 1).collect::<String>() + "…",
    }
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

pub fn format_authors(authors: &[metadata::Author]) -> String {
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
