//! Compare two bibtex files: which entries of the first are missing from the second?
//!
//! Matching is deliberately conservative. An entry is only reported as *missing*
//! when nothing in the second file looks anything like it — everything with a
//! plausible counterpart lands in the "uncertain" bucket for a human to look at.

use crate::bibtex::{BibDatabase, BibEntry};
use crate::text::{containment, extract_year, family_eq, first_author_family, normalize_text, token_set};
use anyhow::{bail, Context};
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// containment score above which two titles are worth a second look
const CANDIDATE_SCORE: f32 = 0.6;
/// lower bar, but only when first author *and* year agree as well
const CANDIDATE_SCORE_WITH_AUTHOR_YEAR: f32 = 0.34;
/// how many candidates to show per uncertain entry
const MAX_CANDIDATES: usize = 3;

pub fn diff(
    a_path: &Path,
    b_path: &Path,
    output: Option<&Path>,
    include_uncertain: bool,
) -> anyhow::Result<()> {
    let a_file = resolve(a_path)?;
    let b_file = resolve(b_path)?;
    let a = BibDatabase::load(&a_file)?;
    let b = BibDatabase::load(&b_file)?;

    let index = Index::build(&b.entries);

    let mut matched_doi = 0usize;
    let mut matched_isbn = 0usize;
    let mut matched_title = 0usize;
    let mut uncertain: Vec<(&BibEntry, Vec<Candidate>)> = Vec::new();
    let mut missing: Vec<&BibEntry> = Vec::new();

    for entry in &a.entries {
        match classify(entry, &b, &index) {
            Verdict::Matched(Reason::Doi) => matched_doi += 1,
            Verdict::Matched(Reason::Isbn) => matched_isbn += 1,
            Verdict::Matched(Reason::Title) => matched_title += 1,
            Verdict::Uncertain(cands) => uncertain.push((entry, cands)),
            Verdict::Missing => missing.push(entry),
        }
    }

    report(
        &a_file,
        &b_file,
        a.entries.len(),
        b.entries.len(),
        matched_doi,
        matched_isbn,
        matched_title,
        &uncertain,
        &missing,
    );

    if let Some(out) = output {
        let mut db = BibDatabase::empty();
        for e in &missing {
            db.add((*e).clone());
        }
        if include_uncertain {
            for (e, _) in &uncertain {
                db.add((*e).clone());
            }
        }
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        db.write(out)?;
        eprintln!(
            "\n{} entries → {}",
            db.entries.len(),
            out.display().to_string().cyan()
        );
    }

    Ok(())
}

/// Accept either a bibtex file or an fflit repository directory. Unlike the
/// scan path, a missing file is an error here — silently treating it as empty
/// would report the whole of `a` as missing.
fn resolve(path: &Path) -> anyhow::Result<PathBuf> {
    let file = if path.is_dir() {
        path.join("literature.bibtex")
    } else {
        path.to_path_buf()
    };
    if !file.exists() {
        bail!("{} does not exist", file.display());
    }
    Ok(file)
}

enum Reason {
    Doi,
    Isbn,
    Title,
}

enum Verdict {
    Matched(Reason),
    Uncertain(Vec<Candidate>),
    Missing,
}

pub struct Candidate {
    key: String,
    title: String,
    score: f32,
    why: &'static str,
}

fn classify(entry: &BibEntry, b: &BibDatabase, index: &Index) -> Verdict {
    if let Some(doi) = entry.field("doi") {
        if b.contains_doi(doi) {
            return Verdict::Matched(Reason::Doi);
        }
    }
    // books are identified by isbn rather than doi
    if let Some(isbn) = entry.field("isbn") {
        if b.contains_isbn(isbn) {
            return Verdict::Matched(Reason::Isbn);
        }
    }

    let title = entry.field("title").unwrap_or_default();
    let norm = normalize_text(title);
    if !norm.is_empty() {
        if let Some(hits) = index.by_title.get(&norm) {
            if !hits.is_empty() {
                return Verdict::Matched(Reason::Title);
            }
        }
    }

    let tokens = token_set(&norm);
    let author = entry.field("author").and_then(first_author_family);
    let year = entry.field("year").and_then(extract_year);

    let mut cands: Vec<Candidate> = Vec::new();
    for &i in &index.candidates(&tokens, author.as_deref(), year) {
        let other = &index.entries[i];
        let score = containment(&tokens, &other.tokens);
        let same_author = match (&author, &other.author) {
            (Some(x), Some(y)) => family_eq(x, y),
            _ => false,
        };
        let same_year = match (year, other.year) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
        let threshold = if same_author && same_year {
            CANDIDATE_SCORE_WITH_AUTHOR_YEAR
        } else {
            CANDIDATE_SCORE
        };
        if score >= threshold {
            cands.push(Candidate {
                key: other.key.clone(),
                title: other.title.clone(),
                score,
                why: match (same_author, same_year) {
                    (true, true) => "title, author, year",
                    (true, false) => "title, author",
                    (false, true) => "title, year",
                    (false, false) => "title",
                },
            });
        }
    }

    // a key that exists verbatim on the other side is worth showing even when
    // the titles disagree — different exports, same underlying paper
    if cands.is_empty() {
        if let Some(other) = b.get(&entry.key) {
            cands.push(Candidate {
                key: other.key.clone(),
                title: other.field("title").unwrap_or_default().to_string(),
                score: 0.0,
                why: "same citation key",
            });
        }
    }

    if cands.is_empty() {
        return Verdict::Missing;
    }
    cands.sort_by(|x, y| y.score.total_cmp(&x.score));
    cands.truncate(MAX_CANDIDATES);
    Verdict::Uncertain(cands)
}

