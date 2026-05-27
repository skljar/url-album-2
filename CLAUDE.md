# URL Album 3 — CLAUDE.md

## Правила версионирования и релизов

### Текущая версия
**2.0.2**

### Формат версий
Семантическое: MAJOR.MINOR.PATCH (например 2.0.1).
- PATCH (+1 к третьему числу): исправление багов
- MINOR (+1 ко второму, PATCH обнуляется): новая функциональность
- MAJOR (+1 к первому): ломающие изменения, только с согласия

### Имена файлов
Все артефакты релиза по шаблону `URL-Album-<version>`:
- ZIP в dist/: `URL-Album-2.0.2.zip`
- EXE внутри ZIP: `URL-Album-2.0.2.exe`
- Tag релиза в Git: `v2.0.2`

### Места, где должна стоять одна и та же версия
- `Cargo.toml` → `version = "2.0.2"`
- `README.md` → ссылка на скачивание
- `dist/package.ps1` → имя ZIP и EXE
- `CLAUDE.md` → эта секция (текущая версия выше)
- Git tag: `v2.0.2`

### Рабочий процесс сборки дистрибутива
```powershell
# 1. Сборка
cargo build --release

# 2. PE-patch (Win7: GetSystemTimePreciseAsFileTime + bcrypt ordinal + synch IAT + combase IAT)
cargo run --manifest-path tools\pe-patch\Cargo.toml --release -- `
    target\i686-pc-windows-msvc\release\url-album-3.exe

# 3. Упаковка
.\dist\package.ps1
# → dist\URL-Album-2.0.2.zip (≈6.5 MB)

# 4. Релиз
gh release create v2.0.2 .\dist\URL-Album-2.0.2.zip `
    --repo skljar/url-album-2 `
    --title "URL-Album 2.0.2 — Win7 import fix" `
    --notes-file release_notes.md `
    --latest
```

---

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

#### ⏳ Следующие шаги → перешли в сессию 2026-05-25

---

### Сессия 2026-05-25 — диагностика Win7 крэшей, pe-patch, анализ delay-load

#### ✅ Сделано
- Удалена функция `favicon_log()` из `net.rs` (убрано создание `favicon_debug.log`)
- Создан инструмент `tools/pe-patch/` — PE binary patcher для Win7-совместимости:
  - Переименовывает `GetSystemTimePreciseAsFileTime` → `GetSystemTimeAsFileTime` в INT
  - Обнуляет 2-байтовый HINT перед именем (0x027F → 0x0000)
  - Обнуляет PE CheckSum (был уже 0 из-за strip=true)
  - Создаёт timestamped backup в `tools/pe-patch/backups/`
  - Отдельный workspace, собирается под x86_64 host (не i686)
- **Win7 крэш #1 диагностирован и исправлен**: `GetSystemTimePreciseAsFileTime` не найдена в kernel32.dll Win7 — жёсткая запись в PE import table от Rust 1.95 libstd. PE-патч переименовывает импорт → Win7 загружает exe
- **Win7 крэш #2 диагностирован**: `STATUS_DELAY_LOAD_FAILURE` (c06d007f) — cdb.exe дал ответ:
  - Падает на `bcryptprimitives.dll` → функция `ProcessPrng` (ordinal 1)
  - Delay-load thunk стреляет вместо нашего `__imp_ProcessPrng` шима
  - **Корень проблемы**: MSVC linker обрабатывает `/DELAYLOAD` ПОСЛЕ разрешения `/FORCE:MULTIPLE`, записывая адрес thunk'а в IAT-слот поверх нашего шима
- **Win10 baseline**: `cargo build --release` → pe-patch → exe запускается ✅

#### ⏳ Следующий шаг — реализовать `__pfnDliFailureHook2` в `src/compat.rs`

Это официальный MSVC механизм перехвата delay-load failures. Принцип:
1. Delay-load thunk пытается `LoadLibrary("bcryptprimitives.dll")` → fails on Win7
2. Хук `dliFailLoadLib (=3)` → возвращаем fake HMODULE (ненулевой)
3. Thunk пытается `GetProcAddress(fake_hmod, "ProcessPrng")` → fails
4. Хук `dliFailGetProc (=4)` → возвращаем адрес `compat_ProcessPrng`
5. Thunk записывает шим в IAT-слот → все последующие вызовы к шиму

**Критические детали для реализации:**

```
SYMBOL NAME: ___pfnDliFailureHook2  (ТРИ подчёркивания)
  На i686-pc-windows-msvc MSVC C-код в delayimp.lib ищет символ с leading
  underscore (cdecl ABI). #[no_mangle] Rust не добавляет это '_' автоматически.
  Решение: #[export_name = "___pfnDliFailureHook2"]
  ИЛИ: build.rs → /ALTERNATENAME:___pfnDliFailureHook2=__pfnDliFailureHook2

