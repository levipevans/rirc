//! State for the `/list` channel browser.

use crate::util;

#[derive(Clone, Debug)]
pub struct ListEntry {
    pub channel: String,
    pub users: u32,
    pub topic: String,
    search: String,
}

impl ListEntry {
    pub fn new(channel: impl Into<String>, users: u32, topic: impl Into<String>) -> Self {
        let channel = channel.into();
        let topic = topic.into();
        let search = format!("{} {}", util::fold(&channel), util::fold(&topic));
        Self {
            channel,
            users,
            topic,
            search,
        }
    }
}

#[derive(Debug)]
pub struct ChannelList {
    pub server: usize,
    pub server_name: String,
    pub args: String,
    pub entries: Vec<ListEntry>,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: String,
    pub filtering: bool,
    /// Index into `entries`.
    pub selected: usize,
}

impl ChannelList {
    pub fn new(server: usize, server_name: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            server,
            server_name: server_name.into(),
            args: args.into(),
            entries: Vec::new(),
            loading: true,
            error: None,
            filter: String::new(),
            filtering: false,
            selected: 0,
        }
    }

    pub fn add(&mut self, entry: ListEntry) {
        self.entries.push(entry);
        self.normalize_selection();
    }

    pub fn finish(&mut self) {
        self.loading = false;
        self.normalize_selection();
    }

    pub fn fail(&mut self, message: &str) {
        self.loading = false;
        self.error = Some(util::sanitize_line(message));
    }

    /// Indices of entries matching the current filter. Multiple whitespace
    /// separated terms must all appear in the channel name or topic.
    pub fn filtered(&self) -> Vec<usize> {
        let terms: Vec<String> = self
            .filter
            .split_whitespace()
            .map(util::fold)
            .filter(|term| !term.is_empty())
            .collect();
        if terms.is_empty() {
            return (0..self.entries.len()).collect();
        }
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                if terms.iter().all(|term| entry.search.contains(term)) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn normalize_selection(&mut self) {
        let filtered = self.filtered();
        if filtered.is_empty() {
            self.selected = 0;
            return;
        }
        if !filtered.contains(&self.selected) {
            self.selected = filtered[0];
        }
    }

    pub fn selected_entry(&self) -> Option<&ListEntry> {
        self.entries.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let filtered = self.filtered();
        if filtered.is_empty() {
            return;
        }
        let position = filtered
            .iter()
            .position(|&idx| idx == self.selected)
            .unwrap_or(0);
        let max = filtered.len() as isize - 1;
        let next = (position as isize + delta).clamp(0, max) as usize;
        self.selected = filtered[next];
    }

    pub fn jump(&mut self, end: bool) {
        let filtered = self.filtered();
        if let Some(&idx) = if end {
            filtered.last()
        } else {
            filtered.first()
        } {
            self.selected = idx;
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.normalize_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> ChannelList {
        let mut list = ChannelList::new(1, "TestNet", "");
        list.add(ListEntry::new(
            "#rust",
            1200,
            "The Rust programming language",
        ));
        list.add(ListEntry::new(
            "#rust-beginners",
            80,
            "New to Rust? Start here",
        ));
        list.add(ListEntry::new("#vim", 400, "The editor"));
        list
    }

    #[test]
    fn filters_by_name_and_topic() {
        let mut list = list();
        list.filter = "rust".to_string();
        list.normalize_selection();
        let filtered = list.filtered();
        assert_eq!(filtered.len(), 2);
        list.filter = "beginners".to_string();
        list.normalize_selection();
        assert_eq!(list.selected, 1);
        list.filter = "nothing-like-this".to_string();
        list.normalize_selection();
        assert!(list.filtered().is_empty());
        assert_eq!(list.selected, 0);
    }

    #[test]
    fn moves_only_through_visible_entries() {
        let mut list = list();
        list.filter = "rust".to_string();
        list.normalize_selection();
        list.selected = 2;
        list.move_selection(1);
        assert_eq!(list.selected, 1);
        list.move_selection(5);
        assert_eq!(list.selected, 1);
        list.move_selection(-1);
        assert_eq!(list.selected, 0);
        list.jump(true);
        assert_eq!(list.selected, 1);
    }
}
