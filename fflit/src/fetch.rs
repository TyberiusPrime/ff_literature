//! Downloading the open access copies of a bibtex file.
//!
//! Files land in `incoming/` under the citation key they came from and are then
//! `fflit scan`'s problem like any other pdf — this module does not touch
//! `literature.bibtex`.

use crate::bibtex::{BibDatabase, BibEntry};
use crate::text::normalize_text;
use crate::unpaywall::{self, OaCopy};
use crate::{pmc, publisher};
use anyhow::Context;
use colored::Colorize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Anything smaller is an error page wearing a pdf's name.
const MIN_PDF_BYTES: usize = 10_000;
/// Politeness between API calls; the downloads themselves are slow enough.
const API_PAUSE: Duration = Duration::from_millis(100);
/// Publishers watch for exactly this kind of traffic and block whole campuses
/// for it, so the subscription route goes slowly.
const PUBLISHER_PAUSE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Tally {
    downloaded: usize,
    already_here: usize,
    /// how each skipped entry was recognised: doi, isbn or title
    already_known: BTreeMap<&'static str, usize>,
    closed: usize,
    no_doi: usize,
    failed: usize,
}

/// What the library already holds, so a run does not fetch it twice. Identity
/// by doi or isbn, and by title for the same paper filed under a different one
/// — a preprint and its published version have different dois.
struct Known {
    db: BibDatabase,
    titles: std::collections::HashSet<String>,
}

impl Known {
    fn load(repository: &Path) -> anyhow::Result<Self> {
        Ok(Self::from_db(BibDatabase::load(&repository.join("literature.bibtex"))?))
    }

    fn from_db(db: BibDatabase) -> Self {
        let titles = db
            .entries
            .iter()
            .filter_map(|e| e.field("title"))
            .map(normalize_text)
            .filter(|t| !t.is_empty())
            .collect();
        Self { db, titles }
    }

    fn holds(&self, entry: &BibEntry) -> Option<&'static str> {
        if entry.field("doi").is_some_and(|d| self.db.contains_doi(d)) {
            return Some("doi");
        }
        if entry.field("isbn").is_some_and(|i| self.db.contains_isbn(i)) {
            return Some("isbn");
        }
        let title = normalize_text(entry.field("title").unwrap_or_default());
        match !title.is_empty() && self.titles.contains(&title) {
            true => Some("title"),
            false => None,
        }
    }
}

