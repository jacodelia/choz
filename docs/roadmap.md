# choz — Pendiente

Qué falta. **Lo cerrado no vive aquí**: está en [CHANGELOG.md](../CHANGELOG.md),
día por día, con los porqués y lo último arriba; cómo encajan las piezas, en
[architecture.md](architecture.md). Este documento se poda cuando un punto se
cierra, para que lo que quede sea sólo lo que queda — el 2026-08-19 se podó
entero: lo que decía "hecho" o "cerrado" se fue al changelog.

Última actualización: 2026-08-19.

## Estado en una línea

Los seis formatos de plugin (CLAP, LV2, LADSPA, DSSI, VST2, VST3) se escanean,
se hostean y abren su ventana nativa; el rack es multi-slot con mixer, FX,
ruteo canal por canal y proyectos en YAML; entra audio en vivo por JACK (cada
jack del grafo) y por ALSA/PulseAudio/PipeWire (un dispositivo de captura
elegible en Settings), así que también es un multiefecto; hay tres capas contra
el código ajeno que revienta —escaneo fuera de proceso, cuarentena y sandbox—;
hay transporte propio con compás, automatización contra ese reloj, `A→M` (audio
a notas), AutoTune y un arpegiador por tab; 45 efectos
propios (**la suite está completa**), que además se publican como un `.clap`
para usarlos en cualquier otro host; un patch de Max se importa hasta donde se
puede, diciendo qué no; hay guardia de acople en la entrada; el mixer tiene un
main y cuatro subgrupos; una tab guarda sus sonidos en botones y puede partir el
teclado entre ellos; y un efecto puede abrirse con el bombo de otra tab. **La 1.0.0 está publicada y sus paquetes
verificados; la 1.3.0 es este árbol, con trabajo sin publicar encima.**
602 tests, `clippy --workspace --all-targets -D warnings` limpio.

Las comprobaciones con hardware delante quedaron dichas en los gotchas, que es
donde se van a leer.

---

## Pendiente

Nada de lo pedido el 2026-08-19 queda abierto como punto: los cinco están
cerrados y contados en el changelog. Lo que sigue son los bordes que cada uno
dejó dichos, y la pieza que se decidió no hacer.

### Fuera de la lista, por decisión del 2026-08-19

La sección **artifact** — arpegiador + piano roll de 128 pasos portado de
seqterm. Es la pieza más grande de las pedidas, no compite con las demás por
tiempo, y se replantea aparte.

### Bordes que quedaron abiertos

| # | Qué falta | Dónde | Por qué no se hizo |
|---|-----------|-------|--------------------|
| 1 | **Medidor por bus** | MIXER | Los grupos tienen fader y mute; el nivel que llevan no se ve. `SlotLevels` ya publica por tab, así que es sumar por bus y dibujar. |
| 2 | **Strips de grupo desde el teclado** | MIXER | Se manejan con el mouse; el teclado del MIXER sigue siendo el de las tabs. |
| 3 | **Gates disparados por nota** | FX | Hoy la fuente es el **nivel de audio** de la otra tab. Para un bombo de SF2 da igual; para un pad silencioso que igual manda notas, no. |
| 4 | **Un bloque de retraso en el gate** | FX | Una tab que se renderiza después de la gateada llega un bloque tarde (< 3 ms). Se arregla ordenando los slots por dependencia, que es más máquina de la que el problema pide. |
| 5 | **El split cambia el patch, no superpone dos** | SOUNDS | Una tab es un instrumento. Instantáneo en SF2 (program change), con el costo de un `set_slot_state` en plugins. Dos sonidos a la vez son dos tabs, que es lo que MULTI hace. |
| 6 | **Secciones de parámetros en VST3** | RACK | VST3 tiene `unitId` + `IUnitInfo`, que es la respuesta del plugin. Hoy VST3 cae en la heurística por nombre, que para Surge XT acierta — pero es una heurística. |

### Y lo de siempre

**Mirar con el equipo delante.** Léase el primer punto de los gotchas antes de
dudar de un número: todo el DSP está verificado contra señales sintéticas, no
contra una habitación.

## Notas / gotchas para el que retome

- **FluidSynth-DSSI no se puede instanciar dos veces en un proceso.** Arrastra
  libinstpatch, cuyo registro de tipos de GLib no sobrevive a que se destruya la
  primera instancia: la segunda construcción **segfaultea**, y antes de eso GLib
  avisa con `cannot register existing type 'IpatchConverter'`. Descubierto
  porque la suite entera empezó a caerse de forma intermitente el 2026-08-17.
  Lo que se hizo: `choz-plugin-ladspa` ahora mantiene mapeadas las bibliotecas
  para la vida del proceso (`LOADED_LIBS`, calcado de `choz-plugin-lv2`, que
  tuvo el mismo problema con los plugins Qt de Rui), y la suite construye cada
  plugin DSSI **una sola vez**. Lo que **no** está resuelto: cargar
  FluidSynth-DSSI en un tab, quitarlo y volver a cargarlo sigue siendo el mismo
  camino. La red que ya existe para esto es la cuarentena y el sandbox, pero la
  sonda instancia el plugin **una** vez en un hijo, así que no lo detecta —
  haría falta que probara dos.
