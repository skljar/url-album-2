mod db;

use db::Database;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

slint::include_modules!();

struct State {
    db: Database,
    expanded: HashSet<i64>,
    active_folder: Option<i64>,
    selected_bookmark: Option<i64>,
    search_query: String,
}

impl State {
    fn new(db: Database) -> Self {
        State { db, expanded: HashSet::new(), active_folder: None, selected_bookmark: None, search_query: String::new() }
    }

    fn build_folder_model(&self) -> ModelRc<FolderNode> {
        let all = self.db.get_all_folders().unwrap_or_default();
        let mut children: std::collections::HashMap<Option<i64>, Vec<usize>> = Default::default();
        for (i, f) in all.iter().enumerate() { children.entry(f.parent_id).or_default().push(i); }
        let mut result: Vec<FolderNode> = Vec::new();
        Self::walk(&all, &children, &self.expanded, self.active_folder, None, 0, &mut result);
        ModelRc::new(VecModel::from(result))
    }

    fn walk(all: &[db::DbFolder], children: &std::collections::HashMap<Option<i64>, Vec<usize>>,
            expanded: &HashSet<i64>, active: Option<i64>, parent: Option<i64>, depth: i32, out: &mut Vec<FolderNode>) {
        if let Some(kids) = children.get(&parent) {
            for &i in kids {
                let f = &all[i];
                let has_ch = children.contains_key(&Some(f.id));
                out.push(FolderNode {
                    id: f.id as i32, title: SharedString::from(f.title.as_str()),
                    depth, expanded: expanded.contains(&f.id),
                    has_children: has_ch, selected: active == Some(f.id),
                });
                if expanded.contains(&f.id) {
                    Self::walk(all, children, expanded, active, Some(f.id), depth + 1, out);
                }
            }
        }
    }

    fn build_bookmark_model(&self) -> ModelRc<BookmarkItem> {
        let bms = if !self.search_query.is_empty() {
            self.db.search(&self.search_query).unwrap_or_default()
        } else {
            self.active_folder.map(|id| self.db.get_bookmarks(id).unwrap_or_default()).unwrap_or_default()
        };
        let vec: Vec<BookmarkItem> = bms.into_iter().map(|b| BookmarkItem {
            id: b.id as i32, title: SharedString::from(b.title.as_str()),
            url: SharedString::from(b.url.as_deref().unwrap_or("")),
            note: SharedString::from(b.note.as_deref().unwrap_or("")),
            selected: self.selected_bookmark == Some(b.id),
        }).collect();
        ModelRc::new(VecModel::from(vec))
    }

    fn status(&self) -> SharedString {
        if !self.search_query.is_empty() {
            let n = self.db.search(&self.search_query).unwrap_or_default().len();
            return SharedString::from(format!("Поиск: \"{}\"  |  Найдено: {n}", self.search_query));
        }
        let (folders, bookmarks) = self.db.total_counts();
        match self.active_folder {
            Some(id) => SharedString::from(format!(
                "Ссылок: {}  |  Всего: папок {folders}, ссылок {bookmarks}  |  Enter=открыть  Del=удалить  F2=переименовать",
                self.db.bookmark_count(id))),
            None => SharedString::from(format!("Папок: {folders}  |  Ссылок: {bookmarks}  |  Выберите папку")),
        }
    }

    fn selected_name(&self) -> String {
        if let Some(id) = self.selected_bookmark { return self.db.get_bookmark_title(id).unwrap_or_default(); }
        if let Some(id) = self.active_folder { return self.db.get_folder_title(id).unwrap_or_default(); }
        String::new()
    }
}

fn refresh_ui(ui: &MainWindow, st: &State) {
    ui.set_folders(st.build_folder_model());
    ui.set_bookmarks(st.build_bookmark_model());
    ui.set_status_text(st.status());
    if let Some(id) = st.selected_bookmark {
        if let Some(bm) = st.db.get_bookmark(id) {
            ui.set_detail_title(SharedString::from(bm.title.as_str()));
            ui.set_detail_url(SharedString::from(bm.url.as_deref().unwrap_or("")));
            ui.set_detail_note(SharedString::from(bm.note.as_deref().unwrap_or("")));
            return;
        }
    }
    ui.set_detail_title(SharedString::default());
    ui.set_detail_url(SharedString::default());
    ui.set_detail_note(SharedString::default());
}

fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000).spawn();
}

