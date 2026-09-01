//! Identifying a pdf that does not say what it is.
//!
//! When no DOI can be read out of a file, the title page still describes the
//! paper. CrossRef is asked what work that page belongs to, and the answer is
//! only believed if it can be found on the page again — a search engine will
//! always return *something*.

use crate::crossref;
use crate::metadata::WorkMetadata;
use crate::text::{containment, normalize_text, token_set};
use std::collections::HashSet;

/// A candidate title has to be almost entirely present on the title page.
const ACCEPT: f32 = 0.85;
/// …unless the author cannot be found there too, then near enough is not enough.
const ACCEPT_WITHOUT_AUTHOR: f32 = 0.95;
/// A handful of words can line up by chance; a long title cannot.
const MIN_TITLE_WORDS: usize = 3;
const MIN_TITLE_WORDS_WITHOUT_AUTHOR: usize = 5;
/// Confirming a DOI we already hold in hand is a lower bar than picking one.
const CONFIRM: f32 = 0.7;
/// Below this a candidate is not worth mentioning even as a guess.
const SUGGEST: f32 = 0.45;

pub struct Candidate {
    pub meta: WorkMetadata,
    pub score: f32,
    /// author name also found on the title page
    pub author_seen: bool,
    /// significant words in the candidate's title
    title_words: usize,
}

/// Ask CrossRef what this title page is, and check the answer against it.
/// `Ok(None)` means nothing came back that resembles the page.
pub fn by_title_page(front_text: &str, info_title: Option<&str>) -> anyhow::Result<Option<Candidate>> {
    let page = token_set(&normalize_text(front_text));
    if page.is_empty() {
        return Ok(None);
    }

    let mut best: Option<Candidate> = None;
    for query in queries(front_text, info_title) {
        for meta in crossref::search(&query, 5)? {
            let cand = score(meta, front_text, &page);
            if !worth_considering(&cand) {
                continue;
            }
            if best.as_ref().is_none_or(|b| rank(&cand) > rank(b)) {
                best = Some(cand);
            }
        }
        // a convincing hit makes the second, vaguer query pointless
        if best.as_ref().is_some_and(accepted) {
            break;
        }
    }
    Ok(best.filter(|c| c.score >= SUGGEST))
}

/// Does this metadata plausibly describe this title page? Used on DOIs found
/// deep in a document, which are as likely to belong to a cited paper.
pub fn confirms(meta: &WorkMetadata, front_text: &str) -> bool {
    let page = token_set(&normalize_text(front_text));
    if page.is_empty() {
        // nothing to check against; no evidence either way
        return true;
    }
    score(meta.clone(), front_text, &page).score >= CONFIRM
}

/// A one or two word title tells us nothing: "Notes" occurs on any page.
fn worth_considering(c: &Candidate) -> bool {
    c.title_words >= MIN_TITLE_WORDS
}

/// Finding the author on the page as well is worth more than a few percent of
/// title overlap, so it decides between otherwise comparable candidates.
fn rank(c: &Candidate) -> f32 {
    c.score + if c.author_seen { 0.15 } else { 0.0 }
}

pub fn accepted(c: &Candidate) -> bool {
    match c.author_seen {
        true => c.score >= ACCEPT && c.title_words >= MIN_TITLE_WORDS,
        // "Introduction" appearing on a page proves nothing about the page
        false => c.score >= ACCEPT_WITHOUT_AUTHOR && c.title_words >= MIN_TITLE_WORDS_WITHOUT_AUTHOR,
    }
}

/// Three shots, narrowest first: the title the pdf claims, the line that looks
/// like a title on the page, and failing both the top of the page as one blob.
fn queries(front_text: &str, info_title: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |q: String| {
        if !q.trim().is_empty() && !out.iter().any(|e| e.eq_ignore_ascii_case(q.trim())) {
            out.push(q.trim().to_string());
        }
    };

    if let Some(t) = info_title {
        push(t.to_string());
    }
    if let Some(t) = title_line(front_text) {
        push(t);
    }
    push(front_text.split_whitespace().take(60).collect::<Vec<_>>().join(" "))
;
    out
}

/// The first line of a title page that is not journal furniture. Wrapped
/// titles are stitched back together with the line below.
fn title_line(front_text: &str) -> Option<String> {
    let mut lines = front_text
        .lines()
        .map(str::trim)
        .filter(|l| l.chars().count() > 3 && !is_furniture(l));

    let first = lines.next()?;
    let mut title = first.to_string();
    if first.split_whitespace().count() < 6 {
        if let Some(next) = lines.next() {
            // an author line ends the title, it does not continue it
            if !looks_like_authors(next) {
                title.push(' ');
                title.push_str(next);
            }
        }
    }
    Some(title.chars().take(200).collect())
}

/// Running heads, download stamps, licences, page numbers — everything a
/// publisher prints above the title.
fn is_furniture(line: &str) -> bool {
    let l = line.to_lowercase();
    const NOISE: &[&str] = &[
        "arxiv:", "doi:", "doi.org", "http://", "https://", "www.", "@", "issn", "isbn",
        "downloaded from", "all rights reserved", "copyright", "©", "creative commons",
        "cc by", "licen", "published online", "received:", "accepted:", "revised:",
        "supplementary", "vol.", "no.", "pp.", "preprint", "manuscript", "elsevier",
        "springer", "wiley", "author manuscript", "open access",
    ];
    NOISE.iter().any(|n| l.contains(n))
        // page numbers, volume lines, dates on their own
        || !line.chars().any(|c| c.is_alphabetic())
        || line.chars().filter(|c| c.is_ascii_digit()).count() * 2 > line.chars().count()
}

