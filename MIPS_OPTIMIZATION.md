# Оптимизация для MIPS процессоров (Keenetic и др.)

## Проблема

На роутерах с MIPS процессорами (особенно без FPU - floating point unit) и малым объёмом ОЗУ (256 МБ) original версия simple-tls могла загружать CPU на 100% через некоторое время работы.

## Причины высокой нагрузки CPU

1. **Рандомизация размера буфера** - в оригинальном коде размер буфера выбирался случайно (4-8KB) с использованием `math/rand` на каждой итерации копирования данных. Это вызывает:
   - Много вычислений с плавающей точкой
   - Частое создание новых объектов `rand.Rand`
   - Фрагментацию памяти

2. **Частые вызовы `time.Now()`** - установка deadline на каждой итерации цикла копирования

3. **Аллокация буфера в цикле** - буфер выделялся и освобождался на каждой итерации вместо выделения один раз

## Применённые оптимизации

### 1. Фиксированный размер буфера (8KB)
```go
const defaultBufSize = 8 * 1024 // 8KB
```
Убрана рандомизация размера буфера. Это:
- Убирает зависимость от `math/rand`
- Снижает фрагментацию памяти
- Уменьшает количество аллокаций

### 2. Буфер выделяется один раз
```go
buf := alloc.GetBuf(defaultBufSize)
defer alloc.ReleaseBuf(buf)
```
Буфер выделяется перед циклом и освобождается после завершения работы функции.

### 3. Оптимизированная логика deadline
```go
// Обновляем deadline только каждые timeout/2
deadlineNext = now.Add(idleTimeout / 2)
```
- `time.Now()` вызывается только когда нужно обновить deadline
- Deadline обновляется каждые половину timeout вместо каждой итерации
- Снижено количество system calls

## Результаты сборки

| Файл | Архитектура | Размер | Для кого |
|------|-------------|--------|----------|
| `simple-tls-linux-arm64` | ARM 64-bit | 10.44 MB | Современные роутеры (512MB+ ОЗУ) |
| `simple-tls-linux-amd64` | x86_64 | 11.11 MB | ПК, серверы Linux |
| `simple-tls-linux-mipsle-softfloat` | MIPS LE | 12.06 MB | Keenetic (MIPS little-endian) |
| `simple-tls-linux-mipsle-float` | MIPS LE | 12.06 MB | Keenetic (с FPU) |
| `simple-tls-linux-mips-softfloat` | MIPS BE | 12.06 MB | Старые роутеры (big-endian) |
| `simple-tls-windows-amd64.exe` | x86_64 | 11.44 MB | Windows ПК |
| `simple-tls-windows-arm64.exe` | ARM 64-bit | 10.51 MB | Windows on ARM |

ARM64 версии самые компактные (~10.4 MB).

## Определение архитектуры вашего роутера

### На роутере через SSH:
```bash
# Посмотреть архитектуру
uname -m
# или
cat /proc/cpuinfo | grep architecture

# Результаты:
# mips  -> big-endian (редко)
# mipsel -> little-endian (часто: Keenetic, OpenWrt)
# mips64 -> 64-bit big-endian
# mips64el -> 64-bit little-endian
```

### Для Keenetic (большинство моделей):
- **mipsel** (little-endian) - большинство современных моделей
- **softfloat** - если процессор без FPU (старые модели)

### Таблица выбора бинарника:

| Результат `uname -m` | Файл | ОС |
|---------------------|------|-----|
| `aarch64` или `arm64` | `simple-tls-linux-arm64` | Linux ARM64 |
| `mipsel` | `simple-tls-linux-mipsle-softfloat` | Linux MIPS LE |
| `mips` | `simple-tls-linux-mips-softfloat` | Linux MIPS BE |
| `x86_64` | `simple-tls-linux-amd64` | Linux AMD64 |
| `ARM64` (PowerShell) | `simple-tls-windows-arm64.exe` | Windows ARM64 |
| `x64` (PowerShell) | `simple-tls-windows-amd64.exe` | Windows AMD64 |

### Для Windows:

```powershell
# Проверить архитектуру
[System.Environment]::Is64BitOperatingSystem  # True = amd64 или arm64

# Windows AMD64 (обычный ПК)
.\simple-tls-windows-amd64.exe -v

# Windows ARM64 (Surface Pro X и подобные)
.\simple-tls-windows-arm64.exe -v
```

## Сборка для разных платформ