// ---------------------------------------------------------------- index

struct Indexed {
    key: String,
    title: String,
    tokens: HashSet<String>,
    author: Option<String>,
    year: Option<u32>,
}

struct Index {
    entries: Vec<Indexed>,
    /// normalized title (stop words included) → entries carrying it
    by_title: HashMap<String, Vec<usize>>,
    /// title token → entries carrying it
    by_token: HashMap<String, Vec<usize>>,
    /// first author family + year → entries
    by_author_year: HashMap<(String, u32), Vec<usize>>,
}

impl Index {
    fn build(entries: &[BibEntry]) -> Self {
        let mut idx = Index {
            entries: Vec::with_capacity(entries.len()),
            by_title: HashMap::new(),
            by_token: HashMap::new(),
            by_author_year: HashMap::new(),
        };
        for (i, e) in entries.iter().enumerate() {
            let title = e.field("title").unwrap_or_default().to_string();
            let norm = normalize_text(&title);
            let tokens = token_set(&norm);
            let author = e.field("author").and_then(first_author_family);
            let year = e.field("year").and_then(extract_year);

            if !norm.is_empty() {
                idx.by_title.entry(norm).or_default().push(i);
            }
            for t in &tokens {
                idx.by_token.entry(t.clone()).or_default().push(i);
            }
            if let (Some(a), Some(y)) = (&author, year) {
                idx.by_author_year.entry((a.clone(), y)).or_default().push(i);
            }
            idx.entries.push(Indexed {
                key: e.key.clone(),
                title,
                tokens,
                author,
                year,
            });
        }
        idx
    }

    /// Entries worth scoring: those sharing a rare-ish title word, plus
    /// everything by the same author in the same year.
    fn candidates(
        &self,
        tokens: &HashSet<String>,
        author: Option<&str>,
        year: Option<u32>,
    ) -> Vec<usize> {
        // very common words ("data", "analysis") would drag in half the file
        let cap = (self.entries.len() / 20).max(200);
        let mut out: HashSet<usize> = HashSet::new();
        for t in tokens {
            if let Some(hits) = self.by_token.get(t) {
                if hits.len() <= cap {
                    out.extend(hits);
                }
            }
        }
        if let (Some(a), Some(y)) = (author, year) {
            if let Some(hits) = self.by_author_year.get(&(a.to_string(), y)) {
                out.extend(hits);
            }
        }
        out.into_iter().collect()
    }
}

// ---------------------------------------------------------------- matching

// ---------------------------------------------------------------- report

#[allow(clippy::too_many_arguments)]
fn report(
    a_file: &Path,
    b_file: &Path,
    a_len: usize,
    b_len: usize,
    matched_doi: usize,
    matched_isbn: usize,
    matched_title: usize,
    uncertain: &[(&BibEntry, Vec<Candidate>)],
    missing: &[&BibEntry],
) {
    eprintln!(
        "{} entries in {}, {} in {}",
        a_len,
        a_file.display().to_string().cyan(),
        b_len,
        b_file.display().to_string().cyan()
    );

    if !uncertain.is_empty() {
        eprintln!("\n{} — look at these yourself:", "uncertain".yellow().bold());
        for (entry, cands) in uncertain {
            eprintln!("  {}  {}", entry.key.cyan(), describe(entry));
            for c in cands {
                let score = if c.score > 0.0 {
                    format!("{:.0}%", c.score * 100.0)
                } else {
                    "  —".to_string()
                };
                eprintln!(
                    "      {} {}  {} [{}]",
                    score.yellow(),
                    c.key.cyan(),
                    truncate(&c.title, 70),
                    c.why
                );
            }
        }
    }

    if !missing.is_empty() {
        eprintln!("\n{}:", "missing".red().bold());
        for entry in missing {
            eprintln!("  {}  {}", entry.key.cyan(), describe(entry));
        }
    }

    eprintln!(
        "\n{} matched ({} by doi, {} by isbn, {} by title), {} uncertain, {} missing",
        matched_doi + matched_isbn + matched_title,
        matched_doi,
        matched_isbn,
        matched_title,
        uncertain.len(),
        missing.len()
    );
}

