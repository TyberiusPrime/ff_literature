use crate::error::FflitError;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

static DOI_RE: OnceLock<Regex> = OnceLock::new();

fn doi_regex() -> &'static Regex {
    DOI_RE.get_or_init(|| Regex::new(r"10\.\d{4,9}/[^\s\x22<>\]\[]+").unwrap())
}

pub fn extract_doi(path: &Path) -> Result<String, FflitError> {
    if let Some(doi) = extract_from_metadata(path) {
        return Ok(clean_doi(doi));
    }
    extract_from_text(path)
}

fn extract_from_metadata(path: &Path) -> Option<String> {
    let doc = lopdf::Document::load(path).ok()?;
    let info_ref = doc.trailer.get(b"Info").ok()?;
    let info_id = info_ref.as_reference().ok()?;
    let info_obj = doc.get_object(info_id).ok()?;
    let dict = info_obj.as_dict().ok()?;

    let keys: &[&[u8]] = &[b"doi", b"DOI", b"Subject", b"subject"];
    for key in keys {
        if let Ok(obj) = dict.get(*key) {
            let text = object_to_string(obj)?;
            if let Some(m) = doi_regex().find(&text) {
                return Some(m.as_str().to_string());
            }
        }
    }
    None
}

fn object_to_string(obj: &lopdf::Object) -> Option<String> {
    match obj {
        lopdf::Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
        lopdf::Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

fn extract_from_text(path: &Path) -> Result<String, FflitError> {
    let output = std::process::Command::new("pdftotext")
        .args(["-l", "3", path.to_str().unwrap_or(""), "-"])
        .output()
        .map_err(|e| FflitError::PdfRead(format!("pdftotext failed: {e}")))?;

    if !output.status.success() {
        return Err(FflitError::PdfRead(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    doi_regex()
        .find(&text)
        .map(|m| clean_doi(m.as_str().to_string()))
        .ok_or(FflitError::NoDoi)
}

fn clean_doi(mut s: String) -> String {
    while s.ends_with(|c: char| matches!(c, '.' | ',' | ';' | ')' | ']')) {
        s.pop();
    }
    // strip common URL prefixes
    let s = s
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:");
    s.to_string()
}
