# choz — Roadmap

Estado y pasos para continuar. Última actualización: 2026-08-08 (sexies).
Historial completo de cambios en [CHANGELOG.md](../CHANGELOG.md).

## Estado actual (lo que ya funciona)

- **Workspace 9 crates**: `choz-ports` (traits RT), `choz-engine` (audio/DSP/MIDI/registry), `choz-ui` (binario `choz`), `choz-plugin-sandbox` (memoria compartida para hostear fuera de proceso) y **un crate por formato de plugin**: `choz-plugin-clap` (clack-host), `choz-plugin-lv2` (ABI LV2 + parser TTL propio, sin lilv), `choz-plugin-ladspa` (LADSPA + DSSI), `choz-plugin-vst2`, `choz-plugin-vst3` (bindings COM puros). Ver [architecture.md](architecture.md).
- **Sin feature flags de hosting**: todos los hosts se compilan siempre. La antigua feature `clap` (off por defecto) hacía que los plugins CLAP no se vieran en un build normal; se eliminó junto con el campo `hosted` del cache.
- **Engine RT-safe**: callback sin locks ni alloc. Modelo **rack multi-source = `Vec<Slot>`** (`Slot{source, fx}`), mixer que suma todos los slots. Handoff por ring `EngineCommand` + ring `Retired` para drops fuera del RT.
- **Fuentes reales**: WAV (`hound`), SF2 (`oxisynth`), **SFZ** (parser + sampler de 32 voces propios) e **instrumentos de plugin en CLAP, LV2, DSSI, VST2 y VST3**. Cada una = un slot/tab en el RACK, con su cadena FX propia. `AudioEngine::load_plugin(slot, format, path, id)` despacha por formato; `path` es el archivo (o el directorio del bundle en LV2/VST3) e `id` el identificador dentro de él (id CLAP, URI LV2, label LADSPA/DSSI).
- **32 FX DSP** built-in verificados (smoke test; los 27 originales + Protocosmos / Space Echo / Reverse Delay / Z5 Texture importados de seqterm + los pedales AMBER FANG / VELVET FUZZ) + **CLAP audio-effects** hosteados en la cadena FX (al final del modal ADD FX) **con sus parámetros reales**: nombres/rangos leídos del plugin (`read_params`), knobs normalizados 0..1, y cambios en vivo por `EngineCommand::SetFxParam` (no se reinstancia el plugin al mover un knob).
- **Mixer por slot**: gain, pan constant-power, mute y solo (solo se resuelve en UI → mute efectivo al engine). Teclas `-`/`+`, `,`/`.`, `m`, `S`; también rueda/click.
- **MIDI hardware** (`midir`): conecta todos los puertos menos los desactivados (`c` en el panel INPUTS), **ruteo por-entrada** (cada nota va solo a las tabs ligadas a esa entrada).
- **OSC** (`rosc`): listener UDP (9000 por defecto, `--osc-port N`). Notas (`/note`, `/note/on`, `/note/off`) + **control remoto**: `/mix/<tab>/{gain,pan,mute}` y `/fx/<tab>/<fx>/<param>`. Comparte el canal `flume` con MIDI (`InputEvent::{Note,Control}`).
- **SF2 presets**: `sources::list_sf2_presets` (via `soundfont`) lista los programas; `AudioSource::program_change` (RT-safe) los cambia por slot.
- **UI**: menú superior (F10/mouse), RACK con tabs por source (`[`/`]` o click), FX por-slot editable, piano QWERTY, About con imagen (`ratatui-image`), log a `~/.local/state/choz/choz.log`. **Todos los modales** (source, ADD FX, salida, bank/preset, MIDI learn, browser, params del instrumento) usan el mismo widget `views/modal.rs`: barra de desplazamiento, chips de filtro, botones SELECT/CANCEL y rueda/click de ratón.
- **Efectos de plugin en la cadena FX**: CLAP, LV2, LADSPA, DSSI, VST2 y VST3 implementan `FxProcessor` y conviven con los built-ins en ADD FX (chips por formato). Sus parámetros reales (nombres/rangos) salen de `choz_engine::read_plugin_params(format, path, id)`.
- **Cache de scan multi-formato**: `<state dir>/plugins.json`; `r` en SYNTH fuerza rescan. En esta máquina: **1209 plugins** (611 efectos LV2 + 36 instrumentos LV2, 342 LADSPA, 18 CLAP + 2 instrumentos, 17 VST2, 18 VST3 + 1 instrumento, 2 instrumentos DSSI, 53 SFZ, 103 SF2). El escaneo corre **fuera de proceso** y la carga pasa por **cuarentena**, así que un plugin roto no se lleva la app.
- **Ventana nativa del plugin** en los cuatro formatos hosteados: VST2 (`effEditOpen`), LV2 (`ui:X11UI` sin suil), CLAP (`clap.gui` + `clap.timer-support` del host) y VST3 (`IPlugView` + `Steinberg::Linux::IRunLoop` implementado por el host). Botones `GUI` en el RACK.
- **Temas y fondo**: once esquemas tipo Notepad++ que fijan texto/marcos/escritorio juntos, y fondo de escritorio en color o imagen (mosaico o estirado). En kitty (y ghostty/WezTerm) la imagen va por el **protocolo gráfico, a resolución real de píxel y por debajo de los fondos de celda**; en el resto de terminales, como color de celda en halfblocks.
- **Verificación**: `cargo build --workspace` limpio, **236 tests**, `clippy --workspace --all-targets -D warnings` = 0. Hay tests de runtime por formato (`choz-plugin-*/tests/*_runtime.rs`) contra los plugins instalados en la máquina; si no hay ninguno, se saltan. Los barridos completos (hostear *todos* los efectos instalados) están `#[ignore]`: `cargo test --release -p choz-plugin-lv2 -- --ignored`. CI en `.github/workflows/ci.yml`.

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
   - ~~**LV2 / LADSPA / DSSI / VST2 / VST3**~~ **HECHO** (2026-07-31): ver "Sesión 2026-07-31" abajo.
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
- **Rutas de plugins tipo Carla** (`choz-engine/src/paths.rs`): `PluginFormat {LADSPA, DSSI, LV2, VST2, VST3, CLAP, SF2, SFZ}` con sus directorios por defecto (`/usr/lib/...`, `/usr/lib64/...`, `/usr/local/lib/...`, `~/.lv2`, `~/.vst`, …) y respetando `LV2_PATH`/`VST_PATH`/`VST3_PATH`/`CLAP_PATH`/… si están definidas. `PluginPaths` se guarda en `<state dir>/plugin-paths.json`, y `scan_all()` recorre todos los formatos (los bundles `.lv2`/`.vst3` se listan como carpeta, el resto por extensión). El caché de scan pasó de CLAP-only a `Vec<FoundPlugin>`.
- **Settings → Plugin paths** (`ModalKind::PluginPaths`): lista cada formato con sus carpetas, con **botones clicables `EDIT / ADD / BROWSE / REMOVE / DEFAULTS`** en la fila de botones del modal (`ListModal.actions`: cada botón equivale a su tecla, así ratón y teclado comparten un único handler). `Enter` activa/desactiva, **`e` edita la ruta escribiéndola en el sitio** (`PathEdit`: caret `█`, ←/→/Home/End/Backspace/Delete, Enter guarda, Esc descarta, dejarla vacía la borra), **`a` escribe una ruta nueva**, `b` la elige con el navegador de directorios (`file_browser::DIR_PICK`), `d` quita, `r` restaura los defaults del formato; al cerrar (Esc) se re-escanea. Nuevo menú **SETTINGS** en la barra superior.
- **Pickers multi-formato**: el modal CHANGE SOURCE filtra por `ALL/CLAP/SF2/WAV/SFZ/LV2/VST2/VST3/DSSI/LADSPA` y ADD FX (`ALL/BUILT-IN/PLUGINS`) lista **todos** los efectos encontrados. Lo que choz aún no puede cargar sale marcado `(not hosted yet)` y al elegirlo lo dice en el log, en vez de fallar en silencio.
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

- **Chips de formato** en el modal ADD FX: `ALL / BUILT-IN / CLAP / LV2 / VST2 / VST3 / LADSPA / DSSI`, y cada entrada lleva su etiqueta (`[BUILT-IN] DELAY`, `[LV2] Calf Reverb`, `[CLAP] ZamDelay  (not hosted yet)` para lo que aún no se hostea).
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

### Sesión 2026-07-31 — hosting de todos los formatos

Motivo: "no veo los instrumentos ni efectos lv2, dssi, vst, clap del sistema". Dos causas: el binario se compilaba sin `--features clap`, y LV2/LADSPA/DSSI/VST2/VST3 solo se escaneaban, nunca se hosteaban.

- **`choz-plugin-lv2`** (portado de `seqterm-plugin-lv2`, edition 2024): parser TTL propio (`rio_turtle`) + ABI LV2 a mano, sin lilv. `scan_directory` (solo lee TTL: barato y no puede petar), `read_params` (los control-input ports), `Lv2Instrument` (`AudioSource`) y `Lv2Effect` (`FxProcessor`).
  - Añadido sobre lo de seqterm: feature **`opts:options`** (min/max/nominalBlockLength + sampleRate) y `bufsz:boundedBlockLength`, sin los cuales DPF/Dragonfly/Zam se negaban a instanciar; el atom del puerto MIDI usa el URID real de `atom:Sequence`; se aplican los defaults del TTL al instanciar.
  - Barrido sobre los 547 efectos instalados: 524 hostean bien, 13 piden `worker#schedule` (no implementado), el resto no tiene salida de audio.
- **`choz-plugin-ladspa`**: LADSPA + DSSI en un crate (comparten descriptor). Escaneo por `ladspa_descriptor`/`dssi_descriptor`, parámetros desde los port range hints (con sus defaults y bounds relativos al sample rate), `LadspaEffect` (`FxProcessor`) y `DssiInstrument` (`AudioSource`, MIDI → eventos ALSA `snd_seq_event_t` de 28 bytes para `run_synth`).
- **`choz-plugin-vst2`** (portado de `seqterm-plugin-vst2`): `VSTPluginMain` + `processReplacing` + `effProcessEvents`. Params normalizados 0..1 con los nombres del plugin.
- **`choz-plugin-vst3`** (portado de `seqterm-plugin-vst3`, **sin** el editor nativo): bindings COM puros (`vst3` 0.3), `factory_info()` lee nombre/vendor/subcategoría (para saber si es instrumento) sin instanciar.
- **Generalización del plumbing** que era CLAP-only: `choz_ports::PluginParam` (era `ClapParamInfo`), `fx_chain::PluginFxRef{format,path,id}` (era `ClapFxRef`), `source::PluginFx` (era `ClapFx`), `SourceAction::Plugin{format,…}`, `PluginFormat::{is_plugin, is_hosted}`. `is_hosted` = CLAP, LV2, LADSPA, DSSI, VST2, VST3, SF2.
- **Plugins que son hosts** (Carla `carlarack`/`carlapatchbay*` en LV2, `carla.vst` en VST2) corrompen el heap al cargarlos: hay una deny-list por prefijo en los dos crates. El arreglo general sería escanear/hostear fuera de proceso (seqterm tiene un crate sandbox).
- **Gotcha de tests**: los plugins JUCE/VST3 hacen init global al cargar y petan si dos hilos cargan a la vez → los tests de runtime de VST2/VST3 son **una sola función** (el harness paraleliza por función).

### Sesión 2026-08-02 — cajones IN/OUT y salida multicanal por slot

- **Cajones laterales**: INPUTS y OUT dejan de ser una columna fija y un modal. Ahora son pestañas colapsables pegadas a los bordes (`views/drawer.rs`): cerradas son un tirador de 3 columnas con el nombre en vertical (`IN` / `OUT`), abiertas se comen su parte del cuerpo. `F2`/`F3` las abren, `Esc` cierra la enfocada, y hay un `[✕]` en la esquina superior derecha de cada cajón abierto. `Tab` sólo se para en cajones abiertos (`next_focus`). Ambos arrancan cerrados: el RACK ocupa todo.
- **Se borró el modal de salida** (`ModalKind::Device`, `open_device_modal`, `Modal.devices`): el cajón OUT lo reemplaza.
- **Backend JACK nativo** (`choz-engine/src/jack_backend.rs`): cpal-jack sólo da un par estéreo fijo, así que choz abre su propio cliente `choz` con **un puerto por canal del dispositivo** (`choz:out_1..out_N`, `choz:in_1..in_M`) y se autoconecta al sink canal por canal. Con la UMC1820: 12 out / 10 in, verificado con `pw-link`. Los `monitor_*` se excluyen a propósito (loopean el rack sobre sí mismo). Sin grafo JACK cae a cpal, estéreo, como antes.
- **Ruteo por slot**: `Slot.out_pair` + `EngineCommand::SetSlotOut`; el mezclador escribe en un buffer por canal (`RtState.mix`) en vez de uno estéreo interleaved. El cajón OUT lista los dispositivos y debajo los pares de canales del que está corriendo, con qué tab usa cada uno; `Enter` sobre un par rutea la tab activa. Se guarda en el proyecto (`mixer.out_pair`).
- **Entradas de audio: la mitad del engine ya está.** `Slot.in_pair` + `EngineCommand::SetSlotIn` + `RtState.capture`: un slot alimentado por un par de captura ignora su instrumento y pasa el audio vivo por su cadena FX. Tiene test. **Falta la UI** para elegirlo (va en el cajón IN, junto a los puertos MIDI).
- `AudioEngine::start` prueba primero el cliente nativo y cae a cpal si no hay grafo; cambiar de dispositivo re-patchea los puertos si el número de canales coincide y **reconstruye el cliente** (perdiendo los slots, como el camino cpal) si no.
- Fuera de choz, dos incendios del sistema diagnosticados: Carla petaba con `AudioDriver=JACK` + `ProcessMode=2` (Continuous Rack) — pipewire-jack le entrega buffer size 0, el rack aloja con eso y corrompe el heap; arreglado con `ProcessMode=3`. Y `~/.config/pipewire/jack.conf.d` tenía `node.latency=64/48000` + `node.lock-quantum=true` global (el mismo quantum que estranguló el xHCI USB); ahora 256/48000 sin lock.
- 94 tests de engine + 59 de UI, clippy limpio.

