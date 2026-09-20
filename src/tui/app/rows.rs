//! Per-result-set cache of the `full_text` scans behind visible list rows.
//!
//! `prepare_list_rows` runs from the frame loop before a draw and fills in
//! evidence for the rows the settled window will show; `render_list` then
//! only reads. Anything that changes what a row would say — a new result
//! set, a different query, a different width, a corpus refresh — changes the
//! key and drops the cache.

use super::{App, ListSearchMode};
use crate::search::QueryMatcher;
use crate::tui::list_layout::ListLayout;
use crate::tui::list_rows::{RowEvidence, RowSource, row_evidence};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RowCacheKey {
    results_version: u64,
    query: String,
    semantic_mode: bool,
    width: usize,
}

#[derive(Default)]
pub(super) struct RowEvidenceCache {
    key: RowCacheKey,
    rows: HashMap<usize, RowEvidence>,
}

impl App {
    /// Called whenever `filtered`, the semantic results or the corpus change.
    pub(super) fn bump_results_version(&mut self) {
        self.results_version = self.results_version.wrapping_add(1);
    }

    /// Ensure every row the next frame will show has its evidence computed.
    pub fn prepare_list_rows(&mut self, layout: &ListLayout) {
        let key = RowCacheKey {
            results_version: self.results_version,
            query: self.query.clone(),
            semantic_mode: self.list_search_mode == ListSearchMode::Semantic,
            width: usize::from(layout.list.width),
        };
        if self.row_evidence.key != key {
            // While the lexical worker is still ranking this query the rows on
            // screen belong to the previous result set; keep their evidence
            // rather than scanning rows that are about to be replaced. The
            // response goes through apply_filtered, which bumps the version.
            if self.search_in_flight {
                return;
            }
            self.row_evidence.key = key;
            self.row_evidence.rows.clear();
        }

        let offset = layout.scroll_offset(self.list_scroll, self.selected, self.filtered.len());
        let rows = layout.rows_per_page().max(1);
        let matcher = QueryMatcher::from_query(&self.query);
        let semantic_mode = self.list_search_mode == ListSearchMode::Semantic;
        let width = usize::from(layout.list.width);
        let mut computed = Vec::new();
        for &conv_idx in self.filtered.iter().skip(offset).take(rows) {
            if self.row_evidence.rows.contains_key(&conv_idx) {
                continue;
            }
            let source = RowSource {
                conversation: &self.conversations[conv_idx],
                matcher: &matcher,
                semantic_mode,
                semantic: self.semantic_search.results.get(&conv_idx),
                width,
                multiple_sources: self.multiple_sources,
            };
            computed.push((conv_idx, row_evidence(&source)));
        }
        self.row_evidence.rows.extend(computed);
    }

    /// Evidence prepared for a conversation, if the last `prepare_list_rows`
    /// covered it. The renderer falls back to computing it inline otherwise.
    pub fn row_evidence(&self, conversation_index: usize) -> Option<&RowEvidence> {
        self.row_evidence.rows.get(&conversation_index)
    }

    pub fn has_multiple_sources(&self) -> bool {
        self.multiple_sources
    }

    pub(super) fn refresh_multiple_sources(&mut self) {
        self.multiple_sources = multiple_sources(&self.conversations);
    }
}

pub(super) fn multiple_sources(conversations: &[crate::history::Conversation]) -> bool {
    let Some(first) = conversations.first() else {
        return false;
    };
    conversations.iter().any(|c| c.source != first.source)
}