- **Un CC ahora sabe de qué controlador vino** (2026-08-17). Una asignación de
  MIDI learn es `(fuente, CC, destino)`, no `(CC, destino)`: en un escenario con
  dos teclados los dos mandan CC 1, y sin la fuente el mod wheel de uno movía lo
  que se había asignado con el otro. La regla, que es la misma que ya seguían
  las notas: **cada controlador maneja las tabs asignadas a él**, esté en
  pantalla la que esté; **sólo cuando varias tabs comparten el mismo puerto**
  decide la tab activa (y antes que ella, un canal reclamado). Dos asignaciones
  pueden compartir un CC si mueven **tabs distintas** o unidades de FX
  distintas; en cualquier otro caso la nueva reemplaza a la vieja. Las
  asignaciones guardadas antes de esto no tienen fuente y siguen respondiendo a
  cualquiera — hay que volver a aprenderlas para que se separen por teclado.
- **Un DSSI que no aparece como instrumento casi seguro exporta
  `run_multiple_synths` y no `run_synth`.** Fue el caso de FluidSynth-DSSI, que
  durante meses se listó como efecto y no se podía cargar. Y un DSSI que carga
  pero no suena casi seguro espera un `configure`: FluidSynth-DSSI no tiene
  **nada** hasta que se le da `load=<sf2>`, y WhySynth sonaba en silencio
  simplemente porque no tenía ningún programa seleccionado. Los tres de esta
  máquina (`/usr/lib/dssi`) cubren las dos formas del formato y son con lo que
  se probó todo.
- **Todo el DSP está verificado contra señales sintéticas, no contra una
  habitación.** Las comprobaciones con hardware delante —la interfaz apagada y
  encendida, los ocho jacks de la UMC1820, la deriva de dos relojes reales, el
  acople con un micro, `A→M` con una guitarra, AutoTune con una voz, la ESP32 y
  la Pi— se cerraron sin hacerse (2026-08-15, decisión del usuario). Lo que eso
  significa: cuando algo suene raro con el equipo delante, **es la primera
  hipótesis**, no la última. Los números que hay que mover están señalados en
  cada sitio (`GROWTH_CHECKS` y `DUCK` en `feedback.rs`, `SENS`/`IN` en el
  rack, `STEADY_ANALYSES` en `pitch.rs`, `MEDIAN_ANALYSES` en el detector de
  AutoTune).
- **No hay "sección de algoritmos".** La hubo durante una tarde —una lista por
  tab con el arpegiador y los patches de Pd que sacan notas— y se quitó entera
  el 2026-08-16 a petición del usuario: **queda el arpegiador y nada más**. Con
  ella se fueron el trait `InputAlgorithm`, el driver que movía un patch desde
  el bucle de interfaz y la vuelta de notas del puente del sandbox, porque
  nada más las usaba. Un patch de Pd es un **efecto**, y punto.
- **Decisiones cerradas que no se vuelven a discutir sin una razón nueva**:
  `A→M` es independiente y tiene su propio interruptor (lee audio, y el audio
  sólo existe en el callback); la tabla de equivalencias de Max es corta a
  propósito y se alarga con un patch real delante; un `.pd` necesita `adc~`
  **y** `dac~`; el dispositivo de audio no cambia solo **nunca**; JSFX no
  existe en choz.
- **Lo que se instala con choz**: sus 45 efectos como `.clap` (`~/.clap` desde
  el instalador, `/usr/lib/clap` desde los paquetes), los wallpapers en
  `share/choz/wallpapers` —una instalación nueva abre con el que trae— y
  `choz-pd-host` cuando hay libpd. `--no-clap` para quien no quiera el plugin.

- **Un patch de Pd sin símbolos de recepción en sus sliders no suena, y no es
  un fallo de choz.** Un `hsl` recién puesto en Pd vale cero y no se puede
  direccionar desde fuera: el patch queda multiplicado por cero. choz nombra
  esos controles en el log al cargarlo. Los que sí tienen símbolo salen como
  knobs. Y ojo: **Pd no crea un `hsl` con campos de menos**, aunque el lector de
  choz lo acepte.