### Sesión 2026-08-03 — ventana nativa del plugin (VST2) + MIDI learn de cualquier parámetro

Es la petición del 2026-08-02, en el orden que decía: VST2 primero.

- **Puerto nuevo `choz_ports::PluginEditor`** (`open(parent_xid) -> tamaño`, `idle`, `close`) + `EditorHandle = Arc<dyn PluginEditor>`, y `editor()` como método por defecto (`None`) de **`AudioSource` y `FxProcessor`**. Cualquier formato que implemente el trait entra sin tocar nada más.
- **VST2 lo implementa** (`choz-plugin-vst2`): opcodes `effEditOpen/GetRect/Idle/Close` (ya existían las constantes). El `AEffect` se comparte con el hilo de la GUI en un `Arc<Mutex<Option<EffectCell>>>`; el `Drop` de `Instance` lo pone en `None` **antes** de cerrar el plugin, así una ventana que sigue abierta cuando se cambia el instrumento deja de llamar a memoria liberada (pasa a no-op). El mutex NO serializa audio: `processReplacing` sigue yendo por el puntero directo, como hace cualquier DAW.
- **La ventana** (`choz-ui/src/editor.rs`): hilo propio con `x11rb` (dep nueva, solo en Linux), `create_window` + `WM_DELETE_WINDOW` + `map_window`, el XID va como `parent`, y bucle de 30 ms con `idle()` + drenaje de eventos X. `EditorWindow` se cierra al dropearlo (join incluido) y `App::poll_editor` lo suelta cuando el usuario cierra desde el gestor de ventanas. Sin `DISPLAY` avisa y no abre.
- **Botones `GUI`**: en la línea INSTR del RACK para el instrumento (tecla `g`) y en la caja `SLOT` para el efecto seleccionado de la cadena (tecla `G`). Solo aparecen si ese plugin declara editor. **Una ventana a la vez**: abrir otra cierra la anterior; volver a pulsar sobre la misma la cierra.
- **El engine guarda los handles**: `AudioEngine.{editors, fx_editors}` se llenan en `add_slot`/`set_slot_source`/`set_slot_fx` — el único momento en que la UI todavía puede tocar el procesador antes de que se lo lleve el hilo RT. `slot_editor(slot)` / `fx_editor(slot, fx)`.
- **MIDI learn universal**: `LearnTarget::InstrParam{slot,param}` — los parámetros del instrumento ya son mapeables, no solo los del FX chain. Se arman con `l` sobre la fila del modal INSTRUMENT, o desde el picker de destinos (que ahora los lista). `set_slot_instr_param` aplica por tab, así un CC mueve un plugin que no está en pantalla.
- **Verificado de verdad** (X11 real): ZamTube VST2 abre su ventana embebida, con el tamaño que pide el plugin (448x315 vía `effEditGetRect`, no el fallback 600x400).
- **Ojo**: en esta máquina **no hay ningún instrumento VST2 instalado** (los 17 VST2 son efectos Zam), así que el botón GUI del instrumento no se ve con Surge XT — Surge es CLAP/LV2/VST3. Los efectos VST2 sí lo tienen.
- 94 tests de engine + 60 de UI, clippy `--all-targets -D warnings` limpio.

**Por qué CLAP no vino detrás**: en CLAP `clap.gui` es **[main-thread]**, y en choz la instancia vive en el slot del hilo RT — la UI no la puede volver a tocar. Hacerlo bien pide que el instrumento se quede en el hilo de la TUI (o en el sandbox de proceso) y que choz implemente `clap.timer_support`/`posix_fd_support`. Es un trabajo aparte, no una extensión de esto. VST3 (`IPlugView`) y LV2 (`ui:X11UI` sin suil) siguen igual que estaban.

### Sesión 2026-08-03 (bis) — TyrellN6 segfaulteaba: `audioMasterGetTime` devolvía NULL

Síntoma: cargar TyrellN6 (VST2, u-he) = violación de segmento. Reproducido con un host mínimo de 30 líneas: **crashea dentro de `processReplacing`, en el primer bloque**, sin MIDI y con cualquier tamaño de bloque.

Causa: el `host_callback` de `choz-plugin-vst2` respondía `0` a todo lo que no fuera versión / sample rate / block size. El plugin pide **`audioMasterGetTime` (opcode 7) en cada bloque y desreferencia la respuesta sin comprobar null**. Cualquier plugin que sincronice algo (LFO, delay, arpegiador) hace lo mismo.

Arreglo: `VstTimeInfo` + `time_flags` en `vst2_abi.rs`, y el callback devuelve un `thread_local!` con transporte fijo (120 BPM, 4/4, playing, ppq 0) — uno por hilo, así el puntero vive sin compartirse. De paso: `audioMasterWantMidi` → 1 y `audioMasterGetCurrentProcessLevel` → 2 (realtime). Marcado `ponytail:`: cuando choz tenga transporte propio, se rellena de verdad.

Verificado: **TyrellN6 y TripleCheese suenan** (pico 0.13 / 0.09 con una nota) y se dropean limpio; Pianoteq 9 carga sin petar. Test nuevo `the_host_callback_answers_get_time_with_a_filled_in_transport` (no necesita plugins instalados).

También: `/home/jorge/repo` añadido a las rutas de **VST2 y VST3** (`plugin-paths.json`). Aporta 3 instrumentos VST2 (TyrellN6, TripleCheese, Pianoteq) y 2 bundles VST3 de Pianoteq — el de `arm-64bit` aparece pero no cargará en x86. Escaneo completo: 1156 plugins, 2.3 s en release.

### Sesión 2026-08-03 (ter) — UI de entradas de audio en el cajón IN

Cierra el punto 2 del "para mañana": el engine ya ruteaba captura a un slot (`set_slot_in`), faltaba la pantalla.

- **El cajón IN pasa a tener dos secciones**, con el mismo modelo de filas que el cajón OUT: `NOTE IN` (puertos MIDI + OSC, como antes) y **`AUDIO IN (n)`** con una fila `(instrument)` y un par de canales de captura por cada dos entradas del dispositivo. `InputRow` gana `header: bool` y `name: String`; el panel dibuja los títulos en gris y el cursor los salta.
- **`App::in_targets()` / `in_select()`** son el equivalente de `out_targets()` / `out_select()`. `InTarget::{None, Note(i), Capture(pair), NoCapture}`. `Enter` sobre una entrada de notas sigue creando/saltando a su tab; sobre un par de captura pone la tab activa a sonar con el audio vivo; sobre `(instrument)` la devuelve a su instrumento.
- **`RackSlot.in_pair`** guarda la elección y va al proyecto (`Mixer.in_pair`, con `#[serde(default)]`). `apply_routing()` reempuja entradas y salidas tras recrear el rack.
- La línea INSTR del RACK muestra `AUDIO IN 3/4` cuando la tab corre con captura, en vez de nombrar un instrumento que no se está oyendo.
- **`out_step` e `in_step` comparten `row_step`** (genérico sobre el tipo de destino): un solo sitio que salta cabeceras.
- Test nuevo `in_drawer_lists_note_inputs_then_audio_capture` (modelo de filas, salto de cabecera, swap a captura y vuelta). 61 tests de UI.
- **Verificado en la TUI real**: el cajón muestra `NOTE IN` con los 3 puertos MIDI + OSC y `AUDIO IN (0)` con `(instrument)` marcado.
- **Limitación honesta (preexistente, ahora visible)**: los canales de entrada salen de `device_channels(sink)`, que mira los puertos del **mismo nodo** que la salida. Vale para una interfaz duplex (UMC1820 = 12 out / 10 in), pero con la tarjeta interna la captura vive en otro nodo (`alsa_input…` frente a `alsa_output…`) y la sección dice `(0)`. Elegir dispositivo de entrada aparte del de salida es el siguiente paso si hace falta. Hoy la UMC1820 no estaba enchufada, así que las filas de pares no se pudieron ver con hardware real.

### Sesión 2026-08-03 (quater) — cargar proyectos

Punto 3 del "para mañana": el YAML ya se escribía y se parseaba, faltaba reconstruir choz desde él.

- **`Project::load(path)`** (archivo o directorio con el nombre por defecto) + **`App::apply_project`**: primero la configuración (rutas de plugins, color, idioma, MIDI desactivado, puerto OSC), luego el rack. Lo que falta se avisa por el log y se salta: un proyecto de otra máquina pierde la tab del plugin que no está instalado, no el archivo entero.
- **`rebuild_rack()` sale de `set_output_device`**: crea un slot de engine por tab y lo llena (instrumento, cadena FX con sus knobs, mixer, ruteo). El cambio de dispositivo de salida y la carga de proyecto usan exactamente el mismo código.
- **Los bindings de MIDI learn ahora se restauran**: `project::Slot.midi_learn` pasa de `(u8, String)` a `Vec<Binding{cc, target, label}>` con `LearnTarget`/`TriggerAction` derivando `Serialize/Deserialize`. La etiqueta se sigue escribiendo (para que el archivo se lea como la UI) pero se ignora al cargar.
- **`File → Open project…`** en la barra de menú, y **`choz proyecto.yml`** en la línea de comandos (la extensión decide: `.wav`/`.sf2` = instrumento, `.yml`/`.yaml` = proyecto). El proyecto se aplica antes que el archivo suelto.
- Tests nuevos: `a_saved_project_loads_back_into_the_same_rack` (snapshot → apply → mismo rack, knobs, mixer, ruteo y bindings) y el round-trip por disco en `project.rs`. 62 tests de UI.
- **Verificado con motor real**: `./target/debug/choz proj.yml` con un proyecto de 2 tabs (SF2 FluidR3 preset 4 ligado a OSC + AmberFang; TyrellN6 VST2) → el log dice `loaded 2 tab(s)`, TyrellN6 instancia, y la barra de estado muestra `RACK: 1/2 SF2:FluidR3_GM ← OSC | FX: 1`.

Dos avisos:
- **Cargar un proyecto pisa la configuración guardada** (rutas de plugins, color, idioma): el archivo lleva las dos mitades a propósito, así que abrir el de otra persona cambia tus ajustes. Si molesta, la salida es un flag "solo el rack".
- **Un plugin que escribe en stdout ensucia la TUI**: choz redirige *stderr* al log, pero u-he (y otros) imprimen en stdout, que es justo donde ratatui dibuja. Se ve como basura en pantalla hasta el siguiente redibujo completo. Arreglarlo = redirigir stdout al log y dibujar por un fd duplicado.

### Sesión 2026-08-03 (quinquies) — LV2 `worker#schedule`

Punto 5 del "para mañana": 32 bundles instalados declaran `worker#schedule` como feature requerida y `Features::supported` los rechazaba de entrada.

- **Feature nueva `worker#schedule`** (`lv2_abi.rs`: `LV2_Worker_Schedule`, `LV2_Worker_Interface`, sus status codes): el plugin recibe el callback en `instantiate`, y `WorkerState` (una caja con `Cell`s) recoge el handle y la interfaz **después** de instanciar — el plugin sólo puede llamar desde `run()`, mucho más tarde.
- **El trabajo se hace inline, la respuesta no**: `schedule_work` llama a `work()` en el acto, pero `respond` sólo **encola una copia** de la respuesta; `run()` la entrega con `work_response` cuando `work()` ya volvió, y después llama `end_run`. Ese orden es el que describe la spec, y reentrar `work_response` desde dentro de `work` es justo lo que rompe a los plugins que tienen su propio hilo. Marcado `ponytail:` — si algún plugin bloquea el hilo de audio cargando samples, el siguiente paso es un hilo con anillo de peticiones.
- **Resultado medido**: de los 32 bundles que piden worker, **29 hostean y procesan finito** (Dragonfly x3, guitarix, a-fluidsynth, synthv1/drumkv1/samplv1, …). Los 3 restantes se rechazan con razón: `midimap` y los wrappers de Carla no tienen salida de audio.
- **Bug gordo de paso: no volver a `dlclose` un binario LV2** (`LOADED_LIBS`). Un plugin puede dejar hilos propios vivos — los `*v1` de Rui arrastran Qt entero, con sus hilos de eventos, D-Bus y XCB — y descargar el `.so` bajo sus pies revienta dentro del *loader* (`_dl_close`, reproducido con padthv1). Los hosts de verdad tampoco descargan plugins. Ahora cada `Library` cargada se guarda para la vida del proceso.
- **`padthv1` sigue petando al destruirse** después de haber procesado: su hilo Qt muere mientras `cleanup` hace el `join`. No es el worker (cargarlo y soltarlo *sin* procesar es limpio) ni el `dlclose`. Se trata como `ZaMaximX2` en CLAP: `leaks_on_teardown` lo deja vivo a propósito, y `CHOZ_LV2_STRICT_TEARDOWN=1` restaura el teardown correcto para depurar.
- Test nuevo `effects_requiring_the_worker_feature_are_hosted` (se salta solo si la máquina no tiene ninguno).