pub fn fetch(
    bibtex: &Path,
    into: &Path,
    limit: Option<usize>,
    dry_run: bool,
    use_publisher: bool,
    worklist: Option<&Path>,
    repository: &Path,
) -> anyhow::Result<()> {
    let db = BibDatabase::load(bibtex)?;

    // whatever the library already holds is not worth downloading again
    let known = Known::load(repository)?;
    if known.db.entries.is_empty() {
        eprintln!(
            "{}: no entries in {} — nothing to subtract",
            "note".yellow(),
            repository.join("literature.bibtex").display()
        );
    }
    if !dry_run {
        std::fs::create_dir_all(into)
            .with_context(|| format!("creating {}", into.display()))?;
    }

    // one batched lookup up front beats a request per paper
    let dois: Vec<String> = db
        .entries
        .iter()
        .filter(|e| known.holds(e).is_none())
        .filter_map(|e| e.field("doi").filter(|d| !d.is_empty()).map(str::to_string))
        .collect();
    let in_pmc = match pmc::pmcids(&dois) {
        Ok(map) => {
            if !map.is_empty() {
                eprintln!("{} of {} are in PubMed Central\n", map.len(), dois.len());
            }
            map
        }
        Err(e) => {
            eprintln!("{}: PubMed Central lookup failed ({e:#})", "note".yellow());
            Default::default()
        }
    };

    let mut tally = Tally::default();
    let mut attempted = 0usize;
    // what a human will have to fetch by hand, and where from
    let mut unobtained: Vec<Unobtained> = Vec::new();

    for entry in &db.entries {
        if limit.is_some_and(|n| attempted >= n) {
            break;
        }
        if let Some(how) = known.holds(entry) {
            *tally.already_known.entry(how).or_default() += 1;
            continue;
        }
        let Some(doi) = entry.field("doi").filter(|d| !d.is_empty()) else {
            tally.no_doi += 1;
            continue;
        };

        // named after the citation key, so an interrupted run resumes
        let dest = into.join(format!("{}.pdf", entry.key));
        if dest.exists() {
            tally.already_here += 1;
            continue;
        }
        attempted += 1;

        let mut copies = match unpaywall::pdf_locations(doi) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}  {}: {e:#}", "error ".red(), entry.key.cyan());
                tally.failed += 1;
                continue;
            }
        };
        std::thread::sleep(API_PAUSE);

        // pmc holds free copies unpaywall does not always list, notably nih
        // funded author manuscripts
        let pmcid = in_pmc.get(doi);
        if let Some(id) = pmcid {
            copies.push(OaCopy {
                url: pmc::pdf_url(id),
                version: "publishedVersion".into(),
                host: "pubmed central".into(),
                direct: true,
            });
        }

        if copies.is_empty() && !use_publisher {
            eprintln!("{}  {}", "closed".yellow(), entry.key.cyan());
            tally.closed += 1;
            continue;
        }
        if dry_run {
            match copies.first() {
                Some(c) => {
                    eprintln!("{}  {}  ({})", "oa    ".green(), entry.key.cyan(), c.describe());
                    tally.downloaded += 1;
                }
                None => {
                    eprintln!("{}  {}  (subscription only)", "closed".yellow(), entry.key.cyan());
                    tally.closed += 1;
                }
            }
            continue;
        }

        if let Some(copy) = download_first_that_works(&copies, &dest) {
            eprintln!("{}  {}  ({})", "got   ".green(), entry.key.cyan(), copy.describe());
            tally.downloaded += 1;
            continue;
        }

        // nothing free worked; the subscription may still cover it
        let mut probed: Option<publisher::Probe> = None;
        if use_publisher {
            std::thread::sleep(PUBLISHER_PAUSE);
            probed = publisher::probe(doi).ok();
            if let Some(url) = probed.as_ref().and_then(|p| p.pdf_url.clone()) {
                let via = OaCopy { url, version: String::new(), host: "publisher".into(), direct: true };
                if download_first_that_works(std::slice::from_ref(&via), &dest).is_some() {
                    eprintln!("{}  {}  (subscription)", "got   ".green(), entry.key.cyan());
                    tally.downloaded += 1;
                    continue;
                }
            }
        }

        // where a person should go, and why a script could not
        let (url, why) = match (&probed, pmcid) {
            // a wall a browser walks through beats a link to the same wall
            (Some(p), _) if p.blocked.is_some() => (p.landing_url.clone(), p.blocked.unwrap()),
            // the publisher named its pdf and then would not part with it
            (Some(p), _) if p.pdf_url.is_some() => (
                p.landing_url.clone(),
                "pdf refused, may work from the subscribing network",
            ),
            (_, Some(id)) => (pmc::article_url(id), "free in pubmed central"),
            _ => (format!("https://doi.org/{doi}"), "closed access"),
        };
        unobtained.push(Unobtained {
            key: entry.key.clone(),
            title: entry.field("title").unwrap_or_default().to_string(),
            url,
            why,
        });

        match copies.is_empty() {
            true => {
                eprintln!("{}  {}", "closed".yellow(), entry.key.cyan());
                tally.closed += 1;
            }
            false => {
                eprintln!("{}  {}  ({} location(s), none served a pdf)", "failed".red(), entry.key.cyan(), copies.len());
                tally.failed += 1;
            }
        }
    }

    report(&tally, into, dry_run);
    write_worklist(&unobtained, worklist)?;
    Ok(())
}

struct Unobtained {
    key: String,
    title: String,
    /// the page to open by hand — resolved past doi.org where we got that far
    url: String,
    why: &'static str,
}

/// The ones a person has to open themselves, grouped by what stood in the way.
/// A bot challenge or a javascript redirect is nothing to a browser, so those
/// links are worth clicking; closed access ones are listed for completeness.
fn write_worklist(unobtained: &[Unobtained], path: Option<&Path>) -> anyhow::Result<()> {
    if unobtained.is_empty() {
        return Ok(());
    }

    let mut by_reason: BTreeMap<&str, Vec<&Unobtained>> = BTreeMap::new();
    for u in unobtained {
        by_reason.entry(u.why).or_default().push(u);
    }

    eprintln!("\n{}", "to open yourself:".bold());
    for (why, items) in &by_reason {
        eprintln!("\n  {} — {}", why.yellow(), format!("{} paper(s)", items.len()).dimmed());
        for u in items {
            eprintln!("    {}  {}", u.key.cyan(), u.url);
        }
    }

    let Some(path) = path else {
        return Ok(());
    };
    let mut out = String::new();
    for u in unobtained {
        out.push_str(&format!("{}\t{}\t{}\t{}\n", u.key, u.title, u.url, u.why));
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "\n{} written to {} as key/title/url/reason",
        unobtained.len(),
        path.display().to_string().cyan()
    );
    Ok(())
}