- **Los cajones IN y OUT hacen scroll** (ya no es el gotcha que era): `drawer::{list_height, list_scroll}` calculan la ventana visible **a partir del cursor**, sin offset guardado, y las llaman tanto el dibujo como los rects de clic — que es lo que impide que se desvíen. La rueda del ratón mueve el cursor. Si aparece otra lista larga en un panel, ésa es la pieza a reusar.
- **Un rect de clic no se calcula con offsets a mano.** Es la raíz del bug de los botones de banco: cualquier texto anterior en la línea puede estar traducido, y entonces el rect apunta a otra columna. Los rects salen de las anchuras reales de los spans (`Span::width`, no `chars().count()`, que miente con CJK).
- **`in_pair` es un índice en esa lista plana de puertos**, y esa lista se mueve: desenchufa una tarjeta y todos los índices posteriores se corren. Por eso un proyecto guarda además **el nombre de los dos jacks** (`Mixer.in_ports`) y al abrir manda el nombre; si el jack ya no está, la tab se queda **sin entrada de audio** en vez de escuchar el micro de otro — `resolve_in_pair`. Dentro de una sesión el índice sigue siendo la moneda.
- **Tener ventana manda el plugin al sandbox, aunque el probe lo vea sano.** `quarantine::check` devuelve `Report{verdict, editor}` y `wants_sandbox` mira las dos cosas, así que en esta máquina casi todo lo que tiene GUI (Zam, guitarix, u-he) pasa a correr fuera de proceso. Si algo suena distinto o el rendimiento cambia, ésa es la razón: `CHOZ_SANDBOX_GUI=0` la apaga.
- **El transporte es global al proceso** (`choz_ports::transport()`), y lo avanza el callback de audio en `render()`. Si algún día hay dos motores en un proceso, ese es el sitio que hay que cambiar.
- **`FxProcessor` es audio y sólo audio.** No hay salida de notas en la cadena de FX; cualquier cosa que *genere* MIDI vive en la UI (donde está el ruteo) o en el engine junto al transporte.
- **Un barrido de UIs bajo Xvfb no prueba nada sobre memoria compartida.** LSP Room Builder mata al probe con `BadMatch` en `MIT-SHM X_ShmPutImage` ahí, y abre sin problema en el X real. Antes de apuntar un plugin como roto, repetirlo en `:0`.
- **Preguntar por la ventana de una UI justo después de `open()` da una moneda al aire.** Varios toolkits la crean en la primera vuelta de su bucle; `ui_probe` bombea `idle` hasta 500 ms esperándola.
- **El directorio de estado de los tests de UI es por proceso, no por test** (`XDG_STATE_HOME` es una variable de entorno, global). `sandbox_state_dir()` lo borra al empezar. Si aparece un test de UI que falla una vez de cada cinco, mirar ahí antes que al render.
- **El idioma y el color son globales del proceso**: un test que los cambie tiene que sostener `ui_guard()` (reentrante por hilo) durante todo el test, no sólo mientras dibuja, y devolverlos con `UiRestore`.
- **Knob, fader o banco lo decide la unidad del plugin, no el nombre del parámetro** (`source::FADER_UNITS`, `fx_chain_panel::fader_groups`). Un plugin sin unidad se queda en knob a propósito.
- **Los temas de Gogh son datos, no código**: `crates/choz-ui/src/gogh_themes.txt` entra por `include_str!`. Para actualizarlos: bajar `data/themes.json` de Gogh-Co/Gogh y regenerar el fichero.
- **Las UIs de guitarix matan el proceso** (nueve rebanadas, nueve `gx_*`, SIGSEGV). El probe levanta la deny-list a propósito; choz no, y el sandbox se las queda porque tienen ventana.
- **La deny-list de UIs es propiedad del proceso, no del plugin.** `choz-plugin-lv2::allow_denied_uis(true)` la levanta, y el único sitio que lo hace es el hijo del sandbox.
- **El host no puede ver si un plugin sandboxeado tiene ventana**: por eso el hijo publica `editor_present` en la cabecera **antes de servir su primer bloque**.
- **Los probes de editores abren ventanas de verdad.** `examples/ui_probe` (LV2) y `examples/gui_probe` (CLAP) instancian plugins y abren su GUI: usar `Xvfb` (o la ventana padre sin mapear) y **matarlos al terminar**. Ningún test abre ventanas, y así debe seguir.
- **En VST3, la GUI no habla con el procesador.** El edit controller reporta al host (`IComponentHandler::performEdit`) y es el host quien lleva el valor al procesador por `inputParameterChanges`. Y `getParameterInfo` toma un **índice** y devuelve un **id arbitrario**: confundirlos mueve otro parámetro.
- **Un valor que no se guarda tampoco se puede editar.** El cap de 7 parámetros truncaba la lista *al construirla*, no sólo al dibujarla.
- **Una nota-off tiene que ir a donde fue su nota-on.** `App.sounding` es la memoria; `PANIC` es la salida de emergencia.
- **Un fondo de celda es opaco, y va por encima de la imagen del protocolo gráfico.** En halfblocks la transparencia se mezcla en las celdas (fg *y* bg); bajo kitty el lavado es una segunda imagen con alfa.
- **Un binario de plugin puede publicar cientos de descriptores.** LSP tiene ~390 UIs en un `.so`: recorrer hasta el primer nulo, nunca con un N fijo.
- **"No carga" y "no tiene ventana" son respuestas distintas.** Mezclarlas escondió un bug dos sesiones.
- **La primera explicación de una medición rara suele ser falsa.** Medir la hipótesis cuesta menos que escribirla en el roadmap como si fuera un hecho.
- **El fondo por protocolo gráfico depende del `z`**: por debajo de -1073741824 la imagen queda bajo los fondos de celda. `ratatui-image` no vale para esto.
- **Un puntero COM prestado no se envuelve en `ComPtr`** (libera al soltarse): para eso está `ComRef`.
- **stdout a un archivo va en bloques.** Un resultado "impreso" pero no volcado se pierde si el proceso siguiente segfaultea.
- **El fondo se dibuja antes que nada en `ui()`**, y depende de que los widgets no fijen `bg`. Todo panel nuevo usa `theme::panel_style()`. **`Color::Reset` no es transparente**: es SGR 49 y pinta encima.
- **Sandbox por plugin, no por tab**: `quarantine::forced` se guarda por `formato|ruta|id`; el toggle del RACK sólo se ve al reinstanciar.
- **Sync engine↔UI slots**: `AudioEngine.slot_count` y `App.slots` se mantienen en el mismo orden. No romper ese invariante.
- **Working copy**: `App.{source, fx_chain, fx_slot, fx_param}` son la copia viva del `active_slot`; se persisten en `persist_active()`.
- **`target/` es efímero en el sandbox**: correr build+test en una sola invocación.
- **Ruteo**: se resuelve en la UI (`note_targets`), no en el engine.
- **`midi_connected` vs `midi_ports`**: `InputSource::Midi(i)` indexa `midi_connected`; `midi_ports` es "todo lo que se vio".
- **Cambiar de output device pierde los slots del engine**; `App::set_output_device` los recrea con `rebuild_rack()`.
- **Canal de notas único**: `App.note_tx/note_rx` se crea una vez al arrancar.
- **Mixer**: `RackSlot.{gain,pan,mute,solo}` viven sólo en `slots[i]` y se empujan con `push_mix()`. El engine no conoce el solo.
- **Teardown de plugins CLAP**: se filtran a propósito; `CHOZ_CLAP_STRICT_TEARDOWN=1` para medir memoria.
- **Verificar la TUI sin terminal**: `(sleep 5; printf '\r') | script -qec "stty rows 45 cols 170; timeout 10 ./target/debug/choz" /dev/null > out`, luego quitar ANSI. Ratatui redibuja incremental: las líneas completas suelen ser de frames viejos.

