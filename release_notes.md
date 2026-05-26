## URL-Album 2.0.1

Первый публичный релиз с полной поддержкой Windows 7 SP1+.

### Что нового

- **Windows 7 SP1 совместимость** — работает без дополнительных обновлений
- Исправлен крэш при загрузке (`GetSystemTimePreciseAsFileTime` отсутствует в Win7 kernel32)
- Исправлен крэш delay-load (`bcryptprimitives.dll!ProcessPrng`) — перенаправлен на `CRYPTBASE.dll` ordinal 9
- Исправлен крэш при загрузке фавиконов (`api-ms-win-core-synch-l1-2-0.dll` отсутствует на Win7) — IAT патч на встроенные шимы
- Убрано создание `favicon_debug.log` в рабочей директории

### Изменения именования

- Приведена к единой версии 2.0.1 (ранее присутствовала путаница между 2.1, 3.0-alpha)
- Исполняемый файл: `URL-Album-2.0.1.exe`

### Тестировано

- Windows 7 SP1 x64 (VirtualBox)
- Windows 10 x64
- Windows 11 — ожидаются отчёты пользователей

Один exe, без установки. 32-битная сборка для совместимости с диапазоном Win7—Win11.

Сообщения о багах — в [issues](https://github.com/skljar/url-album-2/issues).
