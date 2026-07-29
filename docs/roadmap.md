# choz — Roadmap

Estado y pasos para continuar. Última actualización: 2026-07-29.

## Estado actual (lo que ya funciona)

- **Workspace 4 crates**: `choz-ports` (traits RT), `choz-engine` (audio/DSP/MIDI/registry), `choz-plugin-clap` (CLAP host, feature `clap`), `choz-ui` (binario `choz`). Ver [architecture.md](architecture.md).
- **Engine RT-safe**: callback sin locks ni alloc. Modelo **rack multi-source = `Vec<Slot>`** (`Slot{source, fx}`), mixer que suma todos los slots. Handoff por ring `EngineCommand` + ring `Retired` para drops fuera del RT.
- **Fuentes reales**: WAV (`hound`), SF2 (`oxisynth`), CLAP instrument (`clack-host`). Cada una = un slot/tab en el RACK, con su cadena FX propia.
- **32 FX DSP** built-in verificados (smoke test; los 27 originales + Protocosmos / Space Echo / Reverse Delay / Z5 Texture importados de seqterm + los pedales AMBER FANG / VELVET FUZZ) + **CLAP audio-effects** hosteados en la cadena FX (al final del modal ADD FX) **con sus parámetros reales**: nombres/rangos leídos del plugin (`read_params`), knobs normalizados 0..1, y cambios en vivo por `EngineCommand::SetFxParam` (no se reinstancia el plugin al mover un knob).
- **Mixer por slot**: gain, pan constant-power, mute y solo (solo se resuelve en UI → mute efectivo al engine). Teclas `-`/`+`, `,`/`.`, `m`, `S`; también rueda/click.
- **MIDI hardware** (`midir`): conecta todos los puertos menos los desactivados (`c` en el panel INPUTS), **ruteo por-entrada** (cada nota va solo a las tabs ligadas a esa entrada).
- **OSC** (`rosc`): listener UDP (9000 por defecto, `--osc-port N`). Notas (`/note`, `/note/on`, `/note/off`) + **control remoto**: `/mix/<tab>/{gain,pan,mute}` y `/fx/<tab>/<fx>/<param>`. Comparte el canal `flume` con MIDI (`InputEvent::{Note,Control}`).
- **SF2 presets**: `sources::list_sf2_presets` (via `soundfont`) lista los programas; `AudioSource::program_change` (RT-safe) los cambia por slot.
- **UI**: menú superior (F10/mouse), RACK con tabs por source (`[`/`]` o click), FX por-slot editable, piano QWERTY, About con imagen (`ratatui-image`), log a `~/.local/state/choz/choz.log`. **Todos los modales** (source, ADD FX, salida, bank/preset, MIDI learn, browser, params del instrumento) usan el mismo widget `views/modal.rs`: barra de desplazamiento, chips de filtro, botones SELECT/CANCEL y rueda/click de ratón.
- **Cache de scan CLAP**: `<state dir>/plugins.json`; `r` en SYNTH fuerza rescan. Ahorra ~236 ms de arranque.
- **Verificación**: `cargo build --workspace` + `--features clap` limpio, **128 tests** (135 con `--features clap`) (incluye tests de runtime contra plugins `.clap` y SoundFonts reales cuando están instalados; si no, se saltan), `clippy -D warnings` = 0. CI en `.github/workflows/ci.yml`.

## Modelo objetivo (flujo final pedido)

```
[SOURCE]  input seleccionado por el usuario (MIDI device, ej: Keystation Pro 88; luego OSC)
   │  (una o muchas fuentes; se selecciona con mouse o teclado)
   ▼
[RACK]  botón para elegir QUÉ acciona el source:
   │      · SF2 (SoundFont)
   │      · Audio sample (WAV disparado)
   │      · Synth plugin (VST / CLAP / LV2)
   ▼
[FX]  effect1 → effect2 → … → effect5   (máx 5, reordenables como seqterm)
   │      · built-in (los 27) u OPEN plugin de FX: LV2 / VST / CLAP-fx
   ▼
[OUT]  salida seleccionada por el usuario (device de audio)
```

Reglas:
- **SOURCE = solo entradas.** Elegir un input crea/selecciona la tab de RACK ligada a ese input.
- **Ruteo por-entrada** (cambia el OMNI actual): cada input acciona SOLO su propia cadena.
- **Kill tab**: botón `[x]` por tab (mouse + teclado) borra la tab con toda su config.
- **Click en un source** abre la config de rack cargada para ese canal.
- **Cache de plugins**: guardar el resultado del scan en disco para arrancar rápido.

## Pasos (ordenados por ROI / riesgo) — A y B hechas, queda C