### Sesión 2026-08-03 (sexies) — escaneo fuera de proceso

Primera mitad del punto 6: **escanear** ya no puede tirar la app. Hostear todavía sí.

- **`scan_all` lanza un hijo por (formato, directorio)**: el mismo binario de choz con `--choz-scan-worker <FORMATO> <dir> <archivo-salida>`. `choz_engine::scan_worker_main()` es lo **primerísimo** que hace `main()`, antes del log, del terminal y del audio.
- **El resultado va por archivo, no por stdout**: los plugins imprimen banners y avisos en stdout mientras se los sondea (u-he, fluidsynth, guitarix) y esa basura se mezclaría con el JSON. El stdout del hijo se tira; su stderr hereda el del padre, o sea que acaba en el log.
- **Aislamiento por archivo cuando hace falta**: si el hijo muere, el padre reintenta el directorio **entrada por entrada**, un hijo por archivo/bundle. Sólo se pierde el plugin que revienta. Para eso `scan_one` acepta un archivo o un bundle suelto y despacha a los `describe`/`discover_bundle` que ya tenía cada crate (el de CLAP sólo hubo que hacerlo `pub`).
- **Sonda de una vez** (`worker_available`): sólo el binario de choz llama a `scan_worker_main`, así que un test o cualquier otro programa que enlace `choz-engine` se relanzaría a sí mismo con argumentos que no entiende. Se pregunta una vez con un directorio imposible: un worker de verdad contesta `[]`; si no, todo el escaneo sigue en proceso como antes.
- **Verificado**: con un `.so` cuyo constructor desreferencia null junto a ZamComp, el escaneo sobrevive, avisa dos veces (directorio y archivo) y devuelve ZamComp. Test `scan_isolation.rs` (con `harness = false`, porque el binario de test tiene que poder ser worker). En la TUI real, arranque con state dir vacío: 1156 plugins cacheados, 0 crashes.
- **Coste**: ~40 procesos por escaneo completo, 2.4 s frente a 2.3 s en proceso. Sin timeout: un plugin que se *cuelgue* sigue colgando el escaneo (no ha pasado todavía).
- **Las deny-lists de Carla se quedan**: protegen el *hosting*, que sigue en proceso. Cargar `carlarack` en un slot sigue pudiendo llevarse la app.

### Sesión 2026-08-03 (septies) — SFZ

Punto 4: **choz toca instrumentos SFZ**.

- **`choz-engine/src/sfz.rs`** (portado de `seqterm-sfz`, ~500 líneas con tests): parser del subconjunto que usan los instrumentos reales (`<group>`/`<region>`, `sample`, `lokey`/`hikey`/`key`, `pitch_keycenter`, `lovel`/`hivel`, `volume`) y `SfzSampler`, un `AudioSource` con 32 voces, transposición por `pitch_keycenter` e interpolación lineal.
- **Cambio importante respecto a seqterm**: allí `note_on` **abre y decodifica el archivo** — o sea, I/O en el hilo de audio. Aquí **todas las muestras se decodifican al cargar** (una sola vez por archivo distinto, con caché por ruta) y la voz sólo clona un `Arc<Vec<f32>>`. `note_on` no reserva memoria: las voces viven en un `Vec` con capacidad fija y la más vieja se roba al llegar a 32.
- **Bug del parser de seqterm arreglado**: `sample=Saw Samples/Saw_C-3.flac` se cortaba en el espacio. El valor de `sample` corre hasta el final de línea o hasta el siguiente opcode (`is_opcode`), que es lo que hacen los demás hosts. Sin esto, la librería de Renoise —la única SFZ de esta máquina— no cargaba ni una muestra.
- **Dep nueva `symphonia`** (`default-features = false`, features `wav`/`flac`/`pcm`): las muestras SFZ son FLAC tan a menudo como WAV (770 vs 396 en la librería de Renoise), así que `hound` solo no alcanzaba.
- **Cableado**: `PluginFormat::Sfz` entra en `is_hosted()`, `AudioEngine::{add_plugin,load_plugin}` lo aceptan y `build_instrument` construye el sampler. En el picker deja de salir `(not hosted yet)` — se carga por el mismo camino que un plugin aunque no lo sea.
- **Bug de etiqueta arreglado de paso**: `slot_label` ponía `CLAP:` a *todos* los instrumentos de plugin. Desde que entraron LV2/VST/DSSI en julio, cada tab estaba mal etiquetada; ahora usa el formato que la cargó.
- **Verificado en la TUI real**: con la librería de Renoise en las rutas SFZ, el escaneo encuentra **53 instrumentos SFZ** (1209 plugins en total), el picker los ofrece sin marca de "no hosteado" y cargar uno deja `INSTR SFZ:Additive1` en el RACK. Medido aparte: `Saw.sfz` carga en 3.9 ms y suena (pico 0.39, todo finito).

### Sesión 2026-08-03 (octies) — cuarentena: probar el plugin en un hijo antes de cargarlo

Segunda tanda del punto 1 (hosting fuera de proceso). El escaneo ya estaba aislado; **cargar** no lo estaba: elegir el plugin equivocado en el picker se llevaba la app.

- **`choz-engine/src/quarantine.rs`**: la primera vez que se carga un plugin, choz lo prueba en un hijo (`--choz-probe-worker <FORMATO> <ruta> <id> <archivo>`) — instanciar, tocar dos bloques, destruir — y guarda el veredicto en `<state dir>/plugin-verdicts.json`.
- **Tres veredictos, y la diferencia importa**: el hijo va escribiendo su etapa (`started` → `loaded` → `done`), así que si muere se sabe *dónde*. `CrashesOnLoad` → choz se niega a cargarlo, con mensaje. `CrashesOnTeardown` → **sí se carga**: toca bien, sólo revienta al destruirse, y para eso se filtra la instancia. `Ok` → adelante.
- **Se acabó el `padthv1` hardcodeado**: `choz-plugin-lv2` ya no lleva la URI en el código. Tiene `leak_on_teardown(uri)` y el engine lo llama cuando el veredicto dice `CrashesOnTeardown`. Cualquier otro plugin con el mismo bug queda cubierto solo. `CHOZ_LV2_STRICT_TEARDOWN=1` ignora la lista — que es justo lo que permite al hijo descubrirlo.
- **Coste**: un proceso extra la primera vez que se usa cada plugin, cacheado después. `quarantine::clear()` olvida los veredictos si se reemplaza un plugin roto por uno arreglado.
- **Verificado**: padthv1 → el hijo muere con etapa `loaded` → `CrashesOnTeardown`, se carga y se filtra. TyrellN6 → `done`. Test `tests/quarantine.rs` (con `harness = false`, como el de escaneo).
- **Bug que costó rato**: el hijo escribe su etapa en el state dir, que puede no existir todavía. Un `write` fallido se ve exactamente igual que "este binario no es un worker" y **todos** los plugins salían `Ok`. Ahora el padre crea el directorio antes de lanzar.
- **Las deny-lists de Carla vuelven a estar**, y esta vez con la razón medida: se quitaron para ver si la cuarentena bastaba, y el test de escaneo murió con `free(): corrupted unsorted chunks`. El wrapper VST2 de Carla no *peta*, **corrompe el asignador**, así que ni se intenta. La cuarentena cubre a los desconocidos; esos dos siguen por nombre a propósito.

### Sesión 2026-08-03 (nonies) — el stdout de los plugins deja de pintar sobre la TUI

- `log.rs` mandaba **stderr** al archivo de log, pero **fd 1 es donde dibuja ratatui** — y ahí es donde escriben los plugins hosteados: u-he suelta 34 líneas (`AM_VST_base::resume ()`, presets, `setNumInputs`) sólo con cargar TyrellN6; fluidsynth y guitarix también.
- Ahora `log::take_terminal()` hace `dup(1)` para quedarse con **una copia privada de la terminal**, por la que dibuja la TUI, y le entrega el fd 1 al log. Lo que imprima cualquier librería cargada acaba en el archivo.
- El backend pasa de `CrosstermBackend<io::Stdout>` a `CrosstermBackend<BufWriter<File>>` (el `BufWriter` importa: un `File` pelado convierte cada trocito que escribe ratatui en una llamada al sistema).
- Si el `dup` falla, choz dibuja igual por fd 1 como siempre — sólo vuelve a compartirlo con los plugins.
- **Medido**: cargando el proyecto con TyrellN6, **0 líneas de ruido en pantalla** y 34 en `choz.log`. El RACK dibuja normal.

### Sesión 2026-08-03 (decies) — dispositivo de entrada aparte del de salida

Cierra el punto 3. El cajón IN decía `AUDIO IN (0)` en esta máquina porque los canales de captura salían del **mismo nodo JACK que la salida**: eso vale para una interfaz duplex (UMC1820) pero no para una placa normal, donde reproducir es `alsa_output…` y capturar es `alsa_input…`, dos nodos distintos.

- **`AudioEngine.input_device`** + `input_devices()` / `input_device()` / `set_input_device(name)` / `set_input_device_preference(name)`. `jack_sources()` es el gemelo de `jack_sinks()`: todo nodo con puertos de salida que no sean `monitor_*`.
- **`jack_backend::start` acepta sink y source por separado**; `connect` se quedó con la parte de reproducción y la captura se cablea aparte en `connect_capture`. `capture_channels(source)` cuenta los puertos del nodo de entrada.
- **Cambiar de entrada reconstruye el cliente** (otra cuenta de canales = otro juego de puertos), así que se pierden los slots y la UI rehace el rack con `rebuild_rack()` — el mismo contrato que cambiar la salida.
- **El cajón IN lista los dispositivos de captura** antes de la fila `(instrument)` y de los pares. Se guarda en `ui.json` (`audio.input_device`, con `#[serde(default)]`) y se aplica al arrancar.
- **Bug encontrado al probarlo**: `set_input_device` exigía un `output_device` con nombre, pero PipeWire nos autoconecta y ese campo se queda vacío hasta que alguien elige salida — el primer intento moría con "no output device to rebuild the client on". Ahora `restart_jack_native` acepta `Option<&str>` y cae a `jack_current_sink()`.
- **Verificado en el grafo**: elegir "Ryzen HD Audio Controller Stereo Microphone" deja `choz:in_1 ← alsa_input…Mic2:capture_FL` y `in_2 ← capture_FR` (visto con `pw-link -l`), el log pasa de `in=0 ch` a `in=2 ch` y el cajón muestra `AUDIO IN (2)` con el par `1/2`.

### Sesión 2026-08-03 (undecies) — cargar sólo el rack

Un proyecto lleva las dos mitades a propósito, pero abrir el de otra persona te cambiaba las rutas de plugins, el color, el idioma y los ajustes de audio. Ahora se puede elegir.

- Botón **`RACK ONLY`** (tecla `k`) en el modal `OPEN PROJECT`: conmuta, y la nota del modal dice en cada momento qué va a pasar. Al abrir el modal siempre arranca apagado — cargar completo sigue siendo lo normal.
- `apply_project` se partió en dos: la mitad de configuración y `apply_project_rack` (tabs, instrumentos, cadenas FX con sus knobs, mixer, ruteo, bindings de MIDI learn). `RACK ONLY` llama sólo a la segunda.
- Test `loading_rack_only_keeps_the_local_settings`: guarda un proyecto con rutas e idioma ajenos, lo carga con la marca puesta y comprueba que el rack entra y la configuración local no se mueve.

### Sesión 2026-08-03 (duodecies) — hosting fuera de proceso: el transporte

Punto 1. Escanear y probar-antes-de-cargar ya corrían en un hijo; **tocar** no. Esta sesión hace la parte que manda: mover un bloque de audio entre dos procesos, con plazo.

- **Crate nueva `choz-plugin-sandbox`**, dos piezas a propósito separadas:
  - `shm.rs` — memoria compartida POSIX (`shm_open` + `ftruncate` + `mmap`). El creador hace `unlink` en cuanto el hijo se enganchó, así que un crash de cualquiera de los dos no deja nada en `/dev/shm`.
  - `bridge.rs` — el protocolo sobre esos bytes. **No mapea ni reserva nada**, así que el handshake entero se prueba en un proceso sobre un `Vec<u8>`.
- **Cada callback es una cita, no un flujo**: el host escribe su bloque de entrada, sube `request` y espera a que `done` lo alcance; el hijo despierta, procesa, escribe la salida y sube `done`. Un solo bloque en vuelo: no hay cola que desincronizar ni latencia extra más allá del viaje.
- **Lo que lo hace seguro desde el hilo de audio**: `exchange` lleva **plazo**. Si el hijo no llega, el host lee silencio y sigue. Un plugin colgado cuesta un chasquido, no el stream. `missed()` los cuenta.
- **La espera es spin acotado + `sched_yield`, no futex** (marcado `ponytail:`): mantiene la región en atómicos pelados, sin peleas con el layout de `sem_t` ni orden de inicialización. Se cambia si aparece en un perfil.
- MIDI viaja con el bloque: 64 mensajes por bloque, y de ahí en adelante se descartan — el lado host es RT y no puede crecer nada.
- **Medido** (release, bloques de 256 frames): viaje de ida y vuelta **1.26 µs de media, 36 µs el peor**, 0 bloques perdidos en 5000. Un bloque de 256 a 48 kHz dura 5.33 ms, o sea que el transporte se come el **0.02 %** del presupuesto de media y el 0.7 % en el peor caso.
- **Verificado entre procesos de verdad** (`tests/across_a_process.rs`, `harness = false`): 201 bloques ida y vuelta sin perder ninguno, y —lo que importa— un hijo que hace `abort()` a mitad deja al host vivo, contando bloques silenciosos.

