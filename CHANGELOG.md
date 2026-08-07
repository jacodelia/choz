# Changelog

Todos los cambios notables de choz. Formato basado en
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/).

El proyecto todavía no publica versiones: hay un solo tramo `Unreleased` desde
el commit inicial, agrupado por día de trabajo. El detalle largo de cada tanda
(diagnósticos, medidas, callejones sin salida) vive en
[docs/roadmap.md](docs/roadmap.md); aquí va sólo qué cambió.

## [Unreleased]

### 2026-08-06 — MIDI hotplug y latencia real

#### Añadido
- `App::poll_midi_hotplug()`: el bucle principal compara la lista de puertos MIDI cada 2 s y reconecta si cambió. Un controlador enchufado con choz ya abierto ahora suena sin tocar nada.
- `engine::request_pipewire_period()`: exporta `PIPEWIRE_QUANTUM` además de `PIPEWIRE_LATENCY` antes de abrir el cliente JACK, con piso `MIN_FORCED_QUANTUM = 128`.
- `midi::is_disabled()`: coincidencia por nombre de cliente o prefijo `"Cliente:"`.
- [docs/audio-latency.md](docs/audio-latency.md): diagnóstico, configuración de PipeWire/WirePlumber/ALSA de la máquina y comandos para verificarla.
- Este `CHANGELOG.md`.

#### Corregido
- **Un controlador enchufado después del arranque no producía sonido.** `connect_midi()` sólo corría al arrancar, al conmutar un puerto o con `r`; el puerto aparecía en la lista de la UI pero sin suscripción ALSA.
- **`disabled_midi_inputs` no filtraba nada.** El default `["Midi Through"]` nunca coincidía con los nombres de midir (`"Cliente:Puerto n:m"`), así que el puerto de loopback se conectaba igual.
- **Latencia clavada en 1024 frames (21,3 ms)** pese a `buffer_size: 256`. pipewire-jack abre cada cliente JACK con `node.lock-quantum` y un `node.force-quantum` heredado, que le gana a `node.latency`. Ahora el buffer de la UI manda: 128/48000 = 2,7 ms.
- **Guardar un proyecto se llevaba el sample rate, buffer y backend viejos**: el snapshot los leía del engine (que sigue corriendo los valores anteriores, porque esos tres sólo se aplican al siguiente arranque) en vez de la configuración pendiente.
- **`plugin_scan` segfaulteaba una corrida de cada tres**: sus dos tests corrían en paralelo llamando a `scan_all`, y como un binario de test no es un scan worker el escaneo cae en proceso — dos hilos dlopeneando plugins JUCE/VST3, que hacen init global al cargar. Fusionados en una función.
- **`quarantine` fallaba de forma intermitente** (`check(padthv1) == Ok`): el crash de padthv1 es una carrera entre su hilo Qt y `cleanup`. El test pasa a exigir lo estable; el problema de fondo —una sonda que muestrea una vez un crash no determinista— queda como pendiente en el roadmap.

### 2026-08-06 (bis) — ventana nativa de los plugins LV2

#### Añadido
- **Editor X11 para LV2** (`choz-plugin-lv2/src/editor.rs`): `Lv2Editor` implementa `PluginEditor`, así que los botones `GUI` del RACK funcionan sin tocar la UI. Una UI LV2 vive en un binario aparte y nunca toca la instancia — habla con el host por un callback de escritura — así que funciona con el plugin en el hilo RT.
- Descubrimiento de `ui:X11UI` en el TTL, vinculada al plugin por `ui:ui`, `lv2:appliesTo` o descarte cuando el bundle tiene una sola (lo que cubre a DPF).
- Se respeta el `requiredFeature` de la UI: la que pide algo fuera de `SUPPORTED_UI_FEATURES` no recibe editor. `instance-access`/`data-access` quedan fuera a propósito.
- `examples/ui_probe`: abre y cierra cada UI instalada en una ventana X real.

#### Corregido
- El `Drop` de `Lv2Instance` vacía los controles compartidos antes que nada, incluso en el camino de leak: una ventana abierta cuando desaparece su slot deja de escribir en memoria liberada.
- Leer el `requiredFeature` de la UI copiando el grafo del bundle por plugin volvía el escaneo cuadrático y colgaba el barrido en bundles grandes.