### Linux ARM64 (современные роутеры, 512MB+ ОЗУ):
```bash
GOOS=linux GOARCH=arm64 go build -ldflags="-s -w" -o simple-tls-linux-arm64 .
```

### Linux AMD64 (ПК, серверы):
```bash
GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o simple-tls-linux-amd64 .
```

### Windows AMD64:
```bash
GOOS=windows GOARCH=amd64 go build -ldflags="-s -w" -o simple-tls-windows-amd64.exe .
```

### Windows ARM64:
```bash
GOOS=windows GOARCH=arm64 go build -ldflags="-s -w" -o simple-tls-windows-arm64.exe .
```

### MIPS little-endian (Keenetic):
```bash
GOOS=linux GOARCH=mipsle GOMIPS=softfloat go build -ldflags="-s -w" -o simple-tls-linux-mipsle-softfloat .
```

### Для MIPS little-endian с FPU:
```bash
GOOS=linux GOARCH=mipsle GOMIPS=float go build -ldflags="-s -w" -o simple-tls-mipsle .
```

### Для MIPS big-endian без FPU:
```bash
GOOS=linux GOARCH=mips GOMIPS=softfloat go build -ldflags="-s -w" -o simple-tls-mips .
```

### Для MIPS big-endian с FPU:
```bash
GOOS=linux GOARCH=mips GOMIPS=float go build -ldflags="-s -w" -o simple-tls-mips .
```

### Для MIPS64:
```bash
GOOS=linux GOARCH=mips64 go build -ldflags="-s -w" -o simple-tls-mips64 .
```

### Использование скрипта сборки:
```powershell
# Для Keenetic (mipsle softfloat)
.\build.ps1 linux-mipsle

# Все версии
.\build.ps1 all
```

## Параметры сборки

| Переменная | Значение | Описание |
|------------|----------|----------|
| `GOOS` | `linux` | Операционная система |
| `GOARCH` | `mips` | Архитектура процессора |
| `GOMIPS` | `softfloat` | Использовать мягкие вычисления с плавающей точкой (для процессоров без FPU) |
| `ldflags "-s -w"` | - | Удалить отладочную информацию для уменьшения размера бинарника |

## Рекомендации для роутеров с малым ОЗУ

1. **Уменьшите timeout** - установите `-t 60` или меньше для быстрого закрытия неактивных соединений
2. **Избегайте gRPC** - используйте прямой TLS режим (без флага `-grpc`)
3. **Мониторинг** - следите за использованием памяти через `top` или `free -m`

## Пример запуска на Keenetic

```bash
# Клиент режим
./simple-tls-mips -b 127.0.0.1:1080 -d server.example.com:443 -n server.example.com -ca ca.pem -t 60

# Сервер режим  
./simple-tls-mips -s -b 0.0.0.0:443 -d 127.0.0.1:8080 -cert server.pem -key server-key.pem -t 60
```

## Тестирование производительности

После установки оптимизированной версии проверьте:
```bash
# Мониторинг CPU
top -n 1 | grep simple-tls

# Мониторинг памяти
free -m
```

Ожидаемое поведение:
- CPU нагрузка < 50% при активной передаче данных
- Отсутствие роста нагрузки со временем
- Стабильное использование памяти

## Диагностика ошибок

### Ошибка: "syntax error: unexpected ("
```bash
/opt/bin/simple-tls: line 1: syntax error: unexpected "("
```

**Причина:** Неправильная архитектура бинарника.

**Решение:**
1. Проверьте архитектуру роутера: `uname -m`
2. Скачайте правильный бинарник:
   - `aarch64` → `simple-tls-linux-arm64`
   - `mipsel` → `simple-tls-mipsle-softfloat`
   - `mips` → `simple-tls-mips-softfloat`

### Ошибка: "exec format error"
```bash
./simple-tls: exec format error
```

**Причина:** Бинарник для другой архитектуры или 32/64-bit несовместимость.

**Решение:**
- Проверьте `uname -m` и `cat /proc/cpuinfo`
- Убедитесь, что используете правильный бинарник (arm64 vs armhf)

### Ошибка: "not executable"
```bash
Permission denied
```

**Решение:**
```bash
chmod +x simple-tls
```

## Изменённые файлы

- `core/ctunnel/tunnel.go` - оптимизирована функция копирования данных

## Совместимость

Изменения обратно совместимы. Оптимизации не меняют функциональность, только улучшают производительность.