### Fase A — wins acotados (HECHA)
1. ~~**Cache de scan de plugins**~~ **HECHO**: `choz-engine/src/cache.rs` (JSON en `<state dir>/plugins.json`, `serde_json`). `ClapPluginInfo` deriva `Serialize/Deserialize`. `AudioEngine::cached_clap()` (usa cache si ninguna carpeta de búsqueda es más nueva que el archivo) y `rescan_clap()` (fuerza). UI: `discover_synths(force)`, tecla `r` en la categoría SYNTH. Medido en esta máquina: el scan real cuesta **~236 ms / 20 plugins**, ahora solo en el primer arranque o tras instalar plugins.
   - `cache::state_dir()` es ahora la única fuente del directorio de estado; `choz-ui/src/log.rs` la usa para el log.
2. ~~**FX máx 5**~~ **HECHO**: `source::MAX_FX = 5`, usada en los 3 sitios (antes `< 8` en el selector y `< 5` en el dibujo).
3. ~~**Botón `[x]` (kill tab)**~~ **HECHO**: la `✕` va dentro de `tab_label()` (misma fuente para dibujo y hit-test), `rack_tab_close_rects` + `MouseAction::RackTabClose` → `App::remove_slot(idx)` (generalización de `remove_active_slot`: persiste la working copy antes de borrar, corrige `active_slot` si el borrado va antes, y re-empuja el mixer porque los strips son por índice).

### Fase B — restructure del modelo (HECHA)
4. ~~**SOURCE = solo inputs**~~ **HECHO**: el panel izquierdo es ahora **INPUTS** (`views/source_panel.rs` reescrito): arriba la lista de entradas (puertos MIDI + OSC) con marca de conexión y a qué tab está ligada cada una; abajo los presets SF2 del tab activo. `←`/`→` eligen qué lista mueve el cursor. `Enter` sobre una entrada crea (o salta a) la tab del RACK ligada a esa entrada; `c` conecta/desconecta; `r` reescanea. Las categorías MIDI/SF2/AUDIO/SYNTH desaparecieron.
5. ~~**Ruteo por-entrada**~~ **HECHO** (reemplaza OMNI). `choz-engine/src/input.rs`: `InputSource {Midi(idx), Osc, Keyboard}` + `NoteMsg{source,on,note,vel}`; cada conexión de `midi::connect_inputs` etiqueta sus eventos con su índice en la lista devuelta, y OSC con `Osc`. **El ruteo se resuelve en la UI** (`fn note_targets`, con tests): el engine solo recibe `NoteOn{slot,…}` / `NoteOff{slot,…}`, así que el hilo RT no sabe nada de puertos. El piano QWERTY siempre toca la tab activa.
6. ~~**RACK: botón instrument**~~ **HECHO**: una tab nueva arranca sin instrumento (source `Silence` en el engine, etiqueta `(empty)`). El RACK tiene una línea `INSTR <nombre> [1:SF2] [2:WAV] [3:SYNTH]`: teclas 1/2/3 o click abren el browser o el nuevo modal de synths. Cargar reemplaza el source del slot vía `EngineCommand::SetSlotSource` (el viejo se dropea fuera del RT), no crea otra tab.
7. ~~**OUT: selección de salida**~~ **HECHO**: `AudioEngine::output_devices()` / `output_device()` / `set_output_device(name)`; `pick_backend` acepta un nombre y cae al default si desapareció. El panel TRANSPORT muestra `OUT <device> [o=change]` y abre un modal. **Cambiar de device destruye el stream y con él los slots**: `App::set_output_device` reconstruye el rack desde el modelo de la UI (recarga SF2/WAV/CLAP y reaplica FX y mixer).

### Fase C — hosting nuevo (parcialmente hecha)
8. **FX plugins.**
   - ~~CLAP audio-effect~~ **HECHO**: `ClapEffect` (`host.rs`) implementa `FxProcessor`; `ClapProc` es la parte común con `ClapInstrument`.
   - ~~Parámetros propios del plugin~~ **HECHO**: `read_params()` (extensión `params`) da id/nombre/min/max/default; `AudioFxEntry` guarda esa lista y dibuja hasta `MAX_CLAP_PARAMS = 7` knobs + Wet; mover un knob manda `SetFxParam` al slot vivo (índice `FX_MIX_PARAM` = dry/wet de choz) en vez de reconstruir la cadena. Los valores se reaplican al reconstruir (`build_chain_from_specs`).
   - ~~Parámetros del CLAP *instrument*~~ **HECHO** (2026-07-28): `AudioSource::set_param` (nuevo método RT-safe en `choz-ports`, no-op por defecto) + `EngineCommand::SetSlotParam` + `AudioEngine::set_slot_param`. `ClapInstrument` guarda su lista de params y encola el cambio como el efecto (`queue_param` compartido). UI: `RackSlot.{instr_params, instr_values}`, tecla `p` en el RACK abre el modal **INSTRUMENT** (`draw_instr_editor`): lista con scroll (no hay tope de 7 como en los knobs de FX), `↑↓` param, `←→` valor (paso 0.05), muestra el valor en unidades del plugin. Los valores se reaplican al recrear los slots por cambio de output device.
   - **Falta**: LV2 (`livi`) y/o VST2 (portar de seqterm `seqterm-plugin-vst2`/`-lv2`). Es el trozo grande que queda.
   - **Falta**: paginar los knobs de FX si un plugin tiene más de 7 (el modal del instrumento ya no lo necesita).