#### Seguridad
- Deny-list por prefijo para guitarix: sus 31 UIs segfaultean al instanciar, con la ventana mapeada y sin mapear, y aisladas en un proceso propio. LSP **no** está en la lista: las suyas que fallan cambian en cada corrida del barrido, así que no es propiedad de ningún plugin.

### 2026-08-06 (ter) — fuera JSFX

#### Eliminado
- **Todo rastro del formato JSFX**: `PluginFormat::Jsfx` y sus rutas por defecto (`paths.rs`), los chips de los modales **ADD FX** y **CHANGE SOURCE** (`main.rs`), y las ramas de los stubs viejos `scanner.rs` / `registry.rs` / `plugin_types.rs`. Nunca se hosteó: sólo se escaneaba y se listaba como "(not hosted yet)".

#### Corregido
- **Un `plugin-paths.json` con un formato desconocido ya no se descarta entero.** Al quitar JSFX, el archivo guardado dejaba de parsear y `load()` caía a `Default`, borrando en silencio las rutas que el usuario hubiera añadido a mano. Ahora el formato se guarda por su etiqueta y las entradas desconocidas se saltan una a una. Verificado contra el config real de la máquina: los 8 formatos y los directorios propios sobreviven.
- **`lv2_runtime` segfaulteaba de forma intermitente en `cargo test --workspace`**: sus tests corren en hilos distintos y todos dlopenean los mismos plugins, varios de los cuales hacen init global al cargar. Serializados con un mutex del archivo, que conserva los seis nombres de test en vez de fusionarlos como se hizo en VST2/VST3.

### 2026-08-07 (quater) — el fondo pasa por ratatui-image

#### Cambiado
- **El wallpaper se dibuja con `ratatui-image` en modo halfblocks** en vez del downsampling propio: cada celda es `▀` con la mitad de arriba en `fg` y la de abajo en `bg`, o sea **dos píxeles por celda** en vez de uno promediado. Medido sobre 150×40 con `assets/wallpaper.jpg`: 6000/6000 celdas pintadas, **2867 con dos tonos** y **6131 colores distintos** (el techo del método anterior era 6000 colores y cero celdas de dos tonos).
- **Halfblocks a propósito, no kitty/sixel.** Los protocolos gráficos dibujan fuera del modelo de celdas, en una capa que el terminal compone encima: sirven para una imagen en una caja (el logo del About) pero no para un *fondo*, porque la UI dibujada después no la taparía y el texto quedaría ilegible. Halfblocks escribe en el buffer de celdas, así que todo lo demás sigue pintando encima con normalidad.
- La imagen se escala a la medida exacta del área con **Lanczos3** antes de pasar por el picker: `Fit`/`Scale` mantienen la proporción y dejarían franjas sin cubrir, y `Crop` se niega a ampliar una imagen más chica que el área — que es el caso normal, porque una foto de 740×423 es más pequeña que un terminal de 150×40 a 8×16 por celda.

#### Añadido
- El test de imagen exige ahora celdas con `fg != bg`: es lo único que un color promediado por celda no puede producir, así que es la prueba de que la resolución extra está ahí.
- `measure_resolution` (`#[ignore]`): imprime celdas pintadas, celdas de dos tonos y colores distintos para cada wallpaper de `assets/`.

### 2026-08-07 (ter) — el fondo no se veía: `Color::Reset` no es transparente

#### Corregido
- **La imagen de fondo no aparecía.** `theme::panel_bg()` devolvía `Color::Reset` para dejar pasar el fondo, pero `Reset` **no es transparencia**: ratatui lo emite como SGR 49, el fondo por defecto del terminal, que pintaba encima del wallpaper. Lo único que respeta el buffer es **no fijar `bg`**, así que las funciones pasan a devolver un `Style` (`panel_style()` / `app_style()`) sin fondo cuando hay escritorio. Medido en la TUI real: de 34 colores distintos a **1628**.
- **El botón SELECT ahora cierra el modal** aplicando lo que esté bajo el cursor, en vez de depender de que la pestaña decida. Añadida además una fila **"Apply and close"** para quien lo maneja con el teclado, porque en la pestaña THEME Enter mantiene el modal abierto a propósito — probar esquemas no debería costar reabrir Settings cada vez.