fn main() {
    let db = Database::open_default().expect("Cannot open database");
    db.init_schema().expect("Cannot init schema");

    let state = Rc::new(RefCell::new(State::new(db)));
    let ui = MainWindow::new().unwrap();
    refresh_ui(&ui, &state.borrow());

    // Folder clicked
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        st.active_folder = Some(id as i64); st.selected_bookmark = None;
        st.expanded.insert(id as i64); refresh_ui(&ui, &st); }); }

    // Folder toggle
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_folder_toggle(move |id| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        let fid = id as i64;
        if st.expanded.contains(&fid) { st.expanded.remove(&fid); } else { st.expanded.insert(fid); }
        ui.set_folders(st.build_folder_model()); }); }

    // Bookmark clicked
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_bookmark_clicked(move |id| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        st.selected_bookmark = Some(id as i64); refresh_ui(&ui, &st); }); }

    // Open URL
    ui.on_bookmark_open(|url| open_url(url.as_str()));

    // Create folder
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_folder_confirmed(move |name| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        let parent = st.active_folder;
        if let Ok(new_id) = st.db.create_folder(parent, name.as_str()) {
            if let Some(p) = parent { st.expanded.insert(p); }
            st.active_folder = Some(new_id); st.expanded.insert(new_id);
        }
        refresh_ui(&ui, &st); }); }

    // Create bookmark
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_bookmark_confirmed(move |title, url| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        if let Some(fid) = st.active_folder {
            let _ = st.db.create_bookmark(fid, title.as_str(), url.as_str());
        } else {
            // No folder selected — create in root
            if let Ok(root_id) = st.db.create_folder(None, "Ссылки") {
                st.expanded.insert(root_id);
                st.active_folder = Some(root_id);
                let _ = st.db.create_bookmark(root_id, title.as_str(), url.as_str());
            }
        }
        refresh_ui(&ui, &st); }); }

    // Rename requested (F2 or button)
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_rename_requested(move || {
        let ui = w.unwrap(); let st = s.borrow();
        ui.set_rename_prefill(SharedString::from(st.selected_name().as_str()));
        ui.set_show_rename_dlg(true); }); }

    // Rename confirmed
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_rename_confirmed(move |new_name| {
        let ui = w.unwrap(); let st = s.borrow();
        if let Some(id) = st.selected_bookmark { let _ = st.db.rename_node(id, new_name.as_str()); }
        else if let Some(id) = st.active_folder { let _ = st.db.rename_node(id, new_name.as_str()); }
        refresh_ui(&ui, &st); }); }

    // Edit requested (F4)
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_edit_requested(move || {
        let ui = w.unwrap(); let st = s.borrow();
        if let Some(id) = st.selected_bookmark {
            if let Some(bm) = st.db.get_bookmark(id) {
                ui.set_edit_title_val(SharedString::from(bm.title.as_str()));
                ui.set_edit_url_val(SharedString::from(bm.url.as_deref().unwrap_or("")));
                ui.set_edit_note_val(SharedString::from(bm.note.as_deref().unwrap_or("")));
                ui.set_show_edit_dlg(true);
            }
        } }); }

    // Edit confirmed
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_edit_confirmed(move |title, url, note| {
        let ui = w.unwrap(); let st = s.borrow();
        if let Some(id) = st.selected_bookmark {
            let _ = st.db.update_bookmark(id, title.as_str(), url.as_str(), note.as_str());
        }
        refresh_ui(&ui, &st); }); }

    // Delete
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_delete_selected(move || {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        if let Some(id) = st.selected_bookmark {
            let _ = st.db.delete_bookmark(id); st.selected_bookmark = None;
        } else if let Some(id) = st.active_folder {
            let _ = st.db.delete_folder(id); st.expanded.remove(&id); st.active_folder = None;
        }
        refresh_ui(&ui, &st); }); }

    // Search
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_search_changed(move |query| {
        let ui = w.unwrap(); let mut st = s.borrow_mut();
        st.search_query = query.to_string(); st.selected_bookmark = None;
        ui.set_detail_url(SharedString::default());
        ui.set_bookmarks(st.build_bookmark_model()); ui.set_status_text(st.status()); }); }

    // Open DB
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_open_db(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).add_filter("All", &["*"]).pick_file() {
            match Database::open_at(&path) {
                Ok(new_db) => { let _ = new_db.init_schema();
                    let mut st = s.borrow_mut();
                    st.db = new_db; st.expanded.clear(); st.active_folder = None;
                    st.selected_bookmark = None; st.search_query.clear();
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_status_text(SharedString::from(format!("Открыта: {name}")));
                    refresh_ui(&ui, &st); }
                Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
            }
        } }); }

    // New DB
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_new_db(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("Database", &["db"]).set_file_name("album.db").save_file() {
            let _ = std::fs::remove_file(&path);
            match Database::open_at(&path) {
                Ok(new_db) => { let _ = new_db.init_schema();
                    let mut st = s.borrow_mut();
                    st.db = new_db; st.expanded.clear(); st.active_folder = None;
                    st.selected_bookmark = None; st.search_query.clear();
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    ui.set_status_text(SharedString::from(format!("Создана: {name}")));
                    refresh_ui(&ui, &st); }
                Err(e) => { ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))); }
            }
        } }); }

    // Export HTML
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_export_html(move || {
        let ui = w.unwrap(); let st = s.borrow();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html"]).set_file_name("bookmarks.html").save_file() {
            match st.db.export_html(&path) {
                Ok(n) => ui.set_status_text(SharedString::from(format!("Экспорт HTML: {n} ссылок"))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    // Export TXT
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_export_txt(move || {
        let ui = w.unwrap(); let st = s.borrow();
        if let Some(path) = rfd::FileDialog::new().add_filter("Text", &["txt"]).set_file_name("bookmarks.txt").save_file() {
            match st.db.export_txt(&path) {
                Ok(n) => ui.set_status_text(SharedString::from(format!("Экспорт TXT: {n} ссылок"))),
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    // Import ua.dat
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_uadat(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("ua.dat", &["dat"]).add_filter("All", &["*"]).set_file_name("ua.dat").pick_file() {
            let mut st = s.borrow_mut();
            match st.db.import_uadat(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт ua.dat: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    // Import HTML
    { let s = state.clone(); let w = ui.as_weak();
      ui.on_import_html(move || {
        let ui = w.unwrap();
        if let Some(path) = rfd::FileDialog::new().add_filter("HTML", &["html","htm"]).add_filter("All", &["*"]).pick_file() {
            let mut st = s.borrow_mut();
            match st.db.import_html(&path) {
                Ok(n) => { ui.set_status_text(SharedString::from(format!("Импорт HTML: {n} ссылок"))); refresh_ui(&ui, &st); }
                Err(e) => ui.set_status_text(SharedString::from(format!("Ошибка: {e}"))),
            }
        } }); }

    ui.run().unwrap();
}