/// Unpaywall lists locations that 403, redirect to a login, or serve a landing
/// page. Work down the list until one of them is really a pdf.
fn download_first_that_works<'a>(copies: &'a [OaCopy], dest: &Path) -> Option<&'a OaCopy> {
    for copy in copies {
        let Ok(bytes) = get(&copy.url) else { continue };
        if !is_pdf(&bytes) {
            continue;
        }
        if std::fs::write(dest, &bytes).is_err() {
            continue;
        }
        return Some(copy);
    }
    None
}

fn get(url: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = reqwest::blocking::Client::new()
        .get(url)
        .header("User-Agent", "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)")
        .timeout(Duration::from_secs(120))
        .send()?
        .error_for_status()?
        .bytes()?;
    Ok(bytes.to_vec())
}

/// A pdf says so in its first bytes. Anything else is a captcha, a cookie wall
/// or an apology.
fn is_pdf(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_PDF_BYTES && bytes.starts_with(b"%PDF")
}

fn report(t: &Tally, into: &Path, dry_run: bool) {
    let verb = match dry_run {
        true => "available",
        false => "downloaded",
    };
    eprintln!(
        "\n{} {}{}, {} closed access, {} failed{}{}",
        t.downloaded,
        verb,
        match dry_run {
            true => String::new(),
            false => format!(" → {}", into.display().to_string().cyan()),
        },
        t.closed,
        t.failed,
        match t.no_doi {
            0 => String::new(),
            n => format!(", {n} without a doi to look up"),
        },
        match t.already_here {
            0 => String::new(),
            n => format!(", {n} already in {}", into.display()),
        }
    );
    let skipped: usize = t.already_known.values().sum();
    if skipped > 0 {
        eprintln!(
            "{skipped} skipped, already in the library ({})",
            t.already_known
                .iter()
                .map(|(how, n)| format!("{n} by {how}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if t.downloaded > 0 && !dry_run {
        eprintln!("run {} to file them", "fflit scan".cyan());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fields: &[(&str, &str)]) -> BibEntry {
        BibEntry {
            entry_type: "article".into(),
            key: "Key2020Word".into(),
            fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn library(fields: &[(&str, &str)]) -> Known {
        let mut db = BibDatabase::empty();
        db.add(entry(fields));
        Known::from_db(db)
    }

    #[test]
    fn what_the_library_holds_is_not_downloaded_again() {
        let lib = library(&[("doi", "10.1/ABC"), ("title", "Deep learning for genomics")]);
        // the same paper, written differently, from someone else's bibtex
        assert_eq!(lib.holds(&entry(&[("doi", "https://doi.org/10.1/abc")])), Some("doi"));
        assert_eq!(lib.holds(&entry(&[("title", "Deep Learning for {Genomics}")])), Some("title"));
        assert_eq!(lib.holds(&entry(&[("title", "Something else entirely")])), None);
    }

    #[test]
    fn a_preprint_and_its_published_version_are_one_paper() {
        // filed from an arxiv id, wanted under the journal doi
        let lib = library(&[("doi", "10.48550/arxiv.1706.03762"), ("title", "Attention Is All You Need")]);
        let published = entry(&[("doi", "10.5555/3295222.3295349"), ("title", "Attention is all you need")]);
        assert_eq!(lib.holds(&published), Some("title"));
    }

    #[test]
    fn books_are_recognised_by_isbn() {
        let lib = library(&[("isbn", "9781449367374"), ("title", "Bioinformatics Data Skills")]);
        assert_eq!(lib.holds(&entry(&[("isbn", "978-1-4493-6737-4")])), Some("isbn"));
    }

    #[test]
    fn an_empty_library_holds_nothing() {
        let lib = Known::from_db(BibDatabase::empty());
        assert_eq!(lib.holds(&entry(&[("doi", "10.1/abc"), ("title", "T")])), None);
        // an entry with no title at all must not match on the empty string
        assert_eq!(library(&[("doi", "10.9/z")]).holds(&entry(&[("doi", "10.1/abc")])), None);
    }

    #[test]
    fn only_real_pdfs_count() {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.resize(MIN_PDF_BYTES, b'x');
        assert!(is_pdf(&pdf));

        // the shape of a publisher's "verify you are human" page
        let mut html = b"<!DOCTYPE html><html><head>".to_vec();
        html.resize(MIN_PDF_BYTES, b'x');
        assert!(!is_pdf(&html));

        // a truncated download
        assert!(!is_pdf(b"%PDF-1.7 but then nothing"));
    }
}
