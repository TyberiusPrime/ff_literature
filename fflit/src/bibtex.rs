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

pub struct BibDatabase {
    pub entries: Vec<BibEntry>,
    doi_set: HashSet<String>,
    sha256_set: HashSet<String>,
}

impl BibDatabase {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let bibtex = Bibtex::parse(&content)
            .map_err(|e| anyhow::anyhow!("bibtex parse error: {:?}", e))?;

        let mut entries = Vec::new();
        let mut doi_set = HashSet::new();
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
            sha256_set,
        })
    }

    pub fn empty() -> Self {
        Self {
            entries: vec![],
            doi_set: HashSet::new(),
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
        self.doi_set.contains(&normalize_doi(doi))
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
                out.push_str(&format!("  {} = {{{}}},\n", k, v));
            }
            out.push_str("}\n\n");
        }

        std::fs::write(path, out)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

fn normalize_doi(s: &str) -> String {
    s.trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:")
        .to_lowercase()
}

fn ascii_name(s: &str) -> String {
    let clean: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let mut chars = clean.chars();
    match chars.next() {
        None => String::from("Unknown"),
        Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
    }
}

const STOP_WORDS: &[&str] = &[
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
        "journal" => 4,
        "booktitle" => 4,
        "volume" => 5,
        "number" => 6,
        "pages" => 7,
        "publisher" => 8,
        "abstract" => 9,
        "sha256" => 10,
        _ => 50,
    }
}
