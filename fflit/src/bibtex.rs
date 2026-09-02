use anyhow::Context;
use nom_bibtex::Bibtex;
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone)]
pub struct BibEntry {
    pub entry_type: String,
    pub key: String,
    pub fields: Vec<(String, String)>,
}

impl BibEntry {
    /// Field lookup, case insensitive in the field name as bibtex is.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The `keywords` field, split on commas.
    pub fn keywords(&self) -> Vec<String> {
        split_keywords(self.field("keywords").unwrap_or_default())
    }
}

/// bibtex has no formal syntax here; comma separated is what everyone uses.
pub fn split_keywords(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

pub struct BibDatabase {
    pub entries: Vec<BibEntry>,
    doi_set: HashSet<String>,
    isbn_set: HashSet<String>,
    sha256_set: HashSet<String>,
}

impl BibDatabase {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let bibtex = Bibtex::parse(&content).map_err(|e| {
            let culprits = find_unbalanced(&content);
            match culprits.is_empty() {
                true => anyhow::anyhow!("parsing {}: {:?}", path.display(), e),
                false => anyhow::anyhow!(
                    "parsing {}: unbalanced braces in {}\n{}\nrun `fflit repair {}` to fix them",
                    path.display(),
                    match culprits.len() {
                        1 => "one entry".to_string(),
                        n => format!("{n} entries"),
                    },
                    culprits
                        .iter()
                        .take(10)
                        .map(|(line, key, field)| format!(
                            "  line {line}: @{{{key}}}{}",
                            match field.is_empty() {
                                true => String::new(),
                                false => format!(", in the {field} field"),
                            }
                        ))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    path.display()
                ),
            }
        })?;

        let mut entries = Vec::new();
        let mut doi_set = HashSet::new();
        let mut isbn_set = HashSet::new();
        let mut sha256_set = HashSet::new();

        for bib in bibtex.bibliographies() {
            let mut fields: Vec<(String, String)> = bib
                .tags()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            // stable order for re-serialisation
            fields.sort_by(|a, b| field_order_key(&a.0).cmp(&field_order_key(&b.0)));

            for (k, v) in &fields {
                match k.to_lowercase().as_str() {
                    "doi" => {
                        doi_set.insert(normalize_doi(v));
                    }
                    "isbn" => {
                        // stored however it was printed, compared as digits
                        isbn_set.extend(crate::isbn::normalize(v));
                    }
                    "sha256" => {
                        sha256_set.insert(v.clone());
                    }
                    _ => {}
                }
            }

            entries.push(BibEntry {
                entry_type: bib.entry_type().to_string(),
                key: bib.citation_key().to_string(),
                fields,
            });
        }

        Ok(Self {
            entries,
            doi_set,
            isbn_set,
            sha256_set,
        })
    }

    pub fn empty() -> Self {
        Self {
            entries: vec![],
            doi_set: HashSet::new(),
            isbn_set: HashSet::new(),
            sha256_set: HashSet::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&BibEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// For "did you mean" hints on unknown citation keys.
    pub fn find_ignore_case(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(key))
            .map(|e| e.key.as_str())
    }

    pub fn contains_doi(&self, doi: &str) -> bool {
        !doi.is_empty() && self.doi_set.contains(&normalize_doi(doi))
    }

    pub fn contains_isbn(&self, isbn: &str) -> bool {
        crate::isbn::normalize(isbn).is_some_and(|i| self.isbn_set.contains(&i))
    }

    pub fn contains_sha256(&self, hash: &str) -> bool {
        self.sha256_set.contains(hash)
    }

    pub fn key_exists(&self, key: &str) -> bool {
        self.entries.iter().any(|e| e.key == key)
    }

    pub fn generate_key(&self, author_family: &str, year: u32, title: &str) -> String {
        let author = ascii_name(author_family);
        let word = first_significant_word(title);
        let base = format!("{}{}{}", author, year, word);

        if !self.key_exists(&base) {
            return base;
        }
        for c in b'a'..=b'z' {
            let candidate = format!("{}{}", base, c as char);
            if !self.key_exists(&candidate) {
                return candidate;
            }
        }
        panic!("exhausted 26 conflict suffixes for key {base} — something is very wrong");
    }

    pub fn add(&mut self, entry: BibEntry) {
        for (k, v) in &entry.fields {
            match k.to_lowercase().as_str() {
                "doi" => {
                    self.doi_set.insert(normalize_doi(v));
                }
                "isbn" => {
                    self.isbn_set.extend(crate::isbn::normalize(v));
                }
                "sha256" => {
                    self.sha256_set.insert(v.clone());
                }
                _ => {}
            }
        }
        self.entries.push(entry);
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.key.cmp(&b.key));

        let mut out = String::new();
        for entry in &sorted {
            out.push_str(&format!("@{}{{{},\n", entry.entry_type, entry.key));
            for (k, v) in &entry.fields {
                out.push_str(&format!("  {} = {{{}}},\n", k, balance_braces(v)));
            }
            out.push_str("}\n\n");
        }

        std::fs::write(path, out)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// A field value is written inside braces, so a brace in it that never closes —
/// or closes nothing — swallows the rest of the entry and every entry after it,
/// and the file stops parsing. Abstracts arrive full of set notation.
///
/// Balanced braces are meaningful in bibtex (`{DNA}` protects capitalisation)
/// and are left alone. Unmatched ones are dropped rather than escaped: bibtex
/// parsers count raw braces, so `\{` would still leave the file unbalanced, and
/// a delimiter with nothing to delimit carries no meaning to lose.
pub fn balance_braces(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut open_at: Vec<usize> = Vec::new();
    let mut unmatched: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            // an escaped brace is a literal, not a delimiter
            '\\' if i + 1 < chars.len() => i += 1,
            '{' => open_at.push(i),
            '}' => {
                if open_at.pop().is_none() {
                    unmatched.push(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    unmatched.extend(open_at);
    if unmatched.is_empty() && !value.ends_with('\\') {
        return value.to_string();
    }

    let mut out = String::with_capacity(value.len());
    for (i, c) in chars.iter().enumerate() {
        if !unmatched.contains(&i) {
            out.push(*c);
        }
    }
    // a value ending in a backslash would escape the brace that closes it
    while out.ends_with('\\') {
        out.pop();
    }
    out
}

/// Entries whose braces do not balance, as line number, key and the field that
/// went wrong. Used to say something useful when the parser gives up.
pub fn find_unbalanced(content: &str) -> Vec<(usize, String, String)> {
    let mut problems = Vec::new();
    let mut current: Option<(usize, String, String)> = None;
    let mut depth: i32 = 0;

    for (n, line) in content.lines().enumerate() {
        let starts_entry = line.trim_start().starts_with('@');
        if starts_entry {
            // a new entry while still inside the last one means that one broke
            if let Some(open) = current.take() {
                if depth != 0 {
                    problems.push(open);
                }
            }
            depth = 0;
            let key = line
                .split_once('{')
                .map(|(_, rest)| rest.trim_end_matches(',').trim().to_string())
                .unwrap_or_default();
            current = Some((n + 1, key, String::new()));
        }

        let before = depth;
        depth += net_braces(line);
        // remember the last field that left us deeper than we started
        if let Some(entry) = current.as_mut() {
            if depth > before && !starts_entry {
                entry.2 = line.split('=').next().unwrap_or("").trim().to_string();
            }
        }
    }
    if let Some(open) = current {
        if depth != 0 {
            problems.push(open);
        }
    }
    problems
}

fn net_braces(line: &str) -> i32 {
    let chars: Vec<char> = line.chars().collect();
    let mut depth = 0;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => i += 1,
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

pub fn normalize_doi(s: &str) -> String {
    s.trim()
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("https://dx.doi.org/")
        .trim_start_matches("http://dx.doi.org/")
        .trim_start_matches("doi:")
        .trim()
        .to_lowercase()
}

/// Is this actually a DOI? A `doi = {}` field in somebody else's bibtex may
/// hold a url, a note, or nothing at all, and a registry asked about one of
/// those rejects the whole request it arrived in.
pub fn is_doi(s: &str) -> bool {
    let s = normalize_doi(s);
    s.starts_with("10.")
        && s.contains('/')
        && !s.contains(char::is_whitespace)
        && s.len() > 7
}

fn ascii_name(s: &str) -> String {
    let clean: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let mut chars = clean.chars();
    match chars.next() {
        None => String::from("Unknown"),
        Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
    }
}

pub const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "for", "with", "to", "at", "by", "from", "and", "or",
    "is", "are", "was", "were", "be", "been", "that", "this", "these", "those", "it", "its",
    "via", "into", "over", "under", "toward", "using",
];

fn first_significant_word(title: &str) -> String {
    for word in title.split_whitespace() {
        let clean: String = word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if clean.is_empty() {
            continue;
        }
        if STOP_WORDS.contains(&clean.to_lowercase().as_str()) {
            continue;
        }
        let mut chars = clean.chars();
        return match chars.next() {
            None => String::from("Unknown"),
            Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
        };
    }
    String::from("Unknown")
}

// lower number = written earlier in the entry
fn field_order_key(f: &str) -> u8 {
    match f.to_lowercase().as_str() {
        "author" => 0,
        "title" => 1,
        "year" => 2,
        "doi" => 3,
        "isbn" => 4,
        "journal" => 5,
        "booktitle" => 5,
        "volume" => 6,
        "number" => 7,
        "pages" => 8,
        "publisher" => 9,
        "abstract" => 10,
        "keywords" => 11,
        "sha256" => 12,
        _ => 50,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmatched_brace_is_escaped_a_matched_one_is_not() {
        // set notation out of an abstract, missing its close
        assert_eq!(balance_braces("over the alphabet {A,C,G,T and so on"), "over the alphabet A,C,G,T and so on");
        assert_eq!(balance_braces("a stray close } here"), "a stray close  here");
        // capitalisation protection must survive untouched
        assert_eq!(balance_braces("The {DNA} of it"), "The {DNA} of it");
        assert_eq!(balance_braces("nested {a {b} c} fine"), "nested {a {b} c} fine");
        // already escaped braces are literals, not delimiters
        assert_eq!(balance_braces("already \\{ escaped"), "already \\{ escaped");
    }

    #[test]
    fn a_trailing_backslash_would_escape_the_closing_brace() {
        assert_eq!(balance_braces("ends with a backslash \\"), "ends with a backslash ");
    }

    #[test]
    fn a_written_file_survives_a_hostile_abstract() {
        let mut db = BibDatabase::empty();
        db.add(BibEntry {
            entry_type: "article".into(),
            key: "Bbb2005Compressed".into(),
            fields: vec![
                ("title".into(), "Compressed indexes".into()),
                ("abstract".into(), "over the alphabet {A,C,G,T and more".into()),
            ],
        });
        db.add(BibEntry {
            entry_type: "article".into(),
            key: "Ccc2007Another".into(),
            fields: vec![("title".into(), "Another entry".into())],
        });

        let dir = std::env::temp_dir().join(format!("fflit_brace_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("literature.bibtex");
        db.write(&path).unwrap();

        // the whole point: it can be read back, and the entry after it survives
        let reloaded = BibDatabase::load(&path).unwrap();
        assert_eq!(reloaded.entries.len(), 2);
        assert!(reloaded.get("Ccc2007Another").is_some());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_broken_entry_is_named() {
        let content = "@article{Fine2001,\n  title = {ok},\n}\n\n@article{Broken2005,\n  abstract = {alphabet {A,C,G,T},\n}\n\n@article{After2007,\n  title = {ok},\n}\n";
        let found = find_unbalanced(content);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "Broken2005");
        assert_eq!(found[0].2, "abstract");
    }

    #[test]
    fn what_is_and_is_not_a_doi() {
        assert!(is_doi("10.1093/nar/gkr110"));
        assert!(is_doi("https://doi.org/10.1093/nar/gkr110"));
        assert!(is_doi("  doi:10.1093/nar/gkr110  "));
        // the sort of thing that turns up in a foreign bibtex
        assert!(!is_doi("in press"));
        assert!(!is_doi(""));
        assert!(!is_doi("http://example.invalid/paper.pdf"));
        assert!(!is_doi("10.1093 nar gkr110"));
    }

    #[test]
    fn urls_are_stripped_before_comparing() {
        assert_eq!(normalize_doi(" https://dx.doi.org/10.1/ABC "), "10.1/abc");
        assert_eq!(normalize_doi("doi: 10.1/ABC"), "10.1/abc");
    }

    #[test]
    fn isbns_match_however_they_are_written() {
        let mut db = BibDatabase::empty();
        db.add(BibEntry {
            entry_type: "book".into(),
            key: "Buffalo2015".into(),
            fields: vec![("isbn".into(), "978-1-4493-6737-4".into())],
        });
        assert!(db.contains_isbn("9781449367374"));
        assert!(db.contains_isbn("978 1 4493 6737 4"));
        assert!(!db.contains_isbn("9780596520687"));
        // not an isbn at all, so not a match
        assert!(!db.contains_isbn("garbage"));
    }

    #[test]
    fn an_empty_doi_matches_nothing() {
        let mut db = BibDatabase::empty();
        db.add(BibEntry {
            entry_type: "book".into(),
            key: "Buffalo2015".into(),
            fields: vec![("isbn".into(), "9781449367374".into())],
        });
        assert!(!db.contains_doi(""));
    }

    #[test]
    fn keywords_are_split_on_commas() {
        let entry = BibEntry {
            entry_type: "article".into(),
            key: "Smith2020Deep".into(),
            fields: vec![("Keywords".into(), " mouse ,, immunology ".into())],
        };
        assert_eq!(entry.keywords(), vec!["mouse", "immunology"]);

        let untagged = BibEntry {
            entry_type: "article".into(),
            key: "Smith2020Deep".into(),
            fields: vec![("title".into(), "T".into())],
        };
        assert!(untagged.keywords().is_empty());
    }
}
