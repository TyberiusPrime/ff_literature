use crate::error::FflitError;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

static DOI_RE: OnceLock<Regex> = OnceLock::new();
static LABELLED_DOI_RE: OnceLock<Regex> = OnceLock::new();
static XMP_DOI_RE: OnceLock<Regex> = OnceLock::new();
static ARXIV_RE: OnceLock<Regex> = OnceLock::new();

fn doi_regex() -> &'static Regex {
    DOI_RE.get_or_init(|| Regex::new(r"10\.\d{4,9}/[^\s\x22<>\]\[]+").unwrap())
}

/// A DOI announced as one ("doi:10.…", "https://doi.org/10.…") beats a bare
/// match: on a title page the announced one is the paper's own.
fn labelled_doi_regex() -> &'static Regex {
    LABELLED_DOI_RE.get_or_init(|| {
        Regex::new(r"(?i)(?:doi\s*:\s*|doi\.org/|dx\.doi\.org/)(10\.\d{4,9}/[^\s\x22<>\]\[]+)")
            .unwrap()
    })
}

fn xmp_doi_regex() -> &'static Regex {
    XMP_DOI_RE.get_or_init(|| {
        Regex::new(r"(?is)<(?:prism:doi|dc:identifier)[^>]*>\s*(?:doi:)?\s*(10\.\d{4,9}/[^\s\x22<>\]\[]+?)\s*<")
            .unwrap()
    })
}

/// The stamp arXiv prints down the margin of page 1, old and new style.
fn arxiv_regex() -> &'static Regex {
    ARXIV_RE.get_or_init(|| {
        Regex::new(r"(?i)arxiv\s*:\s*((?:[a-z-]+(?:\.[a-z]{2})?/\d{7})|(?:\d{4}\.\d{4,5}))(?:v\d+)?")
            .unwrap()
    })
}

/// arXiv registers every preprint under a DOI of its own.
pub fn arxiv_doi(text: &str) -> Option<String> {
    arxiv_regex()
        .captures(text)
        .map(|c| format!("10.48550/arXiv.{}", &c[1]))
}

/// Where a DOI came from, and whether that origin is trustworthy on its own.
/// A DOI picked out of the body of a paper may well belong to a cited work, so
/// the caller is expected to check the metadata it buys against the title page.
pub struct DoiHit {
    pub doi: String,
    pub source: &'static str,
    pub trusted: bool,
}

impl DoiHit {
    fn trusted(doi: String, source: &'static str) -> Self {
        Self { doi: clean_doi(doi), source, trusted: true }
    }
    fn shaky(doi: String, source: &'static str) -> Self {
        Self { doi: clean_doi(doi), source, trusted: false }
    }
}

pub fn extract_doi(path: &Path) -> Result<DoiHit, FflitError> {
    if let Some(doi) = extract_from_info_dict(path) {
        return Ok(DoiHit::trusted(doi, "pdf metadata"));
    }
    if let Some(doi) = extract_from_xmp(path) {
        return Ok(DoiHit::trusted(doi, "xmp metadata"));
    }
    extract_from_text(path)
}

// ---------------------------------------------------------------- metadata

fn extract_from_info_dict(path: &Path) -> Option<String> {
    let doc = lopdf::Document::load(path).ok()?;
    let info_ref = doc.trailer.get(b"Info").ok()?;
    let info_id = info_ref.as_reference().ok()?;
    let dict = doc.get_object(info_id).ok()?.as_dict().ok()?;

    // publishers invent their own keys (WPS-ARTICLEDOI, ELS-DOI, …), so go by
    // what the key is called rather than by a fixed list
    let mut fallback = None;
    for (key, obj) in dict.iter() {
        let name = String::from_utf8_lossy(key).to_lowercase();
        let Some(text) = object_to_string(obj) else { continue };
        let Some(m) = doi_regex().find(&text) else { continue };
        if name.contains("doi") {
            return Some(m.as_str().to_string());
        }
        if matches!(name.as_str(), "subject" | "keywords" | "title") {
            fallback.get_or_insert_with(|| m.as_str().to_string());
        }
    }
    fallback
}

