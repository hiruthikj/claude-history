//! Case-folded substring scanning over raw (un-normalized) transcript text.
//!
//! Every list row, highlight and quoted-literal filter finds needles in the
//! original `full_text`, so byte ranges can be cut out of it later. Text is
//! compared through full Unicode lowercasing (`İ` matches the two chars
//! `normalize_for_search` produces for it); the needle is expected to be
//! lowercased already.
//!
//! The scan jumps between candidate bytes with `memchr` and only decodes and
//! compares there: for an ASCII first char the candidates are its two cases
//! plus the lead byte of the one non-ASCII char that lowercases to it (`İ` →
//! `i̇`, Kelvin sign → `k`); for a non-ASCII first char every non-ASCII lead
//! byte is a candidate. A megabyte transcript costs well under a millisecond.

/// Reports each non-overlapping match of `needle` in `text` as a byte range
/// until `on_match` returns `false`.
///
/// With `word_start`, a match may only begin where the previous char is not
/// alphanumeric (the caller decides: a needle starting with punctuation such
/// as `.rs` carries its own boundary and passes `false`).
pub fn scan_folded(
    text: &str,
    needle: &[char],
    word_start: bool,
    mut on_match: impl FnMut((usize, usize)) -> bool,
) {
    let Some(&first) = needle.first() else {
        return;
    };
    let candidates = Candidates::for_first_char(first);
    let bytes = text.as_bytes();
    let mut pos = 0;

    while let Some(start) = candidates.next_from(bytes, pos) {
        let rest = &text[start..];
        let valid_start = !word_start
            || !text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        if valid_start && let Some(len) = match_folded_at(rest, needle) {
            let end = start + len;
            if !on_match((start, end)) {
                return;
            }
            pos = end;
            continue;
        }
        pos = start
            + rest
                .chars()
                .next()
                .expect("start is on a char boundary")
                .len_utf8();
    }
}

/// Where a match could begin, decided from the first byte alone so the scan
/// can skip everything else with SIMD.
enum Candidates {
    /// The needle starts with an ASCII char: its two cases, plus the lead byte
    /// of the one non-ASCII char (if any) whose lowercase begins with it.
    Ascii {
        lower: u8,
        upper: u8,
        extra_lead: Option<u8>,
    },
    /// The needle starts with a non-ASCII char, whose upper/lower forms may
    /// have different lead bytes: every non-ASCII lead byte is a candidate.
    NonAscii,
}

impl Candidates {
    fn for_first_char(first: char) -> Self {
        if !first.is_ascii() {
            return Self::NonAscii;
        }
        Self::Ascii {
            lower: first as u8,
            upper: first.to_ascii_uppercase() as u8,
            extra_lead: non_ascii_lead_lowercasing_to(first),
        }
    }

    fn next_from(&self, bytes: &[u8], pos: usize) -> Option<usize> {
        let rest = bytes.get(pos..)?;
        let offset = match *self {
            Self::Ascii {
                lower,
                upper,
                extra_lead: Some(lead),
            } => memchr::memchr3(lower, upper, lead, rest)?,
            Self::Ascii {
                lower,
                upper,
                extra_lead: None,
            } => memchr::memchr2(lower, upper, rest)?,
            Self::NonAscii => rest.iter().position(|&byte| byte >= 0xC0)?,
        };
        Some(pos + offset)
    }
}

/// The lead byte of the non-ASCII char whose full lowercase expansion begins
/// with `first`. Unicode has exactly two: `İ` (U+0130 → `i̇`) and the Kelvin
/// sign (U+212A → `k`); `every_non_ascii_char_lowercasing_to_ascii_is_known`
/// pins that.
fn non_ascii_lead_lowercasing_to(first: char) -> Option<u8> {
    match first {
        'i' => Some(0xC4),
        'k' => Some(0xE2),
        _ => None,
    }
}

/// First match only.
pub fn find_folded(text: &str, needle: &[char], word_start: bool) -> Option<(usize, usize)> {
    let mut first = None;
    scan_folded(text, needle, word_start, |range| {
        first = Some(range);
        false
    });
    first
}

/// All non-overlapping matches.
pub fn find_all_folded(text: &str, needle: &[char], word_start: bool) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    scan_folded(text, needle, word_start, |range| {
        ranges.push(range);
        true
    });
    ranges
}