## Comandos útiles

```bash
cargo build --workspace                 # todos los hosts van en el build normal
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --bin choz          # necesita una terminal real (tty)
tail -f ~/.local/state/choz/choz.log    # ver errores/log en vivo

# lo que cuesta el detector de A→M dentro del callback (sube = xruns en TODO
# el grafo, no sólo en choz)
cargo test --release -p choz-engine --lib -- --ignored --nocapture what_one_block

# Pure Data: el lector de patches siempre; el camino con libpd sólo con la
# feature (necesita libpd-dev instalado).
cargo test -p choz-plugin-pd
cargo test -p choz-plugin-pd --features pd
# El hijo que corre los patches, y la prueba de punta a punta que lo usa. Sin
# construirlo, el test se salta diciéndolo y choz no ofrece efectos de Pd.
cargo build -p choz-plugin-pd --features pd
cargo test -p choz-engine --test pd_patch

# barridos largos: hostear TODOS los plugins instalados de un formato
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored

# Probes de editores: INSTANCIAN PLUGINS Y ABREN SU GUI.
cargo run -p choz-plugin-lv2  --example ui_probe            # --limit N, --skip N
cargo run -p choz-plugin-vst3 --example gui_probe
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run -p choz-plugin-clap --example gui_probe

# Tests de runtime con los instrumentos VST2 del usuario.
CHOZ_VST2_DIR=/ruta/a/tus/vst cargo test -p choz-plugin-vst2

# Los efectos de choz como plugin CLAP: construir, probar y usar fuera.
cargo test -p choz-plugin-clap-export          # ABI en proceso + carga por dlopen
cargo build --release -p choz-plugin-clap-export
cp target/release/libchoz_plugin_clap_export.so ~/.clap/choz.clap

# Comprobar un paquete publicado sin instalarlo.
dpkg-deb -c choz_1.1.0-1_amd64.deb
strings usr/bin/choz | grep -c /home/       # tiene que dar 0
```
