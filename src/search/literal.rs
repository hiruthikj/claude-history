use crate::history::Conversation;
use crate::search::scan;
use rayon::prelude::*;
use std::borrow::Borrow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    Sensitive,
    Insensitive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    text: String,
    case_mode: CaseMode,
}

impl Literal {
    pub fn new(text: String) -> Self {
        let case_mode = if text.chars().any(char::is_uppercase) {
            CaseMode::Sensitive
        } else {
            CaseMode::Insensitive
        };
        Self { text, case_mode }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn case_mode(&self) -> CaseMode {
        self.case_mode
    }

    pub fn matches(&self, text: &str) -> bool {
        if self.text.is_empty() {
            return false;
        }

        match self.case_mode {
            CaseMode::Sensitive => text.contains(&self.text),
            CaseMode::Insensitive => contains_case_insensitive(text, &self.text),
        }
    }

    pub fn match_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        if self.text.is_empty() {
            return Vec::new();
        }

        match self.case_mode {
            CaseMode::Sensitive => find_substring_ranges(text, &self.text),
            CaseMode::Insensitive => find_case_insensitive_ranges(text, &self.text),
        }
    }
}

fn contains_case_insensitive(text: &str, needle: &str) -> bool {
    scan::find_folded(text, &scan::fold_needle(needle), false).is_some()
}

fn find_substring_ranges(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find(needle) {
        let range_start = start + pos;
        let range_end = range_start + needle.len();
        ranges.push((range_start, range_end));
        start = range_end;
    }
    ranges
}

fn find_case_insensitive_ranges(text: &str, needle: &str) -> Vec<(usize, usize)> {
    scan::find_all_folded(text, &scan::fold_needle(needle), false)
}

/// A literal is satisfied by any one part of the conversation's raw text:
/// the transcript, the agent-only text when the caller searches it, or the
/// project name. Parts are not joined, so a literal never matches across the
/// boundary between them.
fn literal_matches_conversation(
    conversation: &Conversation,
    literal: &Literal,
    include_agent_text: bool,
) -> bool {
    literal.matches(&conversation.full_text)
        || (include_agent_text && literal.matches(&conversation.agent_search_text))
        || conversation
            .project_name
            .as_deref()
            .is_some_and(|name| literal.matches(name))
}

pub fn conversation_matches_all_literals(
    conversation: &Conversation,
    literals: &[Literal],
    include_agent_text: bool,
) -> bool {
    literals
        .iter()
        .all(|literal| literal_matches_conversation(conversation, literal, include_agent_text))
}

/// Conversations in `scope` that contain every literal, newest first.
pub fn exact_fallback<C: Borrow<Conversation> + Sync>(
    conversations: &[C],
    literals: &[Literal],
    include_agent_text: bool,
    scope: impl Fn(usize) -> bool + Sync,
) -> Vec<usize> {
    if literals.is_empty() {
        return Vec::new();
    }

    let mut matches = conversations
        .par_iter()
        .enumerate()
        .filter(|(index, conversation)| {
            let conversation: &Conversation = (*conversation).borrow();
            scope(*index)
                && conversation_matches_all_literals(conversation, literals, include_agent_text)
        })
        .map(|(index, conversation)| {
            let conversation: &Conversation = conversation.borrow();
            (index, conversation.timestamp)
        })
        .collect::<Vec<_>>();

    matches.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    matches.into_iter().map(|(index, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::test_fixtures::one_message_conversation;
    use chrono::{Duration, Local};

    fn make_conv_full(
        text: &str,
        project: Option<&str>,
        title: Option<&str>,
        summary: Option<&str>,
        timestamp: chrono::DateTime<Local>,
    ) -> Conversation {
        one_message_conversation(text, timestamp, summary, title, project)
    }

    #[test]
    fn literal_uses_smart_case() {
        assert!(Literal::new("exact_phrase".to_string()).matches("EXACT_PHRASE"));
        assert!(!Literal::new("Exact_Phrase".to_string()).matches("exact_phrase"));
        assert!(Literal::new("Exact_Phrase".to_string()).matches("Exact_Phrase"));
    }

    #[test]
    fn literal_ranges_use_smart_case() {
        let insensitive = Literal::new("exact_phrase".to_string());
        let sensitive = Literal::new("Exact_Phrase".to_string());

        assert_eq!(insensitive.match_ranges("EXACT_PHRASE"), vec![(0, 12)]);
        assert_eq!(sensitive.match_ranges("exact_phrase"), Vec::new());
        assert_eq!(sensitive.match_ranges("Exact_Phrase"), vec![(0, 12)]);
    }

    #[test]
    fn insensitive_literal_ranges_are_original_text_boundaries() {
        let text = "pre İSTANBUL post";
        let literal = Literal::new("i\u{307}stanbul".to_string());
        let ranges = literal.match_ranges(text);

        assert_eq!(ranges, vec![(4, 13)]);
        assert_eq!(&text[ranges[0].0..ranges[0].1], "İSTANBUL");
    }

    #[test]
    fn insensitive_literal_match_uses_smart_case() {
        let literal = Literal::new("i\u{307}stanbul".to_string());

        assert!(literal.matches("pre İSTANBUL post"));
        assert!(!literal.matches("pre constantinople post"));
    }

    #[test]
    fn literals_match_raw_text_metadata_and_project_name() {
        let now = Local::now();
        let conversation = make_conv_full(
            "body_with_under_score and punctuation: yes",
            Some("project_name/raw-value"),
            Some("Title: Raw_Value"),
            Some("Summary.with punctuation"),
            now,
        );
        let matches = |text: &str| {
            conversation_matches_all_literals(
                &conversation,
                &[Literal::new(text.to_string())],
                false,
            )
        };

        assert!(matches("body_with_under_score"));
        assert!(matches("punctuation: yes"));
        assert!(matches("project_name/raw-value"));
        assert!(matches("Title: Raw_Value"));
        assert!(matches("Summary.with punctuation"));
        assert!(!matches("yes project_name"), "parts are not joined");
    }

    #[test]
    fn agent_text_is_searched_only_when_asked() {
        let now = Local::now();
        let mut conversation = make_conv_full("body", None, None, None, now);
        conversation.agent_search_text = "agent_only_token".to_string();
        let literals = [Literal::new("agent_only_token".to_string())];

        assert!(!conversation_matches_all_literals(
            &conversation,
            &literals,
            false
        ));
        assert!(conversation_matches_all_literals(
            &conversation,
            &literals,
            true
        ));
    }

    #[test]
    fn exact_fallback_returns_scoped_matches_newest_first() {
        let now = Local::now();
        let conversations = vec![
            make_conv_full("needle phrase", None, None, None, now - Duration::days(1)),
            make_conv_full("needle phrase", None, None, None, now),
            make_conv_full("needle phrase", None, None, None, now - Duration::hours(1)),
        ];
        let literal = Literal::new("needle phrase".to_string());

        let results = exact_fallback(&conversations, &[literal], false, |index| index != 1);

        assert_eq!(results, vec![2, 0]);
    }

    #[test]
    fn exact_fallback_requires_all_literals() {
        let now = Local::now();
        let conversations = vec![
            make_conv_full("alpha beta", None, None, None, now),
            make_conv_full("alpha gamma", None, None, None, now),
        ];
        let literals = vec![
            Literal::new("alpha".to_string()),
            Literal::new("beta".to_string()),
        ];

        let results = exact_fallback(&conversations, &literals, false, |_| true);

        assert_eq!(results, vec![0]);
    }
}