/// Lowercases the needle with full Unicode expansion, as the text is folded
/// during the scan.
pub fn fold_needle(needle: &str) -> Vec<char> {
    needle.chars().flat_map(char::to_lowercase).collect()
}

/// Tries to match `needle` at the start of `text`, folding each text char,
/// and returns the byte length of the text consumed.
fn match_folded_at(text: &str, needle: &[char]) -> Option<usize> {
    let mut remaining = needle.iter();
    let mut expected = remaining.next()?;
    for (byte_start, ch) in text.char_indices() {
        for lowered in ch.to_lowercase() {
            if lowered != *expected {
                return None;
            }
            match remaining.next() {
                Some(next) => expected = next,
                None => return Some(byte_start + ch.len_utf8()),
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str, needle: &str, word_start: bool) -> Vec<(usize, usize)> {
        find_all_folded(text, &fold_needle(needle), word_start)
    }

    #[test]
    fn folds_ascii_case_both_ways() {
        assert_eq!(
            all("Rust rust RUST", "rust", false),
            vec![(0, 4), (5, 9), (10, 14)]
        );
        assert_eq!(
            all("Rust rust RUST", "RUST", false),
            vec![(0, 4), (5, 9), (10, 14)]
        );
    }

    #[test]
    fn word_start_requires_a_non_alphanumeric_predecessor() {
        assert_eq!(
            all("trust rust _rust 9rust", "rust", true),
            vec![(6, 10), (12, 16)]
        );
        assert_eq!(all("trust rust", "rust", false), vec![(1, 5), (6, 10)]);
    }

    #[test]
    fn punctuation_needles_match_inside_tokens() {
        assert_eq!(
            all("lexical.rs matcher.rs", ".rs", false),
            vec![(7, 10), (18, 21)]
        );
    }

    #[test]
    fn matches_do_not_overlap_and_boundary_follows_the_match() {
        assert_eq!(all("aaaa", "aa", false), vec![(0, 2), (2, 4)]);
        // After matching "ab" the next "ab" starts right after a letter.
        assert_eq!(all("abab ab", "ab", true), vec![(0, 2), (5, 7)]);
    }

    #[test]
    fn non_ascii_text_folds_through_unicode_lowercase() {
        let text = "pre İSTANBUL post";
        assert_eq!(all(text, "i\u{307}stanbul", false), vec![(4, 13)]);
        assert_eq!(&text[4..13], "İSTANBUL");
        // Kelvin sign lowercases to plain k.
        assert_eq!(all("\u{212A}elvin", "kelvin", true), vec![(0, 8)]);
        assert_eq!(
            all("größe", "grösse", false),
            Vec::new(),
            "ß does not fold to ss"
        );
        assert_eq!(all("GRÖSSE größe", "größe", false), vec![(8, 15)]);
    }

    #[test]
    fn non_ascii_needles_and_empty_needles() {
        assert_eq!(all("über Über", "über", false), vec![(0, 5), (6, 11)]);
        assert_eq!(all("anything", "", false), Vec::new());
        assert_eq!(
            find_folded("no match here", &fold_needle("zzz"), false),
            None
        );
    }

    #[test]
    fn every_non_ascii_char_lowercasing_to_ascii_is_known() {
        let found: Vec<(char, char)> = (0..=0x10FFFFu32)
            .filter_map(char::from_u32)
            .filter(|ch| !ch.is_ascii())
            .filter_map(|ch| {
                let first = ch.to_lowercase().next()?;
                first.is_ascii().then_some((ch, first))
            })
            .collect();
        assert_eq!(found, vec![('\u{0130}', 'i'), ('\u{212A}', 'k')]);
        for (ch, first) in found {
            let mut buf = [0u8; 4];
            let lead = ch.encode_utf8(&mut buf).as_bytes()[0];
            assert_eq!(non_ascii_lead_lowercasing_to(first), Some(lead));
        }
    }

    #[test]
    fn candidate_filter_keeps_byte_positions_on_char_boundaries() {
        // Multi-byte chars whose continuation bytes could look like ASCII
        // candidates must never be split.
        let text = "é→ê rust";
        assert_eq!(all(text, "rust", true), vec![(8, 12)]);
        assert_eq!(all(text, "ê", false), vec![(5, 7)]);
    }
}
