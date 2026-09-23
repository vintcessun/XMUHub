//! Text normalisation shared by indexing and querying.
//!
//! Chinese text is indexed as character unigrams + bigrams (the classic CJK approach:
//! no dictionary to hold in RAM, and any substring of two or more characters matches).
//! ASCII words are indexed whole plus every prefix, so course codes and pinyin
//! abbreviations match while the user is still typing.

use pinyin::ToPinyin;

const MAX_PREFIX: usize = 24;

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

enum Run {
    Cjk(Vec<char>),
    Word(String),
}

fn runs(text: &str) -> Vec<Run> {
    let mut out = Vec::new();
    let mut cjk = Vec::new();
    let mut word = String::new();
    let flush = |cjk: &mut Vec<char>, word: &mut String, out: &mut Vec<Run>| {
        if !cjk.is_empty() {
            out.push(Run::Cjk(std::mem::take(cjk)));
        }
        if !word.is_empty() {
            out.push(Run::Word(std::mem::take(word)));
        }
    };
    for c in text.chars() {
        if is_cjk(c) {
            if !word.is_empty() {
                out.push(Run::Word(std::mem::take(&mut word)));
            }
            cjk.push(c);
        } else if c.is_alphanumeric() {
            if !cjk.is_empty() {
                out.push(Run::Cjk(std::mem::take(&mut cjk)));
            }
            word.extend(c.to_lowercase());
        } else {
            flush(&mut cjk, &mut word, &mut out);
        }
    }
    flush(&mut cjk, &mut word, &mut out);
    out
}

fn push_prefixes(word: &str, out: &mut Vec<String>) {
    for (n, (i, c)) in word.char_indices().enumerate() {
        if n >= MAX_PREFIX {
            break;
        }
        out.push(word[..i + c.len_utf8()].to_string());
    }
}

/// Tokens stored in the index for `text`.
pub fn index_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for run in runs(text) {
        match run {
            Run::Cjk(chars) => {
                for (i, c) in chars.iter().enumerate() {
                    out.push(c.to_string());
                    if let Some(n) = chars.get(i + 1) {
                        out.push(format!("{c}{n}"));
                    }
                }
            }
            Run::Word(w) => push_prefixes(&w, &mut out),
        }
    }
    out
}

/// Query units: each unit should match for a document to be relevant.
/// A lone Chinese character stays a unigram; longer runs become bigrams.
pub fn query_units(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    for run in runs(q) {
        match run {
            Run::Cjk(chars) if chars.len() == 1 => out.push(chars[0].to_string()),
            Run::Cjk(chars) => {
                for w in chars.windows(2) {
                    out.push(format!("{}{}", w[0], w[1]));
                }
            }
            Run::Word(w) => out.push(w.chars().take(MAX_PREFIX).collect()),
        }
    }
    out.dedup();
    out
}

/// Looser units for a second pass: every Chinese character on its own, so abbreviations
/// such as "高数" still find "高等数学" (all characters present, not adjacent).
pub fn loose_units(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for run in runs(q) {
        match run {
            Run::Cjk(chars) => {
                for c in chars {
                    let s = c.to_string();
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
            Run::Word(w) => out.push(w.chars().take(MAX_PREFIX).collect()),
        }
    }
    out
}

/// Pinyin forms of the Chinese in `text`: full spelling and initials of each run,
/// e.g. "高等数学" → "gaodengshuxue gdsx".
pub fn pinyin_forms(text: &str) -> String {
    let mut out = String::new();
    for run in runs(text) {
        if let Run::Cjk(chars) = run {
            let s: String = chars.iter().collect();
            let (mut full, mut initials) = (String::new(), String::new());
            for p in s.as_str().to_pinyin().flatten() {
                full.push_str(p.plain());
                initials.push_str(p.first_letter());
            }
            if !full.is_empty() {
                out.push_str(&full);
                out.push(' ');
                out.push_str(&initials);
                out.push(' ');
            }
        }
    }
    out
}

/// ASCII-only rendering of a filename. GitHub mangles non-ASCII asset names,
/// so Chinese is transliterated to pinyin instead of being lost.
pub fn ascii_filename(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 10 && e.chars().all(|c| c.is_ascii_alphanumeric()) => {
            (s, Some(e.to_ascii_lowercase()))
        }
        _ => (name, None),
    };
    let mut out = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if let Some(p) = c.to_pinyin() {
            out.push_str(p.plain());
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 80 {
            break;
        }
    }
    let out = out.trim_matches('_');
    let stem = if out.is_empty() { "file" } else { out };
    match ext {
        Some(e) => format!("{stem}.{e}"),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let t = index_tokens("高等数学A 2023");
        assert!(t.contains(&"高等".to_string()));
        assert!(t.contains(&"学".to_string()));
        assert!(t.contains(&"a".to_string()));
        assert!(t.contains(&"202".to_string()));
        assert_eq!(query_units("高数 MA101"), vec!["高数", "ma101"]);
        assert_eq!(query_units("数"), vec!["数"]);
    }

    #[test]
    fn pinyin() {
        assert_eq!(pinyin_forms("高等数学").trim(), "gaodengshuxue gdsx");
        assert_eq!(ascii_filename("高数 期末(2023).PDF"), "gaoshu_qimo_2023.pdf");
    }
}