#### Añadido
- Test `a_panel_drawn_on_top_does_not_erase_the_background`: pinta el fondo, dibuja un panel encima y comprueba que el color sobrevive. Es el que faltaba — los seis anteriores pasaban con el bug puesto.
- Test `a_theme_and_a_wallpaper_survive_leaving_the_modal_together`: el flujo completo (Obsidian → wallpaper → salir), incluido que quede en disco y que re-marcar el tema no borre la imagen.

### 2026-08-07 (bis) — temas de color y fondo de escritorio

#### Añadido
- **Temas al estilo Notepad++** (`settings::THEMES`): once esquemas —Obsidian, Zenburn, Solarized Dark/Light, Monokai, Deep Black, Vibrant Ink, Ruby Blue, Bespin, Hello Kitty, más el de choz— que fijan **texto, marcos y escritorio a la vez**. La pestaña `COLOR` de Settings pasa a llamarse **`THEME`**.
- **Fondo de escritorio**: por defecto el del terminal, o un color liso, o **una imagen** en modo `STRETCH` o `TILE`. Se dibuja como color de fondo por celda, así que funciona en cualquier terminal — sin sixel ni kitty — y se cachea, decodificando sólo cuando cambia el archivo o el tamaño de la ventana.
- **Selector de imagen tipo explorador**: el navegador de archivos ahora filtra por varias extensiones a la vez (`png`/`jpg`/`jpeg`/`bmp`/`gif`/`webp`) y arranca en `assets/` del proyecto cuando existe.
- `theme::border()` sale del tema en vez de derivarse atenuando el color de texto.

#### Cambiado
- **Los paneles dejan de pintar su fondo opaco cuando hay escritorio configurado** (`theme::panel_bg()` / `app_bg()` devuelven `Reset`). Sin eso el fondo sólo se veía en los huecos entre paneles, que no es un fondo.

### 2026-08-07 — la ventana de los plugins CLAP

#### Añadido
- **Extensiones del lado host** (`host.rs`): `clap.gui` (`HostGuiImpl` sobre `ChozShared`) y `clap.timer-support` (`HostTimerImpl` sobre el nuevo `ChozMainThread`), declaradas en `declare_extensions`. Una UI CLAP dibuja desde `on_timer`, así que sin el timer del host la ventana se creaba y no pintaba nunca.
- `PluginEditor::idle` tiquea los timers que el plugin registró.

#### Cambiado
- El editor CLAP deja de estar detrás de `CHOZ_CLAP_GUI`: **20 de 20 plugins instalados abren su ventana** con el tamaño que piden, Surge XT incluido.

#### Corregido
- **`examples/gui_probe` y `examples/ui_probe` medían sobre un plugin ya destruido**: `.and_then(|i| i.editor())` consume el instrumento, y su `Drop` vacía la celda compartida que protege a las ventanas huérfanas. Por eso la sesión anterior concluyó que CLAP "no dibujaba". Ambos probes mantienen el plugin vivo ahora, y cuentan ventanas X11 reales con `query_tree`.
- **`ui_probe` perdía resultados en silencio**: los acumulaba en un `Vec` volcado al final, y stdout a un archivo va en bloques, así que cada pasada que moría por un segfault se llevaba sus líneas — una corrida perdió 74 sin que se notara. Ahora cada resultado se imprime y se vuelca en el momento.

#### Documentación
- `docs/architecture.md` puesto al día: nueve crates (faltaba `choz-plugin-sandbox`), los módulos nuevos (`jack_backend`, `sfz`, `quarantine`, `sandboxed`, los dos `editor.rs`), las tres capas que sobreviven al código de terceros, una tabla de ventanas nativas por formato, los tres hilos que van y vienen además de los tres fijos, y los archivos de estado que faltaban.
- `README.md`: tabla de formatos con el estado real de cada ventana nativa.

### 2026-08-06 (quater) — CLAP: editor a medias, apagado por defecto