9. ~~**OSC como entrada**~~ **HECHO**, incluido control remoto y `--osc-port N`.

### Bugs de hosting encontrados y arreglados (sesión 2026-07-27)
- **Layout de puertos**: se pasaba un único puerto estéreo fijo. Un plugin mono, o uno con sidechain (ZamCompX2 declara 2 puertos de entrada), rechazaba los buffers — DPF aborta con `assertion failure: "in == DISTRHO_PLUGIN_NUM_INPUTS"`. Ahora `port_layout()` lee **todos** los puertos y `in_buf`/`out_buf` son `[puerto][canal][frame]`; el puerto 0 lleva la señal y los demás van en silencio.
- **SIGSEGV al soltar un plugin**: `ClapProc` no guardaba el `PluginEntry`, así que la librería se dlclose-aba con la instancia viva. Ahora se guarda (y se dropea después de la instancia).
- **`ZaMaximX2` segfaultea dentro de su propio `deactivate`** (reproducido con un host clack mínimo, sin procesar nada: es bug del plugin). Para no perder la sesión, `Drop` **deja el plugin vivo a propósito** (stop_processing + `mem::forget` de processor/instance/entry). `CHOZ_CLAP_STRICT_TEARDOWN=1` hace el teardown correcto (deactivate + destroy) para depurar.
- **NaN del plugin**: `ZamEQ2` devuelve no-finitos antes de fijarle parámetros; `ClapEffect` descarta esas muestras en vez de mezclarlas hacia la salida.
- **Caché de plugins por tipo de build**: la caché escrita sin `--features clap` tiene metadata por nombre de archivo (todo parece instrumento) y hacía desaparecer los efectos CLAP del modal. El archivo ahora guarda `hosted: bool` y se ignora si no coincide con el build.
- Test nuevo `every_installed_effect_is_safe_to_host`: carga, procesa y dropea **todos** los efectos instalados (20 en esta máquina). Fue el que destapó los dos primeros bugs.

### Sesión 2026-07-28

- ~~**Verificación del OSC de control**~~ **CERRADA**, y sin pty: en vez de scrapear la terminal, el test `osc_mix_control_shows_up_in_the_rendered_mixer_strip` (en `main.rs`) aplica `ControlMsg::{Gain,Pan,Mute}` y **renderiza el panel RACK sobre un `TestBackend`**, comprobando que la franja dibuja `0.25` y `L80`. El redibujo incremental de ratatui deja de ser un problema porque el buffer se inspecciona entero.
- ~~**Parámetros del instrumento CLAP**~~ **HECHO** (ver Fase C arriba). Tests nuevos: `instrument_parameters_are_settable_while_playing` (runtime, contra el primer instrumento instalado: recorre sus params a 0/1/0.5 mientras suena y exige salida finita; índice fuera de rango ignorado) y `instrument_param_editor_draws_plugin_names_and_edits_values` (UI, `TestBackend`).
- **35 tests**, `clippy -D warnings` limpio con y sin `--features clap`.
- **No verificado a ojo**: en este sandbox el arranque cogió `none backend` (sin audio), así que el modal INSTRUMENT no llegó a probarse contra un Surge XT real cargado. Cubierto por tests, pero merece un vistazo en una terminal con audio.

### Sesión 2026-07-28 (bis) — import de FX + rework de modales

