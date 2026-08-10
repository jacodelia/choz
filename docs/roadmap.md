# choz — Pendiente

Qué falta. Lo hecho está en [CHANGELOG.md](../CHANGELOG.md), día por día; cómo
encajan las piezas, en [architecture.md](architecture.md).

Última actualización: 2026-08-09.

## Estado en una línea

Los seis formatos de plugin (CLAP, LV2, LADSPA, DSSI, VST2, VST3) se escanean,
se hostean y abren su ventana nativa; el rack es multi-slot con mixer, FX,
ruteo de entradas/salidas **canal por canal** (cualquier jack a cualquier jack)
y proyectos en YAML; hay tres capas contra el código
ajeno que revienta — escaneo fuera de proceso, cuarentena y sandbox, y tener
ventana basta para irse a un proceso aparte —; los parámetros se dibujan según
lo que el plugin dice que son (interruptor, enumerado, fader, banco vertical,
knob); hay transporte propio con compás y posición de compás, que leen VST2,
VST3, CLAP y LV2, y automatización contra ese reloj con la longitud del bucle
ajustable; una entrada de audio se puede convertir en notas (`A→M`) para tocar
un VST con la guitarra — `ftom` de Csound redondeado, una nota exacta por jack
mono, con trim, sensibilidad y lectura de lo que oye; el monitor tiene pestañas
MIDI / WAVE / ACTIVITY;
hay **AutoTune**, corrección de altura en tiempo real como efecto propio (YIN
decimado + PSOLA, sin allocations en el callback); todas las secciones comparten
un fondo traslúcido de color y opacidad configurables; y choz se instala con `.deb`, `.rpm` o `install.sh` y sale en el
menú del escritorio. **335 tests**, `clippy --workspace --all-targets -D warnings`
limpio.

## Pendiente

1. **Terminar de publicar la 1.0.0.** La versión está subida a `1.0.0`, el
   CHANGELOG cerrado con esa etiqueta y los artefactos de x86_64 construidos
   (`target/release/choz`, `target/debian/choz_1.0.0-1_amd64.deb`,
   `target/generate-rpm/choz-1.0.0-1.x86_64.rpm`). Lo que falta:
   - **Commit, tag `v1.0.0` y push** — el árbol sigue sin commitear a propósito;
     el push del tag es lo que dispara `.github/workflows/release.yml`.
   - **El binario aarch64**: `cross build --release --bin choz --target
     aarch64-unknown-linux-gnu` (compila aquí en ~1 min; no se llegó a correr en
     esta tanda).
   - **Mirar con ojos lo que sólo se ve instalado**: el `.desktop` en el menú, el
     icono, el lanzador abriendo kitty al tamaño correcto, un `*.choz.yml` con
     doble clic, y un plugin sincronizado siguiendo la fila `Tempo`.
   - **armv7 sin verificar**: aquí `cross` no pudo bajarse el toolchain (red del
     contenedor cortada). aarch64 sí compila.

   **Éste es el único punto que queda por decisión**: lo demás de esta lista ya
   no es código, es hardware que hay que tener delante.

2. **Lo que sólo se comprueba con el hardware delante.** No queda código
   pendiente aquí: queda mirar.
   - **Las entradas**: los tests corren sin cliente JACK, así que
     `all_capture_ports()` devuelve cero y las filas de canal no existen para
     ellos. Falta ver que las ocho entradas de la UMC1820 salgan bajo su
     tarjeta junto al micro del portátil y la otra placa, que el jack 5 sea el
     jack 5 (`in_order` ordena por el número final del nombre del puerto, y no
     todos los nombres terminan en número), y que registrar ~20 puertos de
     captura no cueste nada en el callback.
   - **`A→M` con un instrumento de verdad delante.** El detector está verificado
     con tonos, con tonos armónicos (fundamental más flojo que el segundo y el
     tercer parcial, como oye una pastilla de puente) y con un vibrato de ±35
     cents que sale como una sola nota sostenida; entra en presupuesto (decimado
     a 16 kHz, un análisis cada 8 ms). Lo que falta es
     mirar la lectura del botón (` A→M● E2-14`) con la señal real puesta:
     cuánto hay que subir `IN` para unos auriculares, dónde queda `SENS` contra
     el ruido de una single-coil, y si 24 ms de espera (`STEADY_ANALYSES`) se
     sienten como retardo tocando rápido.
   - **AutoTune con una voz de verdad por un micrófono de verdad.** El DSP está
     medido (cero allocations, ~10 % del presupuesto de búfer, 33 ms de
     latencia) y probado contra señales sintéticas, que son más amables que una
     habitación. Falta cantarle: si `Retune` a 120 ms se siente natural, si el
     gate a -50 dBFS aguanta el ruido de sala, y si las formantes se sostienen
     en una corrección de más de un tono.
   - **La ESP32-S3 táctil** (`examples/esp32s3-touch/`, hecha como superficie de
     control): flashearla, mirar el panel y medir el retardo de un toque a la
     nota. **No existen versiones del S3 con Linux** — es un Xtensa LX7 sin MMU,
     sin `dlopen`, con ESP-IDF/FreeRTOS y LVGL; hostear plugins *es* cargar
     código nativo en tiempo de ejecución, así que la placa manda y choz hostea.
   - **En una Pi**: que ALSA/JACK abren con buffers pequeños sin xruns, y que el
     escaneo encuentra algo — **los plugins son binarios nativos**, así que una
     Pi sólo carga plugins compilados para ARM, no los `.so` de x86.