ORDINAL vs NAME check в shim_for_proc:
  proc_raw <= 0xFFFF → это ordinal (ProcessPrng = ordinal 1)
  proc_raw > 0xFFFF  → это *const u8 указатель на строку

ENUM dliNotification из <delayimp.h>:
  dliFailLoadLib  = 3
  dliFailGetProc  = 4
  (dliNotePreLoadLibrary = 1, dliNotePreGetProcAddress = 2 — не используем)

build.rs: /DELAYLOAD директивы ОСТАВИТЬ — хук их перехватывает

МЭППИНГ функций:
  bcryptprimitives.dll   ord 1 / "ProcessPrng"          → compat_ProcessPrng
  api-ms-win-core-synch  "WaitOnAddress"                 → compat_WaitOnAddress
  api-ms-win-core-synch  "WakeByAddressAll"              → compat_WakeByAddressAll
  api-ms-win-core-synch  "WakeByAddressSingle"           → compat_WakeByAddressSingle
  api-ms-win-core-winrt  "RoOriginateErrorW"             → compat_RoOriginateErrorW
  combase.dll            "CoTaskMemFree"                 → compat_CoTaskMemFree
  combase.dll            "CoCreateFreeThreadedMarshaler" → compat_CoCreateFreeThreadedMarshaler
```

**Перед сборкой** — проверить exact symbol name в delayimp.lib:
```powershell
dumpbin /SYMBOLS "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\lib\x86\delayimp.lib" | Select-String "pfnDli"
```

**После сборки** — проверить нет ли warning "static is not used" (плохой знак = linker не видит хук).

#### 🔧 Диагностические артефакты
- `cdb.exe`: `C:\Program Files (x86)\Windows Kits\10\Debuggers\x86\cdb.exe`
- Crash dump: `C:\Users\admin\Documents\URL-Album.exe.2336.dmp` (44 MB)
- cdb-анализ показал: `Parameter[0] = 0x002de4dc` → `DelayLoadInfo.szDll = "bcryptprimitives.dll"`, `dlp.raw = 1`
- Win7 VM: VirtualBox, dist → `Desktop\URL-Album-3\URL-Album.exe`
- Crash dumps в Win7: `C:\CrashDumps\` (registry настроен, WER включён)

#### 🐛 Известные проблемы
- Win7: `STATUS_DELAY_LOAD_FAILURE` на `ProcessPrng` — БУДЕТ FIX в следующей сессии
- rfd 0.14 на Win7 не тестировался (теоретически должно работать — IFileDialog Vista+)

---

### Сессия 2026-05-25 (продолжение) — диагностика hook'а

#### Что было сделано
1. **PE-patch для GetSystemTimePreciseAsFileTime — РАБОТАЕТ** (commit fba8f41 ранее)
2. Реализован hook `__pfnDliFailureHook2` в src/compat.rs через `#[export_name = "___pfnDliFailureHook2"]` + `/INCLUDE:___pfnDliFailureHook2` в build.rs
3. На Win10 — exe запускается, hook не ломает нормальный путь
4. На Win7 — три попытки тестирования, все упали с APPCRASH

#### Ключевые находки из cdb анализа

**Crash dump 1724 (тест с константами 3/4):**
- Код ошибки: c06d007f (STATUS_DELAY_LOAD_FAILURE)
- DelayLoadInfo: szDll = "bcryptprimitives.dll", proc = "ProcessPrng"
- hmodCur = 0x73740000 — НАШ hook вернул HMODULE для какого-то notify
- pfnCur = 0 — для другого notify вернул 0 → crash
- Disasm в этом dump'е интерпретирован НЕВЕРНО: думали push 4 для LoadLib и push 5 для GetProc

**Crash dump 2880 (тест с константами 4/5):**
- Тот же код c06d007f
- Disasm ПРАВИЛЬНО показал: **push 3 = dliFailLoadLib**, **push 4 = dliFailGetProc**
- Это совпадает с delayimp.h — стандартные значения 3 и 4 правильные
- `dd 01f70b40 L1 = 00000000` — **hook-pointer был NULL** в этой сборке
- Значит hook вообще не подключился в сборке с константами 4/5