**Lo que falta para cerrar el punto**: el hijo que carga el plugin de verdad (reusando `build_instrument` / `build_plugin_fx`), un `AudioSource`/`FxProcessor` que hable por el bridge, y la decisión de cuándo usarlo — probablemente sólo para lo que la cuarentena marcó como sospechoso, no para todo.

### Sesión 2026-08-03 (terdecies) — el plugin ya toca en su propio proceso

Cierra el punto 1 en lo que se puede cerrar sin volverlo la norma.

- **`choz-engine/src/sandboxed.rs`**: `SandboxedPlugin` es un `AudioSource` normal —un slot del rack no nota la diferencia— que por dentro hace `exchange()` contra un hijo. El hijo es el binario de choz otra vez (`--choz-sandbox-worker <FORMATO> <ruta> <id> <shm> <frames>`), cargando el plugin con el mismo `build_instrument` / `build_plugin_fx` de siempre.
- **MIDI cruza con el bloque** y se traduce de vuelta a `note_on`/`note_off`/`control_change`/`pitch_bend` en el hijo. Si el callback pide menos de un bloque, se responde en un buffer preasignado y se recorta; si pide más, se trocea. `render` no reserva memoria.
- **Política automática**: `build_hosted_instrument` mira el veredicto de la cuarentena y **sandboxea lo que muere al destruirse**. Si el sandbox no arranca, cae a hostear en proceso (con su fuga, como antes). Nada más se sandboxea todavía: el resto de los plugins no paga un proceso por gusto.
- La salida no finita se limpia en el lado host: un plugin que ya sabemos que se porta mal tampoco va a envenenar la mezcla (padthv1 devuelve NaN hasta que tiene patch).
- **Verificado con plugins de verdad** (`tests/sandboxed_plugin.rs`, `harness = false`): ZamComp VST2 devuelve 50 bloques sin perder ninguno, con buffers cortos y largos; y **padthv1 —el que segfaultea el proceso al destruirse— carga sandboxeado, suena y se dropea sin llevarse el test por delante**. Ese `drop` es exactamente el que mataba a choz.
- **Trampa que costó el susto**: el binario de test contestaba sólo a `sandbox_worker_main`, así que cuando la cuarentena lanzó su sonda, el hijo no reconoció el flag, **corrió el test entero otra vez** y se hizo una bomba de procesos. Arreglado en dos capas: `choz_engine::worker_main()` contesta a los tres roles de una (y es lo que llaman `main` y los tests), y cada hijo lleva `CHOZ_WORKER=1` — un worker **nunca** lanza workers, así que la recursión es imposible aunque alguien olvide el primer paso.

### Sesión 2026-08-03 (quaterdecies) — el plugin sandboxeado se repone solo

- **Hilo supervisor** en `SandboxedPlugin`: vigila al hijo y, cuando muere, **arranca otro**. El hilo de audio no participa — mientras tanto `exchange` le devuelve silencio, que es lo que ya hacía. Un plugin que segfaultea deja de ser "la tab queda muda hasta que la recargues" y pasa a ser un chasquido.
- **El hijo nuevo se sincroniza solo**: `Sandbox::attach` arranca su cuenta en `done` (el último bloque contestado) en vez de en cero. Para el primer hijo eso es 0 y contesta la petición pendiente; para un reemplazo es donde llegó su antecesor, así que no corre detrás de una historia que no vio. El host no tiene que hacer nada, que es justo lo que hace falta cuando el host es el hilo de audio.
- `restarts()` y `child_pid()` para verlo desde fuera. En `Drop` el orden importa: primero se le dice al supervisor que se retire y después muere el hijo, o el supervisor arranca otro servicialmente.
- **Verificado matando el hijo con SIGKILL** mientras el "hilo de audio" seguía pidiendo bloques: el log dice `plugin sandbox died (signal: 9 (SIGKILL)); restarting it`, aparece un pid nuevo, `restarts() == 1`, y a partir de ahí **contesta todos los bloques sin perder ninguno**.
- Primer intento fallido, por si alguien lo repite: hacer que el hijo empezara en `request` en vez de en `done` dejaba al **primer** hijo saltándose la petición que ya estaba pendiente — el host esperaba 5 s, el test fallaba y el hijo quedaba huérfano colgado.

### Sesión 2026-08-04 — efectos sandboxeados, y los parámetros cruzan

- **Los parámetros ya viajan**: el header lleva una cola de hasta 32 cambios `(índice, valor)` por bloque, igual que el MIDI. El hijo los aplica antes de procesar. Antes un instrumento sandboxeado no tenía knobs — se cargaba y no se podía tocar.
- **`SandboxedEffect`** (`FxProcessor`) encima del mismo enlace: manda la señal seca, recibe la procesada, y aplica el **wet/dry de choz de este lado** (el hijo nunca lo ve). Entrada y salida no pueden ser el mismo buffer contra la región compartida, así que el bloque va por `tail` y vuelve por `answer`, los dos preasignados: `process_block` no reserva.
- **Misma política que los instrumentos**: `build_plugin_fx` mira la cuarentena y sandboxea lo que muere al destruirse; si el sandbox no arranca, cae a hostear en proceso. `build_plugin_fx_in_process` es la versión llana que usan el hijo y la sonda — llamar a la otra desde ahí sería recursión infinita.
- **Verificado con ZamComp VST2 de verdad**: seno de entrada → salida finita y distinta de silencio; con wet en 0 la señal vuelve **idéntica**; un `set_param` cruza sin romper nada.
- De paso, menos ruido: si un binario no entiende el flag de sonda (un binario de test con harness de libtest), la cuarentena lo aprende **una vez** y deja de lanzar hijos, en lugar de intentarlo por cada plugin.


### Sesión 2026-08-04 (bis) — sandbox a mano, y verlo en el RACK

Cierra el punto 1: hasta ahora el sandbox era automático o nada, y no había forma de saber que una tab estaba corriendo en otro proceso.

- **"Sandboxear este plugin" es una preferencia del plugin, no de la tab**: `quarantine::{forced, set_forced}` guardan una lista de claves `formato|ruta|id` en `<state dir>/plugin-sandbox.json` — la misma clave que los veredictos. Así sobrevive a recargar el proyecto, y un proyecto ajeno no arrastra la decisión. La política de los dos sitios que la aplican (`build_hosted_instrument` y `build_plugin_fx`) pasa a ser una sola función: **`quarantine::wants_sandbox` = lo que el usuario pidió, o lo que la sonda vio morir al destruirse**.
- **Los contadores cruzan a la UI sin tocar la instancia**: `choz_ports::SandboxStatus` (dos `Arc<AtomicU64>`: bloques perdidos y reinicios) + `sandbox()` como método por defecto de `AudioSource` y `FxProcessor`, exactamente al lado de `editor()`. `SandboxedPlugin` publica `bridge.missed()` al final de cada bloque (un store relajado, RT-safe) y comparte el `Arc` de reinicios con el supervisor. El engine los recoge en el único momento en que puede — `add_slot` / `set_slot_source` / `set_slot_fx` — en `sandboxes` / `fx_sandboxes`, gemelos de `editors` / `fx_editors`, con `slot_sandbox(slot)` y `fx_sandbox(slot, fx)`.
- **Botón `SBX` en el RACK**: en la línea INSTR para el instrumento (tecla `x`) y en la caja `SLOT` para el efecto seleccionado (tecla `X`). Sólo aparece si eso es un plugin hosteado. El propio botón es el indicador: `SBX ○` en proceso, `SBX ● (reload)` pedido pero todavía no aplicado, y **en verde** `SBX ● 3 lost 1↻` cuando de verdad está corriendo fuera — con los bloques que se perdieron y las veces que el plugin se cayó y volvió.
- **El cambio se oye ya**: tocar el botón del instrumento re-instancia el plugin (`App::reload_instrument`, que reaplica los valores de los knobs); el del FX llama a `rebuild_fx()`, que reconstruye la cadena y con ella relee la política de cada plugin.
- **Verificado con plugins de verdad** (`tests/sandboxed_plugin.rs`): ZamComp VST2 sale `Ok` de la sonda y se hostea en proceso; con la marca puesta el mismo plugin vuelve con `sandbox()` lleno, procesa 20 bloques finitos, 0 perdidos, 0 reinicios; quitarla lo devuelve a proceso. Test de UI `the_sandbox_button_toggles_the_plugin_preference` (render + click por `handle_mouse` + estado). **Visto en la TUI real** con un tab VST2 cargado desde un proyecto: la línea INSTR dibuja `SOURCE / MIDI LEARN / SBX`.
- **Ojo**: `set_forced` escribe en el state dir, así que un test que lo toque necesita `sandbox_state_dir()` antes, como los de rutas de plugins.
- **Flake preexistente vista de paso**: `sandboxed_plugin` falló una vez con `check(padthv1) == Ok` en vez de `CrashesOnTeardown` y pasó en las siguientes ejecuciones; la sonda a mano segfaultea con etapa `loaded`, así que el diagnóstico es correcto y lo que falla es la repetibilidad (probablemente un archivo de etapa de una corrida anterior). No investigado.

### Sesión 2026-08-06 — el teclado no sonaba, y la latencia era de 21 ms

Motivo: *"seleccioné el Keystation, el EPiano LV2, salida 1/2 de la UMC1820… no escucho nada"*. Tres bugs distintos, ninguno donde parecía.

- **MIDI: no había reconexión por hotplug.** `connect_midi()` sólo corría al arrancar, al conmutar un puerto o con la tecla `r`. El Keystation apareció como `card5` **76 s después** de que choz arrancara, así que quedó en la lista de la UI (que se refresca sola) pero **sin suscripción ALSA**: `aconnect -l` mostraba el cliente `choz-in` conectado a *Midi Through* y los otros dos sin nada. Ahora `App::poll_midi_hotplug()` compara la lista de puertos cada 2 s en el bucle principal y reconecta si cambió. Enchufar el teclado con choz abierto ya funciona.
  - `ponytail:` sondeo en vez de un hilo con udev/ALSA-monitor: el escaneo es abrir un cliente ALSA, barato a 0.5 Hz.
- **`disabled_midi_inputs` no filtraba nada.** El default es `["Midi Through"]`, pero midir nombra los puertos `"Cliente:Puerto n:m"`, así que el `contains` exacto de `midi.rs` nunca acertaba y el puerto de loopback se conectaba igual. Nueva `is_disabled()`: coincide con el nombre del cliente pelado o con el prefijo `"Cliente:"`.
- **Latencia clavada en 1024 frames (21,3 ms)** pese a `buffer_size: 256`. `PIPEWIRE_LATENCY` es sólo un *pedido*: pipewire-jack abre cada cliente JACK con `node.lock-quantum` y un `node.force-quantum` heredado del quantum que el grafo estuviera corriendo, y **force gana**. Nueva `request_pipewire_period()` (`engine.rs`, llamada desde `start_jack_native` y `pick_backend`) exporta también `PIPEWIRE_QUANTUM`, que escribe `node.force-quantum`/`node.force-rate`. El buffer de la UI por fin manda: `128/48000` = **2,7 ms**, medido en el log de arranque.
  - **`MIN_FORCED_QUANTUM = 128`**: por debajo choz pide pero no fuerza. 64 frames es lo que stalleó los endpoints USB y se llevó el xHCI (ver [usb-xhci-crash.md](usb-xhci-crash.md)); el piso existe para que ningún valor de la UI pueda repetirlo.
- **Lo que NO era**: `ERR = 0` en todos los nodos de `pw-top`, o sea que el "clip" que se oía no eran xruns sino saturación de nivel del EPiano. Y la configuración de PipeWire/WirePlumber de la máquina ya estaba bien afinada — no se tocó ningún archivo del sistema.
- Todo el diagnóstico, los comandos y la configuración final en **[audio-latency.md](audio-latency.md)**.

### Sesión 2026-08-06 (bis) — dos flakes y el buffer que se perdía al guardar

- **Guardar proyecto se llevaba el `buffer_size` viejo** (`main.rs`): el snapshot leía `engine.buffer_size` —lo que está corriendo— en vez del valor pendiente. Como sample rate, buffer y backend sólo se aplican al siguiente arranque, guardar tras cambiarlos escribía justo los valores anteriores. Los tres salen ahora de `self.ui.audio`; el device sigue saliendo del engine, porque ése sí cambia en vivo. Test `saving_keeps_the_pending_audio_settings_not_the_running_ones`.
- **`plugin_scan` segfaulteaba una corrida de cada tres**, y no era colisión entre binarios de test como se sospechaba: sus **dos tests corrían en paralelo dentro del mismo binario**, ambos llamando a `scan_all`. Un binario de test no es un scan worker, así que `worker_available()` dice que no y el escaneo cae **en proceso** — dos hilos dlopeneando los mismos plugins, y los JUCE/VST3 hacen init global al cargar. Fusionados en una función (el harness paraleliza por función), que además hace un solo `scan_all` en vez de dos: 0 fallos en 12 corridas y ~6 s menos.
- **`quarantine` fallaba con `left: Ok`**, el flake que estaba anotado sin investigar. Causa medida arriba (punto 3 de Pendiente): el crash de padthv1 es una carrera y la sonda la muestrea una vez. El test pasa a exigir lo que sí es estable (`!= CrashesOnLoad` y `loadable()`) con el porqué escrito al lado; **el problema de fondo queda como pendiente, no silenciado** — un veredicto `Ok` cacheado por suerte deja el plugin sin sandbox.
- `cargo test --workspace` completa ahora entero sin `--no-fail-fast`: **200 tests**, clippy `--all-targets` limpio.