fn looks_like_authors(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains('@')
        || l.contains(" and ")
        || line.matches(',').count() >= 2
        || l.contains("university")
        || l.contains("department")
        || l.contains("institute")
        || l.starts_with("abstract")
}

fn score(meta: WorkMetadata, front_text: &str, page: &HashSet<String>) -> Candidate {
    let title = token_set(&normalize_text(&meta.title));
    // how much of the candidate's title is on the page, not the other way round
    let score = containment(&title, page);

    let normalized_page = normalize_text(front_text);
    let author_seen = meta.authors.iter().filter_map(|a| a.family.as_deref()).any(|family| {
        let f = normalize_text(family);
        f.len() > 2 && normalized_page.contains(&f)
    });

    Candidate { meta, score, author_seen, title_words: title.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(title: &str, authors: &[&str]) -> WorkMetadata {
        WorkMetadata {
            doi: "10.1/x".into(),
            title: title.into(),
            authors: authors
                .iter()
                .map(|f| crate::metadata::Author {
                    family: Some((*f).into()),
                    given: None,
                })
                .collect(),
            year: Some(2017),
            entry_type: "article".into(),
            container_title: None,
            volume: None,
            issue: None,
            pages: None,
            publisher: None,
            abstract_text: None,
        }
    }

    const PAGE: &str = "arXiv:1706.03762v5  [cs.CL]  6 Dec 2017
        Attention Is All You Need
        Ashish Vaswani, Noam Shazeer, Niki Parmar
        Google Brain
        Abstract
        The dominant sequence transduction models are based on complex recurrent networks.";

    fn scored(meta: WorkMetadata) -> Candidate {
        let page = token_set(&normalize_text(PAGE));
        score(meta, PAGE, &page)
    }

    #[test]
    fn the_right_paper_is_accepted() {
        let c = scored(work("Attention is all you need", &["Vaswani"]));
        assert!(c.author_seen);
        assert!(accepted(&c), "score was {}", c.score);
    }

    #[test]
    fn a_plausible_but_different_paper_is_not() {
        let c = scored(work("Recurrent models of visual attention", &["Mnih"]));
        assert!(!accepted(&c), "score was {}", c.score);
    }

    #[test]
    fn a_cited_work_does_not_confirm_the_title_page() {
        // the sort of thing a DOI scraped from the bibliography buys
        let c = scored(work("Long short-term memory", &["Hochreiter"]));
        assert!(c.score < CONFIRM, "score was {}", c.score);
    }

    #[test]
    fn the_subtitle_of_the_page_need_not_be_complete() {
        // crossref knows a longer title than the page shows
        let c = scored(work("Attention is all you need", &["Vaswani", "Shazeer"]));
        assert!(accepted(&c));
    }

    #[test]
    fn a_short_title_alone_is_not_an_identification() {
        // four words of a common phrase, and nobody by that name on the page
        let c = scored(work("Attention is all you need", &["Nobody"]));
        assert!(!c.author_seen);
        assert!(!accepted(&c), "score was {}", c.score);
    }

    #[test]
    fn a_long_title_can_carry_itself() {
        let page = "Deep residual learning for image recognition in complex urban scenes\n\
                    Abstract. We present a method.";
        let meta = work("Deep residual learning for image recognition in complex urban scenes", &["Nobody"]);
        let c = score(meta, page, &token_set(&normalize_text(page)));
        assert!(!c.author_seen);
        assert!(accepted(&c), "score was {}", c.score);
    }

    #[test]
    fn a_two_word_title_is_never_a_candidate() {
        let c = scored(work("Attention", &["Vaswani"]));
        assert_eq!(c.score, 1.0);
        assert!(!worth_considering(&c), "a single word must not identify a paper");
    }

    #[test]
    fn corroborating_authors_outrank_a_bare_title_match() {
        let with_author = scored(work("Attention is all you need", &["Vaswani"]));
        let without = scored(work("Attention is all you need really", &["Nobody"]));
        assert!(rank(&with_author) > rank(&without));
    }

    #[test]
    fn a_generic_title_needs_its_author() {
        let page = "Materials and Methods\nSample preparation followed standard protocols.";
        let c = score(work("Materials and methods", &["Nobody"]), page, &token_set(&normalize_text(page)));
        assert!(!accepted(&c), "score was {}", c.score);
    }

    #[test]
    fn the_info_title_is_asked_about_first() {
        let q = queries("Attention Is All You Need Ashish Vaswani", Some("Attention is all you need"));
        assert_eq!(q[0], "Attention is all you need");
    }

    #[test]
    fn the_title_is_read_off_the_page_when_the_pdf_does_not_say() {
        assert_eq!(
            title_line(PAGE).as_deref(),
            Some("Attention Is All You Need")
        );
    }

    #[test]
    fn journal_furniture_is_skipped() {
        let page = "Downloaded from https://academic.oup.com/nar on 3 May 2021\n\
                    Nucleic Acids Research, 2020, Vol. 48, No. 4\n\
                    Structural basis of ribosome recycling\n\
                    Jane Smith, John Doe and Ann Roe";
        assert_eq!(
            title_line(page).as_deref(),
            Some("Structural basis of ribosome recycling")
        );
    }

    #[test]
    fn a_wrapped_title_is_stitched_together() {
        let page = "Deep learning approaches\nfor protein structure prediction\nJane Smith, John Doe and Ann Roe";
        assert_eq!(
            title_line(page).as_deref(),
            Some("Deep learning approaches for protein structure prediction")
        );
    }

    #[test]
    fn a_short_title_is_not_glued_to_its_authors() {
        let page = "Ribosome recycling\nJane Smith, John Doe and Ann Roe\nAbstract";
        assert_eq!(title_line(page).as_deref(), Some("Ribosome recycling"));
    }
}
