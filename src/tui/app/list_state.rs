use super::App;
use crate::history::Conversation;
use std::collections::HashSet;
use std::path::PathBuf;

impl App {
    pub(super) fn filter_indices<I>(&self, indices: I) -> Vec<usize>
    where
        I: IntoIterator<Item = usize>,
    {
        filter_conversation_indices(
            indices,
            &self.conversations,
            &self.excluded_projects,
            self.workspace_filter,
            self.current_project_dir_name.as_deref(),
            self.source_filter_root(),
        )
    }

    /// The root the list is narrowed to, if any.
    pub(crate) fn source_filter_root(&self) -> Option<&crate::history::SourceRoot> {
        self.source_filter
            .and_then(|index| self.sources.roots().get(index))
            .map(|root| root.as_ref())
    }

    pub(super) fn apply_filtered(&mut self, filtered: Vec<usize>) {
        self.filtered = filtered;
        self.list_scroll = 0;
        self.bump_results_version();
        self.selected = if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    pub(super) fn select_prev(&mut self) {
        if let Some(selected) = self.selected
            && selected > 0
        {
            self.selected = Some(selected - 1);
        }
    }

    pub(super) fn select_next(&mut self) {
        if let Some(selected) = self.selected
            && selected + 1 < self.filtered.len()
        {
            self.selected = Some(selected + 1);
        }
    }

    pub(super) fn select_first(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = Some(0);
        }
    }

    pub(super) fn select_last(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = Some(self.filtered.len() - 1);
        }
    }

    /// Rows one PageUp/PageDown moves: the rows that fit in the last prepared
    /// frame, or a fixed stride before any frame has been prepared.
    fn page_rows(&self) -> usize {
        const FALLBACK_PAGE_ROWS: usize = 10;
        if self.list_rows_per_page == 0 {
            FALLBACK_PAGE_ROWS
        } else {
            self.list_rows_per_page
        }
    }

    pub(super) fn select_page_up(&mut self) {
        let rows = self.page_rows();
        if let Some(selected) = self.selected {
            self.selected = Some(selected.saturating_sub(rows));
        }
    }

    pub(super) fn select_page_down(&mut self) {
        let rows = self.page_rows();
        if let Some(selected) = self.selected {
            let new_selected = (selected + rows).min(self.filtered.len().saturating_sub(1));
            self.selected = Some(new_selected);
        }
    }

    pub(super) fn select_half_page_down(&mut self) {
        let half_page = (self.page_rows() / 2).max(1);
        if let Some(selected) = self.selected {
            let new_selected = (selected + half_page).min(self.filtered.len().saturating_sub(1));
            self.selected = Some(new_selected);
        }
    }

    pub(super) fn scroll_list(&mut self, delta: isize) {
        let Some(selected) = self.selected else {
            return;
        };

        let max = self.filtered.len().saturating_sub(1);
        let new_selected = if delta >= 0 {
            selected.saturating_add(delta as usize).min(max)
        } else {
            selected.saturating_sub((-delta) as usize)
        };
        self.selected = Some(new_selected);
    }

    pub(super) fn get_selected_path(&self) -> Option<PathBuf> {
        self.selected
            .and_then(|sel| self.filtered.get(sel))
            .map(|&idx| self.conversations[idx].path.clone())
    }

    pub(crate) fn get_selected_source(&self) -> Option<crate::history::Source> {
        self.get_selected_conversation_index()
            .map(|index| self.conversations[index].source)
    }

    /// The roots a delete of the selected Claude session may touch: its own
    /// root, or every Claude root when it was not loaded from one.
    pub(crate) fn selected_delete_roots(&self) -> Vec<std::sync::Arc<crate::history::SourceRoot>> {
        let origin = self
            .get_selected_conversation_index()
            .and_then(|index| self.conversations[index].origin.clone());
        match origin {
            Some(root) => vec![root],
            None => self
                .sources
                .of_kind(crate::history::Source::Claude)
                .cloned()
                .collect(),
        }
    }

    pub(super) fn get_selected_conversation_index(&self) -> Option<usize> {
        self.selected
            .and_then(|sel| self.filtered.get(sel))
            .copied()
    }

    pub(crate) fn remove_selected_from_list(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(&conv_idx) = self.filtered.get(selected) else {
            return;
        };

        self.conversations.remove(conv_idx);

        self.searchable.retain_mut(|s| {
            if s.index == conv_idx {
                false
            } else {
                if s.index > conv_idx {
                    s.index -= 1;
                }
                true
            }
        });

        self.filtered.retain(|&idx| idx != conv_idx);
        for idx in &mut self.filtered {
            if *idx > conv_idx {
                *idx -= 1;
            }
        }

        if self.filtered.is_empty() {
            self.selected = None;
        } else if selected >= self.filtered.len() {
            self.selected = Some(self.filtered.len() - 1);
        }

        self.refresh_search_data();
    }
}

pub(super) fn filter_conversation_indices<I>(
    indices: I,
    conversations: &[Conversation],
    excluded_projects: &HashSet<String>,
    workspace_filter: bool,
    current_project_dir_name: Option<&str>,
    source_filter: Option<&crate::history::SourceRoot>,
) -> Vec<usize>
where
    I: IntoIterator<Item = usize>,
{
    indices
        .into_iter()
        .filter(|&idx| {
            source_filter.is_none_or(|root| {
                conversations[idx]
                    .origin
                    .as_deref()
                    .is_some_and(|origin| std::ptr::eq(origin, root))
            })
        })
        .filter(|&idx| {
            conversations[idx]
                .project_name
                .as_deref()
                .is_none_or(|name| !project_is_excluded(name, excluded_projects))
        })
        .filter(|&idx| {
            let Some(project_dir_name) = current_project_dir_name.filter(|_| workspace_filter)
            else {
                return true;
            };
            if conversations[idx].source != crate::history::Source::Claude {
                let Ok(current) = std::env::current_dir() else {
                    return false;
                };
                let current = current.canonicalize().unwrap_or(current);
                return conversations[idx]
                    .project_path
                    .as_ref()
                    .or(conversations[idx].cwd.as_ref())
                    .is_some_and(|path| {
                        path.canonicalize().unwrap_or_else(|_| path.clone()) == current
                    });
            }
            conversations[idx]
                .path
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|name| {
                    crate::history::path::is_same_project(&name.to_string_lossy(), project_dir_name)
                })
        })
        .collect()
}

fn project_is_excluded(project_name: &str, excluded_projects: &HashSet<String>) -> bool {
    excluded_projects.contains(project_name)
        || project_name
            .split_once('/')
            .is_some_and(|(parent, _)| excluded_projects.contains(parent))
}
