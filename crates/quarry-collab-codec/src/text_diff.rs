//! Minimal UTF-16 text diffs shared by the whole-file write path and the
//! session reconciler: the gateway folds review anchors and mark/link ranges
//! through these hunks, and the session splice edits only the changed spans
//! of a live block so collaborator cursors in untouched text survive.

use crate::rows::utf16_len;

/// The minimal common-prefix/suffix diff between two texts, in UTF-16 code
/// units. The changed span is `[prefix, old_mid_end)` in the old text and
/// `[prefix, new_mid_end)` in the new text; a suffix offset `o` maps to
/// `o - old_mid_end + new_mid_end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextDiff {
    pub prefix: u32,
    pub old_mid_end: u32,
    pub new_mid_end: u32,
}

impl TextDiff {
    pub fn is_pure_insertion(&self) -> bool {
        self.prefix == self.old_mid_end
    }

    pub fn shift_suffix(&self, offset: u32) -> u32 {
        offset - self.old_mid_end + self.new_mid_end
    }
}

pub fn utf16_text_diff(old: &str, new: &str) -> TextDiff {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let max_common = old_chars.len().min(new_chars.len());
    let mut prefix_chars = 0;
    while prefix_chars < max_common && old_chars[prefix_chars] == new_chars[prefix_chars] {
        prefix_chars += 1;
    }
    let mut suffix_chars = 0;
    while suffix_chars < max_common - prefix_chars
        && old_chars[old_chars.len() - 1 - suffix_chars]
            == new_chars[new_chars.len() - 1 - suffix_chars]
    {
        suffix_chars += 1;
    }
    let units = |chars: &[char]| chars.iter().map(|ch| ch.len_utf16() as u32).sum::<u32>();
    let prefix = units(&old_chars[..prefix_chars]);
    let old_suffix = units(&old_chars[old_chars.len() - suffix_chars..]);
    let new_suffix = units(&new_chars[new_chars.len() - suffix_chars..]);
    TextDiff {
        prefix,
        old_mid_end: utf16_len(old) - old_suffix,
        new_mid_end: utf16_len(new) - new_suffix,
    }
}

/// Changed middles larger than this (chars per side, after prefix/suffix
/// trimming) fall back to the single-hunk diff: Myers is O((N+M)·D), and a
/// fully rewritten middle has few surviving anchors to buy anyway.
pub const MULTI_HUNK_CHAR_LIMIT: usize = 4096;

/// The changed spans between two texts as [`TextDiff`] hunks, ascending. Each
/// hunk is expressed in the coordinates left by applying the hunks before it,
/// so adjustment is a fold of the single-hunk rules — anchors and ranges in
/// unchanged interior spans survive edits on both sides of them. Over
/// [`MULTI_HUNK_CHAR_LIMIT`] the diff falls back to the one
/// common-prefix/suffix hunk.
pub fn utf16_text_diff_hunks(old: &str, new: &str) -> Vec<TextDiff> {
    utf16_text_diff_hunks_bounded(old, new, MULTI_HUNK_CHAR_LIMIT)
}

pub fn utf16_text_diff_hunks_bounded(old: &str, new: &str, char_limit: usize) -> Vec<TextDiff> {
    let old_chars: Vec<(usize, char)> = old.char_indices().collect();
    let new_chars: Vec<(usize, char)> = new.char_indices().collect();
    let max_common = old_chars.len().min(new_chars.len());
    let mut prefix = 0usize;
    while prefix < max_common && old_chars[prefix].1 == new_chars[prefix].1 {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < max_common - prefix
        && old_chars[old_chars.len() - 1 - suffix].1 == new_chars[new_chars.len() - 1 - suffix].1
    {
        suffix += 1;
    }
    let old_mid_chars = old_chars.len() - prefix - suffix;
    let new_mid_chars = new_chars.len() - prefix - suffix;
    if old_mid_chars.max(new_mid_chars) > char_limit {
        return vec![utf16_text_diff(old, new)];
    }
    let old_mid = mid_slice(old, &old_chars, prefix, suffix);
    let new_mid = mid_slice(new, &new_chars, prefix, suffix);
    let prefix_units: u32 = old_chars[..prefix]
        .iter()
        .map(|(_, ch)| ch.len_utf16() as u32)
        .sum();

    // Walk the char-level changes, grouping contiguous non-equal runs into
    // hunks. `new_pos` tracks the UTF-16 position in the new text, which IS
    // the intermediate coordinate a hunk starts at (everything before it has
    // already been rewritten by earlier hunks).
    let mut hunks = Vec::new();
    let mut new_pos = prefix_units;
    let mut hunk_start: Option<u32> = None;
    let mut old_units = 0u32;
    let mut new_units = 0u32;
    for change in similar::TextDiff::from_chars(old_mid, new_mid).iter_all_changes() {
        let units = change.value().encode_utf16().count() as u32;
        match change.tag() {
            similar::ChangeTag::Equal => {
                if let Some(start) = hunk_start.take() {
                    hunks.push(TextDiff {
                        prefix: start,
                        old_mid_end: start + old_units,
                        new_mid_end: start + new_units,
                    });
                    old_units = 0;
                    new_units = 0;
                }
                new_pos += units;
            }
            similar::ChangeTag::Delete => {
                hunk_start.get_or_insert(new_pos);
                old_units += units;
            }
            similar::ChangeTag::Insert => {
                hunk_start.get_or_insert(new_pos);
                new_units += units;
                new_pos += units;
            }
        }
    }
    if let Some(start) = hunk_start {
        hunks.push(TextDiff {
            prefix: start,
            old_mid_end: start + old_units,
            new_mid_end: start + new_units,
        });
    }
    hunks
}

/// The byte slice between the common char prefix and suffix.
fn mid_slice<'a>(text: &'a str, chars: &[(usize, char)], prefix: usize, suffix: usize) -> &'a str {
    let start = chars.get(prefix).map_or(text.len(), |(byte, _)| *byte);
    let end = chars
        .get(chars.len() - suffix)
        .map_or(text.len(), |(byte, _)| *byte);
    &text[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_of_pure_insertion_is_collapsed_at_the_insertion_point() {
        let diff = utf16_text_diff("Hello world", "Hello brave world");
        assert_eq!(diff.prefix, 6);
        assert_eq!(diff.old_mid_end, 6);
        assert_eq!(diff.new_mid_end, 12);
        assert!(diff.is_pure_insertion());
    }

    #[test]
    fn diff_measures_utf16_units_for_surrogate_pairs() {
        // 😀 is one char but two UTF-16 code units.
        let diff = utf16_text_diff("a😀b", "a😀XYb");
        assert_eq!(diff.prefix, 3);
        assert_eq!(diff.old_mid_end, 3);
        assert_eq!(diff.new_mid_end, 5);
    }

    #[test]
    fn oversized_changed_middles_fall_back_to_the_single_hunk() {
        let hunks = utf16_text_diff_hunks_bounded("AAA middle ZZZ", "BBBB middle YY", 4);
        assert_eq!(
            hunks,
            vec![utf16_text_diff("AAA middle ZZZ", "BBBB middle YY")]
        );
    }
}
