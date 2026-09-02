//! Making a broken literature.bibtex readable again.
//!
//! Works on lines rather than by parsing, because the file that needs repairing
//! is by definition one that will not parse. fflit writes every field on a line
//! of its own, so a line is a unit that can be fixed in isolation.

use crate::bibtex::{balance_braces, find_unbalanced};
use anyhow::{bail, Context};
use colored::Colorize;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

static FIELD_RE: OnceLock<Regex> = OnceLock::new();

/// `  abstract = {…},` as fflit writes it.
fn field_regex() -> &'static Regex {
    FIELD_RE.get_or_init(|| Regex::new(r"^(\s*)([A-Za-z][A-Za-z0-9_-]*)\s*=\s*\{(.*)\}(,?)\s*$").unwrap())
}

pub fn repair(path: &Path, dry_run: bool) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;

    let broken = find_unbalanced(&content);
    if broken.is_empty() {
        eprintln!("{} parses; nothing to repair", path.display().to_string().cyan());
        return Ok(());
    }
    eprintln!(
        "{} with unbalanced braces:",
        match broken.len() {
            1 => "one entry".to_string(),
            n => format!("{n} entries"),
        }
    );
    for (line, key, field) in broken.iter().take(20) {
        eprintln!(
            "  line {line}: {}{}",
            key.cyan(),
            match field.is_empty() {
                true => String::new(),
                false => format!(", in the {field} field"),
            }
        );
    }

    let (fixed, changed) = rewrite(&content);
    if changed == 0 {
        bail!(
            "could not repair {} automatically — the damage is not a field value on one line",
            path.display()
        );
    }
    if !find_unbalanced(&fixed).is_empty() {
        bail!("repairing {} did not balance it; leaving the file alone", path.display());
    }

    if dry_run {
        eprintln!("\n{changed} field(s) would be fixed; run without --dry-run to write");
        return Ok(());
    }

    // never overwrite the only copy of a file that is already damaged
    let backup = path.with_extension("bibtex.bak");
    std::fs::write(&backup, &content).with_context(|| format!("writing {}", backup.display()))?;
    std::fs::write(path, &fixed).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "\n{changed} field(s) fixed in {} (previous version kept as {})",
        path.display().to_string().cyan(),
        backup.display()
    );
    Ok(())
}

/// Rebuild the file with every single line field value brace balanced.
fn rewrite(content: &str) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut changed = 0usize;

    for line in content.lines() {
        match field_regex().captures(line) {
            Some(c) => {
                let balanced = balance_braces(&c[3]);
                if balanced != c[3] {
                    changed += 1;
                }
                out.push_str(&format!("{}{} = {{{}}}{}\n", &c[1], &c[2], balanced, &c[4]));
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    (out, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROKEN: &str = "@article{Fine2001,\n  title = {ok},\n}\n\n\
                          @article{Broken2005,\n  title = {Compressed indexes},\n  \
                          abstract = {over the alphabet {A,C,G,T and more},\n}\n\n\
                          @article{After2007,\n  title = {ok},\n}\n";

    #[test]
    fn the_stray_brace_goes_and_nothing_else_moves() {
        let (fixed, changed) = rewrite(BROKEN);
        assert_eq!(changed, 1);
        assert!(fixed.contains("abstract = {over the alphabet A,C,G,T and more},"));
        // every other line is untouched
        assert!(fixed.contains("@article{Broken2005,"));
        assert!(fixed.contains("  title = {Compressed indexes},"));
        assert!(fixed.contains("@article{After2007,"));
        assert!(find_unbalanced(&fixed).is_empty());
    }

    #[test]
    fn a_healthy_file_is_left_alone() {
        let good = "@article{Fine2001,\n  title = {The {DNA} of it},\n}\n";
        let (fixed, changed) = rewrite(good);
        assert_eq!(changed, 0);
        assert_eq!(fixed, good);
    }

    #[test]
    fn entry_headers_are_not_mistaken_for_fields() {
        let (fixed, _) = rewrite("@article{Key2001Word,\n");
        assert_eq!(fixed, "@article{Key2001Word,\n");
    }
}