### Sesión 2026-08-06 (ter) — ventana nativa de los plugins LV2

Punto 1 de Pendiente, la parte LV2. Es el formato que más ventanas aporta en esta máquina y el que **no** exigía reestructurar el threading: una UI LV2 vive en un binario aparte y nunca toca la instancia del plugin — habla con el host por un callback de escritura, y el host es quien mueve el valor al puerto. Por eso funciona con la instancia en el hilo RT.

- **`choz-plugin-lv2/src/editor.rs`**: `Lv2Editor` implementa `PluginEditor`, así que los botones `GUI` del RACK y `EditorWindow` funcionan sin tocar `choz-ui` (`engine.rs` ya llamaba a `source.editor()`). ABI de UI nuevo en `lv2_abi.rs` (`LV2UI_Descriptor`, `LV2UI_Idle_Interface`, write function).
- **Los controles se comparten, no la instancia**: `SharedControls` = `Arc<Mutex<Option<ControlsCell>>>` con el puntero base de `control_values`, el mismo array al que `connect_port` apunta. El `Drop` de `Lv2Instance` lo vacía **antes que nada, incluso en el camino de leak**, así una ventana abierta cuando desaparece su slot deja de escribir. Mismo contrato que el editor VST2.
- **Descubrimiento** (`discovery.rs`): `ui:X11UI` en el TTL, vinculada al plugin por `ui:ui`, por `lv2:appliesTo`, o —cuando el bundle describe una sola— por descarte, que es lo que cubre a DPF (Zam, Dragonfly), donde nada declara la relación.
- **Tres cosas costaron el rato**, todas medidas y no adivinadas:
  1. `ui:idleInterface` es feature *y* extensión. Una UI que la lista en `requiredFeature` (guitarix) la busca en el array y desreferencia null: segfault, no un `NULL` cortés.
  2. Las UIs de DPF exigen `opts:options` para leer el sample rate. Sin ella, 14 UIs de Zam se descartaban en silencio.
  3. Copiar el grafo del bundle por plugin para leer el `requiredFeature` de la UI volvió el escaneo cuadrático y **colgó** el barrido en bundles grandes. Ahora sólo se parsea el documento del `seeAlso` de la UI.
- **Se respeta `requiredFeature`**: una UI que pide algo fuera de `SUPPORTED_UI_FEATURES` no recibe editor. `instance-access`/`data-access` quedan fuera a propósito — entregan un puntero a la instancia viva, que aquí está en el hilo de audio.
- **Medido con `examples/ui_probe`** (abre y cierra cada UI instalada en una ventana X real). Guitarix segfaultea entera (31 de 31, con la ventana mapeada y sin mapear, y aislada en un proceso propio) → deny-list por prefijo, como la de Carla.
  - **Corrección (2026-08-07)**: la cifra original de "148 UIs limpias" no valía. El probe consumía el instrumento antes de abrir la ventana (`.and_then(|i| i.editor())`), así que medía con el plugin ya destruido, y además sólo comprobaba que `open` no petara.
  - **Y el segundo intento tampoco**: al contar hijos X11 reales daba "0 sin ventana", pero los resultados se acumulaban en un `Vec` volcado al final y **stdout a un archivo va en bloques**, así que cada pasada que moría por un segfault se llevaba sus líneas. Una corrida perdió 74 resultados sin que se notara: el total no cuadraba con la suma y ahí se vio.
  - **Con `say()` (imprime y hace flush) los números cuadran**: sobre 98 plugins probados, **91 abren ventana real**, **1 abre sin crear ventana** y **4 no llegan a dar un editor**. Barrido aún en curso; los fallos duros siguen siendo de LSP y siguen cambiando de corrida en corrida.
- **Lo que NO se metió en la deny-list**: LSP. El barrido revienta en unas pocas UIs suyas cada corrida, pero **son distintas cada vez**, así que no es propiedad de ningún plugin — el probe abre 250+ UIs en un proceso sin descargar ninguna, que no es como choz las usa. Culpar URIs concretos habría sido una adivinanza disfrazada de medición.
- Test `x11_editors_are_discovered_and_the_crashing_families_are_not_offered` (no abre ventanas: eso necesita DISPLAY y CI no tiene). **201 tests**, clippy limpio.
- **Sin verificar todavía**: que la ventana se vea y se use *dentro de la TUI* con un plugin cargado en un tab. El barrido prueba `open`/`idle`/`close` contra una ventana X real, no el flujo del botón `GUI`.

### Sesión 2026-08-06 (quater) — fuera JSFX

- **JSFX eliminado de todo el árbol**: el enum `PluginFormat`, sus rutas por defecto, los chips de ADD FX y CHANGE SOURCE, y las ramas de los stubs viejos (`scanner.rs`, `registry.rs`, `plugin_types.rs`). Nunca se hosteó — sólo se escaneaba y se mostraba como "(not hosted yet)".
- **Trampa que traía**: `plugin-paths.json` guarda una entrada por formato, así que al desaparecer la variante el archivo entero dejaba de parsear y `PluginPaths::load()` caía a `Default` **en silencio**, tirando las rutas que el usuario hubiera añadido a mano. Ahora el formato se persiste por etiqueta (`"LV2"`, no `"Lv2"` — `from_label` es case-insensitive, así que los archivos viejos siguen cargando) y las entradas desconocidas se saltan una a una. Test propio, y verificado contra el archivo real de esta máquina.
- **Efecto lateral que quedó a la vista**: sin JSFX no existe ningún formato que sea plugin y no esté hosteado, así que la rama de `plugin_fx_entries` que trae los "no hosteados" desde `plugins` ya no puede aportar nada al menú de FX. Se dejó (es genérica y sirve el día que entre un formato nuevo sin host), pero el test que la ejercía se reescribió: usaba JSFX justamente por ser el único no hosteado.
- **`lv2_runtime` serializado**: sus 6 tests corrían en hilos distintos dlopeneando los mismos plugins, y el binario moría con SIGSEGV una corrida de cada varias. Un mutex del archivo, que conserva los nombres en vez de fusionarlos como hicieron VST2/VST3.
- 201 tests, clippy `--all-targets` limpio, `cargo test --workspace` sin fallos en corridas repetidas.

### Sesión 2026-08-06 (quinquies) — CLAP: la mitad del editor, y por qué no basta

Punto 1 de Pendiente, la parte CLAP. **No quedó funcionando**, y el detalle de por qué es lo útil.

- **El camino de acceso sí se resolvió**: la API segura de clack para `clap.gui` pide un `&mut PluginMainThreadHandle`, y su constructor es `pub(crate)`, así que desde fuera no hay forma — y para cuando alguien pulsa el botón, la instancia ya vive en el hilo de audio. La salida es el puntero crudo: `instance.raw_instance()` da un `&clap_plugin`, y los tipos (`clap_plugin_gui`, `clap_window`) salen de **`clap-sys` 0.5, la misma versión de la que depende clack**, así que los layouts son exactamente los que el plugin compiló. Dep nueva, y `choz-plugin-clap/src/editor.rs` con `ClapEditor`.
- **Y funciona hasta cierto punto**: `is_api_supported("x11")`, `create`, `set_parent` y `show` devuelven éxito en los 20 CLAP instalados, Surge XT incluido, sin un solo crash.
- **Pero no aparece ninguna ventana.** `examples/gui_probe` no se conforma con el valor de retorno: consulta `query_tree` sobre la ventana padre y cuenta hijos. Resultado: **cero hijos**, con y sin bombear idle 600 ms, y `get_size` no reporta nada (probado en los dos órdenes, antes y después de `set_parent`). Todo el probe corre en un solo hilo, así que la regla `[main-thread]` **no** es la explicación.
- **Lo que falta está del lado del host**: `ChozHost` no declara ninguna extensión (`type MainThread<'a> = ()`). Un plugin que busca `clap.gui` o `clap.timer_support` en el host no encuentra nada y nunca llega a dibujar. Eso es lo siguiente: declarar `clap_host_gui` (con `request_resize` / `closed`) y `clap.timer_support`, y bombear `on_timer`.
- **Queda tras `CHOZ_CLAP_GUI=1`**, apagado por defecto: el botón `GUI` no debe aparecer en un slot CLAP para después no hacer nada. El código y la medición quedan como base del siguiente intento, no como función a medias.

### Sesión 2026-08-07 — la ventana de CLAP, y un defecto en cómo la estaba midiendo

Cierra el punto 1 para CLAP. Y corrige la sesión anterior, que llegó a una conclusión equivocada.

- **Las dos extensiones del host, que era lo que faltaba** (`host.rs`): `ChozShared` implementa `HostGuiImpl` y `ChozMainThread` implementa `HostTimerImpl`, declaradas en `declare_extensions`. `type MainThread<'a>` deja de ser `()`.
  - `clap.gui` es lo que un plugin consulta **antes** de molestarse en construir una UI, y por donde pide resize o avisa que se cerró solo.
  - `clap.timer-support` es lo que de verdad hacía falta: **una UI CLAP dibuja desde `on_timer`**, no desde un idle como VST2. Surge XT registra un timer de 20 ms en cuanto el host ofrece uno — antes no registraba nada.
- **`GuiState`** guarda los timers registrados; `PluginEditor::idle` los tiquea llamando `on_timer` del plugin por FFI (mismo camino crudo que el resto, porque clack pide un `&mut PluginMainThreadHandle` que el hilo del editor no puede tener).
- **El defecto de medición**: `examples/gui_probe` hacía `.and_then(|i| i.editor())`, que **consume el instrumento** — el plugin se dropeaba antes de abrir la ventana, y su `Drop` vacía la celda compartida. Así que `open()` salía por la rama "instancia muerta" sin decir nada. De ahí la conclusión de la sesión anterior ("create y set_parent devuelven true pero no dibuja"): estaba midiendo sobre un plugin ya destruido. Los dos probes tenían el mismo error.
- **Resultado con el plugin vivo**: **20 de 20 CLAP abren ventana real**, con el tamaño que piden (Surge XT incluido). El probe cuenta los hijos X11 del padre con `query_tree`, no los valores de retorno — que es justamente lo que destapó el problema.
- El gate `CHOZ_CLAP_GUI` desaparece: la función está completa.

**Lección para el resto del roadmap**: un probe que consume el objeto bajo prueba mide otra cosa. Las cifras de LV2 de la sesión (ter) se tomaron con el mismo defecto y se re-midieron.

### Sesión 2026-08-07 (bis) — temas de color y fondo de escritorio

Pedido: *"en Edit/Settings, en la tab de color, permitir elegir imagen o color de fondo, con mosaico o estirado, y un buscador de archivos tipo explorer"*, ampliado después a *"enmarca todo en theme, con combinaciones tipo Notepad++"*.

- **`settings::THEMES`**: once esquemas (Obsidian, Zenburn, Solarized Dark/Light, Monokai, Deep Black, Vibrant Ink, Ruby Blue, Bespin, Hello Kitty y el de choz) que fijan **texto, marcos y escritorio juntos** — el sentido de un tema es que los tres funcionen como conjunto. `UiSettings::apply_theme` los aplica de una; los colores siguen siendo editables después.
- **La pestaña `COLOR` pasa a `THEME`**, con dos mitades separadas por cabeceras (mismo truco que la lista de rutas: las cabeceras son etiquetas, no opciones): el listado de esquemas arriba, las filas de escritorio abajo (`Background`, `Fit`, `Pick an image...`, `Clear`). `ThemeRow` traduce índice de fila a acción, así que el orden puede cambiar sin romper nada.
- **`views/background.rs`**: fondo vía **`ratatui-image` en halfblocks** — `▀` con dos píxeles por celda, el doble de resolución vertical que un color promediado. Halfblocks y no kitty/sixel a propósito: los protocolos gráficos dibujan fuera del modelo de celdas y la UI de encima no los taparía. Se decodifica y escala (Lanczos3, a la medida exacta del área) una vez, cacheado por `(archivo, fit, ancho, alto)`.
- **El navegador de archivos acepta varias extensiones** (`exts: &[&str]` en vez de `ext: &str`) y arranca en `assets/` del proyecto cuando existe.
- **Lo que los tests no habrían encontrado**: pasaban todos, pero al mirar la TUI real contando secuencias de color, el wallpaper daba 21 tonos casi negros. Dos causas: `assets/wallpaper.png` es efectivamente casi negra (9,9,9), y —la de fondo— **los paneles pintaban su propio fondo opaco**, así que la imagen sólo se habría visto en los huecos entre ellos. `theme::panel_bg()` / `app_bg()` devuelven `Reset` cuando hay escritorio configurado. Verificado con `wallpaper2.jpg`: 34 colores distintos con gradientes atravesando los paneles.
- 208 tests, clippy limpio. Seis nuevos: el renderizador (color liso, terminal intacto, imagen ausente sobrevivible, decodificación real, mosaico que repite) y el flujo completo del selector.

