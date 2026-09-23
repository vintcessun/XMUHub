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
            // 绿 → "lü": write ü as v (the usual keyboard spelling) to stay ASCII.
            out.extend(p.plain().chars().map(|c| if c == 'ü' { 'v' } else { c }).filter(char::is_ascii));
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
        assert_eq!(ascii_filename("绿色化学_Müller.pdf"), "lvsehuaxue_M_ller.pdf");
    }
}

// ---------------------------------------------------------------- name guessing

/// Name parts guessed from an original file name or path (same rules as the upload page).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guess {
    pub time: String,
    pub type_word: &'static str,
    pub paper: String,
    pub with_answer: bool,
    pub extra: String,
}

/// Rule-based recognition of 学年/学期/类型/卷别/含答案 from free-form names such as
/// "2023-2024学年第一学期微积分I-1期中试卷答案.pdf".
pub fn guess_name(text: &str, ext: &str) -> Guess {
    use regex::Regex;
    use std::sync::LazyLock;
    static YEARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(20\d{2})\s*[-–~至]\s*(20\d{2})").unwrap());
    static SHORT_YEARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\d])(\d{2})\s*[-–]\s*(\d{2})(?:[^\d]|$)").unwrap());
    static MONTH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\d])(20\d{2})(0[1-9]|1[0-2])(?:[^\d]|$)").unwrap());
    static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\d])(20\d{2})(?:[^\d]|$)").unwrap());
    static PAPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)([ABC])\s*卷").unwrap());
    static ANSWER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)答案|解答|解析|参考答案|评分标准|solution|\bsln\b|\bkey\b").unwrap());
    static COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{2,4})\s*题").unwrap());
    static IS_PAPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)试卷|试题|考试|真题|[ABC]\s*卷").unwrap());

    let mut g = Guess::default();
    if let Some(c) = YEARS.captures(text) {
        g.time = format!("{}-{}", &c[1], &c[2]);
    } else if let Some(c) = SHORT_YEARS.captures(text).filter(|c| c[2].parse::<u32>().ok() == c[1].parse::<u32>().ok().map(|y| y + 1)) {
        g.time = format!("20{}-20{}", &c[1], &c[2]);
    } else if let Some(c) = MONTH.captures(text) {
        g.time = format!("{}{}", &c[1], &c[2]);
    } else if let Some(c) = YEAR.captures(text) {
        g.time = c[1].to_string();
    }
    let term = if text.contains("第一学期") || text.contains("秋") {
        "秋"
    } else if text.contains("第二学期") || text.contains("春") {
        "春"
    } else if text.contains("暑") {
        "暑"
    } else {
        ""
    };
    if !term.is_empty() && !g.time.is_empty() && g.time.len() != 6 {
        g.time.push_str(term);
    }
    if let Some(c) = PAPER.captures(text) {
        g.paper = format!("{}卷", c[1].to_uppercase());
    }
    g.with_answer = ANSWER.is_match(text);
    if let Some(c) = COUNT.captures(text) {
        g.extra = format!("{}题", &c[1]);
    }
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    // "期中复习重点" is notes, not a paper: study-material words win unless it says 试卷/考试.
    let notes: &[(&[&str], &str)] = &[
        (&["思考题"], "思考题"),
        (&["题库", "刷题"], "题库"),
        (&["单词"], "单词表"),
        (&["提纲", "大纲"], "提纲"),
        (&["重点", "考点", "复习"], "重点"),
        (&["笔记"], "笔记"),
    ];
    let rules: &[(&[&str], &str)] = &[
        (&["期中", "midterm"], "期中试卷"),
        (&["期末", "final"], "期末试卷"),
        (&["小测", "测验", "quiz"], "小测"),
        (&["思考题"], "思考题"),
        (&["题库", "刷题", "选择题"], "题库"),
        (&["真题", "试卷", "试题", "往年", "卷", "exam"], "往年试卷"),
        (&["单词"], "单词表"),
        (&["提纲", "大纲"], "提纲"),
        (&["重点", "复习", "考点"], "重点"),
        (&["笔记", "note"], "笔记"),
        (&["实验", "报告", "lab"], "实验报告"),
        (&["讲义"], "讲义"),
        (&["课件", "slide", "lecture", "ppt"], "课件"),
        (&["教材", "课本", "教科书", "edition"], "教材"),
        (&["模板", "template"], "模板"),
    ];
    if !IS_PAPER.is_match(text) {
        if let Some((_, w)) = notes.iter().find(|(ws, _)| has(ws)) {
            g.type_word = w;
        }
    }
    if g.type_word.is_empty() {
        if let Some((_, w)) = rules.iter().find(|(ws, _)| has(ws)) {
            g.type_word = w;
        }
    }
    if g.type_word.is_empty() {
        g.type_word = match ext {
            "ppt" | "pptx" | "key" => "课件",
            "zip" | "rar" | "7z" => "合集",
            "html" | "exe" | "py" => "工具",
            "epub" => "教材",
            _ if g.with_answer || !g.paper.is_empty() => "往年试卷",
            _ => "资料",
        };
    }
    g
}

#[cfg(test)]
mod guess_tests {
    use super::guess_name;

    #[test]
    fn guesses() {
        let g = guess_name("2023-2024学年第一学期微积分I-1期中试卷A卷答案", "pdf");
        assert_eq!((g.time.as_str(), g.type_word, g.paper.as_str(), g.with_answer), ("2023-2024秋", "期中试卷", "A卷", true));
        assert_eq!(guess_name("24-25 思修 期中 复习重点", "docx").type_word, "重点");
        assert_eq!(guess_name("第三章 课件", "pptx").type_word, "课件");
        assert_eq!(guess_name("英语四级202412", "zip").time, "202412");
        assert_eq!(guess_name("lab3", "zip").type_word, "实验报告");
    }
}
