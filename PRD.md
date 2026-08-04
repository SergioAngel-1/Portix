# Portix — Product Requirements Document

> **El explorador de servicios de desarrollo local.**
> Responde a una sola pregunta, y la responde mejor que nadie:
> **«¿Qué proyecto está usando este puerto?»**

| Campo | Valor |
|---|---|
| Nombre del producto | **Portix** |
| Binario / comando | `portix` |
| Versión del documento | 1.0 |
| Fecha | 2026-07-09 |
| Estado | **Draft para revisión — listo para iniciar diseño técnico** |
| Autor | Sergio (Product Owner / Maintainer) |
| Audiencia | Ingeniería, contribuidores OSS, packagers |
| Licencia propuesta | MIT OR Apache-2.0 (dual, estándar del ecosistema Rust) |
| Plataforma v1 | Linux (x86_64, aarch64) — foco Arch y derivadas |
| Lenguaje | Rust (ver §5.1 para la justificación) |

---

## Tabla de contenidos

1. [Resumen ejecutivo](#1-resumen-ejecutivo)
2. [El problema](#2-el-problema)
3. [Análisis competitivo](#3-análisis-competitivo)
4. [Producto: visión, usuarios y principios](#4-producto-visión-usuarios-y-principios)
5. [Decisiones técnicas fundamentales](#5-decisiones-técnicas-fundamentales)
6. [Arquitectura](#6-arquitectura)
7. [Descubrimiento: del puerto al proceso](#7-descubrimiento-del-puerto-al-proceso)
8. [Detección de proyecto](#8-detección-de-proyecto)
9. [Detección de framework](#9-detección-de-framework)
10. [Caché, refresco y presupuesto de rendimiento](#10-caché-refresco-y-presupuesto-de-rendimiento)
11. [Interfaz de usuario](#11-interfaz-de-usuario)
12. [Configuración y modo no interactivo](#12-configuración-y-modo-no-interactivo)
13. [Seguridad y privacidad](#13-seguridad-y-privacidad)
14. [Riesgos técnicos](#14-riesgos-técnicos)
15. [Benchmarks y objetivos de rendimiento](#15-benchmarks-y-objetivos-de-rendimiento)
16. [Estrategia de testing](#16-estrategia-de-testing)
17. [Roadmap](#17-roadmap)
18. [Distribución](#18-distribución)
19. [Gobernanza open source](#19-gobernanza-open-source)
20. [Métricas de éxito](#20-métricas-de-éxito)
21. [Preguntas abiertas](#21-preguntas-abiertas)
22. [Apéndices](#22-apéndices)

---

## 1. Resumen ejecutivo

### 1.1 Qué es

Portix es una aplicación **TUI** para Linux que enumera todos los sockets en escucha del sistema, resuelve cada uno hasta el proceso propietario y, a partir de ese proceso, **descubre automáticamente el proyecto al que pertenece**: su directorio raíz, su nombre, su lenguaje, su framework, cómo fue lanzado, cuánto consume y si vive dentro de un contenedor.

Donde `lsof -i :3000` devuelve `node 48213 sergi 23u IPv6`, Portix devuelve:

```
:3000  next dev  ~/work/acme-storefront  Next.js 15  Node 22  pnpm  0.4% CPU  312 MB  ↑ 2h 14m
```

La diferencia no es cosmética. Es la diferencia entre un dato y una respuesta.

### 1.2 Por qué existe

El desarrollador moderno mantiene entre 3 y 15 servicios locales vivos de forma simultánea: dos o tres frontends, un par de APIs, una base de datos en Docker, Redis, un proxy inverso, un worker, un emulador. Las herramientas existentes están diseñadas para **administración de sistemas**, no para **desarrollo**. Muestran procesos y puertos con precisión quirúrgica y cero contexto. El desarrollador termina ejecutando una cadena manual — `ss -ltnp`, `ps -p`, `ls -l /proc/PID/cwd`, `cat package.json` — cada vez que quiere saber si puede matar algo sin romper otra cosa.

Portix automatiza esa cadena y la convierte en la vista por defecto.

### 1.3 Qué **no** es

Portix **no es** un monitor de procesos general, **no es** un reemplazo de `htop`/`btop`, **no es** un analizador de tráfico de red, **no es** un gestor de contenedores. La tentación de crecer hacia cualquiera de esas cuatro direcciones es real y debe resistirse explícitamente: cada una de ellas ya tiene un ganador y ninguna resuelve el problema que Portix resuelve.

### 1.4 Tesis del producto

> Los puertos no son el producto. Los puertos son el **índice**.
>
> El producto es el **contexto del proyecto**, y el puerto es simplemente la clave primaria más natural que el desarrollador ya tiene en la cabeza cuando abre la terminal.

### 1.5 Decisiones clave

| Decisión | Elección | Sección |
|---|---|---|
| Lenguaje | Rust | §5.1 |
| Framework TUI | `ratatui` + `crossterm` | §5.2 |
| Modelo de concurrencia | Hilos + canales (sin runtime async en MVP) | §5.3 |
| Fuente de datos | Lectura directa de `/proc` (sin shell-out) | §7 |
| Optimización futura | `NETLINK_INET_DIAG` (`sock_diag`) para el enumerado de sockets | §7.10 |
| Extensibilidad | Detectores declarativos (TOML) + detectores compilados | §9.5 |
| Distribución v1 | AUR (`portix`, `portix-bin`, `portix-git`) | §18 |

---

## 2. El problema

### 2.1 Contexto

Un stack de desarrollo típico en 2026 luce así:

| Puerto | Servicio | Cómo se lanzó | Dónde vive |
|---|---|---|---|
| 3000 | Next.js (storefront) | `pnpm dev` | `~/work/acme/apps/web` |
| 3001 | Next.js (admin) | `pnpm dev --port 3001` | `~/work/acme/apps/admin` |
| 4000 | NestJS API | `nest start --watch` | `~/work/acme/apps/api` |
| 5173 | Vite (design system) | `vite` | `~/work/acme/packages/ui` |
| 5432 | PostgreSQL | `docker compose up` | `~/work/acme/infra` |
| 6379 | Redis | `docker compose up` | `~/work/acme/infra` |
| 8000 | FastAPI (ML service) | `uvicorn app:main --reload` | `~/work/ml-scoring` |
| 8080 | Traefik | `docker run` | — |
| 7777 | ??? | ??? | ??? |

La última fila es el problema. Y aparece constantemente.

### 2.2 Job stories

Las funcionalidades de este PRD se justifican contra estas nueve situaciones reales. Cualquier feature propuesta que no sirva a al menos una de ellas es candidata a rechazo.

| # | Job story | Frecuencia |
|---|---|---|
| JS-1 | **Cuando** intento arrancar un servidor y obtengo `EADDRINUSE: address already in use :::3000`, **quiero** saber qué proyecto ocupa el 3000, **para** decidir si lo mato o cambio de puerto. | Diaria |
| JS-2 | **Cuando** veo un puerto desconocido escuchando (`:7777`), **quiero** saber qué es y de dónde salió, **para** determinar si es mío, de una herramienta, o algo que no debería estar ahí. | Semanal |
| JS-3 | **Cuando** mi laptop va lenta y hay ocho procesos `node`, **quiero** saber cuál pertenece a qué proyecto y cuánto consume cada uno, **para** matar el que ya no uso. | Diaria |
| JS-4 | **Cuando** vuelvo de comer y no recuerdo qué dejé corriendo, **quiero** un inventario de mi entorno de desarrollo, **para** retomar el contexto. | Diaria |
| JS-5 | **Cuando** un servicio no responde, **quiero** confirmar si sigue escuchando y con qué flags fue lanzado, **para** distinguir «está caído» de «está en otro puerto». | Semanal |
| JS-6 | **Cuando** trabajo en un monorepo, **quiero** ver qué workspace concreto sirve cada puerto, **para** no confundir `apps/web` con `apps/admin`. | Diaria |
| JS-7 | **Cuando** hay servicios en Docker y fuera de Docker, **quiero** verlos en la misma lista y distinguirlos, **para** saber dónde ejecutar el `restart`. | Diaria |
| JS-8 | **Cuando** quiero abrir un servicio en el navegador, **quiero** hacerlo desde la lista sin teclear la URL, **para** ahorrar fricción. | Diaria |
| JS-9 | **Cuando** termino la jornada, **quiero** apagar todos los servicios de un proyecto concreto, **para** liberar la máquina sin tocar los de otro proyecto. | Diaria |

### 2.3 El flujo manual que Portix reemplaza

```
$ ss -ltnp | grep 3000
LISTEN 0 511 *:3000 *:*  users:(("node",pid=48213,fd=23))

$ ls -l /proc/48213/cwd
lrwxrwxrwx ... /proc/48213/cwd -> /home/sergi/work/acme/apps/web

$ tr '\0' ' ' < /proc/48213/cmdline
node /home/sergi/work/acme/node_modules/.bin/next dev

$ cat /home/sergi/work/acme/apps/web/package.json | jq .name
"@acme/web"

$ cat /home/sergi/work/acme/apps/web/package.json | jq '.dependencies.next'
"^15.1.0"
```

Cinco comandos, ~25 segundos, y hay que recordar la sintaxis de cada uno. Portix comprime esto en **una tecla**.

### 2.4 Por qué ahora

Tres tendencias convergen:

1. **Explosión de servicios locales.** Los monorepos y la arquitectura de microservicios han multiplicado por 5 el número de procesos que un desarrollador ejecuta en su máquina respecto a hace una década.
2. **Madurez del ecosistema TUI de Rust.** `ratatui` alcanzó estabilidad y adopción crítica; construir una TUI de calidad `lazygit` en Rust hoy cuesta una fracción de lo que costaba en 2021.
3. **Renacimiento de las herramientas de terminal.** `bat`, `ripgrep`, `fd`, `eza`, `zoxide`, `lazydocker`, `bottom`, `dust` demuestran que existe un mercado enorme y desatendido de desarrolladores dispuestos a instalar herramientas de terminal de alta calidad que hacen **una** cosa bien.

---

## 3. Análisis competitivo

### 3.1 Matriz

| Herramienta | Lista puertos | Resuelve PID | Muestra `cwd` | Detecta proyecto | Detecta framework | TUI | Docker-aware |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| `lsof -i` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `ss -ltnp` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `netstat -tulpn` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `htop` / `btop` | ⚠️ parcial | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `bottom` | ❌ | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `procs` | ⚠️ columna TCP | ✅ | ❌ | ❌ | ❌ | ❌ | ⚠️ |
| `lazydocker` | ⚠️ solo contenedores | ✅ | ❌ | ❌ | ❌ | ✅ | ✅ |
| `sniffnet` | ❌ tráfico | ⚠️ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `whatportis` | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `npx kill-port` | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Portix** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

### 3.2 Lectura del panorama

Ninguna herramienta existente cruza la frontera entre **el proceso** y **el proyecto**. Esa frontera es exactamente el hueco de mercado. Las herramientas de sysadmin no la cruzan porque a un sysadmin no le importa `package.json`; las herramientas de desarrollo no la cruzan porque viven dentro de un único proyecto y no ven el sistema.

Portix se sitúa deliberadamente en la intersección: **conoce `/proc` como un sysadmin y conoce `package.json` como un desarrollador.**

### 3.3 Amenazas competitivas

- **`lsof` con un wrapper de shell.** Cualquiera puede escribir 30 líneas de bash. Nuestra defensa: la calidad de la detección de frameworks, el rendimiento, y la UX. Un script de bash no da búsqueda fuzzy incremental sobre 200 procesos a 60 fps.
- **Que `btop` añada una columna «project».** Improbable — está fuera de su misión — pero posible. Defensa: profundidad. Nosotros tenemos monorepos, workspaces, detección de gestor de paquetes, resolución de contenedores.
- **Una extensión de VS Code.** Cubre a los usuarios de VS Code, no a los de terminal. Segmentos parcialmente disjuntos, y nosotros somos invocables desde CI/scripts (§12.2).

---

## 4. Producto: visión, usuarios y principios

### 4.1 Visión

> En tres años, cuando un desarrollador de Linux vea `EADDRINUSE`, su reflejo muscular será teclear `portix`, no `lsof`.

### 4.2 Misión (v1)

Entregar una TUI que, en menos de 100 ms desde la pulsación de Enter, muestre cada servicio de desarrollo activo con su proyecto, framework y consumo, y permita actuar sobre él sin salir de la terminal.

### 4.3 Personas

**P1 — Elena, Full-Stack en una startup (usuaria primaria, 70% del uso).**
Ejecuta un monorepo con 6 workspaces + Docker Compose. Vive en la terminal con `tmux`. Instala herramientas de la AUR sin pensarlo. Su dolor: nunca sabe qué `node` matar. Usa Portix 5-10 veces al día, sesiones de 5-30 segundos.

**P2 — Marcos, Backend / DevOps (usuario secundario, 25%).**
Corre APIs en Go y Python, Postgres y Redis en contenedores, Traefik como proxy. Su dolor: mapear puertos publicados de contenedores a servicios y detectar colisiones. Valora `--json` para scripting.

**P3 — Yuki, Estudiante / Hobbyista (usuario terciario, 5%).**
Aprende. No sabe qué es un inode. Su dolor: «algo está en el 8080 y no sé qué». Valora que la herramienta sea autoexplicativa, con ayuda embebida y sin necesidad de `sudo`.

**Anti-persona — Raúl, SRE de producción.**
Portix **no** está diseñado para él. No añadiremos alertas, ni exportadores Prometheus, ni monitorización de hosts remotos. Si una feature solo sirve a Raúl, se rechaza.

### 4.4 Principios de producto

1. **El contexto es el producto.** Cualquier columna que no ayude a identificar *qué proyecto es* debe justificar su espacio en pantalla.
2. **Cero configuración para el 90%.** Portix debe ser útil el primer segundo, sin fichero de configuración.
3. **Nunca mentir.** Si un dato no se puede obtener (permisos, namespace), se muestra explícitamente como desconocido. Nunca se adivina y se presenta como hecho. La confianza en la detección es visible.
4. **Sin `sudo` por defecto.** La herramienta debe ser plenamente útil como usuario normal para los procesos del propio usuario (que son el 99% de los servicios de desarrollo).
5. **Rápido de forma perceptible.** Arranque instantáneo, refresco sin parpadeo, búsqueda sin latencia. La velocidad es una feature.
6. **Destructivo requiere confirmación.** `kill` siempre confirma, siempre muestra qué se va a matar, y por defecto envía `SIGTERM`.
7. **Sin telemetría. Nunca.** Ni opt-in.
8. **La terminal es el hogar, no una limitación.** Se explota Unicode, color verdadero, ratón y bracketed paste — con degradación elegante.

### 4.5 No-objetivos explícitos (v1 y v2)

- Monitorización de procesos que no escuchan en ningún socket.
- Gráficas históricas de CPU/memoria a lo largo del tiempo.
- Inspección de tráfico o payloads de red.
- Gestión del ciclo de vida de contenedores (build, pull, compose up).
- Hosts remotos, SSH, Kubernetes.
- Windows / macOS en el MVP (ver §5.5 sobre preparación arquitectónica).
- Edición de ficheros del proyecto.

---

## 5. Decisiones técnicas fundamentales

### 5.1 Lenguaje: Rust

#### 5.1.1 Requisitos que impone el problema

Antes de comparar lenguajes, enumeremos qué exige realmente Portix:

| Requisito | Origen |
|---|---|
| R1. Arranque en frío < 50 ms | La herramienta se invoca decenas de veces al día en sesiones de segundos. Un arranque perceptible destruye el reflejo muscular. |
| R2. Sin GC / sin pausas | Un refresco de UI a 60 fps con pausas de GC produce parpadeo visible. |
| R3. Parseo intensivo de texto sin asignaciones | Leer `/proc/net/tcp` + `/proc/*/stat` cada segundo con 500 procesos son ~2.000 lecturas de fichero y decenas de miles de campos parseados por segundo. |
| R4. RSS < 20 MB | Compite con `htop` (≈5 MB). Una herramienta de diagnóstico que consume más que lo que diagnostica es una broma. |
| R5. Binario único, sin runtime | Empaquetado trivial en AUR; `cargo install` funcional; sin dependencia de una VM. |
| R6. Manejo seguro de `/proc` | Los ficheros de `/proc` mienten sobre su tamaño, cambian entre `open()` y `read()`, y contienen bytes arbitrarios (un `cmdline` puede tener UTF-8 inválido). Los errores aquí son *use-after-free* y *buffer overflow* en lenguajes sin seguridad de memoria. |
| R7. Ecosistema TUI de primer nivel | Ver §5.2. |
| R8. Mantenibilidad por contribuidores OSS | Un proyecto de una persona necesita que un desconocido pueda mandar un detector de framework sin romper nada. |

#### 5.1.2 Matriz de comparación

Puntuación: 🟢 excelente · 🟡 aceptable · 🔴 problemático.

| Criterio | **Rust** | **Go** | **Zig** | **C++** |
|---|:---:|:---:|:---:|:---:|
| Rendimiento bruto (R3) | 🟢 | 🟡 | 🟢 | 🟢 |
| Latencia de arranque (R1) | 🟢 ~2 ms | 🟡 ~8 ms (init runtime) | 🟢 ~1 ms | 🟢 ~2 ms |
| Sin GC / sin pausas (R2) | 🟢 | 🔴 GC | 🟢 | 🟢 |
| Consumo de memoria (R4) | 🟢 ~8 MB | 🔴 ~25-40 MB (heap del runtime) | 🟢 ~4 MB | 🟢 ~6 MB |
| Acceso a `/proc` (R6) | 🟢 `std::fs` + `procfs` crate | 🟢 stdlib | 🟡 manual, sin abstracciones | 🟡 manual, propenso a errores |
| Seguridad de memoria (R6) | 🟢 garantizada | 🟢 garantizada | 🔴 manual (sin borrow checker) | 🔴 manual |
| Binario estático (R5) | 🟢 (musl o glibc) | 🟢 | 🟢 | 🟡 (linkado de libstdc++) |
| Tamaño del binario | 🟡 ~4-6 MB | 🔴 ~10-15 MB | 🟢 ~1 MB | 🟡 ~3 MB |
| Ecosistema TUI (R7) | 🟢 `ratatui` (referencia) | 🟢 Bubble Tea | 🔴 inexistente/inmaduro | 🔴 `ncurses`, `ftxui` (limitado) |
| Estabilidad del lenguaje (R8) | 🟢 1.0 desde 2015 | 🟢 muy estable | 🔴 pre-1.0, breaking changes | 🟢 estable |
| Gestión de dependencias | 🟢 Cargo | 🟢 módulos | 🟡 build.zig, joven | 🔴 CMake/vcpkg/Conan |
| Empaquetado AUR | 🟢 patrón establecido, `cargo` en `makedepends` | 🟢 establecido | 🟡 raro | 🟡 verboso |
| Barrera de entrada al contribuidor | 🟡 borrow checker | 🟢 baja | 🔴 lenguaje nicho | 🔴 alta (C++ moderno) |
| Manejo de errores robusto | 🟢 `Result` obliga a tratarlos | 🟡 `if err != nil` ignorable | 🟢 error unions | 🔴 excepciones o códigos |
| Concurrencia sin data races | 🟢 garantizado en compilación | 🔴 races posibles | 🔴 manual | 🔴 manual |

#### 5.1.3 Análisis por candidato

**Go — el rival más serio.** Bubble Tea es excelente, el lenguaje es fácil de contribuir y la stdlib cubre `/proc` sin ceremonia. Se rechaza por dos motivos que, para este producto concreto, son descalificatorios:

- **Memoria (R4).** El runtime de Go arranca con un heap y un scheduler que sitúan el RSS mínimo realista en 20-40 MB. Para una herramienta que se anuncia como ligera y compite visualmente con `btop` (5-10 MB), es una desventaja estructural que no se puede optimizar.
- **GC (R2).** El parseo de `/proc` genera basura a alta cadencia. Un ciclo de GC durante el `draw()` de la TUI produce jank observable. Es domesticable con `sync.Pool` y tuning de `GOGC`, pero se está luchando contra el lenguaje en el camino caliente.

Menor, pero real: el binario de Go es 2-3× más grande, y `go` como `makedepends` en AUR es igual de trivial que `rust`, así que ahí empatan.

**Zig — el rival ideológico.** Gana en tamaño de binario, arranque y control. Se rechaza por tres motivos:

- **Pre-1.0.** Zig todavía introduce cambios rompedores entre versiones menores. Un proyecto OSS que aspira a estar en AUR durante años no puede permitirse que `zig 0.16` rompa la compilación y bloquee a los packagers.
- **Sin ecosistema TUI.** Habría que construir el renderizador, el manejo de eventos, el layout y el diffing de buffers desde cero. Eso es entre 3 y 6 meses de trabajo que no aporta valor de producto. R7 es un requisito duro.
- **Sin seguridad de memoria (R6).** Parsear `/proc/<pid>/cmdline` — bytes hostiles, longitud variable, contenido arbitrario controlable por cualquier usuario del sistema — sin borrow checker es una fuente de CVEs. Zig tiene defensas (bounds checking en debug/safe), pero no elimina *use-after-free*.

**C++ — el rival histórico.** `btop` está escrito en C++ y es magnífico, lo que demuestra viabilidad. Se rechaza porque en 2026 sus únicas ventajas sobre Rust son inercia y familiaridad. Pierde en seguridad de memoria, gestión de dependencias (crítico para contribuidores casuales), manejo de errores y concurrencia. `ftxui` es competente pero está una generación por detrás de `ratatui`.

#### 5.1.4 Decisión

**Rust**, por la conjunción de:

1. **Sin GC + sin runtime** → satisface R1, R2 y R4 simultáneamente. Ningún otro candidato satisface los tres sin concesiones.
2. **Seguridad de memoria con coste cero** → R6. Portix parsea entrada no confiable procedente de otros usuarios del sistema. Es el punto de mayor exposición de la herramienta, y aquí el compilador es un revisor gratuito.
3. **`ratatui` es, de forma no ambigua, el mejor framework TUI que existe hoy en cualquier lenguaje** → R7. Ver §5.2.
4. **Cargo** → R8. Un contribuidor clona, ejecuta `cargo test`, funciona. Sin excepciones.
5. **`Result<T, E>` obligatorio** → todo acceso a `/proc` es falible (el proceso murió entre `readdir` y `open`). Rust hace imposible olvidar ese caso; Go lo hace fácil de ignorar; C++ lo hace fácil de crashear.
6. **AUR** → `makedepends=(cargo)`, `cargo build --release --locked`, `install -Dm755`. Patrón resuelto, replicado en cientos de paquetes (`ripgrep`, `bat`, `fd`, `bottom`, `eza`).

**Coste aceptado:** la curva del borrow checker eleva la barrera al contribuidor casual. Se mitiga con la arquitectura de detectores declarativos (§9.5), que permite añadir soporte para un framework nuevo **editando un fichero TOML, sin escribir Rust**. Este es un movimiento deliberado de diseño de producto, no un accidente.

**MSRV:** Rust 1.82. Se revisará cada 6 meses; política N-4 (soportar las 4 versiones estables anteriores) para no romper a los packagers de distribuciones conservadoras.

---

### 5.2 Framework TUI: `ratatui` + `crossterm`

#### 5.2.1 El espacio de opciones en Rust

| Opción | Modelo | Veredicto |
|---|---|---|
| `ratatui` | Inmediato (immediate mode), widgets componibles | ✅ **Elegido** |
| `cursive` | Retenido (retained mode), árbol de vistas, callbacks | ❌ |
| `tui-rs` | Predecesor de `ratatui` | ❌ Sin mantenimiento desde 2022 |
| `termwiz` | Bajo nivel, motor de WezTerm | ❌ Demasiado bajo nivel |
| ncurses (FFI) | Retenido, C | ❌ Insegura, fea, no idiomática |

#### 5.2.2 Por qué `ratatui`

**a) El modo inmediato es el modelo correcto para este problema.**
El estado de Portix cambia por completo cada ~1 s (nuevas métricas de CPU, procesos que aparecen y desaparecen). En modo retenido (`cursive`) habría que sincronizar manualmente un árbol de vistas con el estado del mundo: crear, actualizar y destruir nodos. En modo inmediato, se redibuja todo desde el estado y `ratatui` calcula el diff contra el buffer anterior, emitiendo solo las celdas modificadas. **El estado del mundo es la única fuente de verdad y no hay sincronización que mantener.** Para una app cuyo trabajo es *mostrar un mundo que cambia*, esto elimina una clase entera de bugs.

**b) Es el estándar de facto y está vivo.**
`ratatui` es el sucesor mantenido por la comunidad de `tui-rs` (archivado). Es la base de `gitui`, `bottom`, `oha`, `atuin`, `yazi`, `television` y decenas más. Releases regulares, ecosistema de widgets de terceros, `ratatui.rs` con documentación real. Elegir `ratatui` es elegir el lugar donde están los contribuidores.

**c) Calidad visual suficiente para el listón «btop».**
Soporte de color verdadero (24-bit), estilos y modificadores completos, bordes redondeados y sets de caracteres Unicode, `Sparkline`/`Chart`/`Gauge`/`Table` con estado, layout basado en restricciones (`Constraint::Fill`, `Min`, `Length`, `Ratio`, `Percentage`) que resuelve el layout responsive sin aritmética manual, scroll y selección con estado (`TableState`, `ListState`), y **soporte nativo de widgets superpuestos** (`Clear` + área) para modales, imprescindible para los diálogos de confirmación y la ventana de ayuda.

**d) Coste de renderizado alineado con el presupuesto.**
Redibujar el frame completo cuesta O(celdas) — para una terminal de 200×50 son 10.000 celdas, decenas de microsegundos. El diff del buffer garantiza que solo se escriben en el `stdout` las celdas cambiadas, así que el coste real dominante es el del terminal, no el nuestro.

#### 5.2.3 Por qué `crossterm` como backend

`ratatui` es agnóstico del backend y ofrece tres: `crossterm`, `termion` y `termwiz`.

| | `crossterm` | `termion` | `termwiz` |
|---|---|---|---|
| Multiplataforma | ✅ Linux/macOS/Windows | ❌ solo Unix | ✅ |
| Eventos de ratón | ✅ | ⚠️ limitado | ✅ |
| Bracketed paste | ✅ | ❌ | ✅ |
| Kitty keyboard protocol | ✅ | ❌ | ✅ |
| Focus in/out | ✅ | ❌ | ✅ |
| Resize events | ✅ nativo | ⚠️ vía signal | ✅ |
| Mantenimiento | 🟢 activo | 🟡 lento | 🟡 acoplado a WezTerm |
| Peso de dependencias | 🟡 medio | 🟢 mínimo | 🔴 pesado |

**Elegido: `crossterm`.**

1. **Es el backend por defecto de `ratatui`** y el mejor probado; los bugs se descubren antes.
2. **Kitty keyboard protocol** permite distinguir `Ctrl+Enter` de `Enter` y detectar key-release, lo que abre atajos que `termion` no puede expresar. Terminales modernos (`kitty`, `foot`, `ghostty`, `wezterm`, `alacritty` parcialmente) lo soportan; hay degradación limpia en el resto.
3. **Bracketed paste** evita que pegar una ruta en la barra de búsqueda se interprete como una ráfaga de atajos de teclado. Sin esto, pegar `~/work/api` dispara los atajos `w`, `o`, `r`, `k`. Es un bug de UX real, no teórico.
4. **Portabilidad futura sin coste presente** (§5.5): cuando llegue Windows/macOS, la capa de terminal ya está lista y el trabajo se concentra donde debe estar (la capa de plataforma).

`termion` se descarta porque su ausencia de bracketed paste y de eventos de ratón compromete directamente requisitos de UX del §11. `termwiz` se descarta por peso y por acoplamiento a otro proyecto.

#### 5.2.4 Conclusión

> **`ratatui` + `crossterm` es hoy la única combinación en cualquier lenguaje que ofrece simultáneamente: modo inmediato, color verdadero, layout por restricciones, modales, ratón, bracketed paste, protocolo de teclado moderno, cero coste de runtime y un ecosistema activo de contribuidores.** Bubble Tea (Go) es su único competidor real y pierde por el runtime de Go (§5.1.3).

---

### 5.3 Modelo de concurrencia: hilos, no `async`

Portix realiza **I/O de ficheros bloqueante sobre `/proc`**, un pseudo-sistema de ficheros en memoria donde las lecturas **nunca bloquean de verdad** (no hay disco). No hay I/O de red, no hay miles de conexiones concurrentes, no hay nada que justifique un runtime async.

**Decisión: tres hilos y canales (`std::sync::mpsc` o `crossbeam-channel`), sin `tokio`.**

```
┌────────────────┐  Event    ┌──────────────┐  Snapshot  ┌───────────────┐
│  input thread  │──────────▶│  main / UI   │◀───────────│ collector     │
│  (crossterm)   │  channel  │   thread     │  channel   │ thread (proc) │
└────────────────┘           └──────────────┘            └───────────────┘
                                    │                            │
                                    │ draw()                     │ spawn
                                    ▼                            ▼
                              ┌──────────┐              ┌────────────────┐
                              │ terminal │              │ enrichment pool│
                              └──────────┘              │ (rayon, N=4)   │
                                                        └────────────────┘
```

| Hilo | Responsabilidad | Cadencia |
|---|---|---|
| **UI / main** | Posee el `App` state. Recibe eventos, aplica reducers, dibuja. **Nunca hace I/O de `/proc`.** | Reactivo + tick 60 fps máx |
| **Input** | `crossterm::event::read()` bloqueante → canal | Reactivo |
| **Collector** | Escanea `/proc`, produce un `Snapshot` inmutable completo | 1 Hz (configurable) |
| **Enrichment pool** | Tareas caras y falibles: leer `package.json`, resolver rutas de contenedores, `git remote` | Bajo demanda, con `rayon` |

**Justificación de la separación:** la regla de oro es que **el hilo de UI jamás toca el sistema de ficheros**. Un `read()` sobre `/proc/<pid>/cwd` de un proceso en un mount namespace exótico, o un `package.json` en un NFS colgado, puede tardar segundos. Si eso ocurre en el hilo de UI, la aplicación se congela y no responde ni a `q`. Aislar el I/O en hilos garantiza que **la interfaz siempre responde**, aunque los datos lleguen tarde o no lleguen.

**Coste aceptado:** añadir `tokio` sería más «moderno» y facilitaría hablar con el socket de Docker (v1.5). Se rechaza porque introduce ~1,5 MB de binario, aumenta el tiempo de arranque y complica el stack de dependencias, todo para resolver un problema que no tenemos. Si Docker vía API llega en v1.5, se usará un cliente HTTP bloqueante sobre el socket Unix (`ureq` + `UnixStream`) en el pool de enriquecimiento.

**Contrato de datos:** el collector produce un `Arc<Snapshot>` completo e inmutable. La UI reemplaza el puntero. No hay mutex compartidos, no hay locking en el camino de dibujo, no hay estado a medio actualizar visible en pantalla. Cambiar de snapshot es una asignación atómica de puntero.

---

### 5.4 Dependencias propuestas

Cada dependencia es una carga de mantenimiento y una superficie de supply chain. Se justifica una a una. Objetivo duro: **< 90 crates transitivos** en el grafo de release.

| Crate | Uso | ¿Imprescindible? | Alternativa si cae |
|---|---|---|---|
| `ratatui` | Framework TUI | Sí | — |
| `crossterm` | Backend de terminal | Sí | `termion` |
| `libc` | Syscalls puntuales (`getuid`, `sysconf`, `kill`, `getpwuid_r`) | Sí | `nix` (más pesado) |
| `serde` + `serde_json` | Leer `package.json`, `composer.json`; salida `--json` | Sí | — |
| `toml` | Config del usuario y detectores declarativos | Sí | — |
| `nucleo-matcher` | Fuzzy matching (motor de Helix) | Sí | `fuzzy-matcher` |
| `clap` (derive) | CLI | Sí | `lexopt` (más ligero) |
| `clap_complete` | Completions de shell (`--completions`) | Sí | Manual |
| `anyhow` | Errores | Sí | — |
| `directories` | Rutas XDG | Sí | `$XDG_*` a mano |
| `arboard` | Portapapeles (Wayland + X11) | Solo v1 | `wl-copy`/`xclip` externos (usado hoy) |
| `notify` | inotify para invalidar caché | v1.5 | Polling de `mtime` |

**Estado real del `Cargo.toml` (MVP):** `procfs` fue **rechazado** (ver nota abajo) y la capa `linux/` se escribió a mano; `rustix` nunca se añadió — los cuatro syscalls necesarios van por `libc`; `crossbeam-channel` y `rayon` **se retiraron** porque el enriquecimiento aún es síncrono dentro de `scan()` (se reintroducirán con el Tier 3 asíncrono de §10.3); `thiserror` se eliminó por falta de uso (los errores son `anyhow`); `arboard` sigue comentado en favor de `wl-copy`/`xclip` vía `Command` (`core/actions.rs`).

**Nota sobre `procfs`:** el crate es cómodo pero asigna agresivamente (`String` por campo) y expone estructuras pesadas que no necesitamos. En el camino caliente (leer `stat` de 500 procesos a 1 Hz) queremos parsear a `&[u8]` con buffers reutilizados y cero asignaciones. **Recomendación: escribir la capa `linux/` a mano** (es ~600 líneas, totalmente testeable con fixtures) y usar `procfs` únicamente como referencia de formatos y en tests como oráculo cruzado. Esto también elimina una dependencia del camino crítico y nos da control total sobre el manejo de errores parciales.

**Higiene de supply chain:**
- `cargo-deny` en CI: licencias permitidas, avisos de RUSTSEC, duplicados de versión prohibidos.
- `cargo-audit` en CI y semanalmente vía cron.
- `Cargo.lock` **comiteado** (es un binario, no una librería).
- `#![forbid(unsafe_code)]` en todos los módulos excepto `linux/`, donde el `unsafe` (si lo hay) se documenta con un comentario `SAFETY:` obligatorio, revisado por un maintainer.
- Ninguna dependencia con `build.rs` que descargue nada de la red.

---

### 5.5 Preparación para multiplataforma (sin implementarla)

El MVP es solo Linux, pero la arquitectura no debe cerrarse. La regla es simple:

> **Todo el código específico de Linux vive bajo `src/platform/linux/` detrás de un único trait `PlatformProbe`. Ningún otro módulo importa `/proc`, `rustix` o rutas absolutas del sistema.**

El trait expone métodos como `list_listening_sockets()`, `list_processes()`, `process_metadata(pid)`, `container_of(pid)`, `signal(pid, sig)`. Portar a macOS significa implementar `PlatformProbe` con `libproc`/`sysctl`; portar a Windows, con `GetExtendedTcpTable` + `QueryFullProcessImageName`. Los módulos `detect/`, `cache/`, `ui/` y `core/` no cambian ni una línea.

Esto no cuesta nada hoy (es una capa de indirección que ya necesitamos para testear con `/proc` simulado, §16.2) y ahorra una reescritura mañana. **Se implementa el trait desde el día uno; se implementa una sola plataforma.**

---

## 6. Arquitectura

### 6.1 Vista de capas

```
╔══════════════════════════════════════════════════════════════════════╗
║                            PRESENTACIÓN                              ║
║  ui/                                                                 ║
║  ├── app.rs        Estado de la aplicación, reducer de eventos       ║
║  ├── views/        main_table · detail_panel · help · confirm_modal  ║
║  ├── widgets/      Widgets propios reutilizables                     ║
║  ├── theme.rs      Paleta, degradación 24-bit → 256 → 16             ║
║  ├── input.rs      Keymap, modos (Normal / Search / Filter / Modal)  ║
║  └── format.rs     Humanización: bytes, duraciones, truncado         ║
╠══════════════════════════════════════════════════════════════════════╣
║                            APLICACIÓN                                ║
║  core/                                                               ║
║  ├── engine.rs     Orquesta collector ↔ caché ↔ detectores           ║
║  ├── snapshot.rs   Construcción del Snapshot inmutable               ║
║  ├── query.rs      Búsqueda fuzzy, filtros, ordenación               ║
║  ├── actions.rs    kill · open · copy · reveal (efectos)             ║
║  └── config.rs     Carga y validación de configuración               ║
╠══════════════════════════════════════════════════════════════════════╣
║                             DOMINIO                                  ║
║  model/            Tipos puros, sin I/O, sin dependencias            ║
║  ├── service.rs    Service (la entidad central)                      ║
║  ├── process.rs    Process, ProcessMetrics                           ║
║  ├── socket.rs     Socket, Protocol, TcpState, BindScope             ║
║  ├── project.rs    Project, PackageManager, Workspace                ║
║  ├── framework.rs  Framework, Language, Confidence                   ║
║  └── container.rs  Container, Runtime                                ║
║                                                                      ║
║  detect/           Sistema de detectores extensible                  ║
║  ├── registry.rs   Registro, orden, resolución de conflictos         ║
║  ├── signals.rs    Extracción de señales del proceso/proyecto        ║
║  ├── scoring.rs    Puntuación y confianza                            ║
║  ├── project/      Detectores de raíz + nombre de proyecto           ║
║  ├── framework/    Detectores compilados (node, python, go, rust…)   ║
║  └── declarative.rs Motor de detectores definidos en TOML            ║
║                                                                      ║
║  cache/                                                              ║
║  ├── store.rs      Cachés L1/L2/L3 con TTL e invalidación            ║
║  └── keys.rs       Claves estables (inode del cwd, no la ruta)       ║
╠══════════════════════════════════════════════════════════════════════╣
║                          INFRAESTRUCTURA                             ║
║  platform/                                                           ║
║  ├── mod.rs        trait PlatformProbe                               ║
║  └── linux/                                                          ║
║      ├── net.rs        /proc/net/{tcp,tcp6,udp,udp6}                 ║
║      ├── proc.rs       /proc/<pid>/{cmdline,cwd,exe,status,stat}     ║
║      ├── fd.rs         /proc/<pid>/fd → socket:[inode]               ║
║      ├── cgroup.rs     cgroup v1/v2 → container id                   ║
║      ├── netns.rs      /proc/<pid>/ns/net                            ║
║      ├── metrics.rs    CPU (delta jiffies), RSS, uptime              ║
║      ├── signal.rs     kill(2) seguro                                ║
║      └── root.rs       Raíz inyectable de /proc (para tests)         ║
║                                                                      ║
║  util/             Buffers reutilizables, parseo hex, lossy UTF-8    ║
╚══════════════════════════════════════════════════════════════════════╝
```

### 6.2 Responsabilidades por módulo

| Módulo | Responsabilidad | Puede depender de | Prohibido |
|---|---|---|---|
| `model/` | Definir el vocabulario del dominio. Tipos, invariantes, `Display`. | Nada (solo `std` + `serde`) | Cualquier I/O |
| `platform/` | Única puerta al sistema operativo. Traduce bytes del kernel a tipos de `model/`. | `model/`, `rustix` | `ui/`, `detect/` |
| `detect/` | Deducir proyecto y framework a partir de señales. Funciones puras siempre que sea posible. | `model/`, lectura acotada de FS | `platform/` directamente, `ui/` |
| `cache/` | Memorizar resultados caros con invalidación correcta. | `model/` | Lógica de negocio |
| `core/` | Orquestación. El único módulo que conoce a todos los demás. | Todos | `ratatui` |
| `ui/` | Pixels y teclas. Función pura del estado. | `model/`, `core::query` | `platform/`, `std::fs` |
| `util/` | Primitivas sin dominio. | Nada | Todo lo demás |

**Regla de dependencia (invariante arquitectónico, verificado en CI con `cargo-modules` o un test de arquitectura):**

```
ui ──▶ core ──▶ { detect, cache, platform } ──▶ model ──▶ util
```

Las flechas nunca se invierten. `model/` no sabe que existe una terminal. `ui/` no sabe que existe `/proc`. Esta es la propiedad que hace testeable el 85% del código sin un Linux real y que hace posible el port a macOS sin tocar la UI.

### 6.3 El flujo canónico: de puerto a framework

Este es **el corazón del producto**. Se detalla exhaustivamente en §7 y §8; aquí la vista de pájaro.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  ① PUERTO                                                               │
│     Fuente: /proc/net/tcp, tcp6, udp, udp6                              │
│     Se filtra por estado LISTEN (0A) para TCP.                          │
│     Se obtiene: local_addr:port, remote, state, uid, INODE              │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │  inode = 486312
┌──────────────────────────────▼──────────────────────────────────────────┐
│  ② SOCKET → INODE                                                       │
│     El inode es la clave primaria del socket en el kernel.              │
│     No hay atajo: /proc/net/* NO contiene el PID.                       │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────────┐
│  ③ INODE → PID                                                          │
│     Para cada /proc/<pid>/fd/<n>, readlink() devuelve "socket:[486312]" │
│     Se construye UNA SOLA VEZ un mapa inode → (pid, fd).                │
│     Coste: O(total_fds). Ver §7.3 para la optimización crítica.         │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │  pid = 48213
┌──────────────────────────────▼──────────────────────────────────────────┐
│  ④ PID → PROCESO                                                        │
│     /proc/48213/cmdline  → ["node", "/…/next", "dev"]                   │
│     /proc/48213/exe      → /usr/bin/node                                │
│     /proc/48213/cwd      → /home/sergi/work/acme/apps/web               │
│     /proc/48213/status   → Name, Uid, PPid, VmRSS, Threads              │
│     /proc/48213/stat     → utime, stime, starttime, state               │
│     /proc/48213/cgroup   → /system.slice/docker-<id>.scope  (o no)      │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │  cwd = /home/sergi/work/acme/apps/web
┌──────────────────────────────▼──────────────────────────────────────────┐
│  ⑤ PROCESO → PROYECTO                                                   │
│     Ascender desde cwd buscando marcadores: package.json, Cargo.toml,   │
│     pyproject.toml, go.mod, composer.json, .git …                       │
│     Parar en el primer marcador (workspace member).                     │
│     Continuar hasta la raíz del workspace si hay pnpm-workspace.yaml,   │
│     turbo.json, [workspace] en Cargo.toml, etc.                         │
│     → Project { root, name, workspace_root, package_manager }           │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────────┐
│  ⑥ PROYECTO + PROCESO → FRAMEWORK                                       │
│     Se emiten SEÑALES:                                                  │
│       · cmdline contiene "next"          → +0.8 Next.js                 │
│       · dependencies.next existe          → +0.9 Next.js                │
│       · existe next.config.ts             → +0.7 Next.js                │
│       · puerto == 3000                    → +0.1 Next.js                │
│     El registry puntúa, desempata y devuelve                            │
│     Framework { name, version, confidence }                             │
└──────────────────────────────┬──────────────────────────────────────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │      SERVICE        │  ← la entidad que ve el usuario
                    └─────────────────────┘
```

### 6.4 Modelo de datos

`Service` es la entidad central: **la unión de un socket en escucha, su proceso, su proyecto y su framework.**

**`Service`**

| Campo | Tipo | Origen | Puede faltar |
|---|---|---|---|
| `socket` | `Socket` | `/proc/net/*` | No |
| `process` | `Option<Process>` | mapa inode→pid | Sí (proceso de otro usuario sin root) |
| `project` | `Option<Project>` | ascenso desde `cwd` | Sí |
| `framework` | `Option<Framework>` | registry de detectores | Sí |
| `container` | `Option<Container>` | `cgroup` | Sí |
| `id` | `ServiceId` | `(proto, addr, port, inode)` | No |

**`Socket`**

| Campo | Tipo | Notas |
|---|---|---|
| `protocol` | `Tcp \| Tcp6 \| Udp \| Udp6` | |
| `local_addr` | `IpAddr` | |
| `port` | `u16` | |
| `state` | `TcpState` | `Listen` para el caso principal |
| `bind_scope` | `Loopback \| AllInterfaces \| Specific` | Insumo de UX: `0.0.0.0` merece un indicador |
| `inode` | `u64` | Clave de unión |
| `uid` | `u32` | |

**`Process`**

| Campo | Tipo | Origen | Coste |
|---|---|---|---|
| `pid`, `ppid` | `i32` | `stat` | Barato |
| `name` | `String` | `status:Name` | Barato |
| `cmdline` | `Vec<String>` | `cmdline` | Barato |
| `exe` | `Option<PathBuf>` | `exe` (readlink) | Barato |
| `cwd` | `Option<PathBuf>` | `cwd` (readlink) | Barato, falla sin permisos |
| `user` | `Option<String>` | `uid` → `/etc/passwd` (cacheado) | Barato con caché |
| `started_at` | `SystemTime` | `stat:starttime` + `/proc/stat:btime` | Barato |
| `metrics` | `ProcessMetrics` | `stat` + `status` | Barato, necesita delta |
| `state` | `Running \| Sleeping \| Zombie \| …` | `stat:state` | Barato |
| `threads` | `u32` | `status:Threads` | Barato |

**`ProcessMetrics`**

| Campo | Cálculo |
|---|---|
| `cpu_percent` | `(Δ(utime+stime) / Δ(total_jiffies_del_sistema)) × ncpu × 100` — requiere dos muestras |
| `rss_bytes` | `status:VmRSS` (rápido) |
| `virt_bytes` | `status:VmSize` |
| `uptime` | `now − started_at` |

**`Project`**

| Campo | Ejemplo |
|---|---|
| `root` | `/home/sergi/work/acme/apps/web` |
| `workspace_root` | `Some(/home/sergi/work/acme)` |
| `name` | `@acme/web` |
| `display_name` | `acme/web` (derivado, para la UI) |
| `language` | `TypeScript` |
| `package_manager` | `Pnpm` |
| `vcs_branch` | `Option<String>` — v1, no MVP |

**`Framework`**

| Campo | Ejemplo |
|---|---|
| `id` | `nextjs` |
| `display_name` | `Next.js` |
| `version` | `Some("15.1.0")` |
| `language` | `Node` |
| `confidence` | `High \| Medium \| Low` |
| `matched_signals` | `["cmdline:next", "dep:next@15.1.0", "file:next.config.ts"]` (para el panel de detalle y para depurar detectores) |

**`Container`**

| Campo | Ejemplo |
|---|---|
| `runtime` | `Docker \| Podman \| Containerd \| Unknown` |
| `id` | `3f2a9c…` (12 chars mostrados) |
| `name` | `acme-postgres-1` (v1.5, requiere API) |
| `compose_project` | `Some("acme")` (v1.5) |

**`Snapshot`** — lo que el collector entrega a la UI:

| Campo | Notas |
|---|---|
| `services` | `Vec<Service>` |
| `taken_at` | `Instant` |
| `scan_duration` | Para la barra de estado |
| `degradations` | `Vec<Degradation>` — p.ej. `SocketsWithoutPid(3)` por falta de permisos. **Nunca se silencia.** |

### 6.5 Ciclo de vida y flujo de eventos

```
                       ┌──────────────────────────┐
                       │   Event (union type)     │
                       ├──────────────────────────┤
   crossterm ────────▶ │  Key(KeyEvent)           │
                       │  Mouse(MouseEvent)       │
                       │  Resize(w, h)            │
   collector ────────▶ │  Snapshot(Arc<Snapshot>) │
   enrichment ───────▶ │  Enriched(ServiceId, …)  │
   timer ────────────▶ │  Tick                    │
                       │  Error(AppError)         │
                       └────────────┬─────────────┘
                                    ▼
                       ┌──────────────────────────┐
                       │  App::handle(Event)      │  ← puro, testeable
                       │  → muta App, devuelve    │
                       │    Vec<Effect>           │
                       └────────────┬─────────────┘
                                    ▼
                       ┌──────────────────────────┐
                       │  Effect                  │
                       ├──────────────────────────┤
                       │  Kill(pid, signal)       │
                       │  OpenBrowser(url)        │
                       │  CopyToClipboard(text)   │
                       │  SpawnEditor(path)       │
                       │  RequestEnrichment(id)   │
                       │  Quit                    │
                       └────────────┬─────────────┘
                                    ▼
                       ┌──────────────────────────┐
                       │  App::draw(&Frame)       │  ← puro, snapshot-testeable
                       └──────────────────────────┘
```

**Por qué esta forma (Elm-like / MVU):** separar `handle` (estado → estado + efectos) de `draw` (estado → pixels) y de la ejecución de efectos hace que **la lógica de la aplicación sea testeable sin terminal y sin sistema operativo**. Un test envía `Key('/')`, comprueba que `App.mode == Search`, envía `Key('n')`, comprueba que el filtro es `"n"` y que la selección cambió a la fila esperada. Cero mocks, cero `/proc`, cero terminal. Es la razón por la que se puede pedir un 80% de cobertura sin escribir tests frágiles.

**Redibujado:** se dibuja cuando hay un evento, nunca en un bucle ocioso. Un tick de 16 ms solo se activa mientras haya una animación en curso (spinner de carga). En reposo con la terminal enfocada y sin cambios, **el uso de CPU debe ser indistinguible de cero** entre snapshots (§15).

---

## 7. Descubrimiento: del puerto al proceso

Esta es la sección más específica de Linux y la de mayor riesgo de implementación. Se documenta con detalle suficiente para que un ingeniero la implemente sin investigación adicional.

### 7.1 Enumeración de sockets: `/proc/net/*`

Cuatro ficheros: `/proc/net/tcp`, `/proc/net/tcp6`, `/proc/net/udp`, `/proc/net/udp6`. Formato de `tcp`:

```
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486312 1 0000…
   1: 00000000:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486401 1 0000…
```

**Campos que consumimos:** `local_address` (índice 1), `st` (2), `uid` (7), `inode` (9).

**Trampas de parseo — cada una ha causado bugs en herramientas reales:**

1. **Endianness.** `local_address` es `ADDR:PORT` en hexadecimal. El puerto es big-endian (`1F90` = 8080). La dirección IPv4 es un `u32` en **orden de host**, es decir little-endian en x86: `0100007F` → bytes `01 00 00 7F` → leídos al revés → `127.0.0.1`. **`0100007F` NO es `1.0.0.127`.**
2. **IPv6.** 32 caracteres hex = cuatro `u32` en orden de host. Cada grupo de 8 caracteres se invierte por bytes **independientemente**; los cuatro grupos conservan su orden entre sí. Confundir esto produce direcciones plausibles pero erróneas.
3. **`::` vs `0.0.0.0`.** Un socket IPv6 con `IPV6_V6ONLY=0` escuchando en `::` **también acepta IPv4** y no aparece en `/proc/net/tcp`. Si mostramos solo `tcp`, perdemos la mayoría de servidores Node modernos, que escuchan en `::`. **Hay que leer siempre los cuatro ficheros.**
4. **Deduplicación.** Un servidor que hace `bind` explícito a `0.0.0.0` y a `::` produce **dos** entradas para «el mismo servicio». La UI debe agruparlas por `(pid, port, protocol_family)` y mostrar una fila con un indicador de doble stack, no dos filas. Sin esto, `next dev` aparece duplicado y la herramienta parece rota.
5. **Estado.** Solo `st == 0A` (LISTEN) para TCP. UDP no tiene estados de conexión: se incluye todo socket UDP con puerto local ≠ 0 y `rem_address` nulo. Tabla completa de estados en §22.2.
6. **El fichero no es atómico.** Se lee de una vez a un buffer (`read_to_end` a un `Vec<u8>` reutilizado) y se parsea del buffer, nunca línea a línea con `BufReader` haciendo múltiples `read()`.
7. **`uid` no es el dueño del proceso necesariamente**, es el uid del socket. Casi siempre coinciden; no asumir.
8. **`/proc/net/*` es por network namespace.** Ver §7.7. Este es el motivo #1 por el que los contenedores «desaparecen».

### 7.2 De inode a PID

`/proc/net/*` **no contiene el PID**. La única vía es invertir el mapa: recorrer `/proc/<pid>/fd/` y hacer `readlink()` sobre cada descriptor. Los sockets aparecen como enlaces simbólicos con destino `socket:[486312]`.

```
/proc/48213/fd/23 → socket:[486312]
                             ^^^^^^ el inode que buscamos
```

### 7.3 La optimización crítica: no hacer O(n×m)

**La implementación ingenua** —para cada socket, recorrer todos los procesos buscando su inode— es O(sockets × procesos × fds). Con 40 sockets, 400 procesos y 100 fds de media, son **1,6 millones de `readlink()` por refresco**. Es exactamente por esto que `lsof` es lento.

**La implementación correcta** es una única pasada que construye el mapa inverso:

```
1. Leer los 4 ficheros de /proc/net/*.        → conjunto de inodes de interés (≈40)
2. Una pasada por /proc/*/fd/*:                → O(total_fds), ≈40.000 readlink()
   Para cada enlace que empiece por "socket:[", extraer el inode.
   Si el inode ∈ conjunto_de_interés, registrar inode → (pid, fd).
   Descartar inmediatamente en caso contrario.
3. Join.                                        → O(sockets)
```

**Optimizaciones adicionales, en orden de impacto:**

- **Poda por UID (la más grande).** Sin root, `readlink` sobre `/proc/<pid>/fd/*` de otro usuario falla con `EACCES` **de todos modos**. Leer el `uid` de `/proc/<pid>/status` (o hacer `stat()` sobre el directorio `/proc/<pid>`, más barato: un solo `statx`) y **saltar por completo** los procesos que no son nuestros cuando `euid != 0`. En una máquina de escritorio típica esto elimina el 60-70% de los procesos antes de abrir un solo directorio `fd/`.
- **Poda por conjunto de inodes.** El `if inode ∈ set` se hace **antes** de asignar nada. Con un `HashSet<u64>` de 40 elementos es un `lookup` de nanosegundos. Nunca se construye un `String` con el destino del enlace: se hace `readlink` a un buffer de pila de 64 bytes reutilizado y se parsea el número in situ. **Cero asignaciones en el bucle caliente.**
- **Parada temprana.** Cuando `encontrados == inodes_de_interés.len()`, se aborta el recorrido. En el caso común (los servicios de desarrollo son procesos con pocos fds y arrancados recientemente, es decir PIDs altos) esto no ayuda mucho; con el recorrido en orden descendente de PID, ayuda bastante.
- **Paralelismo.** El recorrido de `/proc/<pid>/fd/` es trivialmente paralelo (`rayon::par_iter` sobre la lista de PIDs). Con 8 hilos, el escaneo baja de ~35 ms a ~8 ms en la máquina de referencia.
- **Caché entre refrescos (§10).** Un proceso cuyo `starttime` no ha cambiado y cuyo conjunto de fds ya conocemos no necesita re-escanearse si su inode sigue en el conjunto.

**Presupuesto:** el paso completo puerto→PID debe costar **< 15 ms** con 500 procesos (§15).

### 7.4 Metadatos del proceso

| Fichero | Contenido | Trampas |
|---|---|---|
| `cmdline` | argv separado por `\0` | Puede estar **vacío** (kernel threads). Puede no terminar en `\0`. Puede contener UTF-8 inválido → usar `String::from_utf8_lossy`. Un proceso puede reescribir su propio argv (`setproctitle`), típico en `gunicorn`, y entonces no está `\0`-separado sino que es una única cadena con espacios. **Manejar ambos casos.** |
| `exe` | symlink al ejecutable | Puede terminar en ` (deleted)` si el binario fue reemplazado (¡común durante `cargo build` o actualizaciones!). Falla con `EACCES` sin permisos. Para procesos en contenedor, la ruta es válida **dentro del mount namespace del contenedor**, no del nuestro. |
| `cwd` | symlink al directorio de trabajo | **El campo más valioso de todo Portix.** Falla con `EACCES` sin permisos. Para contenedores, ruta relativa al rootfs del contenedor (`/app`, `/usr/src/app`) → hay que resolverla vía `/proc/<pid>/root/<cwd>` para que sea navegable desde el host. Puede ser `/` (servicio de systemd) o un directorio borrado. |
| `status` | Clave-valor legible | `Name` está **truncado a 15 caracteres** (`TASK_COMM_LEN`). `node` está bien; `python3.11` está bien; pero jamás usar `Name` como identidad. Usar `cmdline[0]` o `exe`. `VmRSS` no existe para kernel threads. |
| `stat` | Campos posicionales | **El campo 2 (`comm`) va entre paréntesis y PUEDE CONTENER ESPACIOS Y PARÉNTESIS.** Un proceso llamado `(evil) 1 2 3` rompe cualquier parser que haga `split_whitespace()`. **Regla obligatoria: buscar el último `)` de la línea y parsear posicionalmente a partir de ahí.** Esto ha sido un CVE en herramientas reales. |
| `cgroup` | Jerarquía de cgroups | Formato distinto en v1 y v2 (§7.6). |
| `ns/net` | symlink al netns | Comparar con el propio para detectar contenedores (§7.7). |
| `root` | symlink al rootfs | Permite leer ficheros dentro del contenedor sin `docker exec`. Requiere permisos. |
| `environ` | Variables de entorno | **Contiene secretos** (`DATABASE_URL`, `AWS_SECRET_KEY`). Ver §13.2. Requiere permisos de dueño. |

**Manejo de errores parciales — política del producto:**

> Un fallo al leer **cualquier** fichero de `/proc/<pid>/` es **normal, esperado y no es un error**. El proceso puede haber muerto entre el `readdir` y el `open` (`ENOENT`), o pertenecer a otro usuario (`EACCES`). Nunca aborta el escaneo, nunca imprime un warning en pantalla, nunca deja la fila fuera de la tabla. Se registra en `Snapshot.degradations` y el campo se muestra como `—` o `🔒` con un tooltip explicativo en el panel de detalle.

### 7.5 Métricas: CPU, memoria, uptime

**CPU.** No existe un «porcentaje de CPU» instantáneo en Linux. `/proc/<pid>/stat` da contadores monótonos en *jiffies*: `utime` (campo 14) y `stime` (campo 15). Se calcula:

```
cpu% = ( (utime₂+stime₂) − (utime₁+stime₁) ) / ( total_jiffies₂ − total_jiffies₁ ) × 100 × ncpu
```

donde `total_jiffies` sale de la primera línea de `/proc/stat`. Implicaciones de producto:

- **La primera muestra no puede mostrar CPU.** Durante el primer segundo, la columna muestra `—`, no `0.0%`. Mostrar `0.0%` sería mentir (principio 3).
- El divisor `HZ` se obtiene con `sysconf(_SC_CLK_TCK)` (típicamente 100), **nunca hardcodeado**.
- Con `×ncpu`, un proceso multihilo puede superar el 100%. Se muestra tal cual (como `htop`), con un ajuste de configuración `cpu_scale = "core" | "system"`.

**Memoria.** `VmRSS` de `status` es rápido y suficiente para el MVP. **Advertencia de exactitud:** RSS cuenta páginas compartidas en cada proceso que las mapea, así que ocho procesos de Node "consumen" ocho veces las mismas páginas de libc. `PSS` (de `/proc/<pid>/smaps_rollup`) es la métrica honesta, pero leerla cuesta ~10× más y requiere permisos. **MVP: `VmRSS`. v1.5: `PSS` opcional (`memory_metric = "pss"`), con un aviso de coste.**

**Uptime.** `stat:starttime` (campo 22) está en jiffies **desde el arranque del sistema**. Se convierte con `btime` de `/proc/stat` (epoch del boot):

```
started_at = btime + starttime / HZ
```

`starttime` es además el **desambiguador de reutilización de PID**: la clave de caché de un proceso es `(pid, starttime)`, jamás solo `pid`. Sin esto, un PID reciclado hereda la caché del proceso muerto y Portix muestra el proyecto equivocado. **Este es el bug silencioso más peligroso de toda la herramienta.**

### 7.6 Contenedores: cgroups v1 y v2

**cgroup v2** (Arch, y por defecto en todo lo moderno) — `/proc/<pid>/cgroup` tiene **una sola línea**:

```
0::/system.slice/docker-3f2a9c1b8e7d6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a.scope
```

**cgroup v1** — múltiples líneas, una por subsistema:

```
12:pids:/docker/3f2a9c1b8e7d…
11:memory:/docker/3f2a9c1b8e7d…
```

**Patrones de reconocimiento por runtime:**

| Runtime | Patrón | Extracción |
|---|---|---|
| Docker (v2, systemd) | `docker-<64hex>.scope` | 64 hex → mostrar 12 |
| Docker (v1) | `/docker/<64hex>` | ídem |
| Podman | `libpod-<64hex>.scope` | ídem |
| containerd / k8s | `cri-containerd-<64hex>.scope` | ídem |
| systemd-nspawn | `machine-<name>.scope` | nombre |
| Sin contenedor | `/user.slice/user-1000.slice/…` | `None` |

**Detección adicional (defensa en profundidad):** existencia de `/proc/<pid>/root/.dockerenv`, o `/proc/1/cgroup` distinto al del proceso. Se usan como confirmación, no como fuente primaria.

**MVP:** detectar que un servicio corre en contenedor, identificar el runtime y mostrar el ID corto. **v1.5:** consultar el socket de Docker (`/var/run/docker.sock`, `GET /containers/<id>/json`) para obtener nombre, imagen, proyecto de Compose y mapeo de puertos. Este es el único punto donde Portix hablará con un daemon externo, y será **estrictamente opcional y degradable**: si el socket no existe o no hay permisos, la funcionalidad se apaga en silencio y el ID corto sigue mostrándose.

### 7.7 Network namespaces: el problema que rompe Docker

**El hecho fundamental:** `/proc/net/tcp` refleja el network namespace **del proceso que lo lee**. Un contenedor con su propio netns (todos, salvo `--network=host`) tiene sus sockets en **otra tabla**, invisible desde el host.

**Lo que sí ve el host:** los **puertos publicados** (`-p 5432:5432`). Los publica `docker-proxy` (un proceso del host que escucha en `0.0.0.0:5432`) o, con `userland-proxy=false`, reglas de `iptables`/`nftables` DNAT — que **no crean ningún socket en el host**.

**Consecuencias, ordenadas por lo doloroso que es descubrirlas tarde:**

1. Con `docker-proxy` activo, Portix ve un proceso `docker-proxy` escuchando en el 5432. Correcto pero inútil: el usuario quiere ver `postgres`, no `docker-proxy`.
2. Sin `docker-proxy` (default de Podman, y de Docker con `userland-proxy=false`), **el puerto 5432 no aparece en absoluto** en `/proc/net/tcp` del host. Portix, ingenuamente, mostraría que nadie escucha en 5432, mientras `psql` conecta perfectamente. **Esto es peor que no mostrar nada: es mostrar una mentira.**

**Estrategia por fases:**

| Fase | Enfoque | Alcance |
|---|---|---|
| **MVP** | Escanear `/proc/net/*` del host. Detectar `docker-proxy` por `cmdline` y **reescribir la fila** para atribuirla al contenedor destino (el `cmdline` de `docker-proxy` contiene `-container-ip` y `-container-port`). Detectar `nftables`/`iptables` DNAT no; en su lugar, emitir la degradación `ContainerPortsHidden` **solo cuando hay un runtime activo y no hay ningún `docker-proxy` visible** (proxy de userland habilitado ⇒ los puertos publicados sí aparecen en `/proc/net` ⇒ no hay nada oculto). La atribución de runtime usa el parser estructurado de `platform/linux/cgroup.rs` (nunca subcadenas) con matching por hoja: cubre `system.slice/` (rootful systemd), `machine.slice/` (podman v2 y systemd-nspawn), `user.slice/…` (docker/podman rootless) y `kubepods.slice/` (K8s); un host que solo ejecuta `dockerd` (su unidad `docker.service`) no dispara el aviso. **Nunca fingir que no hay nada.** El engine recomputa el aviso como mucho cada 10 s (`Engine::container_warning_cached`), no por refresh. |
| **v1** | Enumerar netns únicos vía `/proc/<pid>/ns/net` (comparando el inode del symlink). Para cada netns distinto del nuestro, leer `/proc/<pid_representante>/net/tcp` — **este fichero sí existe y contiene la tabla del netns de ese proceso, y es legible sin `setns()`**. Unir con el mapa global de inodes. Esto resuelve el 95% de los casos de Docker sin privilegios especiales (siempre que el proceso del contenedor sea del mismo usuario o tengamos root). |
| **v1.5** | Consultar la API de Docker para nombres, imágenes y mapeo de puertos declarado. |
| **v2** | `setns()` sobre netns ajenos (requiere `CAP_SYS_ADMIN`); leer reglas de `nftables` para resolver DNAT. Solo si la evidencia de usuarios lo justifica. |

> **Nota arquitectónica clave:** el descubrimiento de la fase v1 —leer `/proc/<pid>/net/tcp` en lugar de `/proc/net/tcp`— es elegante y poco conocido, y es la razón por la que Portix puede ver dentro de contenedores sin ser root ni ejecutar `nsenter`. Merece un artículo de blog en el lanzamiento.

### 7.8 Permisos y degradación

Portix se ejecuta **sin privilegios** por defecto. Matriz de lo que se pierde:

| Dato | Usuario normal, proceso propio | Usuario normal, proceso ajeno | Root |
|---|:---:|:---:|:---:|
| Puerto, protocolo, estado, inode | ✅ | ✅ | ✅ |
| UID del socket | ✅ | ✅ | ✅ |
| PID | ✅ | ❌ `EACCES` en `fd/` | ✅ |
| `cmdline`, `stat`, `status` | ✅ | ✅ (son world-readable) | ✅ |
| `cwd`, `exe`, `root`, `environ` | ✅ | ❌ `EACCES` | ✅ |
| **Proyecto y framework** | ✅ | ❌ (dependen de `cwd`) | ✅ |

**Lectura:** para el **caso de uso real** —los servicios de desarrollo son procesos del propio usuario— la funcionalidad es **del 100% sin `sudo`**. La degradación afecta a servicios del sistema (Nginx como `www-data`, Postgres como `postgres`), donde el proyecto es irrelevante de todos modos.

**Política de producto:**

- **Nunca** ejecutar `sudo` automáticamente. **Nunca** instalar setuid. **Nunca** pedir capabilities en el `PKGBUILD` (§13.3).
- Las filas degradadas se muestran con un icono `🔒` y la barra de estado indica: `3 sockets sin resolver (ejecuta con sudo para verlos)`.
- Un flag `--sudo-hint` (default on) muestra esa pista una sola vez por sesión.

> **Anti-patrón rechazado:** hacer que Portix se re-ejecute con `pkexec`/`sudo` cuando detecta filas degradadas. Es hostil, sorprendente y una superficie de escalada de privilegios. El usuario decide.

### 7.9 Procesos efímeros

Un proceso de desarrollo puede vivir 800 ms (un `vite build`, un script de test que levanta un servidor). Entre `readdir("/proc")` y `open("/proc/<pid>/cmdline")` el proceso puede desaparecer.

**Reglas:**

1. `ENOENT` durante el enriquecimiento **descarta el servicio silenciosamente**, no genera error.
2. **Anti-parpadeo:** un servicio que desaparece del snapshot N+1 pero estaba en el N se marca `Dying` y se muestra atenuado durante 2 s antes de desaparecer. Sin esto, una tabla de 20 filas con procesos que entran y salen es ilegible. (Análogo al *tombstone* de `lazydocker`.)
3. **Estabilidad de la selección:** la fila seleccionada se identifica por `ServiceId`, no por índice. Si desaparece la fila 3, la selección no salta a otro servicio; se mantiene en el más cercano y se anota visualmente.
4. **Estabilidad del orden:** el orden por defecto (puerto ascendente) es determinista. Nunca se reordena bajo el cursor por un cambio de CPU. La ordenación por CPU es opt-in y se advierte que la tabla se moverá.

### 7.10 Alternativa futura: `NETLINK_INET_DIAG`

`/proc/net/tcp` es texto y su generación en el kernel es **O(n²)** en el número de sockets (el famoso problema que hace lento a `netstat` en servidores con 100k conexiones). Para un escritorio con 50 sockets es irrelevante.

La alternativa correcta —la que usa `ss`— es el socket `NETLINK_INET_DIAG` con `sock_diag(7)`: un protocolo binario que devuelve estructuras, permite filtrar en el kernel (solo LISTEN) y, crucialmente, **devuelve el inode del socket y el `cgroup id` directamente**.

**Decisión:** **no en el MVP.** `/proc/net/*` es trivial de testear con fixtures, funciona en cualquier kernel y es suficiente para el presupuesto de rendimiento (§15). Netlink añade complejidad binaria y `unsafe`. Se implementa en **v1.5 detrás del trait `PlatformProbe`**, con selección automática y fallback a `/proc` si el socket netlink falla. La abstracción de §5.5 hace que este cambio no toque una sola línea fuera de `platform/linux/`.

---

## 8. Detección de proyecto

### 8.1 El algoritmo de ascenso

Entrada: `cwd` del proceso. Salida: `Option<Project>`.

```
1. Normalizar cwd. Si es None (sin permisos) o "/" → devolver None. Fin.
2. Desde cwd, ascender directorio a directorio hasta $HOME (exclusive) o "/",
   con un límite duro de 24 niveles.
3. En cada nivel, buscar MARCADORES DE PAQUETE (tabla §8.2).
   El primero encontrado define `project.root`.  ← el "workspace member"
4. Continuar ascendiendo buscando MARCADORES DE WORKSPACE (tabla §8.4).
   Si se encuentra, define `project.workspace_root`.
5. Si no hay ningún marcador de paquete pero sí un `.git`, usar ese directorio
   como `root` con confianza Baja.
6. Derivar name, language y package_manager de los marcadores encontrados.
```

**Decisiones de diseño y su porqué:**

- **Se para en el primer marcador de paquete, no en el último.** En `~/work/acme/apps/web` con `package.json` en `apps/web` y en la raíz, el proyecto que le importa al desarrollador es `apps/web` (JS-6). La raíz es contexto, no identidad.
- **Se detiene en `$HOME`.** Nadie tiene un `package.json` en `~`… salvo que sí (dotfiles). El límite evita ascender hasta `/` y encontrar basura.
- **Límite de 24 niveles.** Defensa contra `cwd` patológicos y symlinks circulares.
- **`cwd` puede no ser el proyecto.** `pnpm dev` desde la raíz del monorepo deja `cwd` en la raíz, aunque el servidor sirva `apps/web`. Mitigación en §8.5.

### 8.2 Marcadores de paquete

Ordenados por precedencia cuando coexisten en el mismo directorio (un `Cargo.toml` junto a un `package.json` en un proyecto Tauri: gana el que coincida con el lenguaje del proceso).

| Marcador | Lenguaje | Nombre del proyecto extraído de |
|---|---|---|
| `package.json` | Node/TS | `.name` |
| `deno.json` / `deno.jsonc` | Deno | `.name` |
| `bunfig.toml` | Bun | dir |
| `pyproject.toml` | Python | `[project].name` o `[tool.poetry].name` |
| `setup.py` / `setup.cfg` | Python | dir |
| `requirements.txt` | Python | dir (confianza baja) |
| `Pipfile` | Python | dir |
| `go.mod` | Go | `module` (última ruta) |
| `Cargo.toml` | Rust | `[package].name` |
| `composer.json` | PHP | `.name` |
| `Gemfile` | Ruby | dir |
| `pom.xml` / `build.gradle(.kts)` | JVM | `<artifactId>` / dir |
| `mix.exs` | Elixir | dir |
| `pubspec.yaml` | Dart | `name` |
| `docker-compose.y(a)ml` | — | dir (marca proyecto de infra) |
| `.git` | — | fallback, confianza baja |

**Resolución del lenguaje del proceso → marcador:** si el `exe` del proceso es `node`/`bun`/`deno`, se prefiere `package.json` aunque haya un `Cargo.toml` en el mismo directorio. Esto resuelve correctamente Tauri, PyO3, napi-rs, y los proyectos poliglota en general. **La identidad del proceso desempata la identidad del proyecto.**

### 8.3 Nombre mostrado

El nombre técnico (`@acme/web`) no siempre es el mejor para la UI. Cascada:

1. `.name` del manifiesto, si existe y no es genérico (`app`, `server`, `main`, `test`, `""`).
2. `workspace_root.basename` + `/` + `root.basename` si hay workspace → `acme/web`.
3. `root.basename` → `acme-storefront`.
4. Nombre del repositorio git (`git remote get-url origin`, parseado) → v1, es I/O extra.
5. `—` si nada.

Se muestra `display_name` en la tabla y el nombre técnico en el panel de detalle.

### 8.4 Monorepos y workspaces

Este es un diferenciador competitivo real: nadie más lo hace, y es donde más duele el problema (JS-6).

| Ecosistema | Marcador de workspace | Cómo se enumeran los miembros |
|---|---|---|
| pnpm | `pnpm-workspace.yaml` | `packages:` globs |
| npm / yarn / bun | `package.json` con `"workspaces"` | globs |
| Turborepo | `turbo.json` | delega en el gestor de paquetes |
| Nx | `nx.json` | `project.json` por proyecto |
| Lerna | `lerna.json` | `packages` |
| Rust | `Cargo.toml` con `[workspace]` | `members` |
| Go | `go.work` | `use` |
| Python | `pyproject.toml` con `[tool.uv.workspace]` / `[tool.rye]` | `members` |
| PHP | `composer.json` con `repositories: path` | paths |

**MVP:** detectar la existencia del workspace y su raíz. Mostrar `acme/web` en vez de `web`. Agrupar visualmente por workspace (`g` = agrupar).
**v1:** enumerar los miembros del workspace para poder ofrecer «matar todos los servicios de este workspace» (JS-9).

### 8.5 Casos degenerados

| Caso | Comportamiento |
|---|---|
| `cwd` inaccesible (`EACCES`) | `project = None`, fila con `🔒`, se sugiere `sudo` |
| `cwd == /` | `project = None`. Típico de servicios systemd. Correcto. |
| `cwd` borrado (` (deleted)`) | `project = None`, aviso en detalle: «el directorio ya no existe» |
| `cwd` en un contenedor (`/app`) | Se intenta `/proc/<pid>/root/app`. Si funciona, se lee el manifiesto desde ahí y se marca la ruta como `container:/app`. Si no, `project = None` y se muestra el ID del contenedor. |
| **`cwd` = raíz del monorepo pero el servidor sirve un subpaquete** | El caso más difícil. Heurística v1: buscar en `cmdline` una ruta relativa (`--filter=web`, `apps/web`, `--dir apps/admin`, `-C apps/api`) y, si existe y contiene un marcador, usarla como `root`. Confianza Media. Si falla, se muestra la raíz del monorepo (que es correcto, solo menos específico). |
| Symlink en el `cwd` | Se resuelve el enlace (`canonicalize`) pero **se muestra la ruta original**. Los desarrolladores reconocen sus symlinks. |
| Proyecto en `/tmp` o `node_modules` | Se detecta normalmente, pero un `root` que contenga `/node_modules/` se descarta y se continúa el ascenso: es un artefacto, no un proyecto. |

---

## 9. Detección de framework

### 9.1 Requisitos del sistema

El PRD exige un sistema **desacoplado y extensible**: añadir soporte para un framework nuevo no debe modificar el núcleo. Además:

- Un framework puede detectarse por **varias señales independientes**, ninguna concluyente por sí sola.
- Las señales tienen **coste muy distinto** (parsear `cmdline` es gratis; leer y deserializar `package.json` cuesta un `open` + `read` + `serde`).
- Los frameworks **se solapan**: Next.js *es* React; NestJS *usa* Express; Uvicorn *sirve* FastAPI. La respuesta correcta no siempre es una sola.
- La confianza debe ser **visible** al usuario (principio 3).

### 9.2 Modelo de señales

Un `Detector` es una función pura `(&ProcessContext, &ProjectContext) → Vec<Match>`, donde cada `Match` es `(framework_id, weight, evidence)`.

**Catálogo de señales, con coste y peso típico:**

| Clase | Ejemplo | Coste | Peso típico | Notas |
|---|---|---|---|---|
| `exe_basename` | `exe` = `/usr/bin/node` | Gratis | 0.3 | Establece el **lenguaje**, no el framework |
| `cmdline_binary` | `argv[0]` termina en `uvicorn` | Gratis | 0.8 | Muy fiable |
| `cmdline_arg` | `argv` contiene `next` | Gratis | 0.7 | Cuidado con falsos positivos (`--next-build`) |
| `cmdline_script_path` | `argv[1]` = `…/node_modules/.bin/vite` | Gratis | 0.85 | **La señal más fiable de Node**; el `.bin` no miente |
| `cmdline_module` | `python -m uvicorn` | Gratis | 0.9 | |
| `env_var` | `NEXT_RUNTIME` presente | Barato* | 0.9 | *Requiere permisos; ver §13.2 |
| `manifest_dep` | `dependencies.next = "^15"` | Medio | 0.9 | Da también la **versión** |
| `project_file` | `next.config.ts` existe | Barato | 0.7 | Un `statx`, sin leer contenido |
| `lockfile` | `pnpm-lock.yaml` | Barato | — | Determina el `package_manager`, no el framework |
| `port_convention` | puerto == 5173 | Gratis | **0.1** | **Solo como desempate.** Nunca decide por sí solo. |
| `container_image` | imagen = `postgres:16` | Caro (API) | 0.95 | v1.5 |

**Regla de oro:** el puerto convencional **jamás** determina el framework por sí solo. Un peso de 0.1 no supera ningún umbral. Su único trabajo es romper un empate entre dos candidatos que ya tienen evidencia real. Violar esto produce la clase de herramienta que dice «puerto 3000 → es Next.js» cuando en realidad es un Rails que alguien lanzó ahí, y destruye la confianza del usuario para siempre.

### 9.3 Puntuación y confianza

```
score(fw) = 1 − Π (1 − wᵢ)      para cada señal i que apunta a fw
```

(Combinación probabilística: dos señales de 0.7 dan 0.91, no 1.4. Es monótona, está acotada en [0,1) y recompensa la evidencia independiente sin saturar.)

| Score | Confianza | Presentación en la UI |
|---|---|---|
| ≥ 0.90 | **High** | `Next.js 15.1.0` |
| 0.60 – 0.89 | **Medium** | `Next.js ~` |
| 0.30 – 0.59 | **Low** | `Next.js ?` (atenuado) |
| < 0.30 | descartado | `—` |

**Relaciones entre frameworks.** Un grafo declarativo `implies` / `supersedes` resuelve los solapamientos:

```
nextjs    supersedes react
nestjs    supersedes express
fastapi   implies    uvicorn      # se muestra "FastAPI (uvicorn)"
gunicorn  implies    wsgi
astro     supersedes vite
```

- `supersedes`: si ambos superan el umbral, se muestra solo el que supersede.
- `implies`: se muestra el principal, con el servidor como sufijo. `Django (gunicorn)` es exactamente lo que el usuario quiere leer.

**Empates y ausencia.** Si dos frameworks empatan por encima de 0.6 sin relación declarada, se muestran ambos: `Vite · Vue`. Si ninguno supera 0.3, se muestra el **lenguaje** (`Node`, `Python`) que casi siempre es deducible del `exe`. Mostrar el lenguaje es mucho mejor que mostrar `—`.

### 9.4 Catálogo de detectores (MVP)

**Node.js** — lenguaje por `exe ∈ {node, bun, deno}` o `argv[0]`.

| Framework | Señales primarias | Puerto conv. |
|---|---|---|
| Next.js | `.bin/next`, `dep:next`, `next.config.{js,ts,mjs}`, `.next/` | 3000 |
| Vite | `.bin/vite`, `dep:vite`, `vite.config.*` | 5173 |
| Astro | `.bin/astro`, `dep:astro`, `astro.config.*` | 4321 |
| Nuxt | `.bin/nuxt`, `dep:nuxt`, `nuxt.config.*` | 3000 |
| Remix / React Router | `.bin/remix`, `dep:@remix-run/*` | 3000 |
| NestJS | `.bin/nest`, `dep:@nestjs/core`, `nest-cli.json` | 3000 |
| Express | `dep:express` (sin superseder) | 3000 |
| React (CRA/otros) | `dep:react` + `dep:react-scripts` | 3000 |
| Bun | `exe:bun` | — |

**Python** — lenguaje por `exe` que empiece por `python`, o venv en `argv[0]`.

| Framework | Señales primarias | Puerto conv. |
|---|---|---|
| FastAPI | `dep:fastapi`, `-m uvicorn` + `app:app`, `main.py` con `FastAPI(` (v1) | 8000 |
| Django | `manage.py` en el proyecto, `argv` contiene `runserver`, `dep:django`, `settings.py` | 8000 |
| Flask | `dep:flask`, `FLASK_APP` en env, `argv[0]:flask` | 5000 |
| Uvicorn | `argv[0]:uvicorn`, `-m uvicorn` | 8000 |
| Gunicorn | `argv[0]:gunicorn` (ojo: reescribe argv, §7.4) | 8000 |

**Go** — `exe` es un binario compilado, así que las señales de `cmdline` no sirven. Se depende del proyecto:

| Framework | Señales |
|---|---|
| Gin | `go.mod` contiene `github.com/gin-gonic/gin` |
| Echo | `go.mod` contiene `github.com/labstack/echo` |
| Fiber | `go.mod` contiene `github.com/gofiber/fiber` |

> **Nota de diseño honesta:** en Go y Rust, el binario compilado no revela su framework. La detección **depende enteramente de encontrar el proyecto** (`go.mod` / `Cargo.toml` vía `cwd`). Si el binario se ejecuta desde otro directorio, no habrá framework. Portix mostrará `Go` a secas, lo cual es correcto y honesto. Esto se documenta como limitación conocida en vez de inventar heurísticas frágiles (p.ej. buscar símbolos en el binario — rechazado por coste y fragilidad).

**Rust:** `Cargo.toml` con `dependencies.axum` / `actix-web` / `rocket` / `warp`.
**PHP:** `artisan` (Laravel), `bin/console` + `symfony.lock` (Symfony), `composer.json` deps.
**Bases de datos y servicios comunes** (detección por `exe`, sin proyecto): `postgres`, `mysqld`, `redis-server`, `mongod`, `nginx`, `traefik`, `caddy`, `minio`, `mailhog`. Son ruido si no se identifican, y contexto valioso si sí.

> **Relaciones entre detectores (implementadas en `detect/registry.rs`):** `Supersedes` colapsa el par cuando ambos puntúan (Next.js supersede React, NestJS supersede Express, Astro supersede Vite) — con `dep:react` en el catálogo, la relación Next.js→React es real: en un proyecto Next.js ambos puntúan y gana Next.js; en un SPA React plano el ganador es React. `Implies` se renderiza como sufijo del nombre mostrado cuando el primario se detecta: `FastAPI · Uvicorn`, `Gunicorn · WSGI`.

### 9.5 Extensibilidad: la decisión que define el proyecto

Tres opciones evaluadas:

| Opción | Añadir un framework requiere | Poder expresivo | Riesgo |
|---|---|---|---|
| **A. Solo detectores compilados** (trait `Detector` en Rust) | PR + revisión + release | Total | Barrera alta al contribuidor; cada framework nuevo es una release |
| **B. Solo detectores declarativos** (TOML) | Editar un `.toml` | Limitado a las señales predefinidas | Cubre ~90% de casos; el 10% restante es imposible |
| **C. Híbrido** | TOML para lo común, Rust para lo raro | Total | Dos caminos que mantener |

**Decisión: C (híbrido), con B como camino por defecto y de primera clase.**

Un detector declarativo, empaquetado con el binario en `detectors/*.toml` (embebido con `include_str!`) y **sobrescribible por el usuario** en `~/.config/portix/detectors/`:

```toml
[detector.nextjs]
display_name = "Next.js"
language     = "node"
supersedes   = ["react"]

[[detector.nextjs.signals]]
kind = "cmdline_script_path"
pattern = "node_modules/.bin/next$"
weight = 0.85

[[detector.nextjs.signals]]
kind = "manifest_dep"
manifest = "package.json"
path = "dependencies.next"
version_from = true          # extrae la versión de aquí
weight = 0.90

[[detector.nextjs.signals]]
kind = "project_file"
any_of = ["next.config.js", "next.config.ts", "next.config.mjs"]
weight = 0.70

[[detector.nextjs.signals]]
kind = "port_convention"
ports = [3000]
weight = 0.10
```

**Por qué esto importa para el producto, no solo para la ingeniería:**

1. **Elimina la barrera del borrow checker** para la contribución más frecuente. Un usuario que quiere soporte para SvelteKit no necesita saber Rust. Manda un TOML de 20 líneas. Esto es lo que convierte un proyecto de una persona en un proyecto de una comunidad.
2. **Los usuarios pueden añadir sus frameworks internos** sin forkear. Una empresa con un framework propietario suelta un TOML en `~/.config/portix/detectors/`.
3. **Los detectores se testean con fixtures declarativas**, no con tests de Rust.
4. **El núcleo permanece estable.** Añadir Svelte no toca `core/`, no toca `ui/`, no toca `platform/`.

Los detectores compilados quedan reservados para lógica que el TOML no puede expresar: reescritura de `docker-proxy`, resolución de Django vía `manage.py` + `DJANGO_SETTINGS_MODULE`, o el parseo de `go.mod`. Se estima que serán **menos de 8** en total.

**Contrato de estabilidad:** el esquema de los detectores TOML se versiona (`schema = 1`). Es API pública desde v1.0 y sigue semver.

**v2 — plugins WASM.** Para lógica arbitraria sin recompilar (`wasmtime` + una interfaz WIT que exponga `read_file(path)` restringido al `project.root`, en sandbox). Potente y seguro, pero es peso y complejidad que solo se justifica si la comunidad topa con los límites del TOML. **No antes de que haya evidencia.**

---

## 10. Caché, refresco y presupuesto de rendimiento

### 10.1 El problema

Los datos de Portix tienen **volatilidades radicalmente distintas**:

| Dato | Cambia cada | Coste de obtenerlo |
|---|---|---|
| CPU %, RSS | 1 s | Barato (2 ficheros por PID) |
| Lista de sockets | segundos | Barato (4 ficheros) |
| Mapa inode → PID | segundos | **Caro** (miles de `readlink`) |
| `cmdline`, `cwd`, `exe` | **nunca** (mientras el proceso vive) | Barato, pero innecesario repetirlo |
| Raíz del proyecto | **nunca** (para un `cwd` dado) | Medio (varios `statx` ascendiendo) |
| Framework y versión | casi nunca (cambia con `pnpm install`) | **Caro** (leer + deserializar `package.json`) |

Refrescarlo todo cada segundo es un desperdicio de dos órdenes de magnitud. La caché no es una optimización: **es la arquitectura**.

### 10.2 Diseño de la caché

| Nivel | Contenido | Clave | Invalidación | Tamaño |
|---|---|---|---|---|
| **L1 · Process** | `cmdline`, `exe`, `cwd`, `user`, `started_at` | `(pid, starttime)` | El proceso desaparece | ≤ 500 entradas |
| **L2 · Project** | `Project` completo | `inode(cwd)` — no la ruta | TTL 60 s + `mtime` del manifiesto | ≤ 200 |
| **L3 · Framework** | `Framework` completo | `(inode(project.root), mtime(manifest))` | `mtime` del manifiesto cambia | ≤ 200 |
| **L4 · Users** | `uid → username` | `uid` | TTL 300 s | ≤ 50 |
| **L5 · Container** | `id → Container` | `container_id` | TTL 30 s | ≤ 100 |

**Detalles que evitan bugs sutiles:**

- **`(pid, starttime)` como clave de L1, jamás `pid` solo.** Los PIDs se reciclan (`/proc/sys/kernel/pid_max` es 4194304 por defecto en Arch, pero un sistema con muchos procesos efímeros lo agota en horas). Un PID reciclado con caché heredada muestra el proyecto de un proceso muerto. **Este es el bug más difícil de reproducir y el más embarazoso de todos.** El test `pid_reuse_does_not_leak_cache` es obligatorio.
- **`inode(cwd)` como clave de L2, no la ruta.** Un `git mv` o un rename del directorio conserva el inode; la ruta cambia. Cachear por ruta produce fallos de caché innecesarios y, peor, entradas obsoletas si otra cosa toma el nombre viejo.
- **L3 depende de `mtime(manifest)`.** Tras `pnpm add next@16`, `package.json` cambia de `mtime`, la entrada se invalida y la versión mostrada se actualiza en el siguiente refresco. Sin polling, sin inotify, sin coste.
- **Negative caching.** Si `cwd` es `/` o no hay proyecto, se cachea el `None` con TTL de 30 s. Sin esto, cada refresco reintenta el ascenso de directorios completo para `nginx`, `sshd` y `systemd-resolved`. Es un tercio del coste del escaneo.

**Estado de implementación (MVP):** L1 no cachea metadata de proceso — decisión deliberada, documentada en `core/engine.rs::resolve_process`: `cmdline`/`exe`/`cwd` cambian con `exec`/`chdir` mientras `(pid, starttime)` permanece estable, así que una copia cacheada se congelaría con valores obsoletos; lo que sí se cachea con clave `(pid, starttime)` son las **muestras de CPU** (`CacheStore::cpu_samples`). L2 y L3 están implementados tal como se describen (L3 añade un `disc` = hash de `(port, exe, cmdline)` a la clave, para que dos procesos que comparten raíz pero corren comandos distintos no colisionen). **L4 (uid→username) está cableada**: `platform/linux/proc.rs` expone el uid y deja `user` sin resolver; `Engine::resolve_process` resuelve el nombre a través de `CacheStore::get_user`/`insert_user` (TTL 300 s), de modo que el `getpwuid_r` ocurre una vez por uid y no por proceso y por scan. L5 (contenedor) no está implementada; por ahora la membresía se memoiza por PID dentro de un mismo scan (`seen_container` en `engine.rs`).

### 10.3 Estrategia de refresco por niveles

```
t=0ms    ┌─ Tier 0: sockets + mapa inode→pid ─┐  cada 1000 ms
         │  (~12 ms)                          │
         └────────────────────────────────────┘
t=0ms    ┌─ Tier 1: métricas (CPU, RSS)       ┐  cada 1000 ms, junto a Tier 0
         │  solo para PIDs con socket (~3 ms) │
         └────────────────────────────────────┘
t=+12ms  ┌─ Tier 2: metadata de proceso       ┐  solo para PIDs nuevos
         │  (cmdline, cwd, exe)   (~0.1 ms/pid)│
         └────────────────────────────────────┘
t=+15ms  ┌─ Tier 3: proyecto + framework      ┐  asíncrono, en rayon pool
         │  solo para (pid, cwd) nuevos       │  el resultado llega vía
         │  (~2 ms/proyecto)                  │  Event::Enriched
         └────────────────────────────────────┘
```

**Consecuencia de UX:** un servicio recién detectado aparece **inmediatamente** en la tabla con puerto, PID y comando, y su columna de proyecto muestra un spinner sutil durante ~5 ms antes de rellenarse. **Nunca se retrasa el frame esperando al enriquecimiento.** Esta es la diferencia entre «se siente instantáneo» y «se siente pesado».

> **Estado MVP:** el Tier 3 asíncrono **no está implementado**. `Engine::scan()` resuelve sockets + métricas + metadata + proyecto + framework de forma **síncrona y secuencial** en el mismo recorrido, así que el primer frame paga el coste de enriquecimiento completo antes de dibujarse. El diseño de este apartado (rayon pool + `Event::Enriched`) es la referencia para v1; de momento el presupuesto se sostiene gracias a las cachés L2/L3 y a la memoización por scan (`seen`/`seen_context` en `engine.rs`). La pausa `FocusLost` × 4 también está pendiente.

**Adaptación:**

- Terminal sin foco (`FocusLost` de crossterm) → intervalo × 4.
- Modo búsqueda activo → se pausa el reordenamiento, no el refresco.
- Flag `--interval` y config `refresh_ms`. Mínimo 200 ms (por debajo, el delta de CPU pierde toda resolución con `HZ=100`: menos de 20 jiffies de ventana).
- `--once` / `--json` → un solo escaneo, sin TUI.

### 10.4 Presupuesto de syscalls por refresco (500 procesos, 40 sockets, estado caliente)

| Operación | Llamadas | Notas |
|---|---|---|
| `open`+`read` de `/proc/net/*` | 4 | |
| `read` de `/proc/stat` | 1 | Para el total de jiffies |
| `statx` de `/proc/<pid>` (poda por uid) | 500 | Baratísimo |
| `openat`+`getdents` de `/proc/<pid>/fd` | ~150 | Solo procesos del usuario |
| `readlinkat` de fds | ~12.000 | El coste dominante |
| `read` de `/proc/<pid>/stat` + `status` | 80 | Solo PIDs con socket |
| L2/L3 (cachés calientes) | **0** | |
| **Total** | **≈ 12.700** | **Objetivo: < 15 ms** |

En estado frío (primer escaneo) se añaden ~40 ascensos de directorio y ~25 lecturas de manifiesto: **objetivo < 60 ms hasta la tabla completamente enriquecida**.

---

## 11. Interfaz de usuario

### 11.1 Principios de diseño visual

1. **La densidad de información gana a la decoración.** Cada carácter en pantalla se gana su sitio.
2. **El color codifica significado, nunca adorna.** Verde = escuchando y sano. Amarillo = confianza media / advertencia. Rojo = destructivo o zombi. Azul = contenedor. Gris = degradado / desconocido.
3. **Cero parpadeo.** El diff de buffers de `ratatui` + el orden estable (§7.9) garantizan que la tabla no «respire».
4. **La ayuda está siempre a una tecla.** `?` en cualquier momento.
5. **Degradación honesta.** En una terminal de 16 colores, en 80 columnas, o sin Unicode, Portix debe seguir siendo **usable y bonito**, no roto.

### 11.2 Wireframe: pantalla principal

```
┌─ Portix ──────────────────────────────────────────────────────────── v1.0.0 ─┐
│ 12 servicios · 4 proyectos · 2 contenedores            ⟳ 0.9s   CPU 0.3%  ▲   │
├──────────────────────────────────────────────────────────────────────────────┤
│ PORT   PROTO  PROJECT             FRAMEWORK        PID     CPU    MEM   UPTIME│
├──────────────────────────────────────────────────────────────────────────────┤
│  3000  tcp6   acme/web            ◆ Next.js 15.1   48213   1.2%  312M   2h14m │
│▸ 3001  tcp6   acme/admin          ◆ Next.js 15.1   48287   0.4%  289M   2h14m │
│  4000  tcp    acme/api            ◆ NestJS 10.4    48350   0.9%  198M   2h13m │
│  5173  tcp    acme/ui             ◆ Vite 6.0       48402   0.1%   94M   1h02m │
│  5432  tcp    🐳 acme-postgres-1  ◆ PostgreSQL 16  49001   0.2%   48M   2h20m │
│  6379  tcp    🐳 acme-redis-1     ◆ Redis 7.4      49055   0.0%   12M   2h20m │
│  8000  tcp    ml-scoring          ◆ FastAPI ~      51200   3.4%  512M   0h44m │
│  8080  tcp    🐳 —                ◆ Traefik 3.2    49102   0.1%   38M   2h20m │
│  7777  tcp    —                   ◆ Node ?         52001   0.0%   28M   0h03m │
│  9000  tcp    🔒 —                  —              —       —       —      —   │
│ 11434  tcp    —                   ◆ Ollama         44120   0.0%  1.2G   6h10m │
│ 54321  udp    —                   ◆ avahi-daemon    1204   0.0%    4M   6h11m │
├──────────────────────────────────────────────────────────────────────────────┤
│ ⚠ 1 socket sin resolver — ejecuta con sudo para verlo                         │
├──────────────────────────────────────────────────────────────────────────────┤
│ ↑↓ nav · ⏎ detalles · / buscar · f filtrar · s ordenar · k matar · o abrir · ?│
└──────────────────────────────────────────────────────────────────────────────┘
```

**Notas de diseño:**

- `▸` marca la fila seleccionada; la fila completa lleva fondo sutil.
- `◆` colorea según la confianza: sólido (High), hueco `◇` (Medium), atenuado (Low). El sufijo `~` o `?` refuerza sin depender del color (**accesibilidad daltónica**).
- `🐳` = contenedor. `🔒` = sin permisos.
- La columna `PROJECT` es la más ancha porque **es el producto**. Se trunca por el centro (`acme/…/storefront`), nunca por el final.
- La cabecera muestra el resumen que responde a JS-4 de un vistazo.
- `⟳ 0.9s` es la edad del snapshot; parpadea suave durante el escaneo. `▲` indica que hubo cambios desde el último frame.

### 11.3 Wireframe: panel de detalles

Se abre con `Enter` o `l`. Ocupa el 45% derecho, o toda la pantalla si el ancho < 120 columnas.

```
┌─ Portix ──────────────────────────────────────────────────────────── v1.0.0 ─┐
│ 12 servicios · 4 proyectos · 2 contenedores            ⟳ 0.4s   CPU 0.3%      │
├──────────────────────────────────┬───────────────────────────────────────────┤
│ PORT   PROJECT        FRAMEWORK  │ ╭─ :3000 ─────────────────────────────╮   │
│  3000  acme/web       Next.js    │ │                                     │   │
│▸ 3001  acme/admin     Next.js    │ │  ◆ Next.js 15.1.0        High conf. │   │
│  4000  acme/api       NestJS     │ │  Node 22.11.0 · pnpm 9.12           │   │
│  5173  acme/ui        Vite       │ │                                     │   │
│  5432  🐳 acme-pg-1   PostgreSQL │ ├─ PROYECTO ─────────────────────────┤   │
│  6379  🐳 acme-rd-1   Redis      │ │  Nombre     @acme/admin             │   │
│  8000  ml-scoring     FastAPI    │ │  Ruta       ~/work/acme/apps/admin  │   │
│  8080  🐳 —           Traefik    │ │  Workspace  ~/work/acme  (pnpm)     │   │
│  7777  —              Node ?     │ │  Rama       feat/rbac ●3            │   │
│  9000  🔒 —           —          │ │                                     │   │
│ 11434  —              Ollama     │ ├─ PROCESO ──────────────────────────┤   │
│ 54321  —              avahi      │ │  PID        48287   (PPID 48210)    │   │
│                                  │ │  Usuario    sergi                   │   │
│                                  │ │  Estado     Sleeping · 23 hilos     │   │
│                                  │ │  Iniciado   2026-07-09 08:14:22     │   │
│                                  │ │  Uptime     2h 14m 08s              │   │
│                                  │ │  Ejecutable /usr/bin/node           │   │
│                                  │ │  CWD        ~/work/acme/apps/admin  │   │
│                                  │ │                                     │   │
│                                  │ │  Comando                            │   │
│                                  │ │  ┌─────────────────────────────────┐│   │
│                                  │ │  │ node                            ││   │
│                                  │ │  │   ../../node_modules/.bin/next  ││   │
│                                  │ │  │   dev --port 3001               ││   │
│                                  │ │  └─────────────────────────────────┘│   │
│                                  │ │                                     │   │
│                                  │ ├─ SOCKET ───────────────────────────┤   │
│                                  │ │  Escucha    [::]:3001  (tcp6)       │   │
│                                  │ │  Alcance    ⚠ todas las interfaces  │   │
│                                  │ │  Inode      486401                  │   │
│                                  │ │  URL        http://localhost:3001   │   │
│                                  │ │                                     │   │
│                                  │ ├─ RECURSOS ─────────────────────────┤   │
│                                  │ │  CPU   0.4%  ▁▂▁▁▃▂▁▁▂▁▁▁▂▁▁       │   │
│                                  │ │  RSS   289 MB ▃▃▃▄▄▄▄▄▄▅▅▅▅▅▅       │   │
│                                  │ │                                     │   │
│                                  │ ├─ EVIDENCIA ────────────────────────┤   │
│                                  │ │  ✓ cmdline: node_modules/.bin/next  │   │
│                                  │ │  ✓ dep: next@^15.1.0                │   │
│                                  │ │  ✓ file: next.config.ts             │   │
│                                  │ │  · port: 3001 (no convencional)     │   │
│                                  │ ╰─────────────────────────────────────╯   │
├──────────────────────────────────┴───────────────────────────────────────────┤
│ h cerrar · y copiar · o abrir · e editor · k matar · d docker logs · ?        │
└──────────────────────────────────────────────────────────────────────────────┘
```

**La sección `EVIDENCIA` es una decisión de producto deliberada.** Hace *legible* la detección: el usuario ve por qué Portix cree lo que cree. Esto (a) construye confianza, (b) convierte cualquier bug de detección en un informe de bug perfecto, y (c) es la documentación viva del sistema de señales. Ninguna herramienta competidora hace esto.

### 11.4 Wireframe: búsqueda

Se activa con `/`. Fuzzy, incremental, sin latencia (`nucleo-matcher` sobre ≤500 filas es < 1 ms).

```
├──────────────────────────────────────────────────────────────────────────────┤
│ PORT   PROTO  PROJECT             FRAMEWORK        PID     CPU    MEM   UPTIME│
├──────────────────────────────────────────────────────────────────────────────┤
│▸ 3001  tcp6   acme/**adm**in      ◆ Next.js 15.1   48287   0.4%  289M   2h14m │
│  4000  tcp    acme/api            ◆ NestJS 10.4    48350   0.9%  198M   2h13m │
├──────────────────────────────────────────────────────────────────────────────┤
│ / adm▏                                                       2 de 12 · ⏎ ir   │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Los caracteres coincidentes se resaltan en negrita/color.
- Busca en: puerto, nombre de proyecto, framework, comando, PID, contenedor, ruta.
- `Esc` limpia y sale. `Enter` fija el filtro y devuelve el foco a la tabla.
- **Se busca sobre el snapshot vivo**: si un servicio nuevo coincide con la búsqueda activa, aparece.

### 11.5 Wireframe: filtros estructurados

Se activa con `f`. Un mini-DSL, no un formulario. Los desarrolladores saben teclear.

```
├──────────────────────────────────────────────────────────────────────────────┤
│ f port:>3000 fw:next docker:false▏                                           │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │ port:3000   port:>8000   port:3000-4000                              │   │
│   │ fw:next     lang:node    project:acme                                │   │
│   │ docker:true user:sergi   proto:tcp    cpu:>1                         │   │
│   └──────────────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────┘
```

Los filtros se combinan con AND implícito. Prefijo `!` para negar. Autocompletado con `Tab`. **v1**, no MVP: en el MVP, `f` cicla entre presets (`todos` → `solo míos` → `solo docker` → `solo dev` (puertos > 1024 y con proyecto)).

### 11.6 Wireframe: confirmación de acción destructiva

```
              ╭─ ⚠  Terminar proceso ──────────────────────────╮
              │                                                │
              │  PID 48287 · node                              │
              │  @acme/admin                                   │
              │  ~/work/acme/apps/admin                        │
              │  Escuchando en :3001                           │
              │                                                │
              │  Se enviará SIGTERM. Si no responde en 5s,     │
              │  se te preguntará antes de enviar SIGKILL.     │
              │                                                │
              │        [ Cancelar ]    ‹ Terminar ›            │
              │           Esc              Enter               │
              ╰────────────────────────────────────────────────╯
```

**Reglas no negociables:**
- El foco por defecto está en **Cancelar** no; está en Terminar, pero **`y` no confirma** — hay que pulsar `Enter` explícitamente. Nada de teclas de una letra para matar.
- `SIGTERM` primero, siempre. `SIGKILL` solo tras un segundo diálogo.
- Se muestra **el proyecto**, no solo el PID. El usuario confirma contra lo que tiene en la cabeza.
- Un proceso en contenedor muestra un aviso: *«Este proceso vive en un contenedor. Docker probablemente lo reiniciará. ¿Prefieres `docker stop`?»*

### 11.7 Wireframe: estados vacíos y de error

**Sin servicios (arranque limpio):**

```
┌─ Portix ──────────────────────────────────────────────────────────── v1.0.0 ─┐
│                                                                              │
│                                                                              │
│                              ╭───────────╮                                   │
│                              │    ⌁      │                                   │
│                              ╰───────────╯                                   │
│                                                                              │
│                    Ningún servicio escuchando en este momento                │
│                                                                              │
│              Arranca algo con `pnpm dev`, `docker compose up`,               │
│                 o pulsa `a` para incluir puertos del sistema.                │
│                                                                              │
│                            r refrescar  ·  q salir                           │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Búsqueda sin resultados:**

```
│                                                                              │
│                    Ningún servicio coincide con «postgre»                    │
│                                                                              │
│                Quizá busques `postgres` (5432) — está filtrado               │
│                        por `docker:false`. Pulsa `F` para limpiar.           │
│                                                                              │
```

**Escaneo degradado por permisos:**

```
├──────────────────────────────────────────────────────────────────────────────┤
│ ⚠ 3 sockets pertenecen a otros usuarios. Portix no puede leer su /proc.       │
│   Ejecuta `sudo portix` para verlos, o pulsa `x` para ocultar este aviso.     │
└──────────────────────────────────────────────────────────────────────────────┘
```

**`/proc` no montado / no es Linux:**

```
  portix: no se pudo leer /proc/net/tcp
  Portix requiere un sistema Linux con /proc montado.
  Si estás en un contenedor, monta /proc y /sys, o usa --proc-root <ruta>.
```

**Estado de carga (primer frame, < 60 ms — casi nunca visible, pero debe existir):**

```
│  3000  tcp6   ⠋ resolviendo…      ⠋              48213   —      312M   2h14m │
```

### 11.8 Wireframe: ayuda (`?`)

```
              ╭─ Portix · Atajos ──────────────────────────────╮
              │                                                │
              │  NAVEGACIÓN                                    │
              │    ↑ k        arriba          g   inicio       │
              │    ↓ j        abajo           G   final        │
              │    ⏎ l        detalles        h   cerrar       │
              │    Tab        cambiar panel   PgUp/PgDn        │
              │                                                │
              │  BUSCAR Y FILTRAR                              │
              │    /          buscar          f   filtrar      │
              │    Esc        cancelar        F   limpiar todo │
              │    s          ordenar         S   invertir     │
              │    a          incl. sistema   D   solo docker  │
              │                                                │
              │  ACCIONES                                      │
              │    k          terminar (SIGTERM)               │
              │    K          forzar   (SIGKILL)               │
              │    o          abrir en el navegador            │
              │    e          abrir proyecto en $EDITOR        │
              │    y          copiar…  (p puerto, c comando,   │
              │                         r ruta, u URL, i PID)  │
              │    d          logs del contenedor              │
              │                                                │
              │  GENERAL                                       │
              │    r          refrescar ya    p   pausar       │
              │    ?          esta ayuda      q   salir        │
              │                                                │
              │              Esc o ? para cerrar               │
              ╰────────────────────────────────────────────────╯
```

### 11.9 Sistema de teclado

**Modal, estilo vim** (`Normal` / `Search` / `Filter` / `Modal`). La modalidad no es dogma: es lo que permite que `/`, `k`, `o` sean teclas simples sin colisionar con la escritura de texto.

| Modo | Entrada | Salida |
|---|---|---|
| `Normal` | — (por defecto) | `q` |
| `Search` | `/` | `Esc`, `Enter` |
| `Filter` | `f` | `Esc`, `Enter` |
| `Modal` | acción destructiva, `?` | `Esc`, `Enter` |

**Reglas:**
- `Esc` **siempre** retrocede un nivel. Nunca cierra la aplicación.
- `Ctrl+C` sale siempre, desde cualquier modo, sin confirmación.
- Todas las acciones tienen alternativa con flecha/`Enter` para quien no venga de vim.
- El keymap es **remapeable** en `config.toml` desde v1.
- **Sin ratón obligatorio**, pero soportado: clic selecciona, doble clic abre detalles, rueda hace scroll, clic en cabecera ordena.

### 11.10 Tema, color y degradación

**Paleta semántica** (no colores literales; los temas los mapean):

| Rol | Uso |
|---|---|
| `accent` | Selección, cabeceras, foco |
| `ok` | Escuchando, confianza alta |
| `warn` | Confianza media, bind en `0.0.0.0`, degradación |
| `danger` | Zombi, acciones destructivas |
| `container` | Servicios en contenedor |
| `muted` | Datos desconocidos, ayuda, bordes |
| `text` / `bg` | Base |

**Detección de capacidades** (en este orden): `NO_COLOR` → monocromo. `COLORTERM=truecolor|24bit` → 24-bit. `TERM` termina en `-256color` → 256. En otro caso → 16 colores ANSI.

**Regla de accesibilidad:** ninguna información se codifica **solo** por color. La confianza tiene además glifos (`◆ ◇ ·`), los contenedores tienen `🐳`, la degradación `🔒`. Contraste mínimo 4.5:1 contra el fondo del tema.

**Unicode:** `--ascii` (y autodetección por `LANG`/`LC_ALL` sin UTF-8) sustituye `🐳`→`[D]`, `◆`→`*`, `▸`→`>`, bordes redondeados→ASCII. **Portix debe ser legible en la consola tty de un arranque roto.**

**Temas:** `default-dark`, `default-light`, `nord`, `gruvbox`, `catppuccin`. Un tema es un TOML en `~/.config/portix/themes/`. Autodetección con la variable `PORTIX_THEME`.

### 11.11 Responsive

| Ancho | Comportamiento |
|---|---|
| ≥ 140 | Tabla completa + panel de detalles lado a lado |
| 100–139 | Tabla completa; los detalles se superponen al 50% |
| 80–99 | Se ocultan `PROTO` y `UPTIME`; detalles a pantalla completa |
| 60–79 | Solo `PORT`, `PROJECT`, `FRAMEWORK`; se ocultan métricas |
| < 60 | Vista de lista de una columna: `:3000 acme/web · Next.js` |
| Altura < 12 | Sin barra de estado; solo tabla y una línea de ayuda |

El layout por restricciones de `ratatui` hace esto declarativo, sin aritmética de columnas a mano.

---

## 12. Configuración y modo no interactivo

### 12.1 Fichero de configuración

`$XDG_CONFIG_HOME/portix/config.toml` (por defecto `~/.config/portix/config.toml`). **Todo tiene default sensato; el fichero es opcional** (principio 2).

| Clave | Default | Descripción |
|---|---|---|
| `refresh_ms` | `1000` | Intervalo de refresco. Mínimo 200. |
| `theme` | `"default-dark"` | |
| `ascii` | `false` | Forzar glifos ASCII |
| `show_system_ports` | `false` | Incluir puertos < 1024 y servicios sin proyecto |
| `show_udp` | `true` | |
| `show_loopback_only` | `false` | Ocultar servicios que no escuchan en `127.0.0.1`/`::1` |
| `default_sort` | `"port"` | `port` \| `cpu` \| `mem` \| `uptime` \| `project` |
| `cpu_scale` | `"system"` | `system` (0-100%) \| `core` (0-100%×ncpu) |
| `memory_metric` | `"rss"` | `rss` \| `pss` (v1.5) |
| `kill_confirm` | `true` | **Cambiarlo a `false` requiere teclear la palabra completa `false`; no se acepta abreviatura.** |
| `browser` | `$BROWSER` o `xdg-open` | |
| `editor` | `$VISUAL` → `$EDITOR` → `vi` | |
| `docker_socket` | `/var/run/docker.sock` | v1.5 |
| `[keys]` | — | Remapeo de atajos (v1) |
| `[detectors]` | — | Habilitar/deshabilitar detectores por id |

**Estado MVP de las claves:** `ascii`, `default_sort`, `memory_metric`, `browser`, `editor` y `docker_socket` están declaradas en `core/config.rs` con sus defaults pero **aún no tienen efecto** — `open_url` (`core/actions.rs`) usa `xdg-open` fijo (no `browser`), el portapapeles va por `wl-copy`/`xclip` (no `arboard`), y `docker_socket` espera a la fase v1.5 de la API de Docker. Se implementan en v1; hasta entonces son inofensivas pero no deben usarse como si funcionaran.

### 12.2 CLI y modo no interactivo

Portix es una TUI, pero **un buen ciudadano de Unix** (JS-1 desde un script, y P2/Marcos):

| Invocación | Comportamiento |
|---|---|
| `portix` | TUI interactiva |
| `portix :3000` | Imprime el servicio del puerto 3000 y sale. **Este es el comando que reemplaza a `lsof -i :3000`.** (En MVP solo el posicional acepta `3000` o `:3000`; el alias `--port` queda para v1.) |
| `portix --json` | Snapshot completo en JSON a stdout, exit 0. Sin TUI. Esquema versionado. |
| `portix --json :3000` | JSON de un solo servicio |
| `portix --once` | Un frame de la tabla en texto plano, sin entrar en modo raw |
| `portix --kill :3000` | Mata lo que ocupe el 3000, con confirmación (`--yes` para saltarla) |
| `portix --watch` | **v1 — no implementado en MVP** (TUI que sale cuando el puerto observado se libera) |
| `portix --proc-root <path>` | Raíz de `/proc` alternativa (tests, contenedores) |
| `portix --interval 500` | |
| `portix --no-color` / `NO_COLOR=1` | |
| `portix completions <shell>` | bash/zsh/fish/nu |
| `portix --version` / `--help` | |

**Códigos de salida:** `0` éxito · `1` error general · `2` uso incorrecto · `3` ningún servicio coincide (útil en scripts: `portix :3000 || pnpm dev`) · `4` permisos insuficientes para la operación pedida. **Estado MVP:** el `2` lo produce `clap` al parsear; el `4` (permisos) no está implementado — la degradación por permisos se comunica vía `Degradation::SocketsWithoutPid` y los códigos `0`/`1`/`3` son los únicos que los scripts deben esperar hoy.

**Contrato del JSON:** campo `schema_version` en la raíz. Los campos desconocidos por el consumidor deben ignorarse. Añadir campos es minor; quitar o cambiar tipo es major. `--json` **nunca** escribe nada en stdout que no sea JSON válido (los avisos van a stderr).

**Nota sobre `--once`/`--json`/`--kill` (MVP):** para mostrar CPU con sentido se ejecutan **dos escaneos separados por un sleep fijo de 200 ms** (`scan_with_cpu` en `main.rs`) — el primer scan solo siembra la muestra de jiffies. El sleep no sigue `refresh_ms`; si el coste importa, revisar antes de v1.

---

## 13. Seguridad y privacidad

### 13.1 Telemetría

**No hay. Ni ahora ni nunca.** Portix no realiza ninguna conexión de red saliente, salvo:
- `o` (abrir en navegador), que ejecuta `xdg-open` con una URL de localhost.
- La comprobación opcional de nuevas versiones, que **está desactivada por defecto y no se activará automáticamente en ninguna versión**.

Esto se declara en el README y se verifica en CI con un test que ejecuta Portix bajo `strace -f -e trace=network` y falla si hay algún `connect()` a algo que no sea `AF_UNIX`.

### 13.2 Superficie de riesgo

| Vector | Análisis | Mitigación |
|---|---|---|
| **Parseo de `/proc`** | Los datos vienen de procesos de otros usuarios; `cmdline` y `comm` son **entrada controlada por el atacante**. Un `comm` de `x) 0 0 0 …` rompe parsers ingenuos (§7.4). | `#![forbid(unsafe_code)]` fuera de `platform/`. Fuzzing del parser (§16.4). Parseo de `stat` por el último `)`. |
| **Inyección de secuencias de escape en la terminal** | Un proceso malicioso con `argv[0] = "\x1b]0;pwned\x07"` puede **secuestrar el título de la terminal, mover el cursor o inyectar texto en el buffer de entrada** cuando Portix lo imprime en la columna «Comando». Esto es una vulnerabilidad real y documentada en herramientas de terminal. | **Todo texto procedente del sistema se sanea antes de dibujarse**: se eliminan los caracteres de control C0/C1 (excepto tab, que se expande) y se sustituyen por `·`. Esto se implementa en `ui/format.rs` como un único punto de paso obligatorio, y hay tests de regresión con secuencias ANSI en `ui/format.rs` y en cada vista que renderiza texto derivado del sistema (incluida la versión de framework del panel de detalle, que era un bypass). **Sin excepciones.** El fuzzing real del parser (§16.4) sigue pendiente. |
| **`environ` con secretos** | `/proc/<pid>/environ` contiene `DATABASE_URL`, tokens, claves. | **No se lee en el MVP.** En v1, la señal `env_var` de los detectores solo comprueba **la existencia de una clave**, jamás lee ni almacena valores. El panel de detalle **nunca** muestra `environ`. Si en el futuro se muestra, será tras un flag explícito y con enmascaramiento de valores por defecto. |
| **`kill` sobre PID equivocado** | PID reciclado entre el snapshot y la pulsación de la tecla → se mata un proceso inocente. | Antes de enviar la señal se **re-verifica `(pid, starttime)`** contra `/proc/<pid>/stat`. Si no coincide, se aborta con un error visible. Este es un chequeo TOCTOU obligatorio y barato. |
| **Ejecución de comandos externos** (`xdg-open`, `$EDITOR`, `docker logs`) | Inyección de comandos si se construye una línea de shell. | **Nunca se usa `sh -c`.** Siempre `Command::new(prog).arg(x)` con argumentos separados. Las URLs se construyen desde tipos, no desde cadenas del proceso. |
| **Symlinks hostiles en el ascenso de directorios** | Un `cwd` que apunta a `/etc` hace que Portix intente leer manifiestos ahí. | El ascenso se limita a `$HOME` y a 24 niveles; solo se abren ficheros con nombres de una lista blanca (`package.json`, etc.); se limita el tamaño de lectura a 1 MiB; `O_NOFOLLOW` en el fichero final. **Estado MVP:** todas las lecturas de manifiesto (`package.json`, `Cargo.toml`, `go.mod`, `app.py`) pasan por `detect::project::read_manifest` — tope de 1 MiB y requisito de fichero regular, de modo que un FIFO plantado donde debería haber un manifiesto no bloquea el scan. |
| **Supply chain** | Ver §5.4. | `cargo-deny`, `cargo-audit`, `Cargo.lock` comiteado, sin `build.rs` con red. |

### 13.3 Privilegios: la decisión de no escalarlos

**Portix NO será setuid. Portix NO pedirá capabilities en el `PKGBUILD`. Portix NO invocará `sudo` ni `pkexec`.**

Alternativas evaluadas y rechazadas:

| Opción | Por qué se rechaza |
|---|---|
| Binario `setuid root` | Inaceptable. Un parser de entrada hostil corriendo como root es la definición de una mala idea. |
| `CAP_SYS_PTRACE` vía `setcap` en el `PKGBUILD` | Permite leer `/proc/<pid>/cwd` de cualquiera. Pero un `PKGBUILD` que hace `setcap` sorprende al usuario, rompe con `paccheck`, e implica que cualquier bug de Portix escala. También rompe el binario si se copia a otro sitio. |
| Helper con `polkit` | Complejidad enorme (política, daemon, D-Bus) para un beneficio marginal: ver el `cwd` de procesos que no son del usuario, que casi nunca son proyectos de desarrollo. |
| **Nada. `sudo portix` cuando haga falta.** | ✅ **Elegido.** Explícito, auditable, cero superficie. El 99% del uso no lo necesita (§7.8). |

`sudo portix` funciona perfectamente y lee la configuración del usuario original si `$SUDO_USER` está definido (para no ignorar el tema del usuario).

### 13.4 SELinux y AppArmor

En Arch ninguno está activo por defecto, pero Portix debe comportarse bien donde lo estén (Fedora, Ubuntu, openSUSE — relevante para v1.5).

- **AppArmor** puede denegar `read` sobre `/proc/<pid>/cwd` incluso siendo root, si hay un perfil restrictivo sobre el proceso objetivo. Se manifiesta como `EACCES` → se trata como degradación normal (§7.8). Ningún manejo especial.
- **SELinux** puede denegar `readlink` sobre `/proc/<pid>/{cwd,exe,root}` por reglas de tipo, y también `getattr`. Igual: `EACCES`/`EPERM` → degradación.
- Portix **nunca** debe interpretar un fallo de permisos como un fallo fatal. La política ya está en §7.4.
- **Diagnóstico:** el comando `portix --doctor` (v1) imprime qué puede y qué no puede leer, con el `errno` exacto y una sugerencia (`¿tienes AppArmor activo? comprueba dmesg`). Esto convierte una hora de confusión del usuario en 5 segundos.
- No se distribuirá perfil de AppArmor ni política SELinux propios. Portix es un cliente, no un servicio.

### 13.5 cgroups v1 y v2

Manejado en §7.6. El detector prueba v2 primero (línea `0::`) y cae a v1 (líneas `n:subsys:`). El modo híbrido (v1 y v2 montados simultáneamente, algunas distros antiguas) se maneja porque el parser acepta ambos formatos en el mismo fichero sin conflicto.

---

## 14. Riesgos técnicos

Escala: probabilidad e impacto de 1 (bajo) a 5 (alto).

| # | Riesgo | P | I | Mitigación | Dueño |
|---|---|:-:|:-:|---|---|
| R-01 | **Docker con `userland-proxy=false` hace invisibles los puertos publicados** (§7.7). El usuario ve una lista incompleta y concluye que Portix está roto. | 4 | 5 | MVP: detectar daemon de contenedores activo y avisar en la barra de estado. v1: escaneo por netns vía `/proc/<pid>/net/tcp`. **Riesgo #1 del proyecto.** | Core |
| R-02 | **Reutilización de PID contamina la caché** y se muestra el proyecto equivocado (§10.2). Bug silencioso, no reproducible, destruye la confianza. | 3 | 5 | Clave `(pid, starttime)` en todas las cachés. Test obligatorio. Re-verificación antes de `kill` (§13.2). | Core |
| R-03 | **Parseo de `/proc/<pid>/stat` con `comm` malicioso** provoca pánico o datos erróneos. | 3 | 4 | Parseo desde el último `)`. Fuzzing con `cargo-fuzz`. Nunca `unwrap()` en `platform/`. | Platform |
| R-04 | **Inyección de escapes ANSI vía `cmdline`** compromete la terminal del usuario (§13.2). | 2 | 5 | Saneamiento obligatorio en un único punto. Test de fuzzing. Auditoría en revisión de PR. | UI |
| R-05 | **Sin permisos, la funcionalidad estrella (proyecto) desaparece** para procesos ajenos. El usuario percibe la herramienta como inútil. | 5 | 2 | Es aceptable: los servicios de desarrollo son del propio usuario. Comunicación clara con `🔒` y pista de `sudo`. `--doctor`. | UX |
| R-06 | **`cwd` = raíz del monorepo** ⇒ todos los servicios parecen del mismo proyecto (§8.5). Rompe JS-6, el diferenciador principal. | 4 | 4 | Heurística de rutas en `cmdline` (v1). Recolección de casos reales vía issues con `--json` adjunto. Es el área de mayor inversión en iteración. | Detect |
| R-07 | **Procesos efímeros** hacen parpadear la tabla y saltar la selección. | 4 | 3 | Tombstones de 2 s, `ServiceId` estable, orden determinista (§7.9). | UI |
| R-08 | **Sockets compartidos entre procesos** (`SO_REUSEPORT`, o un `fork()` que hereda el fd): el mismo inode aparece en N procesos. `nginx` con 8 workers, `gunicorn` con 4. | 4 | 3 | El mapa es `inode → Vec<(pid, fd)>`. La UI muestra el **proceso padre** como propietario y `+7 workers` como sufijo. Colapsable con `z`. Nunca 8 filas idénticas. | Core |
| R-09 | **Rendimiento con 5.000 procesos** (servidor de CI, máquina compartida): el escaneo de `fd/` degrada. | 2 | 3 | Poda por UID (elimina el 90% en ese escenario). Paralelismo con `rayon`. Presupuesto medido en CI con un `/proc` sintético de 5.000 PIDs. Escape hatch: netlink (§7.10). | Platform |
| R-10 | **SELinux/AppArmor** deniegan lecturas y el usuario no entiende por qué. | 2 | 2 | Degradación honesta + `--doctor` con `errno`. | Platform |
| R-11 | **cgroups v1** en distros antiguas rompe la detección de contenedores. | 2 | 2 | Parser que acepta ambos formatos. Fixtures de ambos en los tests. | Platform |
| R-12 | **Namespaces de PID**: un proceso en un contenedor tiene un PID distinto dentro y fuera. Mostrar el PID del host confunde a quien hace `docker exec`. | 3 | 2 | Mostrar el PID del host (que es el útil para `kill`) y, en el detalle, el PID del namespace (`status:NSpid`) si difiere. | Platform |
| R-13 | **`ratatui` introduce breaking changes** entre minors. | 2 | 2 | `Cargo.lock` comiteado, actualizaciones deliberadas, capa fina de widgets propios que aísla la API. | Core |
| R-14 | **El scope crece hacia «htop pero bonito»** y el producto pierde su identidad. | 4 | 4 | §4.5 es normativo. Toda issue de feature debe citar una job story de §2.2 o se cierra con `wontfix: out of scope`. | Product |
| R-15 | **Un solo maintainer**; el proyecto se estanca. | 3 | 4 | Detectores declarativos bajan la barrera (§9.5). `good-first-issue` desde el día 1. Documentación de arquitectura (este PRD) en el repo. | Product |

---

## 15. Benchmarks y objetivos de rendimiento

### 15.1 Máquina de referencia

Se define explícitamente para que los números sean falsables:

> **Ref-A:** AMD Ryzen 7 5800X (8c/16t), 32 GB RAM, NVMe, Linux 6.12, Arch Linux, ~400 procesos, ~35 sockets en escucha, terminal `foot` a 200×50, `HZ=100`.
>
> **Ref-B (caso adverso):** VM de 2 vCPU, 4 GB, ~1.500 procesos, ~120 sockets, terminal `xterm` a 80×24.

### 15.2 Objetivos (SLO del producto)

| Métrica | Objetivo Ref-A | Objetivo Ref-B | Cómo se mide |
|---|---|---|---|
| **Tiempo hasta el primer frame** (arranque en frío, tabla con puertos/PIDs) | **< 50 ms** | < 120 ms | `hyperfine 'portix --once'` |
| **Tiempo hasta la tabla enriquecida** (proyecto + framework resueltos) | **< 100 ms** | < 250 ms | Instrumentación interna, `--bench-startup` |
| **Duración de un refresco caliente** (Tier 0 + 1) | **< 15 ms** | < 60 ms | `criterion` sobre `Engine::scan()` |
| **Duración de un refresco frío** (todas las cachés vacías) | < 60 ms | < 200 ms | `criterion` |
| **RSS en reposo** (12 servicios) | **< 15 MB** | < 20 MB | `/proc/self/status` tras 60 s |
| **RSS con 500 servicios** | < 40 MB | < 50 MB | `/proc` sintético |
| **CPU en reposo, terminal enfocada, 1 Hz** | **< 0.5%** de un core | < 1.5% | `pidstat -p $(pidof portix) 1 60` |
| **CPU en reposo, terminal sin foco** | **< 0.1%** | < 0.4% | ídem |
| **Latencia de un frame de UI** (tecla → pixel) | **< 16 ms** (p99) | < 33 ms | Traza interna con `tracing` |
| **Latencia de la búsqueda incremental** (500 filas) | **< 2 ms** (p99) | < 8 ms | `criterion` sobre `query::filter()` |
| **Tiempo de escaneo con 5.000 procesos** | < 80 ms | < 300 ms | `/proc` sintético en CI |
| **Tamaño del binario** (release, `strip`, `lto=fat`, `opt-level=z` no) | < 6 MB | — | CI |
| **Crates transitivos** | < 90 | — | `cargo tree \| wc -l` |

### 15.3 Comparativa de referencia (para el README)

Se publicará una tabla honesta, medida con `hyperfine`, del tiempo para responder «¿qué usa el puerto 3000?»:

| Comando | Tiempo (mediana, Ref-A) |
|---|---|
| `lsof -i :3000` | ~180 ms |
| `ss -ltnp \| grep 3000` | ~12 ms |
| `portix :3000` | **objetivo < 30 ms** |
| `portix :3000` *(incluyendo proyecto y framework)* | **objetivo < 45 ms** |

> Nota de honestidad: `ss` es y seguirá siendo más rápido en el caso trivial, porque no hace nada más. Portix no compite en «listar un socket»; compite en «responder la pregunta». **La tabla se publicará con esta nota.** No se maquillan benchmarks.

### 15.4 Metodología y prevención de regresiones

- **`criterion`** para microbenchmarks de `net::parse`, `fd::map_inodes`, `query::filter`, `detect::run`.
- **`hyperfine`** para el arranque end-to-end.
- **Un `/proc` sintético** (§16.2) generado por un fixture con N procesos y M sockets, versionado en el repo. Permite medir en CI de forma determinista, sin depender de la máquina del runner.
- **Un job de CI (`bench`)** que compara contra el baseline de `main` y **falla el PR si una métrica clave regresa > 10%**.
- **`heaptrack` / `dhat-rs`** en un job manual para vigilar las asignaciones en el camino caliente. Objetivo: **cero asignaciones** en `fd::map_inodes` por iteración.

### 15.5 Objetivos de experiencia (no medibles, pero normativos)

- Escribir en la barra de búsqueda debe sentirse tan responsivo como escribir en `$EDITOR`. Cualquier latencia perceptible es un bug P1.
- La tabla no debe moverse bajo el cursor salvo por acción del usuario.
- Ningún frame debe mostrar datos a medio actualizar (garantizado por el `Arc<Snapshot>` inmutable).
- Portix debe poder ejecutarse en un `tmux` dentro de un `ssh` sobre una conexión lenta sin sentirse roto (el diff de buffers de `ratatui` minimiza los bytes emitidos; el modo `--interval 3000` existe para eso).

---

## 16. Estrategia de testing

### 16.1 Pirámide

| Nivel | Qué cubre | Herramienta | Objetivo |
|---|---|---|---|
| Unitarios | Parsers, scoring, formateo, ascenso de directorios | `cargo test` | ~70% de los tests |
| Golden / fixtures | `/proc` real capturado de máquinas reales | Ficheros en `tests/fixtures/proc/` | Cobertura de casos raros |
| Integración | `Engine` completo contra un `/proc` sintético | `--proc-root` | Flujo puerto→framework end-to-end |
| Snapshot de UI | `draw()` contra un `TestBackend` de ratatui | `insta` | Regresiones visuales |
| Property / fuzz | Parsers con entrada arbitraria | `proptest`, `cargo-fuzz` | Ausencia de pánicos |
| E2E | Binario real, procesos reales | Script + `bash` en CI | Humo |

**Objetivo de cobertura: 80% de líneas**, con exención explícita para `ui/views/` (cubierto por snapshots) y `main.rs`.

### 16.2 La inyección de `/proc` es la decisión de testabilidad clave

Toda ruta absoluta pasa por `platform::linux::root::ProcRoot`, un tipo que envuelve un `PathBuf` (por defecto `/proc`). El flag oculto `--proc-root` lo sobreescribe.

**Consecuencia:** el 100% del código de descubrimiento se testea sin privilegios, sin procesos reales, de forma determinista y en milisegundos, en cualquier plataforma (incluso en el runner de macOS). Un fixture es un árbol de directorios:

```
tests/fixtures/proc/monorepo-pnpm/
├── net/tcp
├── net/tcp6
├── stat
├── 48213/
│   ├── cmdline      (con \0 reales)
│   ├── stat
│   ├── status
│   ├── cgroup
│   ├── cwd     → symlink a ../../projects/acme/apps/web
│   └── fd/23   → symlink a "socket:[486312]"
└── …
```

Y un fixture paralelo `tests/fixtures/projects/` con `package.json` reales. **Un test es: «dado este `/proc` y estos proyectos, la tabla debe ser exactamente esta».** Se comparan con `insta`.

**Fixtures obligatorios desde el MVP:**

`monorepo-pnpm`, `docker-proxy`, `docker-no-proxy`, `django-manage`, `gunicorn-argv-rewritten`, `comm-with-parens` (`stat` malicioso), `cmdline-with-ansi-escapes`, `pid-reuse`, `so-reuseport-nginx`, `dual-stack-node`, `deleted-exe`, `cwd-permission-denied`, `cgroup-v1`, `cgroup-v2`, `5000-procesos` (generado).

### 16.3 Tests de arquitectura

Un test que falla el build si se viola la regla de dependencias de §6.2 (p.ej. `ui/` importa `std::fs`). Implementable con un test que grepea los `use` o con `cargo-modules`. Barato, y protege la propiedad más valiosa del diseño.

### 16.4 Fuzzing

Targets de `cargo-fuzz`:
- `parse_proc_net_line` — entrada: bytes arbitrarios.
- `parse_proc_stat` — el `comm` entre paréntesis.
- `parse_cmdline` — bytes con `\0` y UTF-8 inválido.
- `sanitize_for_terminal` — debe garantizar que la salida no contiene ningún byte < 0x20 salvo los permitidos. **Test de propiedad, no solo fuzz.**

Se ejecutan en CI semanalmente y en cada PR que toque `platform/` o `ui/format.rs`.

### 16.5 CI

| Job | Contenido |
|---|---|
| `check` | `fmt --check`, `clippy -D warnings`, `cargo-deny`, `cargo-audit` |
| `test` | `cargo test` en `stable`, `beta`, y el MSRV |
| `test-matrix` | x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl |
| `snapshot` | `insta` — falla si hay snapshots pendientes |
| `bench` | `criterion` vs baseline de `main`, falla en regresión > 10% |
| `no-network` | `strace` verifica cero `connect()` de red (§13.1) |
| `msrv` | `cargo-msrv verify` |
| `e2e` | Levanta un servidor Node y un contenedor, verifica que `portix --json` los ve |

---

## 17. Roadmap

Cada fase tiene **criterios de salida falsables**. Una fase no termina porque «ya está», sino porque se cumplen.

### MVP — «Responde la pregunta» · ~6 semanas

> **Objetivo:** un desarrollador con `EADDRINUSE` teclea `portix`, ve su proyecto, y lo mata. Nada más.

| Incluye | Excluye |
|---|---|
| Escaneo de TCP/TCP6 LISTEN + UDP | netns / contenedores más allá de `docker-proxy` |
| Mapa inode → PID (con poda por UID y `rayon`) | Netlink |
| Metadata de proceso (cmdline, cwd, exe, user, uptime) | `environ` |
| CPU (delta) y RSS | PSS, gráficas históricas |
| Detección de proyecto (ascenso + marcadores + workspaces) | Miembros del workspace enumerados |
| Detectores compilados: Node, Python, Go, Rust, PHP, servicios comunes | Detectores TOML de usuario |
| Detección de contenedor vía cgroup (runtime + id corto) | API de Docker, nombres de contenedor |
| Tabla, panel de detalles, ayuda | Filtros DSL |
| Búsqueda fuzzy (`/`) | Filtros estructurados (`f` cicla presets) |
| Ordenación por puerto/cpu/mem/uptime | Ordenación por clic |
| `kill` con confirmación (SIGTERM/SIGKILL) | Otras acciones sobre contenedores |
| `o` abrir en navegador, `y` copiar | `e` abrir en editor |
| Caché L1–L4 con `(pid, starttime)` | inotify |
| Tema por defecto, degradación 16 colores, `--ascii` | Temas de usuario |
| `--json`, `--once`, `:PORT`, `--proc-root` | `--doctor`, `--kill`, `--watch` |
| Saneamiento de escapes ANSI | — |

**Criterios de salida del MVP:**
- ✅ Los 15 fixtures de §16.2 pasan.
- ✅ Cobertura ≥ 70%.
- ✅ Ref-A: primer frame < 50 ms, refresco caliente < 15 ms, RSS < 15 MB.
- ✅ Cero `unwrap()`/`expect()` en `platform/` y `detect/`.
- ✅ `#![forbid(unsafe_code)]` en todos los módulos salvo `platform/linux/`.
- ✅ Detecta correctamente los 9 escenarios de las job stories en una máquina real con el stack de §2.1.
- ✅ Instalable desde `portix-git` en la AUR.

### v1.0 — «Confiable y completa» · +6 semanas

> **Objetivo:** que un desconocido la instale desde la AUR y no encuentre un caso donde mienta.

- **Escaneo por network namespace** (`/proc/<pid>/net/tcp`) → **los contenedores se ven de verdad**. (R-01)
- **Detectores declarativos en TOML**, embebidos y sobrescribibles por el usuario. Esquema v1 congelado. (§9.5)
- Heurística de monorepo desde `cmdline` (R-06).
- Enumeración de miembros del workspace → **`X` matar todos los servicios de un proyecto** (JS-9).
- Filtros estructurados (`f` con DSL y autocompletado).
- `e` abrir el proyecto en `$EDITOR`.
- Agrupación de sockets compartidos (`SO_REUSEPORT`, workers) (R-08).
- Sparklines de CPU/RSS en el panel de detalle.
- Rama de git en el panel de detalle.
- Temas de usuario (`nord`, `gruvbox`, `catppuccin`), remapeo de teclas.
- `portix --doctor`, `--kill`, `--watch`, completions de shell.
- Soporte de ratón.
- **`portix-bin` en la AUR** con releases firmadas.
- Cobertura ≥ 80%. Benchmarks en CI con gate de regresión.
- README con demo en `vhs`/`asciinema` y la tabla honesta de §15.3.

### v1.5 — «Docker de primera clase» · +8 semanas

- Cliente del socket de Docker (opcional, degradable): **nombre del contenedor, imagen, proyecto de Compose, mapeo de puertos declarado**.
- Agrupación por proyecto de Docker Compose.
- `d` → seguir logs del contenedor en un panel.
- Detección de Podman vía su socket compatible.
- **Backend `NETLINK_INET_DIAG`** con fallback automático (§7.10).
- `PSS` opcional vía `smaps_rollup`.
- Invalidación de caché con `inotify` (`notify`).
- Sockets Unix (`/proc/net/unix`) — muchos servicios de desarrollo escuchan ahí (`.next/`, sockets de PHP-FPM).
- Métricas de I/O de red por proceso (v2 candidate; se evalúa el coste).
- **Empaquetado más allá de Arch**: `.deb`, `.rpm`, Nix, Homebrew (Linux), binarios estáticos musl. Se hace aquí, no antes: primero se gana Arch.

### v2 — «Plataforma» · sin fecha, dirigido por evidencia

Solo se construye lo que la comunidad demuestre necesitar (issues, votos, `--json` de casos reales).

| Candidato | Justificación necesaria |
|---|---|
| **Plugins WASM** para detectores | Que el TOML se demuestre insuficiente en ≥ 5 casos reales |
| **macOS** (`libproc`) | Demanda + un maintainer con Mac |
| **Windows** (`GetExtendedTcpTable`) | Demanda (WSL2 cubre buena parte) |
| **`setns()` + lectura de nftables** para DNAT | Que el escaneo por netns de v1 se quede corto |
| **Historial persistente** («el 3000 lo tuvo `acme/web` hasta las 14:03») | Job story nueva y validada |
| **Integración con systemd** (`systemctl status` del servicio) | Demanda de P2 |
| **`portix top`**: modo de solo métricas | **Probablemente se rechace** — es htop (§4.5) |

---

## 18. Distribución

### 18.1 AUR (canal oficial v1)

Tres paquetes, siguiendo la convención de Arch:

| Paquete | Contenido | `makedepends` | Audiencia |
|---|---|---|---|
| `portix` | Compila desde el tarball de la release | `cargo` | Por defecto |
| `portix-bin` | Binario precompilado de la release de GitHub | — | Quien no quiere compilar |
| `portix-git` | Compila desde `main` | `cargo`, `git` | Early adopters, testers del MVP |

**Notas del `PKGBUILD`** (a documentar en `packaging/`):

- `arch=('x86_64' 'aarch64')`.
- `build()`: `export RUSTUP_TOOLCHAIN=stable`, `export CARGO_TARGET_DIR=target`, `cargo build --frozen --release --all-features`. **`--frozen` obliga a que `Cargo.lock` esté comiteado y sea coherente** — requisito de las guías de Arch.
- `check()`: `cargo test --frozen`.
- `package()`: instalar el binario, la licencia (ambas, MIT y Apache-2.0), el man page (generado por `clap_mangen`), y las completions de bash/zsh/fish.
- **Sin `setcap`, sin `setuid`, sin hooks de post-install.** (§13.3)
- `provides`/`conflicts` entre los tres paquetes.
- El `.SRCINFO` se regenera y comitea en cada release (automatizable en CI).

### 18.2 Otros canales

| Canal | Fase | Notas |
|---|---|---|
| `cargo install portix` | MVP | Gratis. crates.io reserva el nombre desde el día 1. |
| GitHub Releases (tar.gz + binario) | MVP | x86_64-gnu, x86_64-musl, aarch64-gnu |
| `.deb` / `.rpm` | v1.5 | `cargo-deb`, `cargo-generate-rpm` |
| Nix / nixpkgs | v1.5 | Contribución de la comunidad |
| Homebrew (Linux) | v1.5 | |
| Flatpak / Snap | ❌ nunca | El sandbox impide leer `/proc` de otros procesos. **Es técnicamente contradictorio con el producto** y se rechazará explícitamente en el issue tracker. |

### 18.3 Releases

- **SemVer estricto.** El esquema JSON de `--json` y el esquema de los detectores TOML son **API pública** desde v1.0.
- `CHANGELOG.md` en formato *Keep a Changelog*, escrito a mano (nunca autogenerado desde commits).
- Tags firmados con GPG. Checksums `SHA256SUMS` y firma `.sig` en cada release.
- Los binarios se construyen en CI con un runner limpio; se publica el hash del commit y de la toolchain para permitir reproducibilidad.
- Cadencia: patch cuando haga falta, minor cada ~6 semanas, sin fechas prometidas.

---

## 19. Gobernanza open source

### 19.1 Licencia

**MIT OR Apache-2.0** (dual, a elección del usuario). Es la convención del ecosistema Rust, maximiza la compatibilidad corporativa y la cláusula de patentes de Apache-2.0 protege a los contribuidores. Los ficheros `LICENSE-MIT` y `LICENSE-APACHE` van en la raíz.

### 19.2 Estructura del repositorio

```
portix/
├── src/                    # ver §6.1
├── detectors/              # detectores TOML embebidos — el punto de entrada al contribuidor
├── themes/
├── tests/fixtures/proc/    # árboles de /proc sintéticos
├── tests/fixtures/projects/
├── benches/
├── fuzz/
├── packaging/aur/          # PKGBUILD de los tres paquetes
├── docs/
│   ├── PRD.md              # este documento
│   ├── ARCHITECTURE.md     # extracto vivo de §6
│   ├── DETECTORS.md        # cómo escribir un detector TOML  ← el doc más importante
│   └── PROC.md             # notas sobre /proc y sus trampas (§7)
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md      # Contributor Covenant 2.1
├── SECURITY.md             # divulgación privada vía GitHub Security Advisories
└── CHANGELOG.md
```

### 19.3 Proceso de contribución

- **`docs/DETECTORS.md` es la puerta de entrada.** «¿Falta tu framework? Copia este TOML, cambia tres líneas, manda un PR.» Un PR de detector debe incluir un fixture de proyecto y un test. Se revisa en < 48 h. Es el camino explícito para convertir usuarios en contribuidores.
- Cambios en `core/`, `platform/` o el esquema de detectores requieren un **RFC** (una issue con la plantilla `rfc.md`) antes del PR.
- **Toda issue de feature debe citar una job story de §2.2** o proponer una nueva con evidencia. Sin eso, se etiqueta `needs-justification` y se cierra en 30 días. Esto es lo que protege del riesgo R-14.
- Etiquetas: `good-first-issue` reservado para detectores, temas y documentación.
- Un `bug` sobre detección debe adjuntar la salida de `portix --json` y de `portix --doctor`. Se pedirá con una plantilla.

### 19.4 Anti-features (documentadas en el README, para cerrar issues rápido)

- Monitorización de procesos sin sockets.
- Gráficas históricas más allá del sparkline de la sesión.
- Gestión de contenedores (build, up, down).
- Kubernetes, hosts remotos, SSH.
- Telemetría, analytics, actualizaciones automáticas.
- Flatpak / Snap.
- Cualquier cosa que requiera privilegios permanentes.

---

## 20. Métricas de éxito

### 20.1 Producto (cualitativas — no hay telemetría)

| Señal | Cómo se observa |
|---|---|
| **Reflejo muscular adquirido** | Usuarios diciendo en issues/redes «ya no uso `lsof`» |
| **Detección percibida como correcta** | Ratio de issues `wrong-detection` por cada 1.000 estrellas < 5 |
| **Comprensible sin documentación** | Issues del tipo «¿cómo hago X?» sobre features visibles en `?` |

### 20.2 Adopción OSS (cuantitativas)

| Métrica | 3 meses | 12 meses |
|---|---|---|
| Estrellas en GitHub | 500 | 4.000 |
| Votos en AUR (`portix` + `portix-bin`) | 50 | 400 |
| Contribuidores únicos | 5 | 30 |
| **Detectores aportados por la comunidad** | 3 | **20** ← la métrica que valida §9.5 |
| Issues abiertas > 90 días | < 10 | < 20 |
| Mediana de tiempo a primera respuesta | < 48 h | < 48 h |

### 20.3 Calidad

| Métrica | Umbral |
|---|---|
| Cobertura de tests | ≥ 80% (v1) |
| Pánicos reportados en un año | 0 |
| CVEs | 0 |
| Regresiones de rendimiento que lleguen a una release | 0 |
| Advisories de `cargo-audit` sin resolver | 0 |

### 20.4 Anti-métricas (que **no** perseguimos)

- Número de features.
- Líneas de código.
- Número de métricas mostradas por fila.
- Comparaciones de velocidad contra `ss` en el caso trivial (§15.3).

---

## 21. Preguntas abiertas

Requieren decisión antes de cerrar el diseño técnico. Cada una lleva una recomendación.

| # | Pregunta | Recomendación | Decide |
|---|---|---|---|
| Q-01 | ¿El MVP muestra UDP por defecto? Genera ruido (`avahi`, `systemd-resolved`) pero oculta servicios legítimos (Vite HMR no, pero sí algunos). | **Sí, mostrar, pero filtrados**: solo UDP con proyecto detectado o puerto > 1024. | Product |
| Q-02 | ¿`portix :3000` imprime texto humano o JSON? | **Texto humano por defecto** (es un reemplazo de `lsof`), JSON con `--json`. | Product |
| Q-03 | ¿Se muestra `docker-proxy` como fila propia, o se reescribe como el contenedor destino? | **Se reescribe.** El usuario quiere ver `postgres`, no `docker-proxy`. Se indica en el detalle. | Product |
| Q-04 | ¿Los servicios del sistema (`sshd`, `cups`, `avahi`) se ocultan por defecto? | **Sí** (`show_system_ports = false`), tecla `a` para mostrarlos. Portix es para desarrollo. | Product |
| Q-05 | ¿Se detecta la versión del framework desde el manifiesto (`"^15.1.0"`, un rango) o desde el lockfile (versión exacta)? | **Manifiesto en el MVP** (una lectura, un formato). **Lockfile en v1** (exacto, pero 4 formatos distintos y ficheros de MB). Mostrar `15.1.0` vs `^15.1.0` importa. | Eng |
| Q-06 | ¿Nombre del binario y del proyecto? `portix` está libre en crates.io y AUR (verificar antes de anunciar). | **Verificar y reservar `portix` en crates.io hoy.** Alternativas: `porto`, `whatport`, `portscope`. | Product |
| Q-07 | ¿`kill` por defecto envía `SIGTERM` al PID o al **grupo de procesos** (`-PID`)? Matar `pnpm dev` deja huérfano al `next dev` hijo. | **Al grupo**, con un aviso claro en el diálogo: «se terminarán 3 procesos». Es lo que el usuario quiere (JS-1) y lo que `Ctrl+C` hace. Riesgo: matar de más. **Requiere prototipo.** | Eng + Product |
| Q-08 | ¿El detector de contenedores del MVP intenta leer manifiestos dentro del rootfs (`/proc/<pid>/root/app/package.json`)? | **Sí, si hay permisos.** Es barato y da la detección de framework dentro de contenedores prácticamente gratis. Degrada limpio. | Eng |
| Q-09 | ¿Cómo se muestra un servicio de un proyecto que ya no existe en disco (`cwd` borrado)? | Fila normal, proyecto en gris con sufijo `(borrado)`. | UX |
| Q-10 | ¿`--json` incluye el `cmdline` completo? Puede contener secretos (`--token=…`). | **Sí, pero con un flag `--redact` que enmascare argumentos que casen con `(token\|key\|secret\|password)=`.** Documentar el riesgo. | Security |

---

## 22. Apéndices

### 22.1 Formato de `/proc/net/tcp` — referencia

```
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486312 1 ffff8… 100 0 0 10 0
   │  │        │    │             │                                           │             │
   │  │        │    │             └ estado (hex)                              │             └ inode del socket
   │  │        │    └ dirección remota                                        └ uid propietario
   │  │        └ puerto local (hex, big-endian) → 0x1F90 = 8080
   │  └ dirección local (hex, u32 en orden de host) → 0x0100007F → 127.0.0.1
   └ índice de línea (ignorar)
```

Para `tcp6`, `local_address` son 32 caracteres hex: cuatro `u32` en orden de host, cada uno invertido por bytes de forma independiente.

### 22.2 Estados TCP en `/proc/net/tcp`

| Hex | Estado | ¿Nos interesa? |
|---|---|---|
| `01` | ESTABLISHED | No (v2: contar conexiones activas) |
| `02` | SYN_SENT | No |
| `03` | SYN_RECV | No |
| `04` | FIN_WAIT1 | No |
| `05` | FIN_WAIT2 | No |
| `06` | TIME_WAIT | No (pero explica «puerto ocupado» sin proceso — mostrar aviso) |
| `07` | CLOSE | No |
| `08` | CLOSE_WAIT | No |
| `09` | LAST_ACK | No |
| `0A` | **LISTEN** | **Sí — el caso principal** |
| `0B` | CLOSING | No |
| `0C` | NEW_SYN_RECV | No |

> **Detalle de UX que nadie implementa:** cuando un puerto da `EADDRINUSE` pero no hay ningún socket en LISTEN, la causa suele ser un socket en `TIME_WAIT`. Portix, en v1, mostrará esa fila con el mensaje *«en TIME_WAIT — se liberará en ~60 s»*. Eso **resuelve** JS-1 en un caso donde `lsof -i` no muestra nada y el desarrollador enloquece. Es exactamente la clase de detalle que convierte una herramienta en *la* herramienta.

### 22.3 Campos relevantes de `/proc/<pid>/stat`

| # | Campo | Uso |
|---|---|---|
| 1 | `pid` | |
| 2 | `comm` | **Entre paréntesis; puede contener espacios y `)`.** Parsear desde el **último** `)`. |
| 3 | `state` | `R S D Z T t W X x K W P I` |
| 4 | `ppid` | Árbol de procesos, agrupación de workers |
| 5 | `pgrp` | Grupo de procesos (Q-07: `kill(-pgrp)`) |
| 14 | `utime` | CPU (jiffies) |
| 15 | `stime` | CPU (jiffies) |
| 22 | `starttime` | Jiffies desde el boot. **Clave de caché + anti reutilización de PID.** |
| 24 | `rss` | En páginas. Preferir `VmRSS` de `status`, que ya está en kB. |

### 22.4 Glosario

| Término | Definición |
|---|---|
| **Servicio** | La entidad central de Portix: un socket en escucha + su proceso + su proyecto + su framework. |
| **Snapshot** | Estado inmutable y completo del mundo en un instante. Lo produce el collector, lo consume la UI. |
| **Señal** | Una pieza de evidencia observable que apunta a un framework, con un peso en [0,1]. |
| **Detector** | Función pura que emite señales. Compilado (Rust) o declarativo (TOML). |
| **Degradación** | Un dato que no se pudo obtener. Se muestra explícitamente, nunca se adivina. |
| **Tombstone** | Fila de un servicio que acaba de morir; se muestra atenuada 2 s para evitar parpadeo. |
| **Jiffy** | Tick del reloj del kernel. `HZ` ticks por segundo, típicamente 100. Obtener con `sysconf(_SC_CLK_TCK)`. |
| **netns** | Network namespace. Cada uno tiene su propia tabla de sockets, su propio `/proc/net/tcp`. |

---

## Cierre

Este documento define un producto con un alcance deliberadamente estrecho y una ejecución deliberadamente profunda. El alcance estrecho —**una** pregunta, respondida **muy** bien— es lo que hace posible la ejecución profunda, y la ejecución profunda es lo único que distinguirá a Portix de un script de bash de treinta líneas.

Las tres apuestas del proyecto, por orden de riesgo:

1. **Que la resolución puerto → proyecto sea correcta el 99% del tiempo.** Si Portix miente, no sirve para nada. De ahí la obsesión con `(pid, starttime)`, la honestidad de la degradación y el panel de evidencia.
2. **Que los contenedores funcionen de verdad.** El escaneo por netns de v1 es la diferencia entre una herramienta para puristas y una herramienta para el mundo real (R-01).
3. **Que los detectores declarativos conviertan usuarios en contribuidores.** Es la única vía por la que un proyecto de un maintainer cubre cincuenta frameworks (§9.5, R-15).

Todo lo demás —la velocidad, la belleza de la TUI, el empaquetado— es ejecución conocida. Estas tres son donde se gana o se pierde.

---

*Fin del documento. Revisiones y RFCs vía issues en el repositorio.*
