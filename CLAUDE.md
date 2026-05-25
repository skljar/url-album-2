# URL Album 3 — CLAUDE.md

Portable bookmark manager. **Windows 7 SP1+ (32-bit exe, работает на x86 и x64)**. Духовный наследник URL Album 2, но на Slint вместо Tauri/WebView2.

> Target: `i686-pc-windows-msvc` (32-bit). Один exe покрывает Win7/8/10/11 × x86/x64 через WoW64.

---

## Стек

| Компонент | Технология |
|---|---|
| UI | Slint 1.7 (собственный рендерер, без WebView2) |
| Backend | Rust 2021 |
| БД | SQLite (rusqlite bundled) |
| HTTP | ureq 2 + rustls (для favicon) |
| Диалоги | rfd 0.14 |
| Кодировки | encoding_rs (Windows-1251 для ua.dat) |
| Иконка | winres (ico в exe) |
| **Win32 платформа** | **platform.rs** — кастомный Slint backend (Win7+, без winit/WinRT) |
| **Win7 совместимость** | **compat.rs** — IAT-шимы для Win8+ API + build.rs /DELAYLOAD |
| **Target** | **i686-pc-windows-msvc** (32-bit, CRT static) |

---

## Структура

```
C:\Projects\url-album-3\
├── src\
│   ├── main.rs       ← State, все callback'и, DnD, favicon, settings
│   ├── db.rs         ← SQLite CRUD, import/export, schema
│   ├── net.rs        ← fetch_favicon (4 стратегии), check_url
│   ├── platform.rs   ← Кастомный Win32 Slint backend (Win7+, без winit)
│   └── compat.rs     ← IAT-шимы Win8+ API → Win7 fallback + WinRT no-ops
├── ui\
│   ├── main_window.slint ← весь UI
│   └── icons\
│       ├── folder-closed.png
│       └── folder-open.png
├── .cargo\
│   └── config.toml   ← [build] target = i686-pc-windows-msvc, crt-static
├── build.rs          ← /DELAYLOAD Win8+ DLL, /FORCE:MULTIPLE для __imp_* шимов
└── Cargo.toml
```

## Как запустить

```powershell
# Kill + Debug build + run (target автоматически i686 из .cargo/config.toml)
Stop-Process -Name "url-album-3" -Force -ErrorAction SilentlyContinue
Set-Location C:\Projects\url-album-3
cargo build
Start-Process ".\target\i686-pc-windows-msvc\debug\url-album-3.exe" -WorkingDirectory ".\target\i686-pc-windows-msvc\debug"

# Release
cargo build --release
# exe: target\i686-pc-windows-msvc\release\url-album-3.exe (~10-12 MB)

# Упаковка дистрибутива
.\dist\package.ps1
# Результат: dist\URL-Album-3\URL-Album.exe + dist\URL-Album-3.zip
```

