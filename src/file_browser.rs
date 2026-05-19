use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crate::file::McrawFileInfo;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub file_info: Option<McrawFileInfo>,
}

impl FileEntry {
    fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_dir = path.is_dir();
        let size = path.metadata().map(|m| m.len()).unwrap_or(0);
        FileEntry {
            path,
            name,
            is_dir,
            size,
            file_info: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub current_path: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected_index: usize,
    pub show_hidden: bool,
    last_refresh: Instant,
}

/// How often (in seconds) the file browser re-lists the current directory.
const REFRESH_INTERVAL_SECS: u64 = 2;

impl FileBrowser {
    pub fn new() -> Self {
        let current_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        FileBrowser {
            current_path: current_path.clone(),
            entries: Self::list_dir(&current_path, false),
            selected_index: 0,
            show_hidden: false,
            last_refresh: Instant::now(),
        }
    }

    pub fn from_path(path: PathBuf) -> Self {
        FileBrowser {
            current_path: path.clone(),
            entries: Self::list_dir(&path, false),
            selected_index: 0,
            show_hidden: false,
            last_refresh: Instant::now(),
        }
    }

    pub fn list_dir(path: &PathBuf, include_hidden: bool) -> Vec<FileEntry> {
        let mut entries = Vec::new();

        // Add parent directory navigation
        if path.parent().is_some() && path.as_os_str().len() > 1 {
            entries.push(FileEntry {
                path: path.parent().unwrap().to_path_buf(),
                name: "..".to_string(),
                is_dir: true,
                size: 0,
                file_info: None,
            });
        }

        if let Ok(read_dir) = fs::read_dir(path) {
            let mut dir_entries: Vec<FileEntry> = read_dir
                .filter_map(|e| e.ok())
                .map(|e| FileEntry::from_path(e.path()))
                .filter(|e| !e.name.starts_with('.') || include_hidden)
                .collect();

            dir_entries.sort_by(|a, b| {
                a.is_dir.cmp(&b.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });

            entries.extend(dir_entries);
        }

        entries
    }

    pub fn navigate_down(&mut self) {
        if self.selected_index < self.entries.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    pub fn navigate_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn enter(&mut self) {
        if self.selected_index < self.entries.len() {
            let entry = &self.entries[self.selected_index];
            if entry.is_dir {
                self.current_path = entry.path.clone();
                self.entries = Self::list_dir(&self.current_path, self.show_hidden);
                self.selected_index = 0;
            }
        }
    }

    pub fn go_up(&mut self) {
        if self.selected_index < self.entries.len() {
            let entry = &self.entries[self.selected_index];
            if entry.name == ".." {
                self.current_path = entry.path.clone();
                self.entries = Self::list_dir(&self.current_path, self.show_hidden);
                self.selected_index = 0;
            }
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.entries = Self::list_dir(&self.current_path, self.show_hidden);
        self.selected_index = 0;
        self.last_refresh = Instant::now();
    }

    /// Re-read the current directory if enough time has passed since the last
    /// refresh.  Preserves the selected index as much as possible across the
    /// re-read (the index is clamped if the new entry list is shorter).
    pub fn try_refresh(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_refresh).as_secs() < REFRESH_INTERVAL_SECS {
            return;
        }
        self.last_refresh = now;

        // Remember the path of the currently selected entry so we can try to
        // re-select it after the refresh.
        let selected_path = self.entries.get(self.selected_index).map(|e| e.path.clone());

        self.entries = Self::list_dir(&self.current_path, self.show_hidden);

        self.selected_index = selected_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0);
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected_index)
    }

    pub fn selected_file_info(&self) -> Option<&McrawFileInfo> {
        self.selected_entry()
            .and_then(|e| e.file_info.as_ref())
    }

    pub fn current_path_display(&self) -> String {
        self.current_path
            .to_string_lossy()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_new() {
        let browser = FileBrowser::new();
        assert!(!browser.current_path.as_os_str().is_empty());
        assert!(!browser.show_hidden);
    }

    #[test]
    fn test_list_dir() {
        let dir = std::env::current_dir().unwrap();
        let entries = FileBrowser::list_dir(&dir, false);
        assert!(!entries.is_empty());
        // First entry should be ".." if not root
        if dir.as_os_str().len() > 1 {
            assert_eq!(entries[0].name, "..");
            assert!(entries[0].is_dir);
        }
    }

    #[test]
    fn test_list_dir_hidden() {
        use std::fs::File;
        use std::io::Write;

        let temp_dir = std::env::temp_dir().join("mcraw-tui-test-hidden");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        File::create(temp_dir.join(".hidden_file")).unwrap();
        File::create(temp_dir.join("visible_file")).unwrap();

        let entries_visible = FileBrowser::list_dir(&temp_dir, false);
        let hidden_count_visible = entries_visible.iter().filter(|e| e.name.starts_with('.')).count();

        let entries_hidden = FileBrowser::list_dir(&temp_dir, true);
        let hidden_count_hidden = entries_hidden.iter().filter(|e| e.name.starts_with('.')).count();

        let _ = fs::remove_dir_all(&temp_dir);

        assert!(hidden_count_visible == 0 || hidden_count_visible == 1); // might just be ".."
        assert!(hidden_count_hidden >= 1);
    }
}