- **FX de seqterm importados y mergeados**: `crates/choz-engine/src/fx/` es ahora la versión de `seqterm-audio-engine/src/fx` (mismos DSP pero con doc-comments y **tests propios** — de ahí que el crate pase de 17 a 78 tests). Cuatro FX nuevos: **Protocosmos**, **Space Echo**, **Reverse Delay** y **Z5 Texture** (16 params), con ids `protocosmos`/`spaceecho`/`reversedelay`/`z5texture` en `fx_chain::build_processor` y entradas en `AudioFxKind` + `fx_param_descs`. **Ojo**: al copiar hubo que re-aplicar a mano el clamp de `Svf::new` (resonancia ≥1 = auto-oscilación) que choz había arreglado y seqterm no tiene; se borró el test de `mixer::Mixer` (no existe aquí) y se movió el `mod tests` de `parametric_eq.rs` por encima del `impl` (clippy `items_after_test_module`).
- **Un solo widget de modal** (`views/modal.rs`): `ListModal` + `draw_list_modal` dan **barra de desplazamiento**, chips de filtro, botones **SELECT/CANCEL** y los rects de click a *todos* los modales. `UiLayout` tiene un único `modal_rects`; `App.modal: Option<Modal>` con `ModalKind::{Source,AddFx,Device,Preset,Learn,Browser,InstrParams}` reemplaza a `fx_selector`/`synth_selector`/`device_selector`/`file_browser`/`instr_editor`. Un único `handle_modal_key` + `handle_modal_mouse`: rueda = scroll, click en fila = seleccionar, click otra vez (o SELECT) = confirmar, click fuera = cancelar.
- **Botón `[1:SOURCE]`** en la línea INSTR del RACK (sustituye a `1:SF2 2:WAV 3:SYNTH`): abre **CHANGE SOURCE / SYNTH** con filtros `ALL/CLAP/SF2/WAV/SFZ/LV2/VST3/DSSI`. CLAP sale del cache de plugins; SF2/WAV se escanean de `/usr/share/sounds/sf2`, `/usr/share/soundfonts`, `~/.local/share/sounds/sf2` y el cwd, más una entrada "Browse..." que abre el navegador de archivos (otro modal del mismo widget). Los formatos que choz aún no hostea salen vacíos a propósito.
- **Botón `[2:BANK/PRESET]`** (solo cuando el tab tiene SoundFont) abre la lista de programas en un modal; **el panel INPUTS ya no muestra la lista de presets** (era el pedido) y perdió su sub-foco `←/→`.
- **Botón `[3:MIDI LEARN]`**: elige el control del rack (VOL/PAN del tab o cualquier param de la cadena FX) y el **siguiente CC** que llegue queda ligado (`App.cc_bindings`). `midi.rs` ahora parsea `0xB0` → `InputEvent::Cc`; el panel INPUTS muestra el banner "MIDI LEARN: move a fader → …" mientras está armado, y el modal marca los controles ya ligados con `[CC n]`.
- Tests nuevos: `views::modal` (scroll + chips), `source_modal_filters_by_format`, `modal_rows_and_buttons_respond_to_the_mouse`, `midi_learn_binds_a_cc_then_drives_the_fader`, `instrument_param_modal_draws_plugin_names_and_edits_values`. **94 tests** (101 con `--features clap`), clippy `-D warnings` limpio en ambos.
- **Verificado en la TUI real** (pty, backend JACK vivo): Enter en INPUTS crea el tab, `1` abre CHANGE SOURCE, `→→` filtra a SF2, Enter carga `FluidR3_GM.sf2`, aparece `2:BANK/PRESET` y `2` abre la lista de programas.

### Sesión 2026-07-28 (ter) — rediseño del RACK + MIDI learn con puntero

- **El panel RACK calcula sus propios rects**: `draw_fx_chain_panel` devuelve un `RackLayout` (tabs, cierre de tab, VOL/PAN/MUTE/SOLO, botones, slots FX, knobs, ON/MOVE/DEL) que `ui()` guarda en `UiLayout.rack`. Desapareció el bloque de `compute_layout` que replicaba a mano los offsets — era la fuente crónica de desalineos entre dibujo y click.
- **Los knobs ya no se salen de pantalla**: `param_grid(width, n)` reparte los parámetros en una **rejilla que baja de línea** (Z5 Texture, 16 params, ocupa 3 filas). Si no caben todas las filas, la caja se desplaza siguiendo al cursor y el título muestra `(fila/total)`. Los botones de la cadena FX también **hacen wrap** a la línea siguiente en vez de salirse por la derecha.
- **Rediseño visual**: separadores con título (`── FX CHAIN ───`), la caja de parámetros es un `Block` con borde titulado `n:NOMBRE`, y **ON / ◀ MOVE / MOVE ▶ / DEL viven en su propia caja `SLOT`** una línea más abajo y con 3 espacios entre botones (antes iban pegados justo debajo de los knobs). Paleta unificada: etiquetas en gris azulado, knobs en azul, selección en amarillo, deshabilitado en gris apagado.
- **MIDI learn con puntero**: el botón `MIDI LEARN` (o `3`) activa el modo puntero — choz pide al terminal el reporte de movimiento (`\u{1b}[?1003h`, crossterm solo activa el drag) y pinta un **`?` amarillo sobre la posición del ratón**. El click elige el control (VOL, PAN o cualquier knob de la cadena) sin moverlo; el `?` sigue visible mientras espera el fader MIDI; en cuanto llega el CC se guarda el binding, se apaga el modo 1003 y **el ratón vuelve a su comportamiento normal**. `Esc` cancela. La tecla `l` sigue abriendo el modal de targets para quien va solo con teclado.
- Tests nuevos: `params_wrap_onto_more_rows_when_they_dont_fit`, `wide_fx_wraps_its_knobs_onto_more_rows`, `fx_chain_buttons_wrap_to_the_next_line`, `pointer_learn_picks_the_clicked_control_then_binds_the_cc`. **98 tests** (105 con `--features clap`), clippy `-D warnings` limpio.

### Sesión 2026-07-28 (quater) — bancos, MIDI learn de botones, rutas de plugins

