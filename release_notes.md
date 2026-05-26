## URL-Album 2.0.2

Патч-релиз: исправлен крэш при импорте базы данных на Windows 7 SP1.

### Что исправлено

- **Win7: крэш при импорте** — `combase.dll` отсутствует на Windows 7 (функции `CoTaskMemFree` и `CoCreateFreeThreadedMarshaler` там живут в `ole32.dll`). pe-patch теперь напрямую патчит IAT delay-load записи для `combase.dll`, перенаправляя вызовы на встроенные шимы, которые динамически загружают `ole32.dll`.

### Тестировано

- Windows 7 SP1 x64 (VirtualBox): старт ✅, UI ✅, фавиконы ✅, импорт базы ✅
- Windows 10 x64: ✅

### Полный список Win7-фиксов (все версии)

| Проблема | Решение |
|---|---|
| `GetSystemTimePreciseAsFileTime` (kernel32) | pe-patch: переименование в INT |
| `ProcessPrng` (bcryptprimitives.dll) | pe-patch: перенаправление на CRYPTBASE.dll ordinal 9 |
| `WaitOnAddress` (api-ms-win-core-synch) | pe-patch: IAT → compat шим (спинлок) |
| `CoTaskMemFree` (combase.dll) | pe-patch: IAT → compat шим (ole32.dll) |

Один exe, без установки. 32-битная сборка для совместимости Win7—Win11 × x86/x64.

Сообщения о багах — в [issues](https://github.com/skljar/url-album-2/issues).