## Petición nueva (2026-08-02) — editor nativo del plugin + MIDI learn universal

Pedido textual: *"he seleccionado un tab con surgext, agrega un botón que abra una ventana emergente con el vst… copia el comportamiento desde seqterm, el sistema debe ser capaz de poder mapear dentro de cualquier plugin los parámetros de edición del sonido del plugin vía midi learn"*.

Son dos cosas y conviene hacerlas en ese orden.

### 1. Botón `[GUI]` en el RACK que abre la ventana nativa del plugin

Hoy el instrumento sólo se edita por el modal `INSTRUMENT` (tecla `p`), que dibuja los parámetros como texto. Falta la ventana del propio plugin (la de Surge XT).

Cómo lo hace seqterm (`crates/seqterm-plugin-sandbox/src/host.rs:472` `run_editor_window`), que es el patrón a copiar:

- **Un hilo dedicado** crea una ventana X11 con `x11rb` (`create_window` + `map_window`), registra `WM_DELETE_WINDOW` para que la cruz del gestor de ventanas cierre limpio, y **todas** las llamadas X11 se quedan en ese hilo.
- El XID de esa ventana es el `parent` que se le pasa al plugin: `editor_open(win as usize as *mut c_void)`, que devuelve el tamaño pedido → `configure_window`.
- Bucle de idle cada ~30 ms: `editor_idle()` (imprescindible en VST2, si no la GUI se congela) + drenar eventos X + atender el resize que pida el plugin (`editor_take_resize`).
- Al salir: `editor_close()` + `destroy_window`, siempre, también si el bucle murió por error.

Estado por formato en choz:

- **VST2**: `choz-plugin-vst2` se portó de seqterm pero **sin** los opcodes de editor. Hay que traer `has_editor` / `editor_open` (`effEditOpen`) / `editor_rect` (`effEditGetRect`) / `editor_idle` (`effEditIdle`) / `editor_close` — están en `seqterm-plugin-vst2/src/lib.rs:127-157`, son ~30 líneas.
- **VST3**: el port se hizo explícitamente **sin** el editor nativo (`IPlugView`). Es el más caro de los cuatro.
- **CLAP**: extensión `clap.gui` (`is_api_supported("x11")` → `create` → `set_parent` → `show`), más `clap.timer_support` para el idle. `clack-extensions` ya está en el árbol.
- **LV2**: los plugins traen su UI en el bundle (`ui:X11UI`); sin `suil` hay que hacer el `instantiate` del descriptor de UI a mano. Empezar por aquí sólo si hace falta LV2 concreto.
- **Nota sobre estabilidad**: seqterm abre las GUIs **dentro del sandbox** (proceso aparte). En choz todo corre en proceso, así que un editor que peta se lleva la app — es el mismo argumento que ya está en la lista para escanear/hostear fuera de proceso.

UI en choz: botón `[4:GUI]` en la línea `INSTR` del RACK (junto a `[1:SOURCE]`, `[3:LEARN]`), activo sólo si el plugin declara editor; abre/cierra la ventana. Igual para un FX del chain (el `SLOT` ya tiene fila de botones).

### 2. MIDI learn sobre cualquier parámetro del plugin

Hoy `LearnTarget` (`choz-ui/src/main.rs:248`) cubre `Gain`, `Pan`, `FxParam{slot,fx,param}` y `Trigger`: **los parámetros del instrumento no son mapeables** — sólo los del FX chain. El pedido es que cualquier parámetro de cualquier plugin (instrumento incluido) se pueda aprender.

- Añadir `LearnTarget::InstrParam { slot, param }` y su rama en `apply_cc` / `learn_label` / el guardado del proyecto (`midi_learn` ya es `(cc, String)`, así que sólo cambia la etiqueta).
- El camino de aplicación ya existe: `AudioEngine::set_slot_param` → `EngineCommand::SetSlotParam` es RT-safe y vale para los cinco formatos.
- Cómo se arma el binding: seqterm usa **learn universal** — `Ctrl+L` aprende el parámetro que tenga el foco en la vista activa (`seqterm-ui/src/lib.rs:2505`), sin lista de destinos. En choz el equivalente es armar learn desde el modal `INSTRUMENT` con el cursor sobre la fila, además del picker de destinos que ya existe.
- Con la ventana nativa abierta el foco lo tiene el plugin, no la TUI: para aprender "el knob que acabo de mover" haría falta escuchar los cambios de parámetro que el plugin reporta (CLAP los manda en el out-event stream, VST2 por `audioMasterAutomate`). Eso es el modo "toca el knob y luego manda el CC", y es lo que hace falta si se quiere mapear desde la GUI.

### Sesión 2026-08-07 (ter) — la ventana de los plugins VST3

Cierra el punto 1 de Pendiente: era el último formato hosteado sin ventana.

- **`choz-plugin-vst3/src/editor.rs`**: `Vst3Editor` implementa `PluginEditor`, con la misma celda compartida que los otros tres formatos (`Arc<Mutex<Option<ViewCell>>>`, vaciada por el `Drop` de la instancia). La secuencia es `setFrame` → `attached(kPlatformTypeX11EmbedWindowID)` → `getSize`; el `GUI` del RACK y `EditorWindow` funcionan sin tocar `choz-ui`.
- **Lo propio de VST3 en Linux es el run loop.** Un plugin VST3 no recibe idle: registra timers y descriptores en el `Steinberg::Linux::IRunLoop` que obtiene **consultando el `IPlugFrame` que le dio el host**. `HostFrame` es las dos cosas a la vez, y `PluginEditor::idle` es quien dispara los handlers (los fds pasan por `poll(2)` con timeout 0 antes de llamar `onFDIsSet`: un handler tiene derecho a bloquearse en un descriptor que el host declaró listo).
- **La vista se crea al cargar el plugin**, no al pulsar el botón: el `IEditController` sólo es alcanzable en el hilo que construye la instancia, antes de que se la lleve el RT. Si el plugin no da vista o no soporta X11, no hay celda y por tanto no hay botón `GUI`.
- **Tres cosas medidas, no adivinadas**:
  1. **`ComPtr` sobre un handler del plugin lo libera al soltarse.** Cada tick le quitaba una referencia a un objeto ajeno; DPF lo cantó con `Host run loop did not give away timer (refcount -29)`. Se usa `ComRef`, que no toma posesión. Misma trampa acecha en cualquier callback que reciba un puntero COM prestado.
  2. **`IHostApplication::createInstance` devolvía `kNotImplemented`.** Lo único que un plugin le pide al host es un `IMessage`: es el canal por el que su mitad UI habla con su mitad DSP. Sin él, toda UI de DPF abortaba con `assertion failure: "message != nullptr"` al abrirse. Ahora hay `HostMessage` + `HostAttributeList` (int/float/string UTF-16/binary) de verdad.
  3. **`removed()` sobre una vista nunca enganchada es un assert duro en DPF.** El `Drop` de la instancia sólo suelta la vista; desengancha `close()`, que es lo que llama el hilo del editor al salir.
- **`examples/gui_probe`**, con las dos lecciones de las sesiones anteriores incorporadas: el plugin sigue **vivo** mientras se usa su editor, y se cuentan los **hijos X11 reales** (`query_tree`), no los valores de retorno. Además la ventana padre **se crea sin mapear**, así que un barrido no llena el escritorio del usuario — esta máquina no tiene `Xvfb`.
- **Resultado**: **20 de 21 VST3 instalados abren ventana real** con el tamaño que piden (Surge XT 911x890, los Zam, Pianoteq). El que no la abre es el bundle `arm-64bit` de Pianoteq, que en x86 tampoco carga.
- 213 tests (dos nuevos, del run loop), clippy `--all-targets -D warnings` limpio.
- **Sin verificar todavía**: pulsar `GUI` sobre un tab VST3 dentro de la TUI. El probe ejercita el mismo `PluginEditor` que `EditorWindow` conduce, y la captura del handle en el engine es la que ya usan VST2/LV2/CLAP, pero el flujo del botón no se ha visto con un VST3 cargado.

### Sesión 2026-08-07 (quater) — la cuarentena ya no cree a la primera, y los modales tapaban el wallpaper con un agujero

Puntos 3 y 5 de Pendiente.

**Cuarentena (punto 3).** El problema estaba escrito pero sin arreglar: la sonda muestrea **una vez** un crash que es una carrera, y un `Ok` con suerte queda cacheado — el plugin no se sandboxea y cerrar esa tab puede tumbar la app.
- Medido antes de tocar nada, 15 corridas del worker real sobre padthv1: **murió 14, salió limpio 1**. O sea ~7% de probabilidad de cachear el veredicto equivocado por plugin.
- `check()` ahora sondea hasta `PROBE_RUNS = 3` veces y se queda con el **peor** veredicto (`Ok` < `CrashesOnTeardown` < `CrashesOnLoad`); corta en cuanto sale `CrashesOnLoad`, que ya es lo más estricto. Con el 7% medido, tres muestras dejan el fallo en ~0,03%.
- Coste: se paga **una vez por plugin** (el veredicto se cachea) y solo la primera vez que se carga. Medido: 0,21 s por sonda con Surge XT, el instrumento más pesado instalado; 0,00 s con un efecto Zam. `CHOZ_PROBE_RUNS` lo ajusta.

**Fondo de escritorio (punto 5).** Aquí lo interesante es que **la métrica anterior no medía lo que decía**.
- Las "~212 secuencias SGR 49" son el **epílogo que ratatui emite al final de cada frame** (`ESC[39m ESC[49m ESC[59m ESC[0m ESC[?25l`): crecen con los fps, no con los defectos. Medido ahora: 54 en la fase de splash y 771 en 10 s de UI principal, y ninguna es un agujero.
- Lo que sí mide es **inspeccionar el buffer**: test `nothing_punches_a_hole_in_the_desktop` (170x45, escritorio puesto, falla con una sola celda en `Color::Reset`), sobre pantalla normal, cajones abiertos, ADD FX, ABOUT y menú desplegado.
- **Y encontró un defecto real**: los modales hacían `Clear` (que resetea celdas) y luego se dibujaban con `panel_style()`, que con escritorio **no fija fondo** — 612 celdas del color del terminal en medio del wallpaper. Arreglado con `theme::overlay_style()`, opaco siempre: un panel puede ser transparente, un modal no.
- `UiRestore` (el guard de tests) también devuelve la bandera global de escritorio a su sitio.

**Barrido de editores LV2 (punto 4)**: `examples/ui_probe` crea ahora su ventana padre **sin mapear** por defecto — así se puede barrer las ~340 UIs instaladas sin llenar la sesión del usuario (esta máquina no tiene Xvfb). `--mapped` reproduce la condición real de choz, que sí mapea antes de entregar el XID. Con la ventana padre sin mapear y en rebanadas de 40 (un proceso por rebanada), el barrido completo corre **sin un solo crash**, que es lo que nunca se había conseguido: **de 259 UIs declaradas, 254 abren ventana real, 0 abren sin crearla y 5 no llegan a instanciarse** (B.SEQuencer, MIDI Step Sequencer8x8 y los QMidiArp: no tienen salida de audio, así que choz no los hostea como efecto). Otros 388 plugins no declaran UI X11 en su TTL. **Y el barrido destapó un bug de verdad, que es lo que estaba escondido detrás de la cifra anterior** — ver la sesión (quinquies).

- 218 tests, clippy `--all-targets -D warnings` limpio.

### Sesión 2026-08-07 (quinquies) — el fondo a resolución real, y 135 plugins LV2 que se quedaban sin ventana

Pedido: *"arregla la configuración del background image para que las imágenes se puedan ver en resolución real y no pixeleadas"*.

- **`choz-ui/src/views/kitty_bg.rs`**: el wallpaper deja de pasar por halfblocks cuando el terminal habla el **protocolo gráfico de kitty**. La imagen se decodifica y escala **al tamaño real en píxeles de la ventana** (`crossterm::terminal::window_size`, con 8x16 de reserva), se transmite **una sola vez** en trozos de 4 KB y se coloca sobre toda la rejilla (`c`/`r`).
- **La clave es el `z`, y no cualquier negativo sirve.** Cualquier `z < 0` deja la imagen debajo del *texto*, pero por debajo de **-1073741824** queda además debajo de los **fondos de celda**. Es el que hace falta: con `z=-1` la foto taparía cada panel y cada resaltado (que son fondos de celda) y la fila seleccionada perdería su color. Con `z=-2000000000` la imagen se ve exactamente donde nadie pintó fondo, que es lo que `theme::panel_style()` ya garantizaba desde la sesión anterior.
- **No se usa `ratatui-image` para esto**: su backend de kitty coloca las imágenes con *placeholders Unicode escritos en el buffer de celdas*, así que el texto de encima las borra. Por eso el módulo habla el protocolo a mano — y por eso el comentario viejo de `background.rs` ("los protocolos gráficos no sirven para un fondo") era cierto sólo para esa forma de usarlos.
- Detección por entorno (`KITTY_WINDOW_ID`, `TERM` con "kitty", ghostty, WezTerm) porque choz ya tiene el terminal en crudo y una consulta se colaría en la entrada; `CHOZ_KITTY_BG=0` vuelve a halfblocks, que sigue siendo el camino de todos los demás terminales. Al salir se borra la imagen: es del terminal, no del buffer, y no se va con la pantalla alternativa.
- Verificado sobre la salida real: una única `a=T` con `s=1360,v=720,c=170,r=45,z=-2000000000,C=1` precedida de su borrado, y ninguna retransmisión por frame. **Falta verlo con los ojos en una kitty de verdad**, que es lo que el usuario tiene.