/// The XMP packet hanging off the catalogue — where most modern publishers
/// actually put the DOI, and where fflit never used to look.
fn extract_from_xmp(path: &Path) -> Option<String> {
    let doc = lopdf::Document::load(path).ok()?;
    let meta_ref = doc.catalog().ok()?.get(b"Metadata").ok()?;
    let meta_id = meta_ref.as_reference().ok()?;
    let stream = doc.get_object(meta_id).ok()?.as_stream().ok()?;
    let bytes = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let xmp = String::from_utf8_lossy(&bytes);

    if let Some(c) = xmp_doi_regex().captures(&xmp) {
        return Some(c[1].to_string());
    }
    doi_regex().find(&xmp).map(|m| m.as_str().to_string())
}

fn object_to_string(obj: &lopdf::Object) -> Option<String> {
    match obj {
        lopdf::Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
        lopdf::Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

/// PDF text strings are either PDFDocEncoded or UTF-16BE with a BOM.
fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `/Title` from the info dictionary, when it looks like a real title and
/// not a leftover file name.
pub fn info_title(path: &Path) -> Option<String> {
    let doc = lopdf::Document::load(path).ok()?;
    let info_id = doc.trailer.get(b"Info").ok()?.as_reference().ok()?;
    let dict = doc.get_object(info_id).ok()?.as_dict().ok()?;
    let title = object_to_string(dict.get(b"Title").ok()?)?;
    plausible_title(title.trim())
}

fn plausible_title(title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let junk = title.len() < 12
        || title.split_whitespace().count() < 3
        || lower.contains("microsoft word")
        || lower.starts_with("untitled")
        || [".doc", ".docx", ".pdf", ".dvi", ".tex", ".qxd", ".indd"]
            .iter()
            .any(|ext| lower.ends_with(ext));
    match junk {
        true => None,
        false => Some(title.to_string()),
    }
}

// ---------------------------------------------------------------- text

/// Text of pages `first..=last`, 1-based. `last == 0` means "to the end".
pub fn page_text(path: &Path, first: u32, last: u32) -> Result<String, FflitError> {
    let mut args: Vec<String> = vec!["-f".into(), first.to_string()];
    if last > 0 {
        args.push("-l".into());
        args.push(last.to_string());
    }
    args.push(path.to_str().unwrap_or("").to_string());
    args.push("-".into());

    let output = std::process::Command::new(option_env!("NIX_PDF_TO_TEXT").unwrap_or("pdftotext"))
        .args(&args)
        .output()
        .map_err(|e| FflitError::PdfRead(format!("pdftotext failed: {e}")))?;

    if !output.status.success() {
        return Err(FflitError::PdfRead(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_from_text(path: &Path) -> Result<DoiHit, FflitError> {
    // the title page first — that is where a paper states its own DOI
    let front = page_text(path, 1, 1)?;
    if let Some(doi) = find_doi(&front) {
        return Ok(DoiHit::trusted(doi, "page 1"));
    }
    // a preprint states an arXiv id instead, which is just as good an identifier
    if let Some(doi) = arxiv_doi(&front) {
        return Ok(DoiHit::trusted(doi, "arxiv id"));
    }
    // asking for pages a short document does not have is not an error here
    let early = page_text(path, 2, 3).unwrap_or_default();
    if let Some(doi) = find_doi(&early) {
        return Ok(DoiHit::trusted(doi, "pages 2-3"));
    }
    // anywhere at all: likely a reference to someone else's work, so it is
    // handed over for verification rather than believed
    let rest = page_text(path, 4, 0).unwrap_or_default();
    if let Some(doi) = find_doi(&rest) {
        return Ok(DoiHit::shaky(doi, "body text"));
    }
    Err(FflitError::NoDoi)
}

fn find_doi(text: &str) -> Option<String> {
    if let Some(c) = labelled_doi_regex().captures(text) {
        return Some(c[1].to_string());
    }
    doi_regex().find(text).map(|m| m.as_str().to_string())
}

fn clean_doi(mut s: String) -> String {
    while s.ends_with(|c: char| matches!(c, '.' | ',' | ';' | ')' | ']')) {
        s.pop();
    }
    let s = s
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:");
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_labelled_doi_wins_over_a_bare_one() {
        let page = "Journal of Things 10.9999/journal.template\nArticle\ndoi: 10.1234/real.paper";
        assert_eq!(find_doi(page).as_deref(), Some("10.1234/real.paper"));
    }

    #[test]
    fn url_forms_are_recognised() {
        assert_eq!(
            find_doi("available at https://doi.org/10.1234/abc").as_deref(),
            Some("10.1234/abc")
        );
        assert_eq!(
            find_doi("see http://dx.doi.org/10.1234/abc for more").as_deref(),
            Some("10.1234/abc")
        );
    }

    #[test]
    fn bare_dois_still_work() {
        assert_eq!(find_doi("blah 10.1234/abc blah").as_deref(), Some("10.1234/abc"));
        assert_eq!(find_doi("no identifier here"), None);
    }

    #[test]
    fn trailing_sentence_punctuation_is_dropped() {
        assert_eq!(clean_doi("10.1234/abc.".into()), "10.1234/abc");
        assert_eq!(clean_doi("https://doi.org/10.1234/abc".into()), "10.1234/abc");
    }

    #[test]
    fn utf16_metadata_strings_decode() {
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .iter()
            .copied()
            .chain("10.1234/abc".encode_utf16().flat_map(|u| u.to_be_bytes()))
            .collect();
        assert_eq!(decode_pdf_string(&utf16), "10.1234/abc");
        assert_eq!(decode_pdf_string(b"10.1234/abc"), "10.1234/abc");
    }

    #[test]
    fn xmp_packets_give_up_their_doi() {
        let xmp = r#"<rdf:RDF><rdf:Description><prism:doi>10.1234/xmp.paper</prism:doi>
            </rdf:Description></rdf:RDF>"#;
        assert_eq!(xmp_doi_regex().captures(xmp).map(|c| c[1].to_string()).as_deref(), Some("10.1234/xmp.paper"));
        let dc = "<dc:identifier>doi:10.1234/dc.paper</dc:identifier>";
        assert_eq!(xmp_doi_regex().captures(dc).map(|c| c[1].to_string()).as_deref(), Some("10.1234/dc.paper"));
    }

    #[test]
    fn arxiv_stamps_become_dois() {
        assert_eq!(
            arxiv_doi("arXiv:1706.03762v5  [cs.CL]  6 Dec 2017").as_deref(),
            Some("10.48550/arXiv.1706.03762")
        );
        // the old scheme, still on plenty of pdfs
        assert_eq!(
            arxiv_doi("arXiv:hep-th/9901001").as_deref(),
            Some("10.48550/arXiv.hep-th/9901001")
        );
        assert_eq!(
            arxiv_doi("arXiv:math.GT/0309136v2").as_deref(),
            Some("10.48550/arXiv.math.GT/0309136")
        );
        assert_eq!(arxiv_doi("no preprint here"), None);
    }

    #[test]
    fn word_export_titles_are_not_titles() {
        assert_eq!(plausible_title("Microsoft Word - paper final v3.doc"), None);
        assert_eq!(plausible_title("untitled document here"), None);
        assert_eq!(plausible_title("main.tex"), None);
        assert_eq!(plausible_title("Short"), None);
        assert_eq!(
            plausible_title("Attention is all you need").as_deref(),
            Some("Attention is all you need")
        );
    }
}
