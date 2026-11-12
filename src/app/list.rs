//! Sortable, multi-selectable list model backing the folder and file tables.

use std::cmp::Ordering;
use std::collections::HashSet;

use eframe::egui::Modifiers;

/// Case-insensitive string comparison without allocating.
pub fn cmp_ignore_case(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortState<C> {
    pub column: C,
    pub ascending: bool,
}

impl<C: Copy + PartialEq> SortState<C> {
    /// Clicking the active column flips the direction; a new column starts ascending.
    pub fn toggle(&mut self, column: C) {
        if self.column == column {
            self.ascending = !self.ascending;
        } else {
            self.column = column;
            self.ascending = true;
        }
    }
}

/// Set of selected row names plus the anchor used for shift-click ranges.
#[derive(Default, Debug)]
pub struct Selection {
    names: HashSet<String>,
    anchor: Option<usize>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.names.clear();
        self.anchor = None;
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Apply a click on display row `idx`. `names` are the row names in display order.
    pub fn click(&mut self, idx: usize, names: &[&str], mods: Modifiers) {
        let Some(&name) = names.get(idx) else { return };
        let toggle = mods.command || mods.ctrl;
        match (mods.shift, self.anchor) {
            (true, Some(anchor)) => {
                let (lo, hi) = (anchor.min(idx), anchor.max(idx));
                if !toggle {
                    self.names.clear();
                }
                for n in &names[lo..=hi.min(names.len() - 1)] {
                    self.names.insert((*n).to_string());
                }
                // The anchor stays put so the range can be adjusted.
            }
            _ if toggle => {
                if !self.names.remove(name) {
                    self.names.insert(name.to_string());
                }
                self.anchor = Some(idx);
            }
            _ => {
                self.names.clear();
                self.names.insert(name.to_string());
                self.anchor = Some(idx);
            }
        }
    }

    /// Select only `idx` unless it's already part of the selection (right-click behaviour).
    pub fn right_click(&mut self, idx: usize, names: &[&str]) {
        let Some(&name) = names.get(idx) else { return };
        if !self.names.contains(name) {
            self.names.clear();
            self.names.insert(name.to_string());
            self.anchor = Some(idx);
        }
    }
}

/// Items, their sort order and the selection.
pub struct ListModel<T, C> {
    pub items: Vec<T>,
    /// Indices into `items` in display order.
    pub order: Vec<usize>,
    pub sort: SortState<C>,
    pub sel: Selection,
    cmp: fn(&T, &T, C) -> Ordering,
    name: fn(&T) -> &str,
}

impl<T, C: Copy + PartialEq> ListModel<T, C> {
    pub fn new(sort_column: C, cmp: fn(&T, &T, C) -> Ordering, name: fn(&T) -> &str) -> Self {
        Self {
            items: Vec::new(),
            order: Vec::new(),
            sort: SortState {
                column: sort_column,
                ascending: true,
            },
            sel: Selection::default(),
            cmp,
            name,
        }
    }

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.sel.clear();
        self.resort();
    }

    pub fn clear(&mut self) {
        self.set_items(Vec::new());
    }

    pub fn resort(&mut self) {
        let (items, cmp, asc, col) = (&self.items, self.cmp, self.sort.ascending, self.sort.column);
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by(|&a, &b| {
            let o = cmp(&items[a], &items[b], col);
            if asc {
                o
            } else {
                o.reverse()
            }
        });
        self.order = order;
    }

    pub fn toggle_sort(&mut self, column: C) {
        self.sort.toggle(column);
        self.resort();
    }

    /// Row names in display order.
    pub fn display_names(&self) -> Vec<&str> {
        self.order
            .iter()
            .map(|&i| (self.name)(&self.items[i]))
            .collect()
    }

    /// Names of the selected rows in display order.
    pub fn selected_names(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|&i| (self.name)(&self.items[i]))
            .filter(|n| self.sel.contains(n))
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };

    fn names() -> Vec<&'static str> {
        vec!["a", "b", "c", "d", "e"]
    }

    fn sel_sorted(s: &Selection) -> Vec<String> {
        let mut v: Vec<String> = s.names.iter().cloned().collect();
        v.sort();
        v
    }

    #[test]
    fn plain_ctrl_and_shift_clicks() {
        let n = names();
        let mut s = Selection::default();
        s.click(1, &n, NONE);
        assert_eq!(sel_sorted(&s), ["b"]);
        s.click(3, &n, CTRL);
        assert_eq!(sel_sorted(&s), ["b", "d"]);
        s.click(1, &n, CTRL); // toggles off
        assert_eq!(sel_sorted(&s), ["d"]);
        s.click(0, &n, NONE);
        s.click(2, &n, SHIFT);
        assert_eq!(sel_sorted(&s), ["a", "b", "c"]);
        s.click(4, &n, SHIFT); // anchor stays at 0
        assert_eq!(sel_sorted(&s), ["a", "b", "c", "d", "e"]);
        s.click(1, &n, SHIFT);
        assert_eq!(sel_sorted(&s), ["a", "b"]);
    }

    #[test]
    fn right_click_keeps_existing_selection() {
        let n = names();
        let mut s = Selection::default();
        s.click(0, &n, NONE);
        s.click(2, &n, CTRL);
        s.right_click(2, &n); // inside selection: unchanged
        assert_eq!(sel_sorted(&s), ["a", "c"]);
        s.right_click(4, &n); // outside: replaces
        assert_eq!(sel_sorted(&s), ["e"]);
    }

    #[test]
    fn sorting_toggles_and_is_case_insensitive() {
        #[derive(Clone, Copy, PartialEq)]
        enum Col {
            Name,
        }
        let mut m: ListModel<String, Col> =
            ListModel::new(Col::Name, |a, b, _| cmp_ignore_case(a, b), |s| s.as_str());
        m.set_items(vec!["banana".into(), "Apple".into(), "cherry".into()]);
        assert_eq!(m.display_names(), ["Apple", "banana", "cherry"]);
        m.toggle_sort(Col::Name);
        assert_eq!(m.display_names(), ["cherry", "banana", "Apple"]);
        let names: Vec<String> = m.display_names().into_iter().map(String::from).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        m.sel.click(0, &refs, NONE);
        assert_eq!(m.selected_names(), ["cherry"]);
        m.set_items(vec!["x".into()]);
        assert_eq!(m.sel.len(), 0, "new items clear the selection");
    }
}