- **Botones `◀ ▶` de banco** en el RACK, con el **nombre del programa actual** (`BANK  ◀  000:000 Black_Pearl_4pc  ▶`). `App::step_preset(±1)` mueve el cursor de presets y aplica el program change; el modal BANK/PRESET sigue disponible para saltar a uno concreto.
- **MIDI learn de botones**: `LearnTarget::Trigger(TriggerAction)` — MUTE, SOLO, BANK ◀/▶, FX ON/OFF, ◀MOVE, MOVE▶, ADD FX y cada botón de la fila FX CHAIN (`FxSelect(n)`). **DEL queda fuera a propósito** (nadie quiere borrar un efecto porque rozó un fader). Los triggers disparan en **flanco de subida** del CC (cruzar 64 hacia arriba), con `App.cc_last[128]` guardando el último valor por CC. Se asignan igual que los faders: botón MIDI LEARN → puntero `?` → click en el botón → mover el fader.
- **Fuera el diagrama `IN → fx1 → … → OUT`**: duplicaba los nombres de la fila de arriba.
- **Rutas de plugins tipo Carla** (`choz-engine/src/paths.rs`): `PluginFormat {LADSPA, DSSI, LV2, VST2, VST3, CLAP, SF2, SFZ, JSFX}` con sus directorios por defecto (`/usr/lib/...`, `/usr/lib64/...`, `/usr/local/lib/...`, `~/.lv2`, `~/.vst`, …) y respetando `LV2_PATH`/`VST_PATH`/`VST3_PATH`/`CLAP_PATH`/… si están definidas. `PluginPaths` se guarda en `<state dir>/plugin-paths.json`, y `scan_all()` recorre todos los formatos (los bundles `.lv2`/`.vst3` se listan como carpeta, el resto por extensión). El caché de scan pasó de CLAP-only a `Vec<FoundPlugin>`.
- **Settings → Plugin paths** (`ModalKind::PluginPaths`): lista cada formato con sus carpetas, con **botones clicables `EDIT / ADD / BROWSE / REMOVE / DEFAULTS`** en la fila de botones del modal (`ListModal.actions`: cada botón equivale a su tecla, así ratón y teclado comparten un único handler). `Enter` activa/desactiva, **`e` edita la ruta escribiéndola en el sitio** (`PathEdit`: caret `█`, ←/→/Home/End/Backspace/Delete, Enter guarda, Esc descarta, dejarla vacía la borra), **`a` escribe una ruta nueva**, `b` la elige con el navegador de directorios (`file_browser::DIR_PICK`), `d` quita, `r` restaura los defaults del formato; al cerrar (Esc) se re-escanea. Nuevo menú **SETTINGS** en la barra superior.
- **Pickers multi-formato**: el modal CHANGE SOURCE filtra por `ALL/CLAP/SF2/WAV/SFZ/LV2/VST2/VST3/DSSI/LADSPA/JSFX` y ADD FX (`ALL/BUILT-IN/PLUGINS`) lista **todos** los efectos encontrados. Lo que choz aún no puede cargar sale marcado `(not hosted yet)` y al elegirlo lo dice en el log, en vez de fallar en silencio.
- **Sigue faltando el hosting real de LV2 / VST2 / VST3**: hoy sólo CLAP (efectos + instrumentos) y SF2/WAV. El puerto desde seqterm (`seqterm-plugin-lv2` = ABI LV2 a mano + parser TTL, ~1.5k líneas; `-vst2` ~900; `-vst3` ~1.3k, todo sobre `libloading` sin lilv) es el trozo grande que queda y va aparte.
- **Barra de menú reducida a FILE / SETTINGS / HELP** (RACK, FX y TRANSPORT se fueron: eso ya vive en los paneles). SETTINGS tiene `Plugin paths...` y **`Rescan plugin paths`**. El panel INPUTS estrena botón **`SCAN INPUTS`** (`views::source_panel::BTN_SCAN` -> `MouseAction::ScanInputs` -> `connect_midi()`), que desplazó la lista de entradas una línea (`INPUT_LIST_TOP = 3`).
- **107 tests** (114 con `--features clap`), clippy `-D warnings` limpio. Verificado en la TUI real: línea de banco con `◀ ▶`, modal de rutas con las 9 secciones, sus directorios y el editor de texto en línea.
- **Ojo con los tests que guardan config**: `PluginPaths::save()` escribe en el state dir real; los tests de la UI que lo tocan llaman antes a `sandbox_state_dir()` (redirige `XDG_STATE_HOME` a `/tmp`). Sin eso, un test escribe en `~/.local/state/choz/plugin-paths.json` del usuario.

### Sesión 2026-07-28 (v) — por qué un directorio añadido no mostraba nada

Diagnóstico real (el usuario añadió `~/repo/free-soundfonts-sf2-2019-04`, con 73 `.sf2`, y no aparecían):
1. La carpeta se guardó bajo **SFZ**, no SF2 — el `a`/ADD del modal añade al formato de la fila donde está el cursor, y **nada en pantalla decía a qué formato**.
2. Además quedó **desactivada** (el `Enter` que sigue al alta la conmuta).
3. Y aunque se hubiera corregido: el caché de scan sólo miraba el mtime de los *directorios*, así que **editar la lista de rutas no lo invalidaba**; el rescan sólo ocurría al cerrar el modal con `Esc` (cerrar con el botón CANCEL o clicando fuera no rescaneaba).

