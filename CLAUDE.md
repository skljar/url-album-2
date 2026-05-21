# URL Album 3 — CLAUDE.md

Portable bookmark manager. Работает на Windows 7 SP1 / 8.1 / 10 / 11.

---

## Стек

| Компонент | Технология |
|---|---|
| UI | Slint 1.7 (собственный рендерер, без WebView2) |
| Backend | Rust 2021 |
| БД | SQLite (rusqlite bundled) |
| Диалоги | rfd 0.14 (native Windows dialogs) |
| Кодировки | encoding_rs (Windows-1251 для ua.dat) |

**Почему Slint а не Tauri:** Tauri требует WebView2 (только Windows 10+). Slint рисует UI сам через OpenGL/software renderer — работает на Win7+.

---

## Структура

```
C:\Projects\url-album-3\
├── src\
│   ├── main.rs       ← UI-логика, все callback'и, AppState
│   └── db.rs         ← SQLite CRUD, import/export
├── ui\
│   └── main_window.slint ← весь UI (дерево, список, диалоги)
├── build.rs          ← компиляция .slint
├── Cargo.toml
└── CLAUDE.md
```

## Как запустить

```powershell
# Рабочая директория: C:\Projects\url-album-3

# Kill если запущен
Stop-Process -Name "url-album-3" -Force -ErrorAction SilentlyContinue

# Debug
cargo build
Start-Process ".\target\debug\url-album-3.exe" -WorkingDirectory ".\target\debug"

# Release (маленький, быстрый)
cargo build --release
# exe в target\release\url-album-3.exe (~12 MB)
```

Portable-файлы рядом с exe:
- `album.db` — база по умолчанию
- `Data\` — зарезервировано для будущего (скриншоты через IE/shdocvw)

---

## Архитектура

### State (main.rs)
```rust
struct State {
    db: Database,
    expanded: HashSet<i64>,    // раскрытые папки
    active_folder: Option<i64>, // выбранная папка
    selected_bookmark: Option<i64>,
    search_query: String,
}
```

### UI → Rust (callbacks)
- `folder_clicked(id)` → выбрать папку, показать ссылки
- `folder_toggle(id)` → раскрыть/закрыть в дереве
- `bookmark_clicked(id)` → выбрать ссылку, показать детали
- `bookmark_open(url)` → открыть в браузере (rundll32)
- `new_folder_confirmed(name)` → создать папку
- `new_bookmark_confirmed(title, url)` → создать ссылку
- `rename_requested()` → показать диалог с текущим именем
- `rename_confirmed(name)` → сохранить переименование
- `edit_requested()` → F4, показать диалог редактирования
- `edit_confirmed(title, url, note)` → сохранить
- `delete_selected()` → Del, удалить папку или ссылку
- `search_changed(query)` → live search
- `open_db()` / `new_db()` → file dialogs через rfd
- `export_html()` / `export_txt()` → file dialogs
- `import_uadat()` / `import_html()` → file dialogs

### Rust → UI (свойства)
- `folders: [FolderNode]` — плоский список в порядке дерева
- `bookmarks: [BookmarkItem]` — ссылки текущей папки (или поиска)
- `detail_title/url/note` — выбранная ссылка
- `status_text` — строка статуса

### DB schema (db.rs)
```sql
CREATE TABLE nodes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    parent   INTEGER,
    kind     TEXT NOT NULL DEFAULT 'bookmark',  -- 'folder' | 'bookmark'
    title    TEXT NOT NULL,
    url      TEXT,
    note     TEXT,
    sort_idx INTEGER DEFAULT 0,
    created  TEXT DEFAULT (datetime('now'))
);
```

---

## Клавиатурные шорткаты

| Клавиша | Действие |
|---|---|
| Del | Удалить выбранное |
| F2 | Переименовать |
| F4 | Редактировать ссылку (свойства) |
| Enter | Открыть URL выбранной ссылки |
| ↑ / ↓ | Навигация по ссылкам |
| Ctrl+F | Фокус на поиск |
| Ctrl+N | Новая ссылка |
| Ctrl+Shift+N | Новая папка |
| Escape | Сбросить поиск |
| Двойной клик | Открыть URL |

---

## Архитектура потоков

State использует `Arc<Mutex<State>>` для совместного доступа с фоновыми потоками:
- Главный поток: Slint event loop + все UI callbacks
- Фоновые потоки: fetch_favicon, check_url (по одному потоку на операцию)
- `slint::invoke_from_event_loop(...)` для обновления UI из фоновых потоков

## Что реализовано

- [x] Дерево папок с раскрытием/закрытием
- [x] Список ссылок (Название + URL)
- [x] Создание папки / ссылки (диалоги)
- [x] Редактирование ссылки (F4 — title, url, note)
- [x] Переименование (F2)
- [x] Удаление (Del, рекурсивное для папок)
- [x] Поиск по названию, URL, заметке
- [x] Открытие URL в браузере (rundll32)
- [x] Импорт из ua.dat (старый URL Album, Win-1251)
- [x] Импорт из HTML (Netscape bookmarks, Chrome/Firefox/Edge экспорт)
- [x] Экспорт в HTML (Netscape format)
- [x] Экспорт в TXT (url\tname)
- [x] Открыть/создать другую БД (file dialog)
- [x] Portable (всё рядом с exe)
- [x] Windows 7 SP1 / 8.1 / 10 / 11

## Что TODO

- [ ] Drag & Drop в дереве
- [ ] Resizable панели (сплиттер)
- [ ] Скриншоты через shdocvw (IE WebBrowser Control — как в оригинальном URL Album)
- [ ] Несколько колонок (resizable)
- [ ] Резервные копии БД
- [ ] Favicon — показывать в дереве тоже