fn describe(entry: &BibEntry) -> String {
    let author = entry
        .field("author")
        .and_then(first_author_family)
        .unwrap_or_else(|| "?".into());
    let year = entry.field("year").unwrap_or("????");
    format!(
        "{} {}  {}",
        author,
        year,
        truncate(entry.field("title").unwrap_or("(no title)"), 70)
    )
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, fields: &[(&str, &str)]) -> BibEntry {
        BibEntry {
            entry_type: "article".into(),
            key: key.into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn verdict(a: &BibEntry, b_entries: &[BibEntry]) -> Verdict {
        let mut db = BibDatabase::empty();
        for e in b_entries {
            db.add(e.clone());
        }
        let index = Index::build(&db.entries);
        classify(a, &db, &index)
    }

    fn is_missing(a: &BibEntry, b: &[BibEntry]) -> bool {
        matches!(verdict(a, b), Verdict::Missing)
    }

    #[test]
    fn matches_on_doi_despite_different_key_and_title() {
        let a = entry("Smith2020Deep", &[("doi", "10.1/ABC"), ("title", "Deep nets")]);
        let b = entry("smith_deep_2020", &[("doi", "https://doi.org/10.1/abc"), ("title", "Something else entirely")]);
        assert!(matches!(verdict(&a, &[b]), Verdict::Matched(Reason::Doi)));
    }

    #[test]
    fn books_match_on_isbn_however_it_is_written() {
        let a = entry("Buffalo2015Bioinformatics", &[("isbn", "9781449367374"), ("title", "Bioinformatics Data Skills")]);
        let b = entry("buffalo_bioinf", &[("isbn", "978-1-4493-6737-4"), ("title", "Bioinformatics data skills: reproducible research")]);
        assert!(matches!(verdict(&a, &[b]), Verdict::Matched(Reason::Isbn)));
    }

    #[test]
    fn matches_on_title_when_only_one_side_has_a_doi() {
        let a = entry("Smith2020Deep", &[("title", "Deep Learning for {Genomics}"), ("doi", "10.1/abc")]);
        let b = entry("x", &[("title", "Deep learning for genomics")]);
        assert!(matches!(verdict(&a, &[b]), Verdict::Matched(Reason::Title)));
    }

    #[test]
    fn latex_accents_do_not_hide_a_match() {
        let a = entry("a", &[("title", "Gr{\\\"o}bner bases in cryptography")]);
        let b = entry("b", &[("title", "Gröbner bases in cryptography")]);
        assert!(matches!(verdict(&a, &[b]), Verdict::Matched(Reason::Title)));
    }

    #[test]
    fn subtitle_only_on_one_side_is_uncertain_not_missing() {
        let a = entry("a", &[("title", "Attention is all you need: a retrospective"), ("author", "Vaswani, Ashish"), ("year", "2017")]);
        let b = entry("b", &[("title", "Attention is all you need"), ("author", "Vaswani, Ashish"), ("year", "2017")]);
        assert!(!is_missing(&a, &[b]));
    }

    #[test]
    fn unrelated_entry_is_missing() {
        let a = entry("a", &[("title", "Attention is all you need"), ("author", "Vaswani, Ashish"), ("year", "2017")]);
        let b = entry("b", &[("title", "The structure of scientific revolutions"), ("author", "Kuhn, Thomas"), ("year", "1962")]);
        assert!(is_missing(&a, &[b]));
    }

    #[test]
    fn empty_database_means_everything_is_missing() {
        let a = entry("a", &[("title", "Whatever"), ("doi", "10.1/x")]);
        assert!(is_missing(&a, &[]));
    }

    #[test]
    fn same_citation_key_is_reported_even_without_title_overlap() {
        let a = entry("Smith2020Deep", &[("title", "Deep nets")]);
        let b = entry("Smith2020Deep", &[("title", "A totally different paper")]);
        assert!(!is_missing(&a, &[b]));
    }


}