Arreglos:
- **Cada directorio dice lo que aportó**: `✓ /usr/share/sounds/sf2   (13)`, `· /ruta   (off)`, `✓ /ruta   (missing)` y, lo importante, `✓ /ruta   (0 — holds 73 SF2 file(s), move it to SF2)` usando `paths::formats_present()` (mira las extensiones que hay en la carpeta). Un directorio en el formato equivocado ahora se ve a simple vista.
- **El editor muestra el formato**: `✎ [SF2] /ruta█`, y la nota dice `typing a SF2 path`.
- **`PluginPaths::config_file()` entra en la comprobación de frescura del caché** → editar las rutas fuerza rescan en el siguiente arranque.
- **`App::close_modal()`** es ahora la única salida de cualquier modal (Esc, botón CANCEL, click fuera, selección); si las rutas cambiaron (`paths_dirty`) rescanea al cerrar.
- 110 tests (117 con `--features clap`).

### Sesión 2026-07-28 (vi) — ADD FX por tipo y categoría

- **Chips de formato** en el modal ADD FX: `ALL / BUILT-IN / CLAP / LV2 / VST2 / VST3 / LADSPA / DSSI / JSFX`, y cada entrada lleva su etiqueta (`[BUILT-IN] DELAY`, `[LV2] Calf Reverb`, `[CLAP] ZamDelay  (not hosted yet)` para lo que aún no se hostea).
- **Agrupado por categoría** con cabeceras de sección: DELAY, REVERB, DYNAMICS, EQ / FILTER, MODULATION, DISTORTION, SPATIAL, TEXTURE, UTILITY, OTHER. Los built-ins la declaran (`AudioFxKind::category()`); los plugins la **adivinan por el nombre** (`FxCategory::guess`) porque ni CLAP ni LV2 la dan sin cargar el plugin o parsear su TTL — lo que no encaja cae en OTHER.
- Las cabeceras son etiquetas, no opciones: el cursor las salta con las flechas (`skip_fx_headers`), Enter sobre una no añade nada ni cierra el modal, y el modal se abre ya posicionado en la primera entrada real.
- 111 tests (118 con `--features clap`).

### Sesión 2026-07-28 (vii) — dos pedales de distorsión, color de texto e i18n

- **Dos distorsiones nuevas** en `choz-engine/src/fx/pedal.rs`, ambas con waveshaping a 2x oversampling (reusa `Oversampler2x`/`Biquad` de `utility.rs`, ahora `pub(crate)`):
  - **AMBER FANG** (voz de pedal naranja de clipping duro): high-pass de entrada, etapa de ganancia, **clipping asimétrico** (la mitad positiva recorta más que la negativa = armónicos pares) y un knob de tono que inclina entre cuerpo y brillo. Knobs: Dist / Tone / Level / Wet.
  - **VELVET FUZZ** (fuzz violeta grande): **dos etapas de soft-clip en cascada** con high-pass entre ellas + tone stack con **medios escarbados**. Knobs: Sustain / Tone / Level / Wet.
  - Tests propios: comprimen de verdad (18:1 de entrada -> <8:1 de salida), no producen no-finitos, el knob de tono cambia el espectro y en dry el buffer sale intacto. Ambas aparecen en ADD FX bajo DISTORTION.
- **Settings con pestañas** (chips del modal): `PLUGIN PATHS` / `TEXT COLOR` / `LANGUAGE`.
  - **TEXT COLOR**: paleta de 9 colores; el elegido se guarda en `<state dir>/ui.json` y lo leen los paneles vía `views::theme::text()` (un `AtomicU32` con el RGB empaquetado, para no pasar un contexto por cada función de dibujo).
  - **LANGUAGE**: inglés, español, portugués, francés, italiano, alemán, ruso, japonés y chino. `choz-ui/src/i18n.rs`: las **claves son el texto en inglés** (`t("RACK")`), así lo no traducido cae en inglés por construcción. Al arrancar el idioma sale de `$LC_ALL`/`$LC_MESSAGES`/`$LANG` si no hay uno guardado.
  - Traducidos los títulos de panel, la barra de menú, los botones de los modales y las etiquetas del RACK/TRANSPORT. El resto (nombres de FX, rutas, hints largos) sigue en inglés.
- **Ojo con el estado global en tests**: idioma y color son globales de proceso; `App::new()` ya **no** los aplica (lo hace `main`), y los tests que dibujan o cambian idioma comparten un `UI_LOCK`. Sin eso, un test en español rompía a otro que esperaba "SELECT".
- 120 tests (127 con `--features clap`).

### Sesión 2026-07-28 (viii) — barra lateral de categorías en ADD FX