#### Архитектурные тупики (НЕ применять снова)
- ❌ Runtime registration через `extern static __pfnDliFailureHook2` — переменная в **read-only странице** (.rdata/.didata), запись вызывает access violation в init()
- ❌ Константы 4/5 в DLI_FAIL_* — НЕВЕРНЫЕ, правильные 3 и 4

#### ПРАВИЛЬНЫЕ значения констант
```rust
const DLI_FAIL_LOAD_LIB: u32 = 3;  // подтверждено delayimp.h + dump 2880 disasm
const DLI_FAIL_GET_PROC:  u32 = 4;  // подтверждено delayimp.h + dump 2880 disasm
```

#### Открытые вопросы для следующей сессии
1. **Подключается ли hook вообще** — в первом dump'е 1724 hmodCur=0x73740000 говорит что да, но во втором dump'е 2880 hook-pointer = NULL. Между этими тестами были разные сборки. Нужно проверить через cdb на свежем exe.
2. **Если /FORCE:MULTIPLE создаёт дубликат symbol** — наш `___pfnDliFailureHook2` static может оказаться по одному адресу, а delayimp читает другой (NULL). Возможные решения:
   - Использовать `/WHOLEARCHIVE:delayimp.lib` чтобы заставить линкер брать только наш symbol
   - Вынести hook в отдельный crate без LTO
   - Использовать `/INCLUDE` + проверить через `dumpbin /SYMBOLS` что наш symbol победил
3. **Calling convention** — `extern "system"` правильно для i686, но возможно нужно явно `extern "stdcall"`

#### ПЛАН ДЛЯ СЛЕДУЮЩЕЙ СЕССИИ

**Вариант А — Дожать hook (попробовать первым):**
1. Восстановить hook-блок в compat.rs с **КОНСТАНТАМИ 3 и 4** (правильными)
2. Восстановить `/INCLUDE:___pfnDliFailureHook2` в build.rs
3. `cargo build --release`
4. **Проверить hook-pointer через cdb/dumpbin** — если pointer = NULL → hook не подключился
5. pe-patch → ZIP → Win7 тест
6. Если crash на ProcessPrng — hook не сработал; если на другой функции — расширить shim_for

**Вариант Б — Plan B (если А не работает за 1 час):**
- Расширить tools/pe-patch для подмены имён функций в delay-load таблице (надёжнее, без зависимости от linker symbol resolution)

#### Обязательно сделать ПЕРВЫМ делом в следующей сессии
1. Прочитать этот раздел CLAUDE.md ПОЛНОСТЬЮ
2. Прочитать src/compat.rs и build.rs текущее состояние
3. **НЕ повторять ошибки этой сессии**:
   - НЕ менять константы DLI_FAIL_* на 4/5
   - НЕ пытаться runtime-registration через extern static
4. Начать с проверки реального состояния hook-pointer через cdb/dumpbin на свежей сборке

---

### Сессия 2026-05-26 — Win7 полностью работает, фавиконы ОК ✅

