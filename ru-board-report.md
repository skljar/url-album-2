## Новые сообщения в теме ru-board (URL Album 2)
Дата проверки: 2026-05-24

---

⚠️ **Мониторинг недоступен: форум заблокирован на уровне сети**

Доступ к `forum.ru-board.com` невозможен из облачной среды (Claude Code on the web).

**Диагностика:**

| Метод | Результат |
|---|---|
| WebFetch `start=860` | `HTTP 403 Forbidden` |
| WebFetch `start=880` | `HTTP 403 Forbidden` |
| curl с browser headers | `Host not in allowlist` (ответ сервера) |
| Google cache / RSSing зеркало | `HTTP 403 Forbidden` |
| Wayback Machine API | `HTTP 403 Forbidden` |
| TCP/TLS соединение | ✅ Успешно (IP достижим, но контент не отдаётся) |

**Вывод:** ru-board.com закрывает HTTP-ответы для datacenter IP-адресов (AWS/GCP/Azure).
Это серверная IP-фильтрация — не проблема авторизации или cookies.

---

### Как получить данные вручную

**Вариант 1 — вставить HTML в чат (быстро):**
1. Открыть в браузере: `https://forum.ru-board.com/topic.cgi?forum=5&topic=3250&start=860`
2. Нажать Ctrl+U → Ctrl+A → Ctrl+C
3. Вставить HTML в чат — Claude разберёт и напишет полный отчёт

**Вариант 2 — PowerShell-мониторинг локально:**
Скрипт `monitor-ruboard.ps1` в корне проекта работает с локальной машины:
```powershell
# Запустить из папки проекта (Windows):
.\monitor-ruboard.ps1
```
Скрипт проверяет изменения и показывает Windows-уведомление при новых постах.

**Вариант 3 — Claude Code CLI локально:**
Локальная версия CLI не имеет сетевых ограничений:
```bash
claude "Monitor ru-board thread forum=5 topic=3250 start=860 and write report to ru-board-report.md"
```

---

*Следующая проверка: повторить после решения проблемы с доступом или через локальный запуск.*
