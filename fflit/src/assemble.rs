use crate::bibtex::BibDatabase;
use anyhow::{bail, Context};
use colored::Colorize;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static REF_RE: OnceLock<Regex> = OnceLock::new();
static LABEL_RE: OnceLock<Regex> = OnceLock::new();

// @key — but not when the @ is glued to a word, a dot or a slash, so that
// mail@example.com and https://x.invalid/@handle don't look like citations.
fn ref_regex() -> &'static Regex {
    REF_RE.get_or_init(|| Regex::new(r"(?:^|[^\w.@/])@([A-Za-z0-9_][\w.:-]*)").unwrap())
}

// <label> definitions in the document itself — those are internal cross
// references (figures, sections), not citations.
fn label_regex() -> &'static Regex {
    LABEL_RE.get_or_init(|| Regex::new(r"<([A-Za-z0-9_][\w.:-]*)>").unwrap())
}

struct Reference {
    key: String,
    /// file the key was first seen in, for warning messages
    origin: PathBuf,
}

pub fn assemble(
    repo: &Path,
    out_bibtex: &Path,
    pdf_dir: Option<&Path>,
    typst_files: &[PathBuf],
) -> anyhow::Result<bool> {
    let lit = repo.join("literature.bibtex");
    if !lit.exists() {
        bail!("{} is not an fflit repository (no literature.bibtex)", repo.display());
    }
    let db = BibDatabase::load(&lit)?;

    let refs = collect_references(typst_files)?;
    if refs.is_empty() {
        eprintln!("no references found in {} typst file(s)", typst_files.len());
    }

    let mut out = BibDatabase::empty();
    let mut ok = true;
    let mut wanted: Vec<String> = Vec::new();

    for r in &refs {
        match db.get(&r.key) {
            Some(entry) => {
                out.add(entry.clone());
                wanted.push(r.key.clone());
            }
            None => {
                ok = false;
                let hint = match db.find_ignore_case(&r.key) {
                    Some(k) => format!(" (did you mean @{k}?)"),
                    None => String::new(),
                };
                eprintln!(
                    "{}: unknown reference {} in {}{}",
                    "warning".yellow().bold(),
                    format!("@{}", r.key).cyan(),
                    r.origin.display(),
                    hint
                );
            }
        }
    }

    if let Some(parent) = out_bibtex.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    out.write(out_bibtex)?;

    let mut copied = 0usize;
    let mut missing_pdfs = 0usize;
    if let Some(dir) = pdf_dir {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        for key in &wanted {
            let src = repo.join("pdfs").join(format!("{key}.pdf"));
            if !src.exists() {
                eprintln!(
                    "{}: no pdf for {} in {}",
                    "note".yellow(),
                    key.cyan(),
                    repo.join("pdfs").display()
                );
                missing_pdfs += 1;
                continue;
            }
            let dest = dir.join(format!("{key}.pdf"));
            std::fs::copy(&src, &dest)
                .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
            copied += 1;
        }
    }

    let unknown = refs.len() - wanted.len();
    eprintln!(
        "{} references, {} entries → {}{}{}",
        refs.len(),
        wanted.len(),
        out_bibtex.display().to_string().cyan(),
        match pdf_dir {
            Some(d) => format!(", {} pdfs → {}", copied, d.display().to_string().cyan()),
            None => String::new(),
        },
        if missing_pdfs > 0 {
            format!(" ({missing_pdfs} without pdf)")
        } else {
            String::new()
        }
    );
    if unknown > 0 {
        eprintln!("{}: {} unknown reference(s)", "warning".yellow().bold(), unknown);
    }

    Ok(ok)
}

/// Scan typst files for `@key` citations, in order of first appearance.
/// Keys that the documents define themselves (`<key>`) are cross references,
/// not citations, and are skipped.
fn collect_references(typst_files: &[PathBuf]) -> anyhow::Result<Vec<Reference>> {
    let mut sources = Vec::new();
    for path in typst_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        sources.push((path.clone(), strip_raw(&content)));
    }

    let labels: BTreeSet<String> = sources
        .iter()
        .flat_map(|(_, text)| {
            label_regex()
                .captures_iter(text)
                .map(|c| c[1].to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut refs = Vec::new();
    for (path, text) in &sources {
        for cap in ref_regex().captures_iter(text) {
            let m = cap.get(1).unwrap();
            // package specs, `#import "@preview/cetz:0.2.0"`, are not citations
            if text[m.end()..].starts_with('/') {
                continue;
            }
            let key = m.as_str().trim_end_matches(['.', ':', '-']).to_string();
            if key.is_empty() || labels.contains(&key) {
                continue;
            }
            if seen.insert(key.clone()) {
                refs.push(Reference {
                    key,
                    origin: path.clone(),
                });
            }
        }
    }
    Ok(refs)
}

/// Blank out typst raw blocks (```…``` and `…`), so code samples containing
/// decorators or mail addresses don't turn into citations.
fn strip_raw(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let ticks = after.chars().take_while(|c| *c == '`').count();
        let fence = "`".repeat(ticks);
        let body = &after[ticks..];
        match body.find(&fence) {
            Some(end) => rest = &body[end + ticks..],
            // unterminated — drop the remainder
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fflit_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{n}.typ"));
        std::fs::write(&path, text).unwrap();
        let refs = collect_references(std::slice::from_ref(&path)).unwrap();
        std::fs::remove_file(&path).ok();
        refs.into_iter().map(|r| r.key).collect()
    }

    #[test]
    fn finds_plain_citations() {
        assert_eq!(keys("As shown in @Smith2024Attention we ..."), vec!["Smith2024Attention"]);
    }

    #[test]
    fn strips_trailing_punctuation() {
        assert_eq!(keys("see @Smith2024."), vec!["Smith2024"]);
    }

    #[test]
    fn deduplicates_keeping_order() {
        assert_eq!(keys("@b and @a and @b"), vec!["b", "a"]);
    }

    #[test]
    fn ignores_mail_and_urls() {
        assert_eq!(keys("write to me@example.com or https://x.invalid/@handle"), Vec::<String>::new());
    }

    #[test]
    fn ignores_own_labels() {
        assert_eq!(keys("#figure()<fig:one>\nsee @fig:one and @Smith2024"), vec!["Smith2024"]);
    }

    #[test]
    fn ignores_raw_blocks() {
        assert_eq!(keys("```python\n@decorator\n```\n@Smith2024"), vec!["Smith2024"]);
        assert_eq!(keys("`@inline` @Smith2024"), vec!["Smith2024"]);
    }

    #[test]
    fn ignores_package_specs() {
        assert_eq!(keys("#import \"@preview/cetz:0.2.0\": *\n@Smith2024"), vec!["Smith2024"]);
    }

    #[test]
    fn handles_supplements() {
        assert_eq!(keys("@Smith2024[p. 4]"), vec!["Smith2024"]);
    }
}