**El bug que destapó el barrido de editores LV2** (punto 4 de Pendiente, ahora cerrado):

- Primera corrida completa: 124 con ventana y **135 "sin editor"**, casi todos LSP. La primera hipótesis —que un proceso que carga cientos de UIs sin descargarlas se queda sin algo— **era falsa, y medirla la tiró**: barrer en rebanadas de 40 procesos frescos dio exactamente los mismos números, y `LSP Mixer x4 Mono` probado **solo** también fallaba.
- Causa real: `descriptor_for` recorría `lv2ui_descriptor(0..64)`. **LSP publica un único binario de UI para sus ~390 plugins**, así que todo lo que caía tras el índice 64 no encontraba su descriptor y se quedaba sin ventana en silencio. Ahora el recorrido acaba donde el plugin dice (primer nulo), con un tope de 4096 sólo para que un binario roto no cuelgue choz.
- Resultado tras el arreglo: **254 de 259 UIs abren ventana real, 0 crashes, 0 sin ventana**; las 5 restantes ni siquiera instancian (secuenciadores sin salida de audio, correctamente rechazados).
- `ui_probe` ahora distingue **NOCARGA** de **NOEDITOR**. Mezclar "no instancia" con "no ofrece ventana" es lo que mantuvo el bug escondido durante dos sesiones. Test de regresión en `lv2_runtime.rs`: el **último** plugin LSP con UI declarada tiene que devolver editor (no abre ventana: eso necesita DISPLAY).

### Sesión 2026-08-07 (sexies) — los knobs del plugin llegan al DSP, y el tinte de tema sobre el wallpaper

Cuatro peticiones del usuario en una: tinte con opacidad sobre el fondo, los knobs de la GUI de Surge XT que no hacían nada, todos los parámetros editables y guardados, y MIDI learn desde la ventana del plugin.

**1. "Muevo los knobs en la GUI del VST3 y no pasa nada."** Es el bug de la tanda, y la causa es estructural en VST3: la GUI vive en el **edit controller**, un objeto distinto del procesador. Mover un knob llama `IComponentHandler::performEdit` en el **host** — y si el host no lleva ese valor a `ProcessData.inputParameterChanges`, el knob se mueve en pantalla y el DSP nunca se entera. choz no daba handler y mandaba una lista vacía.
  - `HostComponentHandler` recoge los `performEdit`; `HostParamChanges` deja de ser un stub y publica colas reales (una por parámetro que se mueve, agrupadas en un pool que no vuelve a alojar).
  - Verificado con **Surge XT Effects VST3**: barrer su parámetro 0 cambia el audio renderizado (test de runtime, sin ventana).
  - **Segundo bug encontrado en el mismo sitio**: los parámetros se leían por índice y se escribían como si el índice fuera el id. `getParameterInfo(index)` devuelve un `ParamID` **arbitrario**, y todas las llamadas de valor toman ese id — en Surge y en casi todo lo hecho con JUCE eso movía otro parámetro, o ninguno. Ahora se lee una vez la tabla índice→id.

**2. Todos los parámetros editables y guardados.** `MAX_PLUGIN_PARAMS = 7` truncaba la lista **al construir la entrada de FX**, así que un parámetro más allá del séptimo no se podía tocar ni guardar en el proyecto. La constante desaparece: se guardan todos, y la rejilla de knobs ya se envolvía en filas y hacía scroll desde la sesión del 07-28 (ter), así que no había nada que capar.

**3. MIDI learn dentro de la ventana del plugin.** Con la GUI nativa abierta el ratón es del plugin, no de la TUI, así que "aprende el knob que estoy tocando" sólo puede funcionar si el plugin dice cuál es.
  - Puerto nuevo `choz_ports::ParamTouch` — `take_touched() -> Option<(índice, valor)>`, y leerlo lo consume, para que un gesto viejo no capture un CC que llega mucho después. Lo captura el engine en los mismos puntos que `editor()`/`sandbox()`.
  - La traducción id→índice vive en el crate del formato (`TouchByIndex`), porque todo lo de arriba —knobs, targets de learn, proyectos guardados— direcciona por posición.
  - `App::poll_plugin_touch` lo consulta cada vuelta, sólo para el plugin cuya ventana está abierta. Sirve para dos cosas: **MIDI learn** (learn armado + knob tocado = target elegido) y mantener los valores de choz —y por tanto el **proyecto guardado**— al día con lo que se hizo dentro de la GUI.
  - Hoy lo reporta **VST3**. VST2 (`audioMasterAutomate`), CLAP (stream de eventos de salida) y LV2 (write callback de la UI) tienen su equivalente y quedan pendientes.

**4. Tinte de tema sobre la imagen de fondo, con deslizador.** Un fondo de celda es **opaco**: "un panel translúcido sobre el wallpaper" no existe en un terminal. El único sitio donde puede vivir un color a medias es **dentro de la propia imagen**, así que el color de escritorio del tema se mezcla en los píxeles antes de dibujarlos — mismo `tint_rgba` para el camino halfblocks y para el gráfico de kitty, desde el mismo valor.
  - Fila `Theme tint` en Settings → THEME con barra y porcentaje, `←/→` en pasos de 5 %. Sólo aparece con una imagen puesta. Al moverlo se tiran los dos cachés (la clave de ambos incluye el tinte), así que se ve al instante.
  - Por defecto **45 %**: suficiente para leer knobs y etiquetas sobre una foto cargada sin que la foto desaparezca.
  - **Lo que no se puede hacer así**: tintar cada panel por separado con su propio color. La imagen se transmite entera y una vez; teñir por rectángulo obligaría a retransmitirla en cada frame (megabytes), y con los fondos de celda no hay alfa. Si hace falta, la salida es dejar la imagen sin teñir y que los paneles pinten opaco.

- 220 tests, clippy `--all-targets -D warnings` limpio.
- **Sin ver a ojo**: el tinte y los knobs de Surge en la TUI real. El sonido sí está medido; la pantalla no.

### Sesión 2026-08-07 (septies) — el deslizador sin lag, y paneles translúcidos con el color del tema

Dos quejas del usuario que resultaron ser la misma decisión mal tomada.

- **El lag**: el tinte se mezclaba dentro de la imagen, así que cada pulsación del deslizador (y cada cambio de FIT) pagaba **decodificar el JPEG + Lanczos3 + retransmitir megabytes** por el protocolo de kitty. La imagen no tenía por qué depender del tinte.
- **El arreglo es también lo que el usuario pedía después**: el lavado se mueve a los **paneles**. `views::theme::wash(buf, rect)` mezcla el color de escritorio del tema con **lo que la foto muestra en esa celda**, a la opacidad elegida. Mover el deslizador es entonces un redibujado normal — nada se reconstruye — y cada cuadro se ve translúcido, con las letras legibles y el fondo visible.
- **Cómo sabe un panel lo que hay detrás**: `background::cell_colors` reduce la imagen a **un color por celda** y ambos caminos lo publican (`theme::set_backdrop`). Bajo kitty esto no es una optimización sino la única vía: la imagen está *por debajo* de los fondos de celda y el buffer no la contiene.
- **La decodificación se cachea aparte** (`decode_cached`, una entrada por ruta). Es el paso caro y no depende del tamaño, del fit ni del tema; sin eso, cambiar FIT seguía costando una decodificación entera.
- Barras de menú y estado: lavado al 60 %, o dos franjas sólidas cruzarían la imagen.
- El deslizador pasa a llamarse **`Panel opacity`**; el ajuste guardado sigue siendo `background_tint` en `ui.json`.
- **Cerrojo de tests compartido**: `views::theme::ui_guard`. `has_desktop` y el backdrop son globales de proceso, y un test de `background` que no lo tomaba fallaba de vez en cuando contra el `UiRestore` de otro.
- 222 tests, clippy limpio.

### Sesión 2026-08-07 (octies) — el lavado no puede ser un fondo de celda

Reporte del usuario tras la sesión anterior: *"se perdió la resolución de la imagen de background"*. Cierto, y la causa es la misma frase de siempre desde el otro lado: **un fondo de celda es opaco**. Lavar cada panel pintando fondos de celda tapa la imagen que dibuja el terminal y la reduce a un color por celda — la cuadrícula que el protocolo gráfico existía para evitar.

- **kitty: el lavado es una segunda imagen.** Color del tema con canal alfa, colocada con `z` **una unidad por encima de la foto** y aún por debajo de los fondos de celda, así que el terminal compone foto + lavado y el texto se dibuja sobre ambos. La foto se transmite **una vez**; la máscara se re-manda sólo cuando cambian los rectángulos, el color o la opacidad.
  - Máscara a **4 píxeles por celda** (680×180 contra 1360×720 de la foto): el terminal la escala, y a 1 píxel por celda la interpolación difuminaba el borde de un panel a lo ancho de una celda entera. Sigue siendo ~100× más pequeña que la foto, que es lo que mantiene instantáneo el deslizador.
  - Los rectángulos los publica `ui()` en `App.wash_rects`; la máscara se manda en el bucle, un fotograma por detrás de la distribución (invisible) y sin recalcular ninguna disposición aparte — el error que el roadmap ya tenía anotado.
- **halfblocks: mezclar los dos píxeles de la celda.** El `▀` lleva el píxel superior en `fg` y el inferior en `bg`; lavar sólo el fondo tiraba la mitad de la resolución vertical antes de que nada se dibujara encima. Ahora se mezclan los dos y el glifo se deja como está.
- `theme::Backdrop` gana `graphics: bool`: con la imagen en manos del terminal, `wash` no toca ninguna celda.
- **Trampa de tests repetida**: `CHOZ_KITTY_BG` es global del proceso y el harness paraleliza por función, así que los dos tests que lo tocaban se pisaban. Fusionados en uno, como ya hubo que hacer con `plugin_scan` y con los runtime de VST2/VST3.
- 222 tests, clippy limpio. Verificado en la salida real: la foto sale **una** vez a 1360×720, la máscara **una** vez a 680×180, y la UI principal no emite un solo fondo de celda por encima.

### Sesión 2026-08-07 (nonies) — MIDI learn desde la GUI del plugin: faltaban VST2 y LV2

Reporte: *"el botón midi learn aún no afecta la gui de los plugin… traté de asignar el volumen de TyrellN6 con el Keystation sin resultados"*. El mecanismo estaba, pero **sólo VST3 reportaba lo que el usuario tocaba**, y **TyrellN6 es VST2**: no llegaba nada que aprender.

- **VST2**: los movimientos de la GUI llegan por `audioMasterAutomate`. El problema de fondo es que el callback del host en VST2 es un **puntero a función sin contexto**: lo único que identifica al emisor es el `AEffect`. Así que los feeds viven en una tabla indexada por ese puntero, dada de alta al construir la instancia y **borrada en su `Drop`** — otro plugin puede aterrizar en la misma dirección más tarde.
- **LV2**: la UI ya escribía por el callback del host (es la única forma que tiene de mover un control), así que sólo hubo que anotar el puerto y el valor. `Lv2Touch` traduce **puerto → índice de parámetro** y el valor plano a 0..1: la UI de LV2 habla de puertos y unidades propias, choz de posiciones normalizadas.
- **Falta CLAP**: reporta los cambios en el stream de eventos de salida del `process`, que corre en el hilo de audio — es el único de los cuatro que exige tocar el camino RT.
- **Medido con TyrellN6 de verdad** (`CHOZ_VST2_DIR=/ruta cargo test -p choz-plugin-vst2`): carga, expone parámetros, ofrece el feed, y **barrer sus parámetros desde choz cambia el sonido**, que es exactamente lo que hará el CC una vez asignado. La variable de entorno existe porque los directorios estándar de esta máquina sólo tienen efectos Zam; sin ella el test se salta esa mitad y CI sigue verde.
- Tests nuevos: el callback de VST2 sin plugin (llega al feed correcto, leerlo lo consume, una instancia muerta no cruza feeds) y la cadena entera en la UI (knob tocado → learn elige → CC liga → CC mueve; con learn desarmado sólo se actualiza el valor).

**Cómo se usa**: abrir la ventana del plugin con `GUI`, pulsar `MIDI LEARN`, mover el knob **dentro de la ventana del plugin** y luego el fader del controlador. El banner del panel INPUTS lo dice mientras está armado.

### Sesión 2026-08-08 — CLAP cierra `ParamTouch`, y el patch del plugin entra en el proyecto