## Notas / gotchas para el que retome

- **Ningún cajón hace scroll**, y ahora el cajón IN lista **todos** los puertos de captura del sistema (una UMC1820 son ocho, más el micro del portátil, más la otra tarjeta), con una cabecera por tarjeta. En una terminal normal entra; en una baja las últimas filas quedan fuera y no hay forma de llegar a ellas. `draw_output_panel` y `draw_input_panel` pintan un `Paragraph` entero sin desplazamiento — y los rectángulos de clic tendrían que moverse con él, que es el trabajo de verdad. **Es lo primero que va a doler.**
- **`in_pair` es un índice en esa lista plana de puertos.** Si se desenchufa una tarjeta, los índices se corren y un proyecto guardado apunta a otro jack. Guardar el nombre del puerto en el proyecto lo arreglaría; hoy la respuesta honesta es volver a asignar.

- **Tener ventana manda el plugin al sandbox, aunque el probe lo vea sano.** `quarantine::check` devuelve `Report{verdict, editor}` y `wants_sandbox` mira las dos cosas, así que en esta máquina casi todo lo que tiene GUI (Zam, guitarix, u-he) pasa a correr fuera de proceso. Si algo suena distinto o el rendimiento cambia, ésa es la razón: `CHOZ_SANDBOX_GUI=0` la apaga. El probe **pregunta** por el editor (`editor()`, sólo construye el mango) y nunca lo abre — abrir ventanas en el probe es lo que cuelga los barridos.
- **El transporte es global al proceso** (`choz_ports::transport()`), y lo avanza el callback de audio en `render()`. Si algún día hay dos motores en un proceso, ese es el sitio que hay que cambiar; hoy hay uno, y `audioMasterGetTime` de VST2 no tiene por dónde recibir contexto.
- **Un barrido de UIs bajo Xvfb no prueba nada sobre memoria compartida.** LSP Room Builder (Mono y Stereo) mata al probe con `BadMatch` en `MIT-SHM X_ShmPutImage` ahí, y abre sin problema en el X real. Antes de apuntar un plugin como roto, repetirlo en `:0` — un plugin, una ventana.
- **Preguntar por la ventana de una UI justo después de `open()` da una moneda al aire.** Varios toolkits la crean en la primera vuelta de su bucle; `ui_probe` bombea `idle` hasta 500 ms esperándola. Las "UIs sin ventana" de barridos viejos eran eso.
- **El directorio de estado de los tests de UI es por proceso, no por test** (`XDG_STATE_HOME` es una variable de entorno, global). Un test que guarde `ui.json` se lo pasa al siguiente por `App::new()`; `sandbox_state_dir()` lo borra al empezar. Si aparece un test de UI que falla una vez de cada cinco, mirar ahí antes que al render.
- **Knob, fader o banco lo decide la unidad del plugin, no el nombre del parámetro** (`source::FADER_UNITS`, `fx_chain_panel::fader_groups`: tres o más faders seguidos con la misma unidad son un banco vertical). LV2 llama `pc` al porcentaje; `ms` y `pc` son las dos unidades más comunes de las 21 291 que declaran los puertos instalados aquí. Un plugin sin unidad se queda en knob a propósito.
- **Los temas de Gogh son datos, no código**: `crates/choz-ui/src/gogh_themes.txt` (`nombre|texto|escritorio`) entra por `include_str!` y se parsea en `settings::gogh_themes()`. Para actualizarlos: bajar `data/themes.json` de Gogh-Co/Gogh y regenerar el fichero (el marco se deriva, no viene en el JSON). La lista de la pestaña THEME va ordenada y los controles de escritorio van **arriba** — con 372 filas, lo que quede debajo de la lista no se alcanza.
- **Las UIs de guitarix matan el proceso, medido con el barrido entero** (nueve rebanadas, nueve `gx_*`, SIGSEGV). El probe levanta la deny-list a propósito; choz no, y el sandbox se las queda porque tienen ventana.
- **La deny-list de UIs es propiedad del proceso, no del plugin.** `choz-plugin-lv2::allow_denied_uis(true)` la levanta, y el único sitio que lo hace es el hijo del sandbox: ahí una UI que segfaultea cuesta un proceso que el supervisor repone. Llamarlo en el proceso de choz devuelve el crash que la lista evita.
- **El host no puede ver si un plugin sandboxeado tiene ventana**: cuando captura el mango, el hijo todavía está cargando. Por eso el hijo publica `editor_present` en la cabecera **antes de servir su primer bloque**, y `SandboxedPlugin::editor()` sólo ofrece botón `GUI` si dice que sí. Cualquier cosa que el host necesite saber del plugin sigue ese mismo camino.
- **Los probes de editores abren ventanas de verdad.** `examples/ui_probe` (LV2) y `examples/gui_probe` (CLAP) instancian plugins y abren su GUI: usar `Xvfb` (o la ventana padre sin mapear, que es lo que hacen ahora) y **matarlos al terminar**. `sweep.sh` reanuda tras cada segfault por diseño, así que colgado sigue insistiendo indefinidamente. Ningún test abre ventanas, y así debe seguir — `vst2_runtime.rs` lo dice explícitamente donde toca un editor.
- **En VST3, la GUI no habla con el procesador.** El edit controller reporta al host (`IComponentHandler::performEdit`) y es el host quien lleva el valor al procesador por `inputParameterChanges`. Un host que no lo hace tiene knobs que se mueven sin sonar. Lo mismo con los ids: `getParameterInfo` toma un **índice** y devuelve un **id arbitrario**; confundirlos mueve otro parámetro.
- **Un valor que no se guarda tampoco se puede editar.** El cap de 7 parámetros truncaba la lista *al construirla*, no sólo al dibujarla: lo que no está en el `Vec` no viaja al proyecto.
- **Una nota-off tiene que ir a donde fue su nota-on.** El ruteo depende de la pestaña activa, así que resolverlo dos veces (una al pulsar, otra al soltar) deja notas colgadas en cuanto el usuario cambia de tab. `App.sounding` es la memoria; `PANIC` es la salida de emergencia.
- **Un fondo de celda es opaco, y va por encima de la imagen del protocolo gráfico.** De ahí las dos reglas: en halfblocks la transparencia se mezcla en las celdas (fg *y* bg, o se pierde la mitad de la resolución); bajo kitty **no se tocan las celdas** y el lavado es una segunda imagen con alfa.
- **Un binario de plugin puede publicar cientos de descriptores.** LSP tiene ~390 UIs en un `.so`. Cualquier bucle `for i in 0..N` sobre `lv2_descriptor`/`lv2ui_descriptor` con N fijo miente en silencio: se recorre hasta el primer nulo, y el tope existe sólo contra un binario roto.
- **"No carga" y "no tiene ventana" son respuestas distintas.** Un probe que las imprime igual esconde bugs: dos sesiones creyendo que LSP no ofrecía editores cuando en realidad no las estábamos encontrando.
- **La primera explicación de una medición rara suele ser falsa.** "Un proceso que abre cientos de UIs se queda sin recursos" sonaba bien; barrer en rebanadas de procesos frescos dio los mismos números y la tiró en cinco minutos. Medir la hipótesis cuesta menos que escribirla en el roadmap como si fuera un hecho.
- **El fondo por protocolo gráfico depende del `z`**: por debajo de -1073741824 la imagen queda bajo los fondos de celda (lo que choz quiere); con `z=-1` taparía paneles y resaltados. Y `ratatui-image` no vale para esto: coloca por placeholders Unicode, que el texto de encima borra.
- **Un puntero COM prestado no se envuelve en `ComPtr`.** `ComPtr` libera al soltarse, así que cada uso le quita una referencia a un objeto que es del plugin (VST3: los handlers del run loop). Para eso está `ComRef`. Vale para cualquier callback que reciba punteros ajenos.
- **Un probe que consume el objeto bajo prueba mide otra cosa.** `.and_then(|i| i.editor())` dropea el plugin antes de usar el editor, y el `Drop` vacía la celda compartida: las llamadas salen por la rama "instancia muerta" sin decir nada. Costó una conclusión equivocada entera sobre CLAP.
- **stdout a un archivo va en bloques.** Un resultado "impreso" pero no volcado se pierde si el proceso siguiente segfaultea. `ui_probe::say()` imprime y hace flush; sin eso una corrida perdió 74 resultados y el total pareció limpio.
- **El fondo se dibuja antes que nada en `ui()`**, y depende de que los widgets no fijen `bg`. Cualquier panel nuevo debe usar `theme::panel_style()`, no una constante de color ni `Color::Reset`, o abrirá un agujero opaco en el wallpaper.
- **`Color::Reset` no es transparente.** Es SGR 49 — el fondo por defecto del terminal — y pinta encima de lo que haya. Lo único que deja el buffer intacto es no fijar `bg` en absoluto, que es por qué `panel_style()` devuelve un `Style` y no un `Color`.
- **Sandbox por plugin, no por tab**: `quarantine::forced` se guarda por `formato|ruta|id` en `<state dir>/plugin-sandbox.json`; el toggle del RACK sólo se ve al reinstanciar, y por eso el botón dice `(reload)` mientras tanto. `SandboxStatus` se captura junto a `editor()` — si aparece otro sitio que crea slots, hay que capturarlo ahí también.
- **Sync engine↔UI slots**: `AudioEngine.slot_count` y `App.slots` se mantienen en el mismo orden (append/remove espejados). No romper ese invariante.
- **Working copy**: `App.{source, fx_chain, fx_slot, fx_param}` son la copia viva del `active_slot`; se persisten a `App.slots[active]` en `persist_active()` al cambiar de tab / borrar. Los handlers de FX operan sobre la copia y llaman `rebuild_fx()` → `engine.set_slot_fx(active_slot, …)`.
- **`target/` es efímero en el sandbox**: correr build+test en una sola invocación para ver el binario en `target/debug/choz`.
- **Ruteo**: se resuelve en la UI (`note_targets`), no en el engine. Si algún día hace falta que el RT rutee, hay que meterle el binding — hoy no lo tiene a propósito.
- **`midi_connected` vs `midi_ports`**: `InputSource::Midi(i)` indexa `midi_connected` (lo que devolvió `connect_inputs`, en ese orden exacto); `midi_ports` es "todo lo que se vio". No confundirlos.
- **Cambiar de output device pierde los slots del engine** (se dropea el stream); `App::set_output_device` los recrea. Cualquier estado que solo viva en el engine hay que recrearlo ahí también.
- **Canal de notas único**: `App.note_tx/note_rx` se crea una vez al arrancar; `connect_midi()` clona el sender. No volver a crear el canal al reconectar MIDI (dejaría huérfano el hilo OSC).
- **Mixer**: `RackSlot.{gain,pan,mute,solo}` NO están en la working copy; viven solo en `slots[i]` y se empujan con `push_mix()` (que resuelve el solo). El engine no conoce el solo.
- **Teardown de plugins CLAP**: por defecto se filtran a propósito (ver arriba). Si aparece un leak raro o quieres medir memoria, corre con `CHOZ_CLAP_STRICT_TEARDOWN=1` — y prepárate a que un plugin roto tire la app.
- **Verificar la TUI sin terminal**: `(sleep 5; printf '\r') | script -qec "stty rows 45 cols 170; timeout 10 ./target/debug/choz" /dev/null > out`, luego quitar ANSI con `re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]','',...)`. **Ojo**: ratatui hace redibujo incremental, así que las líneas completas del scrape suelen ser de frames viejos.
- **`registry.rs`/`scanner.rs`/`plugin_types.rs`**: infra vieja, en gran parte stub (`#[allow(dead_code)]`). El camino real son los crates `choz-plugin-*`. Decidir si se borra.

## Comandos útiles

```bash
cargo build --workspace                 # todos los hosts van en el build normal
cargo test --workspace
# barridos largos: hostear TODOS los plugins instalados de un formato
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --bin choz          # necesita una terminal real (tty)
tail -f ~/.local/state/choz/choz.log    # ver errores/log en vivo

# Probes de editores: INSTANCIAN PLUGINS Y ABREN SU GUI. Los de LV2 y VST3
# crean la ventana padre sin mapear (no ensucian el escritorio); `--mapped`
# reproduce lo que hace choz de verdad, y entonces conviene un display aparte.
cargo run -p choz-plugin-lv2  --example ui_probe            # --limit N, --skip N
cargo run -p choz-plugin-vst3 --example gui_probe
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run -p choz-plugin-clap --example gui_probe
DISPLAY=:99 cargo run -p choz-plugin-lv2  --example ui_probe -- --mapped

# Tests de runtime con los instrumentos VST2 del usuario (los directorios
# estándar sólo tienen efectos).
CHOZ_VST2_DIR=/ruta/a/tus/vst cargo test -p choz-plugin-vst2
```