- El widget de modales (`views/modal.rs`) admite una **barra lateral**: `ListModal.{sidebar, sidebar_cursor, sidebar_focused}` (etiqueta + contador por sección) y `ModalRects.sidebar` para el hit-test. Si hay barra, la lista se dibuja a su derecha y el scrollbar la sigue.
- **ADD FX** la usa: a la izquierda `ALL` + cada categoría con entradas (`DELAY 40`, `REVERB 22`, …), a la derecha sólo los efectos de la categoría elegida. Se acabaron las cabeceras intercaladas (y con ellas el `skip_fx_headers` que había que saltar).
- Manejo: `←`/`→` cambian de panel, `↑↓` mueven el panel enfocado, `Enter` en la barra entra a la lista y en la lista añade el efecto, `Tab` cicla los chips de formato (que además recortan la propia barra: bajo LV2 sólo quedan las secciones con plugins LV2). Con ratón: click en una sección la muestra, click en una fila la elige y el segundo click (o SELECT) la añade.
- El modal abre con el foco en la barra lateral, que es el camino rápido.
- 121 tests (128 con `--features clap`).

### Sesión 2026-07-28 (ix) — menú EDIT, color de bordes, y guardar proyecto en YAML

- **Barra de menú**: `SETTINGS` pasa a **`EDIT`**, y dentro el ítem `Plugin paths...` pasa a **`Settings...`** (sigue abriendo el mismo modal de 3 pestañas).
- **Pestaña `TEXT COLOR` -> `COLOR`**, y el color elegido ahora tiñe **también los bordes** de paneles y modales: `views::theme::border()` devuelve el color de texto al 45% de brillo (estructura sin competir con el contenido). Los paneles dejaron de tener su propia const `BORDER`.
- **SELECT cierra el modal de Settings** cuando lo que se elige es color o idioma (esas pestañas aplican y guardan al instante); en la pestaña de rutas sigue abierto, porque ahí `Enter` conmuta un directorio.
- **BUG arreglado**: en ADD FX no se podían elegir las categorías con el ratón — el bloque que atiende `ModalRects.sidebar` en `handle_modal_mouse` nunca llegó a aplicarse en la sesión anterior (un script de parcheo abortó a mitad y sólo escribió parte). Ahora hay test que hace el click por el camino real (`handle_mouse` + rects de un render).
- **File -> Save project...**: elige carpeta con el navegador de directorios y escribe `choz-project.yml` (`choz-ui/src/project.rs`, dep `serde_yaml`). Guarda **las dos mitades**: sonido (tabs del rack, instrumento con su banco/preset o params de plugin, cadena FX completa con cada knob y su wet, mixer gain/pan/mute/solo, entrada MIDI ligada, bindings de MIDI learn) y configuración (rutas de plugins por formato, color e idioma, sample rate/buffer/backend/dispositivo de salida, puerto OSC, puertos MIDI desactivados). Los structs derivan `Deserialize`, así que **cargar** es cuestión de cablearlo — hoy solo se guarda.
- `serde_yaml` 0.9 está marcado como deprecated upstream pero es lo que hay en caché y funciona; si molesta, migrar a `serde_yml` es un cambio de nombre.
- 125 tests (132 con `--features clap`).

### Sesión 2026-07-29 — AUDIO SETTINGS como las de seqterm (Engine / Plugin Paths / OSC)

- La pestaña `PLUGIN PATHS` del modal de Settings pasa a ser **`AUDIO`**, con las **tres subcategorías de seqterm en la barra lateral**: `Engine`, `Plugin Paths`, `OSC`.
- **Engine** (mismas filas que seqterm): `Backend` (AUTO/JACK/PIPEWIRE/ALSA — con `AUTO -> JACK` mostrando lo que el motor eligió de verdad), `Device`, `Sample rate`, `Buffer size`, `SF2 engine`, `Latency` (calculada) y la fila específica del backend (`PW quantum` / `ALSA hw dev` / `JACK server`). `←→` cambia el valor. **Honestidad**: backend, sample rate y buffer se aplican al **siguiente arranque** (la fila lo dice: `48000 (restart: running 44100)`); el device sí cambia en vivo. `SF2 engine` es de solo lectura porque choz solo compila oxisynth — el ajuste existe para que un proyecto guardado signifique lo mismo en seqterm.
- **Plugin Paths**: lo que ya había (formatos con sus directorios, contadores, editor de texto y botones EDIT/ADD/BROWSE/REMOVE/DEFAULTS), ahora dentro de su subcategoría.
- **OSC**: `Enable OSC`, `Port mode` (Specific/Random), `UDP port`, `TCP port` (se guarda pero el servidor es solo UDP, igual que en seqterm) y una línea de **estado en vivo** (`UDP :9000 ● listening` / `○ stopped`). `Enter` conmuta o abre un editor numérico de puerto; los cambios se aplican **al instante**.
- Para que OSC se pueda mover de puerto sin reiniciar, `osc::listen` ahora devuelve un **`OscHandle`** (socket con timeout de 200 ms + flag atómica): al soltarlo el hilo termina y libera el puerto. Ojo: **quien llama debe guardar el handle** — el test de socket lo destapó al dejarlo caer.
- Ajustes nuevos persistidos en `ui.json` (`AudioSettings` + `OscSettings`, con `#[serde(default)]` para que los archivos viejos sigan cargando), y usados al arrancar: sample rate, buffer, backend, device y OSC. `--osc-port` sigue mandando sobre lo guardado.
- 128 tests (135 con `--features clap`).

