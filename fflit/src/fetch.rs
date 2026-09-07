//! Downloading the open access copies of a bibtex file.
//!
//! Files land in `incoming/` under the citation key they came from and are then
//! `fflit scan`'s problem like any other pdf — this module does not touch
//! `literature.bibtex`.

use crate::bibtex::{is_doi, normalize_doi, BibDatabase, BibEntry};
use crate::text::normalize_text;
use crate::unpaywall::{self, OaCopy};
use crate::http::{self, host_of};
use crate::{pmc, publisher};
use anyhow::Context;
use colored::Colorize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Anything smaller is an error page wearing a pdf's name.
const MIN_PDF_BYTES: usize = 10_000;
/// More than this many places to try by hand is not help, it is a wall of text.
const MAX_LINKS: usize = 4;
/// Politeness between API calls; the downloads themselves are slow enough.
const API_PAUSE: Duration = Duration::from_millis(100);
/// How long one publisher is left alone between requests. Publishers watch for
/// exactly this kind of traffic and block whole campuses for it, so the wait is
/// long — it costs nothing, because a paper waiting on its publisher goes on a
/// pile and the run moves on to somebody else's papers rather than sleeping.
const HOST_PAUSE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Tally {
    downloaded: usize,
    already_here: usize,
    /// how each skipped entry was recognised: doi, isbn or title
    already_known: BTreeMap<&'static str, usize>,
    closed: usize,
    no_doi: usize,
    /// a doi field holding something that is not a doi
    junk_doi: usize,
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
        .filter_map(|e| e.field("doi").filter(|d| is_doi(d)).map(normalize_doi))
        .collect();
    let in_pmc = pmc::pmcids(&dois);
    if !in_pmc.is_empty() {
        eprintln!("{} of {} are in PubMed Central\n", in_pmc.len(), dois.len());
    }

    let mut tally = Tally::default();
    let mut attempted = 0usize;
    // what a human will have to fetch by hand, and where from
    let mut unobtained: Vec<Unobtained> = Vec::new();
    // papers the free routes did not get, waiting their turn at a publisher
    let mut pile: Vec<Task> = Vec::new();
    let mut throttle = Throttle::default();
    let mut order = 0usize;

    for entry in &db.entries {
        if limit.is_some_and(|n| attempted >= n) {
            break;
        }
        if let Some(how) = known.holds(entry) {
            *tally.already_known.entry(how).or_default() += 1;
            continue;
        }
        // somebody else's bibtex puts urls and notes in doi fields, and a
        // registry asked about one of those rejects the request it came in
        let Some(doi) = entry.field("doi").filter(|d| is_doi(d)).map(normalize_doi) else {
            match entry.field("doi").is_some_and(|d| !d.trim().is_empty()) {
                true => tally.junk_doi += 1,
                false => tally.no_doi += 1,
            }
            continue;
        };
        let doi = doi.as_str();

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

        if dry_run {
            match copies.first() {
                Some(c) => {
                    eprintln!("{}  {}  ({})", "oa    ".green(), entry.key.cyan(), c.describe());
                    tally.downloaded += 1;
                }
                None => {
                    eprintln!(
                        "{}  {}  {}  (subscription only)",
                        "closed".yellow(),
                        entry.key.cyan(),
                        format!("https://doi.org/{doi}").dimmed()
                    );
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

        // nothing free worked; the subscription may still cover it, but this
        // paper's publisher may have been asked about another paper a moment
        // ago, so it goes on the pile rather than holding up the run
        let task = Task::new(
            order,
            Pending {
                key: entry.key.clone(),
                title: entry.field("title").unwrap_or_default().to_string(),
                doi: doi.to_string(),
                dest,
                copies,
                pmcid: pmcid.cloned(),
            },
        );
        order += 1;
        match use_publisher {
            true => pile.push(task),
            // nobody to ask, so what is known now is the whole answer
            false => finish(task, &mut tally, &mut unobtained),
        }
    }

    if !pile.is_empty() {
        eprintln!(
            "\n{} waiting on a publisher, {}s apart per publisher\n",
            format!("{} paper(s)", pile.len()).bold(),
            HOST_PAUSE.as_secs()
        );
    }
    work_the_pile(pile, &mut throttle, &mut tally, &mut unobtained);
    // the pile answers papers out of order; the report reads better in the
    // order they were asked for
    unobtained.sort_by_key(|u| u.order);

    report(&tally, into, dry_run);
    write_worklist(&unobtained, worklist)?;
    Ok(())
}

/// A paper the free routes did not get, and everything already learnt about it.
struct Pending {
    key: String,
    title: String,
    doi: String,
    dest: PathBuf,
    /// the free locations that were tried and refused
    copies: Vec<OaCopy>,
    pmcid: Option<String>,
}

/// One paper's turn at its publisher, which takes as many requests as it takes:
/// the landing page first, then each url the publisher's own scheme suggests,
/// one per turn, so that nothing asks a publisher twice in a row.
struct Task {
    /// where it came in the bibtex, so the report can be put back in order
    order: usize,
    pending: Pending,
    /// the landing page has been asked for, whatever came of it
    asked: bool,
    probe: Option<publisher::Probe>,
    /// which candidate url is next
    next: usize,
}

impl Task {
    fn new(order: usize, pending: Pending) -> Self {
        Self { order, pending, asked: false, probe: None, next: 0 }
    }

    /// Whether this paper has anything left to ask for. One that has not is
    /// finished where it stands, without waiting for a turn it will not use.
    fn needs_request(&self) -> bool {
        match (self.asked, &self.probe) {
            (false, _) => true,
            (true, Some(p)) => self.next < p.candidates.len(),
            // the doi would not resolve; there is nothing else to try
            (true, None) => false,
        }
    }
}

/// When each publisher may next be asked.
///
/// The publisher is not known until a doi has been resolved once, so it is
/// remembered against the doi prefix — the registrant — which is what tells
/// the second Elsevier paper of a run where it is going before it asks.
#[derive(Default)]
struct Throttle {
    host_of_prefix: HashMap<String, String>,
    last_hit: HashMap<String, Instant>,
}

impl Throttle {
    /// When this paper's publisher is free again, or `None` for right now.
    fn ready_at(&self, doi: &str) -> Option<Instant> {
        let host = self.host_of_prefix.get(prefix(doi))?;
        let ready = *self.last_hit.get(host)? + HOST_PAUSE;
        (ready > Instant::now()).then_some(ready)
    }

    fn hit(&mut self, doi: &str, url: &str) {
        let host = host_of(url).to_string();
        self.last_hit.insert(host.clone(), Instant::now());
        self.host_of_prefix.insert(prefix(doi).to_string(), host);
    }
}

/// The registrant half of a doi: `10.1016` is Elsevier, whatever the paper.
fn prefix(doi: &str) -> &str {
    doi.split_once('/').map_or(doi, |(prefix, _)| prefix)
}

enum Outcome {
    Got,
    /// back on the pile: another request to make, or nothing left and the
    /// drain will finish it
    Again,
}

/// Work through the papers waiting on a publisher, always taking one whose
/// publisher is ready rather than whichever is next in line. A busy publisher
/// therefore holds up its own papers and nobody else's, and the run only ever
/// sleeps when every paper left is waiting on a publisher just asked.
fn work_the_pile(
    mut pile: Vec<Task>,
    throttle: &mut Throttle,
    tally: &mut Tally,
    unobtained: &mut Vec<Unobtained>,
) {
    loop {
        // anything with nothing left to ask for is done, and waits for nobody
        while let Some(i) = pile.iter().position(|t| !t.needs_request()) {
            finish(pile.remove(i), tally, unobtained);
        }
        if pile.is_empty() {
            return;
        }
        let Some(i) = next_ready(&pile, throttle) else {
            // everything left is cooling off; wait for the first one that is not
            if let Some(at) = pile.iter().filter_map(|t| throttle.ready_at(&t.pending.doi)).min() {
                std::thread::sleep(at.saturating_duration_since(Instant::now()));
            }
            continue;
        };
        let mut task = pile.remove(i);
        match step(&mut task, throttle) {
            Outcome::Got => {
                eprintln!("{}  {}  (subscription)", "got   ".green(), task.pending.key.cyan());
                tally.downloaded += 1;
            }
            Outcome::Again => pile.push(task),
        }
    }
}

/// The next paper to ask about: any one whose publisher is free, rather than
/// whichever happens to be at the front. Position in the pile carries no
/// meaning, which is the whole point — ten Elsevier papers in a row do not stop
/// the Springer paper behind them.
fn next_ready(pile: &[Task], throttle: &Throttle) -> Option<usize> {
    pile.iter().position(|t| throttle.ready_at(&t.pending.doi).is_none())
}

/// One request at one publisher: the landing page, or the next url it suggested.
fn step(task: &mut Task, throttle: &mut Throttle) -> Outcome {
    if !task.asked {
        task.asked = true;
        task.probe = publisher::probe(&task.pending.doi).ok();
        if let Some(p) = &task.probe {
            throttle.hit(&task.pending.doi, &p.landing_url);
        }
        return Outcome::Again;
    }
    let Some(probe) = &task.probe else { return Outcome::Again };
    let Some(url) = probe.candidates.get(task.next) else { return Outcome::Again };
    task.next += 1;
    throttle.hit(&task.pending.doi, url);
    match publisher::download(url, &probe.landing_url) {
        Ok(bytes) if save_if_pdf(&bytes, &task.pending.dest) => Outcome::Got,
        _ => Outcome::Again,
    }
}

/// Nothing else to try: say what stood in the way and where a person should go.
fn finish(task: Task, tally: &mut Tally, unobtained: &mut Vec<Unobtained>) {
    let Task { order, pending, probe, .. } = task;
    let Pending { key, title, doi, copies, pmcid, .. } = pending;

    // what stood in the way overall, for grouping the report
    let why = match (&probe, &pmcid, copies.is_empty()) {
        (Some(p), _, _) if p.blocked.is_some() => p.blocked.unwrap(),
        (Some(p), _, _) if !p.candidates.is_empty() => "pdf refused, may work from the subscribing network",
        (_, Some(_), _) => "free in pubmed central",
        (_, _, false) => "listed as free, but nothing served a pdf",
        _ => "closed access",
    };

    // every place worth a click, in the order fflit tried them
    let mut links: Vec<Link> = Vec::new();
    for copy in &copies {
        links.push(Link {
            url: copy.url.clone(),
            note: format!("{} (refused a download)", copy.describe()),
        });
    }
    if let Some(id) = &pmcid {
        links.push(Link {
            url: pmc::article_url(id),
            note: "pubmed central, free to read in a browser".into(),
        });
    }
    if let Some(p) = &probe {
        // the landing page first: it is the one a person actually opens, and
        // from a browser its pdf button works even when ours did not
        links.push(Link {
            url: p.landing_url.clone(),
            note: p.blocked.unwrap_or("publisher page").to_string(),
        });
        for url in &p.candidates {
            links.push(Link { url: url.clone(), note: "publisher pdf (refused a download)".into() });
        }
    }
    if links.is_empty() {
        links.push(Link {
            url: format!("https://doi.org/{doi}"),
            note: "publisher page via the doi".into(),
        });
    }
    // the same url reached two ways — unpaywall's publisher copy and the one
    // the publisher's scheme suggests — is one link, and they are not next to
    // each other, so dedup_by is no use here
    let mut seen: Vec<String> = Vec::new();
    links.retain(|l| match seen.contains(&l.url) {
        true => false,
        false => {
            seen.push(l.url.clone());
            true
        }
    });
    links.truncate(MAX_LINKS);

    // the line below quotes it, so read it before the links are moved out
    let best = links[0].url.clone();
    // closed means nobody had a copy to give. A publisher that offered a pdf
    // url and then refused the file is a failure, not a closed paper: that one
    // is worth opening in a browser
    let tried = copies.len() + probe.as_ref().map_or(0, |p| p.candidates.len());
    match tried {
        0 => {
            eprintln!("{}  {}  {}", "closed".yellow(), key.cyan(), best.dimmed());
            tally.closed += 1;
        }
        n => {
            eprintln!("{}  {}  ({n} location(s), none served a pdf)", "failed".red(), key.cyan());
            tally.failed += 1;
        }
    }
    unobtained.push(Unobtained { order, key, title, links, why });
}

/// Somewhere a person could try, and what fflit found there.
struct Link {
    url: String,
    note: String,
}

struct Unobtained {
    /// where it came in the bibtex, so the report can be put back in order
    order: usize,
    key: String,
    title: String,
    /// every place worth trying by hand, best first
    links: Vec<Link>,
    /// what stood in the way overall, for grouping
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

    eprintln!("\n{}", "to chase up yourself:".bold());
    for (why, items) in &by_reason {
        eprintln!("\n  {} — {}", why.yellow(), format!("{} paper(s)", items.len()).dimmed());
        for u in items {
            eprintln!("    {}  {}", u.key.cyan(), truncate(&u.title, 64).dimmed());
            for link in &u.links {
                eprintln!("      {}  {}", link.url, format!("({})", link.note).dimmed());
            }
        }
    }

    let Some(path) = path else {
        return Ok(());
    };
    // one row per link, so the file is a list of things to open
    let mut out = String::from("key\ttitle\turl\tfound\treason\n");
    let mut rows = 0usize;
    for u in unobtained {
        for link in &u.links {
            out.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", u.key, u.title, link.url, link.note, u.why));
            rows += 1;
        }
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "\n{} papers, {rows} links → {}",
        unobtained.len(),
        path.display().to_string().cyan()
    );
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.chars().count() <= max {
        true => flat,
        false => flat.chars().take(max - 1).collect::<String>() + "…",
    }
}

/// Unpaywall lists locations that 403, redirect to a login, or serve a landing
/// page. Work down the list until one of them is really a pdf.
fn download_first_that_works<'a>(copies: &'a [OaCopy], dest: &Path) -> Option<&'a OaCopy> {
    for copy in copies {
        let Ok(bytes) = http::get_pdf(&copy.url, None) else { continue };
        if save_if_pdf(&bytes, dest) {
            return Some(copy);
        }
    }
    None
}

/// Keep a download only if it is really the file, so a captcha or a cookie wall
/// never lands in `incoming/` under a citation key.
fn save_if_pdf(bytes: &[u8], dest: &Path) -> bool {
    is_pdf(bytes) && std::fs::write(dest, bytes).is_ok()
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
    if t.junk_doi > 0 {
        eprintln!(
            "{}: {} entries have a doi field holding something that is not a doi",
            "note".yellow(),
            t.junk_doi
        );
    }
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

    fn waiting(doi: &str) -> Task {
        Task::new(
            0,
            Pending {
                key: "K".into(),
                title: "T".into(),
                doi: doi.into(),
                dest: PathBuf::from("/dev/null"),
                copies: Vec::new(),
                pmcid: None,
            },
        )
    }

    #[test]
    fn the_registrant_is_read_off_the_doi() {
        assert_eq!(prefix("10.1016/j.cell.2020.02.052"), "10.1016");
        assert_eq!(prefix("10.1038/nature14539"), "10.1038");
        // a doi with no slash is not one, but must not panic on the way out
        assert_eq!(prefix("nonsense"), "nonsense");
    }

    #[test]
    fn a_publisher_just_asked_is_left_alone() {
        let mut throttle = Throttle::default();
        assert_eq!(throttle.ready_at("10.1016/a"), None, "nothing is known yet, so nothing waits");

        throttle.hit("10.1016/a", "https://www.sciencedirect.com/science/article/pii/S1/pdfft");
        // another paper from the same registrant now knows where it is going
        assert!(throttle.ready_at("10.1016/b").is_some());
        // and one from anybody else does not wait for it
        assert_eq!(throttle.ready_at("10.1038/x"), None);
    }

    #[test]
    fn two_registrants_at_one_publisher_share_its_cooling_off() {
        // Wiley registers both 10.1111 and 10.1002, and it is one publisher
        let mut throttle = Throttle::default();
        throttle.hit("10.1111/a", "https://onlinelibrary.wiley.com/doi/pdfdirect/10.1111/a");
        throttle.hit("10.1002/b", "https://onlinelibrary.wiley.com/doi/pdfdirect/10.1002/b");
        assert!(throttle.ready_at("10.1111/c").is_some());
        assert!(throttle.ready_at("10.1002/d").is_some());
    }

    #[test]
    fn a_busy_publisher_holds_up_its_own_papers_and_nobody_elses() {
        let mut throttle = Throttle::default();
        throttle.hit("10.1016/a", "https://www.sciencedirect.com/x");

        // a run of Elsevier papers with one Springer paper stuck behind them
        let pile: Vec<Task> = ["10.1016/b", "10.1016/c", "10.1016/d", "10.1007/e", "10.1016/f"]
            .iter()
            .map(|d| waiting(d))
            .collect();
        // the Springer paper is taken now rather than in five minutes
        assert_eq!(next_ready(&pile, &throttle), Some(3));

        // and once every publisher in the pile has just been asked, there is
        // nothing to get on with and the run waits
        throttle.hit("10.1007/e", "https://link.springer.com/x");
        assert_eq!(next_ready(&pile, &throttle), None);
    }

    #[test]
    fn a_paper_with_nothing_left_to_ask_for_does_not_wait_for_a_turn() {
        let mut task = waiting("10.1016/a");
        assert!(task.needs_request(), "the landing page has not been asked for yet");

        // asked, and the doi did not resolve: there is nothing else to try
        task.asked = true;
        assert!(!task.needs_request());

        task.probe = Some(publisher::Probe {
            landing_url: "https://x.invalid/a".into(),
            candidates: vec!["https://x.invalid/a.pdf".into()],
            blocked: None,
        });
        assert!(task.needs_request());
        task.next = 1;
        assert!(!task.needs_request(), "every candidate has been tried");
    }

    #[test]
    fn one_url_reached_two_ways_is_listed_once() {
        // wiley's pdf is both what unpaywall named and what its url scheme
        // says, with the landing page in between, so they are not adjacent
        let pdf = "https://onlinelibrary.wiley.com/doi/pdfdirect/10.1111/x";
        let mut task = waiting("10.1111/x");
        task.pending.copies.push(OaCopy {
            url: pdf.into(),
            version: "publishedVersion".into(),
            host: "publisher".into(),
            direct: true,
        });
        task.asked = true;
        task.probe = Some(publisher::Probe {
            landing_url: "https://onlinelibrary.wiley.com/doi/10.1111/x".into(),
            candidates: vec![pdf.into()],
            blocked: None,
        });

        let mut tally = Tally::default();
        let mut unobtained = Vec::new();
        finish(task, &mut tally, &mut unobtained);

        let urls: Vec<&str> = unobtained[0].links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, vec![pdf, "https://onlinelibrary.wiley.com/doi/10.1111/x"]);
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