Portable-файлы рядом с exe:
- `album.db` — база по умолчанию
- `last_db.txt` — авто-resume последней базы
- `recent_dbs.txt` — список последних 10 баз
- `settings.json` — настройки (tree_width, col_name_width, confirm_delete, no_dup_urls, expanded, active_folder)
- `Data\favicons\` — кэш favicon файлов (filename только, без пути в DB)

---

## DB Schema

```sql
CREATE TABLE nodes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    parent   INTEGER,
    kind     TEXT NOT NULL DEFAULT 'bookmark',
    title    TEXT NOT NULL,
    url      TEXT,
    thumb    TEXT,    -- полный путь к thumbnail PNG (опционально)
    note     TEXT,
    created  TEXT DEFAULT (datetime('now')),
    visited  TEXT,
    sort_idx INTEGER DEFAULT 0,
    favicon  TEXT     -- только filename (напр. "github.com.png")
);
```

---

## Архитектура

### State (main.rs)
```rust
struct State {
    db: Database,
    expanded: HashSet<i64>,
    active_folder: Option<i64>,
    selected_bookmark: Option<i64>,
    search_query: String,
    sort_by: SortBy,      // Title | Url
    sort_asc: bool,
    data_dir: PathBuf,    // папка Data/ рядом с exe
    check_results: HashMap<i64, (bool, String)>,
    tree_width: f32,
    col_name_width: i32,
    confirm_delete: bool,
    no_dup_urls: bool,
    favicon_cancel: Arc<AtomicBool>,
}
```

### Потоки
- Главный поток: Slint event loop + все callbacks
- Фоновые: favicon batch (5 параллельных потоков, domain dedup), check_url
- `slint::invoke_from_event_loop(...)` для обновления UI из фоновых потоков

---

## Что реализовано (актуально на 2026-05-22)

### UI (main_window.slint)
- Menubar: Файл | Ссылки | Перенос | Поиск | Вид
- Toolbar: иконочные кнопки (папка, ссылка, F4, F2, Del, ++, --, ==, ok)
- Левая панель: дерево (папки + закладки-листья)
  - [+]/[-] кнопки (toggle expand/collapse)
  - PNG иконки папок (folder-closed/open)
  - Серое выделение только на названии (не вся строка)
  - Hover эффект
  - Drag & Drop: перетаскивание из дерева (порог 5px)
- Правая панель: компактный список
  - Два столбца "Название" | "Адрес" с resizable splitter (drag заголовок)
  - Папки: ▶ + жирный текст + "N ссылок"
  - Ссылки: favicon/● + название + URL
  - Hover подсветка строк
  - Drag & Drop: перетаскивание из правой панели в дерево
- Splitter между панелями (resizable, сохраняется)
- Breadcrumb над правой панелью
- Detail card (при клике на ссылку):
  - Breadcrumb вверху
  - Большая область превью (thumbnail если есть, или favicon 64px по центру)
  - URL-бар снизу (как в URL Album 2)
  - Кнопки действий: ← Список, Ред., Имя, Удалить
- Статусбар: "Записей: N | База: filename.db"
- Favicon progress panel (левый нижний угол, кнопка "Отмена")
- Контекстное меню у курсора (папки и ссылки)

### Меню Файл
- Новая/Открыть/Последние базы.../Резервная копия/Свойства базы/Настройки/Выход

### Меню Ссылки
- + Ссылка / + Папка / Свойства F4 / Переименовать F2 / Удалить Del
- Загрузить favicon'ы / Проверить ссылки / Поиск дублей / Все ссылки

### Меню Перенос
- Экспорт HTML/TXT / Импорт ua.dat/HTML/TXT/Браузер

### Контекстное меню папки
- Новая папка / + Ссылка в папку / Переименовать F2 / Удалить Del
- Импорт в папку... / Экспорт папки...
- Проверить ссылки / Обновить favicon'ы
- Сортировка А→Я
- Свойства F4

### Контекстное меню ссылки
- Открыть URL / Загрузить favicon
- Свойства F4 / Копировать URL / Переместить в...
- Переименовать F2 / Удалить Del

### Backend (db.rs + main.rs + net.rs)
- CRUD: create/update/delete/rename для папок и ссылок
- move_node (через контекстное меню "Переместить" и DnD)
- sort_folder (А→Я)
- get_bookmarks_recursive (для favicon batch в подпапках)
- Импорт: ua.dat (Win-1251), HTML (Netscape), TXT (URL per line), Chrome/Edge/Firefox JSON
- Экспорт: HTML, TXT (для папки или всей базы)
- Favicon: 4 стратегии (favicon.ico → HTML link → DuckDuckGo → Google S2)
  - Domain dedup (1 загрузка на домен)
  - Cache validation (is_valid_image на кэш)
  - Full Chrome UA (обход Cloudflare)
  - SVG исключён (Slint не поддерживает)
  - 5 параллельных потоков
  - Обновление дерева после каждой загруженной иконки
- Check links (HTTP HEAD, таймаут 10с)
- Несколько БД: открыть/создать/последние/свойства/очистить/резервная копия
- Настройки: confirm_delete, no_dup_urls, tree_width, col_name_width
- Диалог настроек с чекбоксами
- Диалог свойств БД (путь, размер, кол-во)
- Диалог последних баз (список recent_dbs.txt)

### Drag & Drop
- Из дерева: нажать и потянуть папку/ссылку, бросить на другую папку (подсветка синей рамкой)
- Из правой панели: нажать и потянуть ссылку/папку, бросить на папку в дереве
- Глобальный Y (абсолютный) для DnD из правой панели
- Относительный Y (от tree-content) для DnD внутри дерева
- move_node вызывается при drop, target папка раскрывается автоматически

### Клавиатурные шорткаты
- Del: удалить / F2: переименовать / F4: свойства
- Enter: открыть URL / Esc: назад/сброс поиска
- ↑↓: навигация / Ctrl+F: поиск / Ctrl+N: новая ссылка

---

## Известные ограничения
- Нет скриншотов сайтов (требует Edge/Chrome headless, Win10+ only)
- Drag & Drop: автораскрытие папки при hover не реализовано
- Open with browser: нет выбора конкретного браузера
- Accordion mode дерева: не реализован

---

## Паттерны

### Rust
- `State::build_tree_model()` — строит плоский список узлов в порядке отображения
- `dedup_by_domain(bms)` — группирует закладки по домену для favicon
- `collect_tree_order()` — строит порядок узлов для DnD hit-testing
- Favicon: `net::fetch_favicon(url, dir)` → `db.set_favicon(id, filename)`
- Путь к иконке: `exe_dir/Data/favicons/{filename}`

### Slint
- `sel` property на каждом узле дерева (inline условие, без перестройки модели)
- `active-folder-id` property на window (not in model — fix double-click detection)
- `drag-active/drag-item-id/drag-target-id` — DnD состояние
- `drag-global-y` — абсолютный Y для DnD из правой панели
- `tree-content := VerticalBox` — именованный элемент для rel_y вычислений
- Контекстное меню: два отдельных `if root.show-ctx && ctx-is-folder` блока с fixed height (не растягиваются)

---

## История сессий

### Сессия 2026-05-24 — переход на i686, очистка, релиз

#### ✅ Сделано
- Переход на `i686-pc-windows-msvc` как основной target
  - `.cargo/config.toml`: добавлен `[build] target = i686-pc-windows-msvc`
  - `package.ps1`: упрощён до единого универсального 32-бит пакета
- Добавлены в git критические файлы: `compat.rs`, `platform.rs`, `.cargo/config.toml`
- Обновлён `.gitignore`: exe, zip, favicons, log, backup файлы
- Обновлён `CLAUDE.md`: реальная совместимость Win7 SP1+, документирован compat-слой
- Очистка кэша сборки: удалено ~22 GB (x64 debug/release/кросс-компиляция)
- `cargo build --release` → `URL-Album.exe` 14 MB (i686), ZIP 6 MB
- **Тест запуска на Win10: ✅ пройден успешно**

#### ⏳ Следующие шаги
- Тестирование на реальной **Windows 7 SP1** (с KB2670838) — проверить:
  - Запуск exe
  - Файловые диалоги (rfd — IFileDialog, Vista+)
  - Favicon загрузку (ureq + rustls)
- Проверка и доработка функциональности (баги при использовании)
- Планирование браузерных расширений:
  - **Native Messaging** (прямая интеграция с браузером, сложнее) vs
  - **HTTP-сервер** внутри exe (проще, браузерно-независимо)

#### 🐛 Известные проблемы
- rfd 0.14 на Win7 не тестировался практически (теоретически должно работать)