- **`ParamTouch` en CLAP** (los cuatro formatos con ventana lo tienen ya, instrumentos y efectos). CLAP no tiene callback de host para esto: el plugin empuja eventos `param_value` en el **stream de salida de `process`**, o sea en el **hilo de audio**. Por eso se lee con `try_lock` y se descarta bajo contención — perder un evento de un barrido de knob no se nota, bloquear el hilo de audio sí.
- **Bug que sólo se ve al juntar los cuatro**: la UI buscaba el parámetro tocado **por id** cuando cada host ya traduce a **índice**. Con VST3 pasaba desapercibido (sus ids arrancan en 0..n); un id de CLAP o un puerto LV2 son arbitrarios y el knob no se habría encontrado nunca. El test usa ids que a propósito no son posiciones.
- **Punto 3 de Pendiente, cerrado para tres formatos: el estado del plugin.** `choz_ports::PluginState` (`save`/`restore`), capturado por el engine donde se capturan el editor y el feed — el único momento en que la UI puede alcanzar al plugin antes de que se lo lleve el hilo RT.
  - **VST2**: chunks (`effGetChunk` devuelve un puntero que es del plugin; se copia antes de soltarlo). **VST3**: `IComponent::getState` sobre el `MemStream` que ya existía, y el `setComponentState` del controller aparte, o su ventana mostraría el patch viejo. **CLAP**: `clap.state`, con los dos callbacks de stream escritos a mano.
  - **El orden importa al reconstruir un rack**: primero el patch, después los valores de los knobs. Restaurar el estado mueve todos los parámetros, así que al revés la pestaña sonaría al patch guardado y se vería con los knobs guardados.
  - Se guarda como **base64 en el YAML** (`instrument.state` y el `state` de cada FX): un proyecto sigue siendo un archivo legible y un patch son unos pocos kilobytes.
  - Un patch cuyo plugin no está en esta máquina **se conserva**: borrarlo convertiría un plugin que falta en un sonido perdido la próxima vez que se guarde.
  - **Falta LV2**: su extensión `state` necesita además las features de mapeo de rutas.
- Verificado con plugins reales: un VST3 (mover parámetro → guardar → instancia nueva → mismo blob) y TyrellN6 en VST2 (mismo viaje con su chunk), más el viaje completo por el YAML en la UI.
- 228 tests, clippy limpio.

### Sesión 2026-08-08 (bis) — el estado de los plugins LV2

Cierra el punto 3 de Pendiente: VST2, VST3, CLAP y ahora LV2 guardan su patch en el proyecto.

- **LV2 no entrega un blob.** El plugin llama de vuelta una vez por propiedad, con un **URID** de clave y otro de tipo. Y un URID **sólo significa algo dentro de una ejecución**: los números los reparte el mapa de este host. Así que lo que se escribe son las **URIs**, resueltas por el mismo store que acuñó los números (`UridStore::uri`), y vueltas a mapear al restaurar.
- **Formato del blob**: plano y autodescriptivo — `[count]` y luego, por propiedad, clave / tipo / flags / valor con sus longitudes. Nada en él depende del proceso que lo escribió. Un archivo truncado o de otro sitio se rechaza **entero**, en vez de entregarle media propiedad al plugin.
- Los valores devueltos durante `restore` se aparcan en el mismo `Bag`: el puntero que recibe el plugin tiene que seguir vivo durante toda la llamada.
- **Lo que no se implementa**: `state:mapPath` (para plugins que guardan rutas de archivos) ni `state:threadSafeRestore`. Un plugin que exija el primero fallará al guardar y choz lo tratará como "sin estado"; el segundo es un apretón de manos que choz no hace — se restaura entre bloques, como con el editor.
- Medido con los plugins instalados: **3 LV2 guardan y recuperan su estado** a través de una instancia nueva, byte a byte.
- 230 tests, clippy limpio.

### Sesión 2026-08-08 (ter) — los knobs genéricos del plugin en el RACK

Pedido: *"Carla, al abrir un plugin, muestra knobs y sliders y el engranaje abre la GUI; poné esos knobs sobre FX CHAIN para asignar MIDI sin abrir la ventana"*.

- **Caja `INSTRUMENT` en el panel RACK**, encima de `FX CHAIN`: los parámetros del plugin de la pestaña como knobs, con su nombre y su valor. El botón `GUI` sigue donde estaba — la ventana real es una opción, no el único camino.
- **Es sobre todo un atajo de MIDI learn**: `learn_target_at` reconoce los rects de esos knobs, así que `MIDI LEARN` → clic en el knob → mover el fader liga el CC **sin abrir la ventana del plugin**, que en un sinte grande es la parte lenta.
- `k` pasa el cursor de una caja a la otra; las flechas y `w`/`s` actúan sobre la enfocada, y el título muestra `[k]` cuando no lo está. Clic selecciona, rueda gira.
- **Una sola función dibuja las dos cajas** (`draw_knob_box`): el bloque que dibujaba los knobs del FX se extrajo tal cual, así que ambas heredan el envoltorio en filas y el scroll que sigue al cursor que ya existían. La del instrumento se limita a `INSTR_KNOB_ROWS = 2` filas para no comerse la cadena; el resto se alcanza con el cursor.
- Test: con una pestaña de plugin y dos parámetros, el render real dibuja sus nombres, deja un rect clicable por knob, el clic mueve el foco y el cursor, `w` gira **ese** knob (no uno del FX) y el puntero de learn devuelve `InstrParam`.
- **Sin ver a ojo**: la caja con un plugin de verdad cargado en la TUI. El render está cubierto por test y los nombres/valores salen de `read_params`, que sí está medido contra plugins reales.
- 231 tests, clippy limpio.

### Sesión 2026-08-08 (quater) — la nota congelada y el botón PANIC

Reporte: *"abrí un LV2 e-piano y TyrellN6 en otra pestaña, ambas funcionan, pero TyrellN6 se quedó congelado sonando una nota"*.

- **Causa**: el ruteo de notas se resuelve **por evento** y depende de la pestaña activa — el piano QWERTY toca siempre la activa (`note_targets` lo dice explícitamente), y varias pestañas sobre un mismo puerto MIDI se turnan. Soltar la nota después de cambiar de pestaña mandaba el note-off al **otro** instrumento y dejaba el primero sonando. No es un fallo del plugin.
- **Arreglo**: `App.sounding` recuerda a qué slots fue cada note-on (`start_note`) y el note-off vuelve a esos mismos (`end_note`); lo que choz nunca vio empezar cae en el ruteo actual, como antes. Vale para el piano QWERTY (incluida su auto-suelta por tiempo) y para el MIDI real.
- **Botón `PANIC`** en TRANSPORT, tecla `P` global (y `p` con TRANSPORT enfocado):
  - `EngineCommand::Panic` recorre los slots y manda **un note-off real por cada nota pisada** — el engine lleva un `u128` por slot — y después el `all notes off` general. Los note-offs exactos son lo único que funciona en todos los formatos: `all notes off` es un **CC**, y un VST3 no ve los CC como eventos.
  - **Un solo comando para todo el rack**, no una ráfaga: llenar el anillo a medias dejaría el último slot sonando, justo lo que el botón existe para evitar.
  - `AudioSource::all_notes_off()` por defecto = CC 120 + CC 123; SF2 usa `AllSoundOff` de oxisynth (corta las colas), SFZ suelta sus voces.
  - Cambiar el instrumento de un slot pone su máscara a cero: el nuevo no hereda notas del anterior.
- **Trampa de tests, otra vez**: añadir un test de runtime CLAP puso dos funciones a cargar plugins de u-he en paralelo y el binario se caía. Mutex de archivo, como en LV2 (VST2/VST3 lo resolvieron fusionando funciones; el mutex conserva los nombres).
- 233 tests, clippy limpio.

### Sesión 2026-08-08 (quinquies) — LIVE / MULTI

Pedido: *"un switch en la esquina superior derecha para ejecutar choz para un instrumento o para muchas entradas MIDI"* — tocar en vivo cambiando de pestaña con los botones del teclado, o ser un módulo multitímbrico tipo Kontakt para una plantilla orquestal en Reaper.

- Son dos oficios que tiran del ruteo en direcciones opuestas, así que es un **switch**, no una heurística: `settings::RackMode::{Live, Multi}`, guardado en `ui.json`.
  - **LIVE**: suena una pestaña; varias sobre un puerto son alternativas. **Un program change sin asignar selecciona pestaña** (PC 0 = tab 1) — lo que hace un rig en vivo. Una asignación de MIDI learn gana, para no pisar a quien ya mapeó ese botón.
  - **MULTI**: todas suenan a la vez, cada una por **su canal MIDI**. Qué pestaña esté activa no interviene.
- **El canal MIDI ya no se tira.** `parse` lo sacaba del status byte y lo descartaba; ahora viaja en `NoteMsg`/`CcMsg` hasta el ruteo. Sin eso, el modo MULTI no era implementable.
- **Switch en la barra de menú, pegado a la derecha** (`UiLayout.mode_switch_rect`), clic o `F4`. `App::targets_for(source, channel)` es el único punto que consulta el modo; `note_targets` (LIVE) y `multi_targets` (MULTI) siguen siendo funciones puras con tests.
- **Cambiar de modo o de canal hace panic primero**: los dos ruteos mandan las notas a pestañas distintas, así que lo que estuviera sonando nunca recibiría su note-off. Es la misma trampa que dejaba notas colgadas al cambiar de pestaña, ahora prevista.
- Pestañas nuevas en canales consecutivos (1, 2, 3…), botón `CH n` en la línea INSTR solo en MULTI, y el canal va al proyecto con `#[serde(default)]` para los archivos viejos.
- 235 tests, clippy limpio.

### Sesión 2026-08-08 (sexies) — la ventana del plugin dentro del sandbox

Punto 1 de Pendiente, y la razón por la que el sandbox existe: la GUI es donde más revienta el código de terceros — **las 31 UIs de guitarix segfaultean en esta máquina** — y hasta ahora un editor que caía se llevaba choz.

- **La ventana la abre el hijo, en su proceso.** El transporte gana un canal de control (`editor_seq`/`editor_cmd`/`editor_parent`/`editor_ack`/`editor_size`), mismo estilo de atómicas que el audio.
- **El XID cruza el proceso**: un identificador de ventana X11 vale en todo el display. choz crea la ventana (título, botón de cierre del WM, tamaño) y el hijo empotra la del plugin dentro — lo que hacen los puentes de plugins desde siempre. No hace falta un protocolo de dibujo.
- **En el hijo, la ventana tiene su propio hilo.** Abrir la GUI de un sinte grande tarda cientos de milisegundos; el rendezvous de audio no puede esperar a un toolkit. El hilo hace `open`, manda el tamaño por un canal, y luego bombea `idle` cada 30 ms. El bucle de audio sólo mira si hay petición entre bloques.
- **`SandboxEditor` es un `PluginEditor` como cualquier otro**, así que el botón `GUI` y `EditorWindow` no se enteran. Su `idle` está vacío a propósito: el toolkit sólo se puede tocar desde el hilo del hijo.
- `Shm` pasa a ser `Arc` y `Sync`: el mango de la ventana lo tiene el hilo de UI y tiene que mantener viva la misma zona compartida.
- **Lo que queda de este punto**: (1) ofrecer de nuevo los editores de la deny-list (guitarix) **cuando el plugin va sandboxeado** — el mecanismo ya está, falta la política; (2) un plugin sandboxeado sin ventana abre un marco vacío, porque el host no puede saber si la tiene sin preguntarle al hijo, y al capturar el mango el hijo aún está cargando.
- Tests: el apretón de manos entero sobre un `Vec<u8>` (sin mapear nada ni lanzar procesos) y, con un plugin real en su proceso, que la petición cruza sin que el audio pierda un solo bloque.
- 236 tests, clippy limpio.

## Pendiente (en orden de ROI)

Lo hecho vive en las secciones de sesión de arriba; aquí sólo queda lo que falta.

1. **Política del editor sandboxeado**: el mecanismo está (08-08 sexies), falta usarlo. (a) Ofrecer los editores que hoy están en la deny-list — guitarix — **cuando el plugin corre sandboxeado**, que es justo lo que el aislamiento hace seguro. (b) Un plugin sandboxeado sin ventana abre un marco vacío: el hijo tendría que publicar "tengo editor" al arrancar y el host esperarlo.
2. **Mirar con audio real** el modal INSTRUMENT: Surge XT (CLAP), Yoshimi (LV2), WhySynth/hexter (DSSI) y los VST2 nuevos (TyrellN6, TripleCheese).
3. **`state:mapPath` para LV2**: un plugin que guarde rutas de archivos (samplers) no puede guardar su estado hoy. Necesita las dos funciones de mapeo de rutas y devolver cadenas que el plugin libera con `free`.
4. **Mirar el fondo de kitty con los ojos**: la secuencia emitida está verificada byte a byte, pero nadie ha visto todavía la imagen bajo la TUI en una kitty real. Y probar `--mapped` en `ui_probe` alguna vez: el barrido limpio se hizo con la ventana padre sin mapear.
5. Nice-to-have: ruteo por canal MIDI dentro de un puerto; automatización.

## Notas / gotchas para el que retome

- **Los probes de editores abren ventanas de verdad.** `examples/ui_probe` (LV2) y `examples/gui_probe` (CLAP) instancian plugins y abren su GUI: usar `Xvfb` (ver Pendiente 4) y **matarlos al terminar**. `sweep.sh` reanuda tras cada segfault por diseño, así que colgado sigue insistiendo indefinidamente. Ningún test abre ventanas, y así debe seguir — `vst2_runtime.rs` lo dice explícitamente donde toca un editor.
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
cargo build --workspace                 # todos los hosts van en el build normal
cargo test --workspace
# barridos largos: hostear TODOS los plugins instalados de un formato
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --bin choz          # necesita una terminal real (tty)
tail -f ~/.local/state/choz/choz.log    # ver errores/log en vivo

# Probes de editores: ABREN UNA VENTANA POR PLUGIN. En un display aparte,
# y matarlos al terminar.
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run -p choz-plugin-lv2  --example ui_probe
DISPLAY=:99 cargo run -p choz-plugin-clap --example gui_probe
# El de VST3 crea su ventana padre SIN mapear, así que no ensucia el escritorio.
cargo run -p choz-plugin-vst3 --example gui_probe
```