#### Añadido
- `choz-plugin-clap/src/editor.rs`: acceso a `clap.gui` por el puntero crudo (`clap-sys` 0.5, la misma versión que usa clack, así que los layouts coinciden). `create`/`set_parent`/`show` responden bien en los 20 CLAP instalados, sin crashes.
- `examples/gui_probe`: comprueba con `query_tree` si el plugin creó ventana de verdad, en vez de creer el valor de retorno.

#### Conocido
- **No se crea ninguna ventana**, aunque las llamadas digan que sí. Falta declarar las extensiones del lado host (`clap_host_gui`, `clap.timer_support`) sobre `ChozHost`, que hoy no expone ninguna. Por eso el editor queda tras `CHOZ_CLAP_GUI=1` y el botón `GUI` no aparece en slots CLAP.

### 2026-08-04 — sandbox completo

#### Añadido
- `SandboxedEffect` (`FxProcessor`) sobre el mismo enlace de memoria compartida; el wet/dry se aplica del lado host.
- Los cambios de parámetro cruzan al hijo: cola de hasta 32 `(índice, valor)` por bloque, como el MIDI. Un instrumento sandboxeado ya tiene knobs.
- **Sandbox a mano por plugin**: `quarantine::{forced, set_forced}` en `<state dir>/plugin-sandbox.json`, con clave `formato|ruta|id`. Sobrevive a recargar el proyecto y un proyecto ajeno no arrastra la decisión.
- `choz_ports::SandboxStatus` + `sandbox()` como método por defecto de `AudioSource`/`FxProcessor`. Botón **`SBX`** en el RACK (`x` instrumento, `X` efecto): `○` en proceso, `● (reload)` pedido, y en verde `● 3 lost 1↻` corriendo fuera.

#### Cambiado
- La política de sandbox de los dos sitios que la aplican es ahora una sola función, `quarantine::wants_sandbox`.

### 2026-08-03 — hosting fuera de proceso, editores nativos, SFZ

#### Añadido
- **Ventana nativa del plugin (VST2)**: puerto `choz_ports::PluginEditor` + `editor()` por defecto en `AudioSource`/`FxProcessor`; ventana X11 con `x11rb` en su propio hilo (`choz-ui/src/editor.rs`). Botones `GUI` en el RACK (`g` instrumento, `G` efecto), una ventana a la vez. Verificado con ZamTube al tamaño que pide el plugin.
- **MIDI learn universal**: `LearnTarget::InstrParam{slot,param}` — los parámetros del instrumento ya son mapeables, no sólo los del FX chain.
- **Instrumentos SFZ** (`choz-engine/src/sfz.rs`): parser del subconjunto real + `SfzSampler` de 32 voces. A diferencia de seqterm, **todas las muestras se decodifican al cargar** — `note_on` no hace I/O ni reserva. Dep nueva `symphonia` (FLAC además de WAV).
- **Cargar proyectos**: `Project::load` + `App::apply_project`, `File → Open project…` y `choz proyecto.yml`. Botón **`RACK ONLY`** (`k`) para no pisar la configuración local.
- **Dispositivo de entrada aparte del de salida**: `input_device()` / `set_input_device()`, `jack_sources()`, `capture_channels()`. El cajón IN lista los dispositivos de captura.
- **Escaneo fuera de proceso**: un hijo por (formato, directorio) vía `--choz-scan-worker`; si muere, el padre reintenta entrada por entrada. Sólo se pierde el plugin que revienta.
- **Cuarentena** (`quarantine.rs`): antes de cargar un plugin por primera vez se lo prueba en un hijo y se guarda el veredicto en `plugin-verdicts.json`. `CrashesOnLoad` se rechaza; `CrashesOnTeardown` se carga y se filtra la instancia.
- **Transporte de sandbox** (`choz-plugin-sandbox`): memoria compartida POSIX + protocolo de cita con **plazo** — si el hijo no llega, el host lee silencio y sigue. Medido: 1,26 µs de viaje medio, 36 µs el peor, 0 bloques perdidos en 5000.
- **`SandboxedPlugin`** (`sandboxed.rs`): un `AudioSource` normal que por dentro habla con un hijo. **Hilo supervisor** que relanza el proceso si muere — verificado con SIGKILL en vivo: un chasquido, no una tab muda.
- **Feature LV2 `worker#schedule`**: de los 32 bundles instalados que la piden, 29 hostean y procesan finito.