#### Проблемы, с которыми пришли
- Win7 крэш при старте (#1 исправлен pe-patch'ом ранее)
- Win7 крэш при "Обновить фавиконы": `STATUS_DELAY_LOAD_FAILURE` c06d007e `api-ms-win-core-synch-l1-2-0.dll`

#### Что попробовали и не сработало

**`__pfnDliFailureHook2` (все предыдущие попытки):**
- hook-pointer `___pfnDliFailureHook2` = NULL в финальном exe, несмотря на `#[export_name]` + `/INCLUDE`
- Причина: **LTO ordering**. MSVC linker при LTCG обрабатывает COFF-архивы (`delayimp.lib`) ДО битокода Rust. С `/FORCE:MULTIPLE` побеждает первое определение → NULL из delayimp.lib, а не наш Rust static.
- Это фундаментальное ограничение: hook через linker symbol override не работает с `-C lto` на MSVC.

**Убрать `/DELAYLOAD:api-ms-win-core-synch-l1-2-0.dll`:**
- Проверено: import lib из libstd.rlib всё равно побеждает в гонке символов.
- DLL переехала из delay-load в regular import → на Win7 краш при старте (хуже). Реверт.

#### Что сработало: pe-patch delay-load IAT напрямую

**Техника для случая "функции нет ни в одной Win7 DLL":**

1. Экспортируем наши шим-функции из exe через `/EXPORT:` в build.rs:
   ```
   /EXPORT:compat_WaitOnAddress
   /EXPORT:compat_WakeByAddressAll
   /EXPORT:compat_WakeByAddressSingle
   ```
   + `#[no_mangle] pub` на функциях в compat.rs.

2. В pe-patch: читаем export table exe → находим VA каждого шима.

3. Находим delay-load descriptor для `api-ms-win-core-synch-l1-2-0.dll` (DataDirectory[13]).

4. Идём по INT (Import Name Table) и параллельному IAT, находим `WaitOnAddress` / `WakeByAddressAll` / `WakeByAddressSingle` по имени.

5. **Патчим IAT slot**: записываем VA нашего шима вместо адреса delay-load thunk'а.

**Почему это правильно (в отличие от bcrypt-patch):**
- Для bcrypt мы ХОТЕЛИ чтобы `__delayLoadHelper2` сработал (LoadLibrary("CRYPTBASE.dll") + GetProcAddress(ord 9)) → патчили только INT + DLL name.
- Для synch нет никакой Win7 DLL с нужными функциями → патчим **IAT напрямую**, тем самым полностью обходя thunk. Первый `CALL [IAT]` прыгает сразу на шим.

**Ключевой факт: Windows loader НЕ трогает delay-load IAT при старте.**
- При загрузке exe loader обрабатывает только Regular Import Directory (DataDirectory[1]).
- Delay Import Directory (DataDirectory[13]) **игнорируется loader'ом** — инициализация через thunk происходит только при первом runtime вызове функции.
- Наш pe-patch записывает VA шима в IAT до запуска exe. Loader его не трогает. Патч выживает.

#### Полная цепочка исправлений (итого 4 проблемы → 2 pe-patch расширения)

| Проблема | Причина | Решение |
|---|---|---|
| `GetSystemTimePreciseAsFileTime` | Жёсткий import в kernel32, Win7 не имеет | pe-patch: переименовать в INT |
| `ProcessPrng` (bcryptprimitives.dll) | Delay-load thunk, DLL есть но функции нет | pe-patch: DLL→CRYPTBASE.dll, ordinal 9 (SystemFunction036) |
| `WaitOnAddress` (api-ms-win-core-synch-l1-2-0.dll) | Delay-load thunk, DLL вообще нет на Win7 | pe-patch: IAT→VA шима напрямую |
| `___pfnDliFailureHook2` | LTO ordering: delayimp.lib NULL побеждает | Не используется — заменено pe-patch подходом |

#### Диагностические команды (для будущих сессий)

```powershell
# Анализ crash dump
$cdb = "C:\Program Files (x86)\Windows Kits\10\Debuggers\x86\cdb.exe"
& $cdb -z <dump> -c "!analyze -v; dc <Parameter[0]> L9; da poi(<Parameter[0]>+c); q"
# Parameter[0] из !analyze → DelayLoadInfo struct, +c (offset 12) → szDll

# Проверка import/delay-load DLL списка
$dumpbin = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x86\dumpbin.exe"
& $dumpbin /IMPORTS $exe 2>&1 | Select-String "\.dll$" | Sort-Object -Unique
```

#### Артефакты сессии
- Crash dump: `C:\Users\admin\Documents\URL-Album.exe.2648.dmp` (78 MB, c06d007e на synch DLL)
- Win7 VM: VirtualBox, dist → `Desktop\URL-Album-3\URL-Album.exe`
- Crash dumps в Win7: `C:\CrashDumps\`

#### ⏳ Возможные следующие шаги
- Предупреждения компилятора: `non_snake_case` в pe-patch (BCRYPT/CBASE locals) — косметика
- `api-ms-win-core-winrt-error-l1-1-0.dll` и `combase.dll` — delay-loaded, но не падают в текущих тестах (возможно не вызываются в типичных сценариях Win7). Если упадут — применить ту же IAT-patch технику.

---

### Сессия 2026-05-26 (продолжение, поздний вечер) — релиз v2.0.2

#### Что выпущено
- v2.0.2: https://github.com/skljar/url-album-2/releases/tag/v2.0.2
- ZIP: `URL-Album-2.0.2.zip` (~6 MB), прикреплён к релизу, помечен Latest
- v2.0.1 сохранён как историческая версия (не удалён)

#### Какой баг чинили
- **Симптом**: на Win7 SP1 x64 крэш `c06d007e` (ERROR_MOD_NOT_FOUND) при импорте базы данных
- **Корень**: `combase.dll` отсутствует на Win7 — функции `CoTaskMemFree` и `CoCreateFreeThreadedMarshaler` живут там в `ole32.dll`
- **Диагностика**: cdb dump → `dc <Param0> L9; da poi(<Param0>+c)` → `DelayLoadInfo.szDll = "combase.dll"`, `szProcName = "CoTaskMemFree"`

#### Что добавлено в этой сессии

| Компонент | Изменение |
|---|---|
| `src/compat.rs` | `#[no_mangle] pub` на `compat_CoTaskMemFree` и `compat_CoCreateFreeThreadedMarshaler` |
| `build.rs` | `/EXPORT:compat_CoTaskMemFree`, `/EXPORT:compat_CoCreateFreeThreadedMarshaler` |
| `tools/pe-patch/src/main.rs` | новая функция `patch_combase_iat()` — IAT шим (ole32.dll runtime load) |

#### Тестировано в этой сессии
- Win7 SP1 x64 VirtualBox: старт ✅, UI ✅, фавиконы ✅, импорт базы ✅
- Win10 x64: smoke test ✅

#### WER LocalDumps настроены для exe
- `URL-Album-2.0.1.exe` → `C:\CrashDumps\`
- `URL-Album-2.0.2.exe` → `C:\CrashDumps\` (на Win7 VM)

#### Что в очереди (если упадёт в следующих сессиях)
- `api-ms-win-core-winrt-error-l1-1-0.dll` — пока не падает, но если упадёт — IAT-патч по той же схеме (`RoOriginateError*` шимы уже есть в `compat.rs`)
- Расширения для браузеров (Chrome/Firefox/Edge/Opera) — следующий большой этап

#### Уроки сессии
- Heredoc-синтаксис bash `$(cat <<'EOF' ... EOF)` НЕ работает в PowerShell — использовать многократный `-m` или here-string `@"..."@`
- `gh` CLI и `git` имеют разные credentials — `gh auth login` нужно делать ИНТЕРАКТИВНО в отдельном PowerShell (Claude Code не может это запустить)
- При rebase из remote всегда возможны merge-конфликты в README — наша актуальная версия побеждает

---

### Сессия 2026-05-27 — UX правой панели + разведка скриншотов

#### Что сделано
- Tree-sync при клике на закладку в правой панели (commit fb4c4cb)
- TouchArea на превью карточки и URL-баре: double-click открывает URL, right-click показывает контекстное меню (commit 1879441)
- Финальный commit: push 1879441 в origin/master
- Win7 SP1 x64 VM (Opera 95 portable) + Win10 — обе фичи протестированы

#### Изменения
- src/db.rs: добавлена get_node_parent(id) для обхода предков
- src/main.rs: функция expand_path_to(), изменены on_tree_bookmark_clicked и on_right_bookmark_clicked
- ui/main_window.slint: TouchArea в Preview Rectangle и в URL bar Rectangle карточки

#### Разведка по скриншотам (для следующей сессии)
- Подход: установленный Chromium-браузер через headless CLI
- Проверено на Win7 с Opera 95 (Chromium 109): команда работает
- Поиск браузера: сначала default через реестр Windows (HKCU\...\UserChoice\ProgId), потом fallback по стандартным путям
- Chromium ProgId: ChromeHTML, MSEdgeHTM, OperaStable, BraveHTML, VivaldiBrowser, YandexBrowser
- Хранение: Data/screenshots/<id>.png
- Контекстное меню: добавить пункты "Сделать скриншот" и "Удалить скриншот"
- Отображение: Image в Preview Rectangle вместо/поверх фавиконки
- Threading: запуск браузера в отдельном thread + slint::invoke_from_event_loop для UI

#### Win7 фиксы (полная цепочка)
| Проблема | Решение |
|---|---|
| GetSystemTimePreciseAsFileTime | pe-patch rename → GetSystemTimeAsFileTime |
| ProcessPrng (bcryptprimitives.dll) | pe-patch → CRYPTBASE.dll ordinal 9 |
| WaitOnAddress (api-ms-win-core-synch) | pe-patch IAT → compat shim |
| CoTaskMemFree (combase.dll) | pe-patch IAT → compat shim |

#### Текущая версия
2.0.2 — не менялась в этой сессии. Релиз 2.0.3 откладывается до завершения скриншотов.

#### Уроки сессии
- Сначала разведка, потом план без кода, потом реализация — это работает, минимизирует переделки
- Slint TouchArea: размещение last child для overlay (превью), first child когда есть Button (URL bar) — для корректного z-order
- Headless screenshot через Chromium CLI работает на любой Windows (Win7+) где есть установленный Chromium-браузер