## Para mañana (en orden)

1. **Cargar proyectos** (`project.rs` ya modela y parsea el YAML; falta reconstruir el rack desde él).
2. **Hosting LV2 y/o VST2** (Fase C, punto 8): el escaneo, los filtros y las entradas de los pickers ya están; falta el host. Portar `seqterm-plugin-lv2` (ABI a mano + TTL, sin lilv) envolviéndolo en `choz_ports::{FxProcessor, AudioSource}`, luego `seqterm-plugin-vst2`.: crate nuevo tipo `choz-plugin-lv2` con `livi`, wrappers `AudioSource`/`FxProcessor` iguales a los de CLAP. Es varias sesiones.
2. Mirar el modal INSTRUMENT en una terminal con audio real (cargar Surge XT, `p`, mover un knob y oírlo).
3. Nice-to-have: paginar knobs de FX cuando el plugin tiene más de 7 params; ruteo por canal MIDI dentro de un puerto; automatización.

## Notas / gotchas para el que retome

- **Sync engine↔UI slots**: `AudioEngine.slot_count` y `App.slots` se mantienen en el mismo orden (append/remove espejados). No romper ese invariante.
- **Working copy**: `App.{source, fx_chain, fx_slot, fx_param}` son la copia viva del `active_slot`; se persisten a `App.slots[active]` en `persist_active()` al cambiar de tab / borrar. Los handlers de FX operan sobre la copia y llaman `rebuild_fx()` → `engine.set_slot_fx(active_slot, …)`.
- **Category selection NO debe tocar `app.source`** (corrompe el slot activo al persistir). Solo cambia `source_cat`. Ya arreglado; mantenerlo así al reestructurar SOURCE.
- **`target/` es efímero en el sandbox**: correr build+test en una sola invocación para ver el binario en `target/debug/choz`.
- **Ruteo**: se resuelve en la UI (`note_targets`), no en el engine. Si algún día hace falta que el RT rutee, hay que meterle el binding — hoy no lo tiene a propósito.
- **`midi_connected` vs `midi_ports`**: `InputSource::Midi(i)` indexa `midi_connected` (lo que devolvió `connect_inputs`, en ese orden exacto); `midi_ports` es "todo lo que se vio". No confundirlos.
- **Cambiar de output device pierde los slots del engine** (se dropea el stream); `App::set_output_device` los recrea. Cualquier estado que solo viva en el engine hay que recrearlo ahí también.
- **Canal de notas único**: `App.note_tx/note_rx` se crea una vez al arrancar; `connect_midi()` clona el sender. No volver a crear el canal al reconectar MIDI (dejaría huérfano el hilo OSC).
- **Mixer**: `RackSlot.{gain,pan,mute,solo}` NO están en la working copy; viven solo en `slots[i]` y se empujan con `push_mix()` (que resuelve el solo). El engine no conoce el solo.
- **Teardown de plugins CLAP**: por defecto se filtran a propósito (ver arriba). Si aparece un leak raro o quieres medir memoria, corre con `CHOZ_CLAP_STRICT_TEARDOWN=1` — y prepárate a que un plugin roto tire la app.
- **Verificar la TUI sin terminal**: `(sleep 5; printf '\r') | script -qec "stty rows 45 cols 170; timeout 10 ./target/debug/choz" /dev/null > out`, luego quitar ANSI con `re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]','',...)`. **Ojo**: ratatui hace redibujo incremental, así que las líneas completas del scrape suelen ser de frames viejos.
- **Layout del panel RACK**: la línea del mixer desplazó el contenido 1 fila (`fx_cy = fx_inner.y + 2`); los rects del mixer en `compute_layout` están calculados a mano con los anchos de los spans de `draw_fx_chain_panel` — si cambian los textos, actualizar ambos.
- **`registry.rs`/`scanner.rs`/`plugin_types.rs`**: infra de plugins vieja, en gran parte stub (`#[allow(dead_code)]`). El path real de plugins hoy es `choz-plugin-clap`. Decidir si se unifica o se borra al hacer Fase C.

## Comandos útiles

```bash
cargo build --workspace                 # build default
cargo build -p choz-ui --features clap  # con hosting CLAP real
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo run --bin choz                    # necesita una terminal real (tty)
tail -f ~/.local/state/choz/choz.log    # ver errores/log en vivo
```
