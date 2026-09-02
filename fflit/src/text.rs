//! Small text helpers shared by bibtex matching and pdf discovery: titles and
//! author names have to be compared across LaTeX escapes, accents, casing and
//! punctuation before any of it means anything.

use crate::bibtex::STOP_WORDS;
use std::collections::HashSet;

/// Share of the shorter title's words that also occur in the longer one.
/// Containment rather than Jaccard, so a truncated title still matches its
/// full version.
pub fn containment(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    inter as f32 / a.len().min(b.len()) as f32
}

/// Zero of every decimal digit block a typeset book might use. Digits within a
/// block are consecutive, so the offset from zero is the value.
const DIGIT_ZEROS: &[u32] = &[
    0x0030, // ascii
    0x0660, // arabic-indic
    0x06F0, // extended arabic-indic
    0x0966, // devanagari
    0x09E6, // bengali
    0x0A66, // gurmukhi
    0x0AE6, // gujarati
    0x0B66, // oriya
    0x0BE6, // tamil
    0x0C66, // telugu
    0x0CE6, // kannada
    0x0D66, // malayalam
    0x0E50, // thai
    0x0ED0, // lao
    0x0F20, // tibetan
    0x1040, // myanmar
    0x17E0, // khmer
    0xFF10, // fullwidth
    0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, // mathematical bold, double struck, sans, sans bold, mono
];

/// The ASCII value of a digit written in any of those blocks.
pub fn digit_value(c: char) -> Option<u32> {
    let cp = c as u32;
    DIGIT_ZEROS
        .iter()
        .find(|&&zero| cp >= zero && cp < zero + 10)
        .map(|&zero| cp - zero)
}

/// Undo typesetting: a number copied out of a book is not necessarily made of
/// ASCII digits, and its "hyphens" are usually not hyphen-minus either.
pub fn fold_typography(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            // hyphens, dashes and the minus sign
            '\u{2010}'..='\u{2015}' | '\u{2043}' | '\u{2212}' | '\u{00AD}' | '\u{FE58}'
            | '\u{FE63}' | '\u{FF0D}' => '-',
            // spaces that are not the space
            '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            '\u{FF38}' => 'X',
            _ => match digit_value(c) {
                Some(v) => char::from_digit(v, 10).unwrap_or(c),
                None => c,
            },
        })
        .collect()
}

/// Lowercase, de-accent, drop LaTeX markup, keep alphanumerics.
pub fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.peek().is_some_and(|n| n.is_alphabetic()) {
                // a command: \emph{x} → x, with a separator in its place
                while chars.peek().is_some_and(|n| n.is_alphabetic()) {
                    chars.next();
                }
                out.push(' ');
            } else {
                // an accent: M{\"u}ller → muller, no separator
                chars.next();
            }
            continue;
        }
        // braces group, they do not separate words
        if c == '{' || c == '}' {
            continue;
        }
        let c = fold(c);
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn token_set(normalized: &str) -> HashSet<String> {
    normalized
        .split_whitespace()
        .filter(|w| w.len() > 1 && !STOP_WORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

fn fold(c: char) -> char {
    match c {
        'á'..='å' | 'à' | 'ā' | 'ă' | 'ą' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'ó'..='ö' | 'ò' | 'ō' | 'ø' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ñ' | 'ń' => 'n',
        'ç' | 'ć' | 'č' => 'c',
        'š' | 'ś' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        'ł' => 'l',
        'ß' => 's',
        other => other,
    }
}

/// "Neumann, John and Turing, Alan" → "neumann"; "John von Neumann" → "von neumann"
pub fn first_author_family(authors: &str) -> Option<String> {
    let first = split_authors(authors).next()?;
    let family = match first.split_once(',') {
        Some((family, _)) => normalize_text(family),
        None => {
            let words = normalize_text(first);
            let mut it = words.split_whitespace().rev();
            let last = it.next()?.to_string();
            // keep nobiliary particles: "ludwig van beethoven" → "van beethoven"
            match it.next() {
                Some(p) if is_particle(p) => format!("{p} {last}"),
                _ => last,
            }
        }
    };
    let family = family.trim().to_string();
    if family.is_empty() { None } else { Some(family) }
}

fn split_authors(authors: &str) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let mut rest = authors;
    loop {
        match find_and(rest) {
            Some((start, end)) => {
                parts.push(rest[..start].trim());
                rest = &rest[end..];
            }
            None => {
                parts.push(rest.trim());
                break;
            }
        }
    }
    parts.into_iter().filter(|p| !p.is_empty())
}

/// byte range of the next " and " separator, case insensitively
fn find_and(s: &str) -> Option<(usize, usize)> {
    let lower = s.to_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(" and ") {
        let start = from + pos;
        // only split on char boundaries of the original string
        if s.is_char_boundary(start) && s.is_char_boundary(start + 5) {
            return Some((start, start + 5));
        }
        from = start + 1;
    }
    None
}

const PARTICLES: &[&str] = &["van", "von", "de", "del", "della", "di", "da", "der", "den", "ten", "ter", "le", "la", "dos", "das"];

fn is_particle(w: &str) -> bool {
    PARTICLES.contains(&w)
}

/// "van Beethoven" vs "Beethoven" should still count as the same person.
pub fn family_eq(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(" {b}")) || b.ends_with(&format!(" {a}"))
}

pub fn extract_year(field: &str) -> Option<u32> {
    let digits: String = field
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.len() == 4 {
        digits.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_families() {
        assert_eq!(first_author_family("Neumann, John and Turing, Alan").as_deref(), Some("neumann"));
        assert_eq!(first_author_family("John von Neumann").as_deref(), Some("von neumann"));
        assert_eq!(first_author_family("M{\\\"u}ller, Hans").as_deref(), Some("muller"));
        assert_eq!(first_author_family("").as_deref(), None);
    }

    #[test]
    fn nobiliary_particle_does_not_break_author_equality() {
        assert!(family_eq("von neumann", "neumann"));
        assert!(!family_eq("neumann", "neuman"));
    }

    #[test]
    fn years() {
        assert_eq!(extract_year("2017"), Some(2017));
        assert_eq!(extract_year("{2017}"), Some(2017));
        assert_eq!(extract_year("2017-04"), Some(2017));
        assert_eq!(extract_year("in press"), None);
    }
}
