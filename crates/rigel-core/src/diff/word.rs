//! Word-level editing inside one line-level hunk (roadmap 123).
//!
//! Tokens keep their whitespace, so a change to spacing is visible and a Unicode word is never
//! cut inside a code point. The edit list is data; the caller decides how to paint it.

/// One token in the ordered edit script for a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordChange {
    /// Present on both sides.
    Same(String),
    /// Present only in the old text.
    Removed(String),
    /// Present only in the new text.
    Added(String),
}

impl WordChange {
    pub fn text(&self) -> &str {
        match self {
            Self::Same(text) | Self::Removed(text) | Self::Added(text) => text,
        }
    }
}

/// Split at Unicode whitespace boundaries, keeping every space as its own token. Punctuation
/// stays attached to a word; splitting it out would make a rename look like ten edits.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_is_space: Option<bool> = None;
    for character in text.chars() {
        let is_space = character.is_whitespace();
        if current_is_space.is_some_and(|previous| previous != is_space) && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current_is_space = Some(is_space);
        current.push(character);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Longest-common-subsequence edit script over tokens. Equal tokens keep their original spelling,
/// so case-only changes show as a removal and an addition rather than a silent substitution.
pub fn word_diff(old: &str, new: &str) -> Vec<WordChange> {
    let old_tokens = tokens(old);
    let new_tokens = tokens(new);
    let (n, m) = (old_tokens.len(), new_tokens.len());
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old_tokens[i] == new_tokens[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }

    let mut edits = Vec::new();
    let merge = |edits: &mut Vec<WordChange>, change: WordChange| {
        // Joining neighbouring edits of one kind keeps the output readable: a five-word removal
        // is one removed span, not five separate spans.
        if let Some(last) = edits.last_mut()
            && matches!(last, WordChange::Removed(_) | WordChange::Added(_))
            && std::mem::discriminant(&*last) == std::mem::discriminant(&change)
        {
            let joined = format!("{}{}", last.text(), change.text());
            *last = match change {
                WordChange::Removed(_) => WordChange::Removed(joined),
                WordChange::Added(_) => WordChange::Added(joined),
                WordChange::Same(_) => unreachable!("unchanged tokens are kept separate"),
            };
            return;
        }
        edits.push(change);
    };

    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old_tokens[i] == new_tokens[j] {
            merge(&mut edits, WordChange::Same(old_tokens[i].clone()));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            merge(&mut edits, WordChange::Removed(old_tokens[i].clone()));
            i += 1;
        } else {
            merge(&mut edits, WordChange::Added(new_tokens[j].clone()));
            j += 1;
        }
    }
    while i < n {
        merge(&mut edits, WordChange::Removed(old_tokens[i].clone()));
        i += 1;
    }
    while j < m {
        merge(&mut edits, WordChange::Added(new_tokens[j].clone()));
        j += 1;
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_a_token_so_spacing_changes_are_visible() {
        assert_eq!(tokens("one  two"), vec!["one", "  ", "two"]);
        assert_eq!(tokens(""), Vec::<String>::new());
        assert_eq!(tokens("  "), vec!["  "]);
    }

    #[test]
    fn a_unicode_word_is_never_split_and_whitespace_survives() {
        assert_eq!(tokens("naïve 世界"), vec!["naïve", " ", "世界"]);
        // Punctuation stays attached to a word, but a code point is never cut in half.
        assert_eq!(tokens("世界!"), vec!["世界!"]);
        let edits = word_diff("naïve 世界", "naïve 世界 again");
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("世界".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("\t".to_string())
                    || edit == &WordChange::Same(" ".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Added(" again".to_string()))
        );
    }

    #[test]
    fn neighbouring_edits_are_joined_into_one_span() {
        let removed = word_diff("one two three", "one three");
        assert!(
            removed
                .iter()
                .any(|edit| edit == &WordChange::Removed("two ".to_string()))
        );
        let added = word_diff("one three", "one two three");
        assert!(
            added
                .iter()
                .any(|edit| edit == &WordChange::Added("two ".to_string()))
        );
    }

    #[test]
    fn the_edit_script_reconstructs_both_sides() {
        for (old, new) in [
            ("same", "same"),
            ("a b", "a c d"),
            ("", "added"),
            ("gone", ""),
        ] {
            let edits = word_diff(old, new);
            let rebuilt_old: String = edits
                .iter()
                .filter(|edit| !matches!(edit, WordChange::Added(_)))
                .map(WordChange::text)
                .collect();
            let rebuilt_new: String = edits
                .iter()
                .filter(|edit| !matches!(edit, WordChange::Removed(_)))
                .map(WordChange::text)
                .collect();
            assert_eq!(rebuilt_old, old);
            assert_eq!(rebuilt_new, new);
        }
    }
}
