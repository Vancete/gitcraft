//! Saved state, the active tab on start, the window icon and palette matching.

use super::palette::fuzzy;
use super::{Saved, active_among_kept, window_icon};
use crate::tab_look::{TabColor, TabLook};

/// A storage that keeps strings in memory, as eframe's own does on disk.
#[derive(Default)]
struct Memory(std::collections::HashMap<String, String>);

impl eframe::Storage for Memory {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn tab_looks_survive_a_restart() {
    let look = TabLook {
        icon: Some("rocket".to_owned()),
        color: TabColor::Purple,
    };
    let mut saved = Saved::default();
    saved.tab_looks.insert("C:/code/app".into(), look.clone());
    let mut storage = Memory::default();
    eframe::set_value(&mut storage, eframe::APP_KEY, &saved);
    let back: Option<Saved> = eframe::get_value(&storage, eframe::APP_KEY);
    assert!(
        back.is_some_and(|b| b.tab_looks.get(std::path::Path::new("C:/code/app")) == Some(&look))
    );
}

/// A tab whose folder is gone doesn't make the one after the active tab active.
#[test]
fn active_tab_survives_a_gone_folder() {
    assert_eq!(active_among_kept(&[true, true, true], 2), 2);
    assert_eq!(active_among_kept(&[false, true, true], 2), 1);
    assert_eq!(active_among_kept(&[true, false, true], 2), 1);
    assert_eq!(active_among_kept(&[true, true, false], 1), 1);
    assert_eq!(active_among_kept(&[true, false, true], 1), 0);
    assert_eq!(active_among_kept(&[false, true], 0), 0);
    assert_eq!(active_among_kept(&[], 3), 0);
    assert_eq!(active_among_kept(&[true, true], usize::MAX), 1);
}

#[test]
fn window_icon_decodes() {
    let icon = window_icon();
    assert_eq!((icon.width, icon.height), (256, 256));
    assert!(
        icon.rgba.chunks(4).any(|p| p[3] > 0),
        "the icon isn't blank"
    );
}

#[test]
fn fuzzy_matching() {
    assert!(fuzzy("check out main", "cmain"));
    assert!(fuzzy("check out main", "co main"));
    assert!(!fuzzy("fetch", "push"));
    assert!(fuzzy("anything", ""));
}