#### Corregido
- **TyrellN6 segfaulteaba**: el `host_callback` de VST2 respondía `0` a `audioMasterGetTime`, y el plugin desreferencia la respuesta sin comprobar null. Ahora devuelve un `VstTimeInfo` con transporte fijo (120 BPM, 4/4). Afecta a cualquier plugin que sincronice un LFO, delay o arpegiador.
- **No volver a `dlclose` un binario LV2** (`LOADED_LIBS`): los `*v1` de Rui arrastran Qt con sus hilos, y descargar el `.so` bajo sus pies revienta dentro del *loader*.
- **El stdout de los plugins pintaba sobre la TUI**: `log::take_terminal()` hace `dup(1)` para quedarse una copia privada de la terminal y le entrega el fd 1 al log. Medido: 34 líneas de u-he al log, 0 en pantalla.
- `slot_label` etiquetaba `CLAP:` a todos los instrumentos de plugin desde que entraron LV2/VST/DSSI.
- El parser SFZ cortaba `sample=` en el primer espacio, así que la única librería SFZ de la máquina no cargaba ni una muestra.

### 2026-08-02 — cajones IN/OUT y salida multicanal

#### Añadido
- **Cajones laterales colapsables** (`views/drawer.rs`): IN y OUT dejan de ser una columna fija y un modal. `F2`/`F3` los abren, `Esc` cierra el enfocado. Ambos arrancan cerrados: el RACK ocupa todo.
- **Backend JACK nativo** (`jack_backend.rs`): cliente propio con **un puerto por canal del dispositivo** (`choz:out_1..out_N`), autoconectado canal por canal. Con la UMC1820: 12 out / 10 in. cpal-jack sólo daba un par estéreo.
- **Ruteo por slot**: `Slot.out_pair` + `EngineCommand::SetSlotOut`; el mezclador escribe en un buffer por canal.
- **Entradas de audio**: `Slot.in_pair` + `RtState.capture` — un slot alimentado por un par de captura pasa el audio vivo por su cadena FX.

#### Eliminado
- Modal de salida (`ModalKind::Device`), reemplazado por el cajón OUT.

### 2026-07-31 — hosting de todos los formatos

#### Añadido
- **`choz-plugin-lv2`**: parser TTL propio (`rio_turtle`) + ABI LV2 a mano, sin lilv. Añadido sobre el port de seqterm: features `opts:options` y `bufsz:boundedBlockLength`, sin las cuales DPF/Dragonfly/Zam no instanciaban. De 547 efectos instalados, 524 hostean.
- **`choz-plugin-ladspa`**: LADSPA + DSSI (comparten descriptor). `DssiInstrument` traduce MIDI a eventos ALSA de 28 bytes para `run_synth`.
- **`choz-plugin-vst2`**: `VSTPluginMain` + `processReplacing` + `effProcessEvents`.
- **`choz-plugin-vst3`**: bindings COM puros (`vst3` 0.3), sin editor nativo.

#### Cambiado
- Se generalizó el plumbing que era CLAP-only: `PluginParam` (era `ClapParamInfo`), `PluginFxRef` (era `ClapFxRef`), `SourceAction::Plugin{format,…}`.
- **Sin feature flags de hosting**: todos los hosts se compilan siempre. La antigua feature `clap` (off por defecto) hacía que los plugins CLAP no se vieran en un build normal.

#### Seguridad
- Deny-list por prefijo para los plugins que son hosts (Carla `carlarack`, `carla.vst`): no petan, **corrompen el asignador**, así que ni se intenta cargarlos.

### 2026-07-29 — AUDIO SETTINGS

#### Añadido
- Pestaña `AUDIO` del modal de Settings con las tres subcategorías de seqterm: **Engine** (backend, device, sample rate, buffer, latencia calculada), **Plugin Paths** y **OSC**.
- `osc::listen` devuelve un `OscHandle` (socket con timeout + flag atómica): mover OSC de puerto ya no pide reiniciar.
- Ajustes persistidos en `ui.json` con `#[serde(default)]`, para que los archivos viejos sigan cargando.

### 2026-07-28 — modales, FX importados, i18n, proyecto en YAML

#### Añadido
- **Un solo widget de modal** (`views/modal.rs`): scrollbar, chips de filtro, botones SELECT/CANCEL y rects de click para *todos* los modales. Con **barra lateral** de categorías en ADD FX.
- **Cuatro FX de seqterm**: Protocosmos, Space Echo, Reverse Delay y Z5 Texture (16 params).
- **Dos pedales de distorsión** con waveshaping a 2× oversampling: **AMBER FANG** (clipping asimétrico, armónicos pares) y **VELVET FUZZ** (dos soft-clips en cascada, medios escarbados).
- **MIDI learn con puntero**: modo `?` sobre el ratón; click elige el control sin moverlo, el siguiente CC queda ligado. Incluye **botones** (`LearnTarget::Trigger`) con disparo en flanco de subida — DEL queda fuera a propósito.
- **Rutas de plugins tipo Carla** (`paths.rs`): 8 formatos con sus directorios por defecto, respetando `LV2_PATH`/`VST_PATH`/…, editables en la UI con EDIT/ADD/BROWSE/REMOVE/DEFAULTS.
- **Settings**: paleta de 9 colores de texto (que tiñe también los bordes) e **i18n** en 9 idiomas, con el texto en inglés como clave.
- **`File → Save project…`**: `choz-project.yml` con las dos mitades, sonido y configuración.
- **Rediseño del RACK**: el panel calcula sus propios rects; los knobs se reparten en una rejilla que baja de línea; ON/MOVE/DEL viven en su propia caja `SLOT`.

#### Corregido
- Un directorio añadido no mostraba nada: se guardaba en el formato de la fila donde estaba el cursor (sin decirlo), quedaba desactivado, y editar las rutas no invalidaba el caché. Ahora cada directorio dice lo que aportó, incluido `(0 — holds 73 SF2 file(s), move it to SF2)`.
- En ADD FX no se podían elegir las categorías con el ratón.

### 2026-07-27 — primeros bugs de hosting CLAP

#### Corregido
- **Layout de puertos**: se pasaba un único puerto estéreo fijo; un plugin mono o con sidechain rechazaba los buffers. Ahora `port_layout()` lee todos los puertos.
- **SIGSEGV al soltar un plugin**: `ClapProc` no guardaba el `PluginEntry`, así que la librería se `dlclose`-aba con la instancia viva.
- **NaN del plugin**: `ZamEQ2` devuelve no-finitos antes de fijarle parámetros; ahora se descartan en vez de mezclarlos.
- `ZaMaximX2` segfaultea dentro de su propio `deactivate`: se deja vivo a propósito. `CHOZ_CLAP_STRICT_TEARDOWN=1` restaura el teardown correcto.

### 2026-06-09 — base

#### Añadido
- Workspace de crates: `choz-ports` (traits RT), `choz-engine` (audio/DSP/MIDI), `choz-ui` (binario `choz`).
- Engine RT-safe: callback sin locks ni alloc, rack multi-source `Vec<Slot>`, handoff por ring `EngineCommand` + ring `Retired` para los drops fuera del RT.
- Fuentes WAV (`hound`) y SF2 (`oxisynth`); 27 FX DSP built-in.
- Mixer por slot (gain, pan constant-power, mute, solo).
- MIDI hardware (`midir`) y OSC (`rosc`).
- TUI con ratatui: menú, RACK con tabs, piano QWERTY, log a `~/.local/state/choz/choz.log`.

---

## Estado actual

- **211 tests** con harness + 4 binarios de test propios (`quarantine`, `sandboxed_plugin`, `scan_isolation`, `across_a_process`, todos con `harness = false` porque tienen que poder ser workers).
- `cargo clippy --workspace --all-targets -D warnings` limpio.
- **1209 plugins** escaneados en la máquina de desarrollo (611 efectos LV2 + 36 instrumentos, 342 LADSPA, 18 CLAP + 2 instrumentos, 17 VST2, 18 VST3 + 1 instrumento, 2 DSSI, 53 SFZ, 103 SF2).
- `cargo test --workspace` completa entero, sin `--no-fail-fast`.
