# Changelog

Todos los cambios notables de choz. Formato basado en
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/).

El proyecto todavía no publica versiones: hay un solo tramo `Unreleased` desde
el commit inicial, agrupado por día de trabajo. **Éste es el historial**: los
diagnósticos, las medidas y los callejones sin salida se cuentan aquí.
[docs/roadmap.md](docs/roadmap.md) sólo lleva lo que falta, y
[docs/architecture.md](docs/architecture.md) cómo encajan las piezas.

## [Unreleased]

### 2026-08-08 (sexies) — la ventana del plugin, dentro del sandbox

#### Añadido
- **Un plugin que corre en su propio proceso abre ahí su ventana.** Es el punto 1 de Pendiente y la razón de ser del sandbox: la GUI es donde más revienta el código ajeno (en esta máquina, **todas** las UIs de guitarix segfaultean), y hasta ahora un editor que caía se llevaba choz entero. Ahora cae el hijo, y el supervisor que ya existía levanta otro.
  - El transporte de `choz-plugin-sandbox` gana un canal de control para la ventana: `editor_seq` / `editor_cmd` / `editor_parent` / `editor_ack` / `editor_size`, con el mismo estilo de rendezvous por atómicas que el audio.
  - **El XID viaja entre procesos**: un identificador de ventana X11 vale en todo el display, así que choz crea la ventana y el hijo empotra la del plugin dentro — que es lo que hacen los puentes de plugins de toda la vida.
  - En el hijo, la ventana vive en **su propio hilo**: abrir una GUI grande tarda cientos de milisegundos y el rendezvous de audio no puede esperar a un toolkit. El hilo hace `open`, responde el tamaño por un canal y luego bombea `idle` cada 30 ms.
  - `SandboxEditor` (lado host) es un `PluginEditor` normal, así que el botón `GUI` del RACK y `EditorWindow` funcionan sin enterarse. Su `idle` no hace nada: el que bombea es el hijo, único sitio donde se puede tocar el toolkit.
- Tests: el apretón de manos completo sobre un `Vec<u8>` sin mapear memoria ni lanzar procesos (petición → el hijo la ve **una** vez → responde tamaño → el host lo lee; y el cierre igual sin ventana), y con un plugin real en su proceso, que la petición cruza y **el audio no se entera** (mismos bloques perdidos antes y después).

### 2026-08-08 (quinquies) — LIVE / MULTI: el mismo rack para dos oficios

#### Añadido
- **Switch `LIVE | MULTI` en la esquina superior derecha** (clic, o `F4` desde cualquier sitio). Decide qué hace cada nota que entra, así que vive a la vista y no dentro de un menú. Se guarda en `ui.json`: es cómo está montada la máquina, no un capricho de sesión.
  - **LIVE** (como hasta ahora): suena **una** pestaña. Las pestañas son los temas o los patches de un set, varias pueden compartir un puerto y son alternativas, no capas. **Un program change sin asignar selecciona pestaña** (PC 0 = tab 1), que es lo que hacen los botones de un teclado en un rig en vivo; una asignación de MIDI learn sigue teniendo prioridad.
  - **MULTI**: **todas las pestañas suenan a la vez**, cada una respondiendo a **su canal MIDI** — un módulo multitímbrico para la plantilla orquestal de un DAW (Reaper → choz, al estilo de Kontakt). El canal manda; qué pestaña esté activa da igual.
- **El canal MIDI se conserva** desde el cable hasta el ruteo (`NoteMsg.channel`, `CcMsg.channel`); antes se descartaba. Las pestañas nuevas caen en canales consecutivos (1, 2, 3…), que es la disposición que manda un DAW por defecto.
- **Botón `CH n`** en la línea INSTR del RACK, solo en MULTI: clic para cambiar el canal de la pestaña. Se guarda en el proyecto (`slot.channel`, con default para proyectos viejos).
- Cambiar de modo o de canal **hace panic primero**: los dos ruteos mandan las notas a pestañas distintas, así que lo que estuviera sonando nunca recibiría su note-off — la misma trampa que dejaba una nota colgada al cambiar de pestaña.

### 2026-08-08 (quater) — la nota que se quedaba sonando, y el botón PANIC

#### Corregido
- **Una nota podía quedarse sonando para siempre al cambiar de pestaña** (reportado con TyrellN6 mientras sonaba también un e-piano LV2). El ruteo se resolvía **por evento** y depende de cuál es la pestaña activa: el piano QWERTY toca siempre la activa, y varias pestañas sobre un mismo puerto MIDI se turnan. Soltar la tecla después de cambiar de pestaña mandaba el note-off **al otro instrumento** y dejaba el primero sonando. Ahora choz recuerda a qué slots fue cada note-on y **el note-off sigue a su note-on**; una nota que choz nunca vio empezar cae en el ruteo actual, como antes.

#### Añadido
- **Botón `PANIC`** en el panel TRANSPORT (`P` desde cualquier sitio, `p` con el TRANSPORT enfocado): mata todo lo que esté sonando. Un `EngineCommand::Panic` recorre todos los slots y, por cada uno, manda un **note-off real por cada nota que ese slot tiene pisada** (el engine lleva un `u128` de notas por slot) y después el `all notes off` general.
  - Los note-offs exactos son lo que funciona en todas partes: `all notes off` es un **CC de MIDI**, y un plugin VST3 no ve los CC como eventos.
  - Un solo comando para todo el rack, no una ráfaga de note-offs: llenar el anillo a medias dejaría el último slot sonando, que es justo lo que este botón existe para arreglar.
  - `choz_ports::AudioSource::all_notes_off()` por defecto manda CC 120 y CC 123; el motor de SoundFont usa `AllSoundOff` de oxisynth (corta también las colas) y el sampler SFZ simplemente suelta sus voces.

#### Corregido (tests)
- Los tests de runtime de CLAP se serializan con un mutex, como los de LV2: al añadir uno más, dos funciones cargaban plugins de u-he a la vez y el proceso se caía — la misma trampa que ya obligó a fusionar los de VST2/VST3.

### 2026-08-08 (ter) — los knobs del plugin en el RACK, al estilo Carla

#### Añadido
- **Caja `INSTRUMENT` en el panel RACK**: los parámetros del plugin de la pestaña, como knobs, sin abrir su ventana. Es lo que hace Carla — una fila de knobs genéricos y un botón aparte para la GUI real — y sirve para lo que pediste: **asignar MIDI rápido**. El puntero de MIDI learn los reconoce, así que `MIDI LEARN` + clic en un knob + mover el fader liga el CC sin pasar por la ventana del plugin.
  - `k` cambia entre la caja del instrumento y la del FX seleccionado; las flechas y `w`/`s` mueven la que tenga el cursor. El título de la caja muestra `[k]` cuando no lo tiene, para que se vea cuál manda.
  - Clic en un knob lo selecciona (y le pasa el foco); la rueda encima lo gira.
  - Comparte el dibujo con la caja de knobs del FX (`draw_knob_box`), que ya envolvía en varias filas y hacía scroll siguiendo al cursor. La del instrumento se limita a `INSTR_KNOB_ROWS = 2` filas para no comerse la cadena de FX; el resto se alcanza con el cursor.

### 2026-08-08 (bis) — el estado de los plugins LV2

#### Añadido
- **`state#interface` de LV2** (`choz-plugin-lv2/src/state.rs`), que cierra el punto 3 de Pendiente: los cuatro formatos con ventana guardan ya su patch en el proyecto.
  - LV2 no entrega un blob: el plugin llama de vuelta una vez por propiedad, con un **URID** de clave y otro de tipo. Y un URID **sólo significa algo dentro de una ejecución** — los números los reparte el mapa de este host —, así que lo que se guarda son las **URIs** que representan, resueltas por el mismo store que las acuñó y vueltas a mapear al restaurar.
  - Formato del blob: plano y autodescriptivo (`[count]`, y por propiedad clave, tipo, flags y valor con sus longitudes), para que un proyecto escrito aquí cargue en cualquier máquina con el mismo plugin. Un archivo truncado o ajeno se rechaza entero en vez de entregarle media propiedad al plugin.
  - Verificado con plugins instalados: **3 LV2 guardan y recuperan su estado** a través de una instancia nueva, byte a byte.

### 2026-08-08 — el patch del plugin viaja en el proyecto, y CLAP también reporta sus knobs

#### Añadido
- **`ParamTouch` para CLAP**, que cierra los cuatro formatos con ventana (VST3, VST2, LV2, CLAP), **instrumentos y efectos**. CLAP no tiene callback para esto: el plugin anuncia los movimientos de su GUI empujando eventos `param_value` en el **stream de salida de `process`**, que corre en el hilo de audio — de ahí que se lea con `try_lock` y se descarte si hay contención: perder un evento de un barrido no cuesta nada, bloquear el hilo de audio cuesta un corte.
- **El estado propio del plugin se guarda en el proyecto** (`choz_ports::PluginState`): el patch elegido en el navegador del plugin, una wavetable, la ruta de un sample — nada de eso es un parámetro y todo se perdía al reabrir. Implementado en **VST2** (chunks, `effGetChunk`/`effSetChunk`), **VST3** (`IComponent::getState` sobre el `IBStream` de memoria que ya existía) y **CLAP** (`clap.state`, con las dos devoluciones de llamada de stream escritas a mano). Va en el YAML como base64, en `instrument.state` y en el `state` de cada FX.
  - Al reconstruir un rack (cargar proyecto, cambiar de dispositivo de salida) **el patch se aplica primero y los valores de los knobs encima**: restaurar el estado mueve todos los parámetros, así que al revés la pestaña sonaría al patch guardado y se vería con los knobs guardados.
  - Un patch cuyo plugin no está instalado en esta máquina **se conserva igualmente**, para que abrir el proyecto en otro sitio no lo borre al volver a guardarlo.
  - Falta **LV2** (extensión `state`, que además necesita las features de mapeo de rutas).

#### Corregido
- La UI buscaba el parámetro tocado **por id del plugin** cuando el feed ya entrega **índice**. Con VST3 no se notaba (sus ids empiezan siendo 0..n), pero un id de CLAP o un número de puerto LV2 son arbitrarios: el knob movido en la ventana no se habría encontrado. El test lo fija con ids que deliberadamente no son posiciones.

#### Verificado
- VST3 real: mover un parámetro, guardar el estado, cargarlo en una instancia nueva y recuperar el mismo blob.
- VST2 real (TyrellN6): mismo viaje con su chunk.
- UI: el patch entra en el YAML, vuelve por él y los knobs se aplican encima.

### 2026-08-07 (undecies) — MIDI learn desde la ventana del plugin, ahora también en VST2 y LV2

#### Corregido
- **`MIDI LEARN` no aprendía nada al mover un knob dentro de la ventana del plugin** (reportado con TyrellN6 y un Keystation Pro 88). El mecanismo existía desde la tanda anterior, pero **sólo VST3 reportaba** lo que el usuario tocaba, y TyrellN6 es **VST2**: no había nada que aprender.
  - **VST2**: el plugin anuncia los movimientos de su GUI con `audioMasterAutomate`, y el callback del host es un puntero a función **sin contexto** — lo único que recibe es el `AEffect` de quien llama. Los feeds se guardan ahora en una tabla indexada por ese puntero, dada de alta al cargar la instancia y **borrada en su `Drop`** (otro plugin puede reutilizar la misma dirección).
  - **LV2**: la UI ya escribía por el callback del host (es como mueve los controles); ahí se anota el puerto y su valor. `Lv2Touch` traduce **índice de puerto → índice de parámetro** y el valor a 0..1, que es como choz direcciona los knobs.
  - Queda **CLAP**, que reporta los cambios en su stream de eventos de salida durante el proceso.

#### Verificado
- Con **TyrellN6 real** (`CHOZ_VST2_DIR=… cargo test -p choz-plugin-vst2`): carga, expone parámetros, ofrece el feed de la ventana y **barrer sus parámetros desde choz cambia el sonido** — que es lo que hace un CC ya asignado.
- Test de la cadena entera en la UI: knob tocado en la ventana → learn elige ese parámetro → el siguiente CC lo liga → los CC posteriores lo mueven, y un toque con learn desarmado sólo actualiza el valor (y con él el proyecto guardado) sin re-ligar nada.
- Test del callback VST2 sin plugin: `audioMasterAutomate` aterriza en el feed de la instancia que llamó, leerlo lo consume, y una llamada desde una instancia ya destruida no cruza feeds ni revienta.

### 2026-08-07 (decies) — el lavado ya no aplasta la imagen

#### Corregido
- **El fondo perdía resolución en cuanto había paneles encima.** El lavado pintaba **un fondo de celda opaco** por cuadro: bajo el protocolo gráfico eso tapa la imagen (los fondos de celda van por encima de ella) y la deja en un color por celda, justo la cuadrícula que este camino existía para evitar. Ahora:
  - **En kitty el lavado es una segunda imagen**: el color del tema con canal alfa, colocada **encima de la foto y debajo del texto** (`z` una unidad por encima). La foto se transmite **una vez** y no se vuelve a tocar — mantiene todos sus píxeles — y sólo se re-manda la máscara cuando cambia la distribución, el color o la opacidad. La máscara va a 4 píxeles por celda (680×180 frente a 1360×720 de la foto), lo bastante nítida en los bordes y dos órdenes de magnitud más pequeña, que es lo que hace instantáneo el deslizador.
  - **En halfblocks se mezclan los dos píxeles de la celda**, no sólo el fondo. Un `▀` lleva el píxel de arriba en `fg` y el de abajo en `bg`; lavar sólo el fondo tiraba la mitad de la resolución vertical antes de dibujar nada encima.
- Los tests del camino gráfico que dependen del entorno se fusionan en **una** función: `CHOZ_KITTY_BG` es global del proceso y el harness paraleliza por función, así que dos tests tocándolo se pisaban.

### 2026-08-07 (nonies) — el deslizador va fluido, y los paneles se ven a través

#### Corregido
- **El deslizador de tinte y el cambio de FIT iban a tirones.** El tinte se mezclaba **dentro de la imagen**, así que cada pulsación pagaba decodificar el JPEG, reescalarlo con Lanczos3 y —en kitty— retransmitir varios megabytes. Ahora el lavado vive en los **paneles**, no en la imagen: mover el deslizador es un redibujado, no una reconstrucción. Y la decodificación se cachea aparte (`background::decode_cached`), que es lo que arregla el FIT: cambiarlo ya sólo reescala.

#### Añadido
- **Los cuadros de cada sección se dibujan translúcidos sobre el fondo**, con el color de escritorio del **tema activo**: `views::theme::wash` mezcla ese color con lo que la foto muestra **en esa misma celda**, usando una tabla de un color por celda (`background::cell_colors`) que ambos caminos de dibujo publican. Así las letras y los knobs se leen y el fondo se sigue viendo. Las barras de menú y de estado usan un lavado más suave (60 %), para no convertirse en dos franjas sólidas.
  - El deslizador de Settings → THEME pasa a llamarse **`Panel opacity`** (0–100 %, `←/→` en pasos de 5). Sigue guardándose en `ui.json` como `background_tint`.
  - Bajo el protocolo gráfico de kitty el buffer de celdas no contiene la imagen (está por debajo), así que la tabla por celda es lo **único** que los paneles pueden saber de lo que hay detrás. Se calcula al transmitir.
- Tests: la mezcla por celda (mitad foto, mitad tema; 0 % y 100 % en los extremos) y un render completo de `ui()` sobre una foto real comprobando que dentro del RACK no hay ni celdas al fondo del terminal, ni un color plano, ni el color puro del tema.
- El cerrojo de los tests que tocan estado global vive ahora en `views::theme::ui_guard`, para que los tests de los paneles puedan tomarlo también (uno de ellos fallaba de forma intermitente por eso).

### 2026-08-07 (octies) — los knobs del plugin ya llegan al sonido, y un tinte de tema sobre el wallpaper

#### Corregido
- **Mover un knob dentro de la ventana del plugin VST3 no cambiaba el sonido** (reportado con Surge XT). En VST3 la GUI vive en el **edit controller**, que es otro objeto que el procesador: al mover un knob el plugin llama `IComponentHandler::performEdit` en el host y nada más. choz no daba handler y mandaba una lista de cambios vacía, así que el knob se movía en pantalla y el DSP no se enteraba. Ahora hay `HostComponentHandler`, los cambios se encolan y viajan en `ProcessData.inputParameterChanges` del bloque siguiente. Verificado con Surge XT Effects: barrer su parámetro 0 cambia el audio renderizado.
- **Los parámetros VST3 se leían por índice y se escribían como si el índice fuera el id.** `getParameterInfo` toma un índice y devuelve un `ParamID` arbitrario; todas las llamadas de valor toman ese id. En plugins cuyos ids no coinciden con la posición (Surge y casi todo lo hecho con JUCE) eso movía otro parámetro, o ninguno. La tabla índice→id se lee una vez al cargar.
- **Los efectos de plugin sólo guardaban 7 parámetros.** `MAX_PLUGIN_PARAMS` truncaba la lista al construir la entrada, así que lo que pasaba del séptimo no se podía editar **ni guardar en el proyecto**. La constante desaparece: se almacenan todos, y la rejilla de knobs ya se envolvía en varias filas y hacía scroll desde julio.

#### Añadido
- **Tinte de tema sobre la imagen de fondo, con deslizador** (Settings → THEME → `Theme tint`, `←/→` en pasos de 5 %): el color de escritorio del tema elegido se mezcla **dentro de la imagen**, que es el único sitio donde un terminal puede dar media transparencia — el fondo de una celda es opaco. Vale igual para el camino halfblocks y para el gráfico de kitty, desde el mismo valor. Por defecto 45 %, suficiente para leer knobs y etiquetas sobre una foto cargada.
- **choz sigue los knobs que el usuario mueve dentro de la ventana del plugin**: nuevo puerto `choz_ports::ParamTouch` (`take_touched() -> Option<(índice, valor)>`), capturado por el engine junto al editor. La UI lo consulta cada vuelta (`poll_plugin_touch`) y con eso (1) sus propios valores —y por tanto **el proyecto guardado**— siguen lo que se hizo en la GUI del plugin, y (2) **MIDI learn puede aprender un knob de la ventana del plugin**: con learn armado, el knob que se toque allí queda seleccionado y el siguiente CC lo controla. Hoy lo reporta VST3; VST2/CLAP/LV2 tienen su equivalente (`audioMasterAutomate`, el stream de eventos de salida, el write callback de la UI) y quedan pendientes.

### 2026-08-07 (septies) — fondo a resolución real en kitty, y el tope que dejaba a 135 plugins LV2 sin ventana

#### Añadido
- **Fondo de escritorio por el protocolo gráfico de kitty** (`choz-ui/src/views/kitty_bg.rs`): la imagen se transmite una vez, escalada al **tamaño real en píxeles de la ventana**, y se coloca **por debajo de los fondos de celda** (`z=-2000000000`), así que el texto y los resaltados se dibujan encima sin taparla. Se acabó el pixelado del modo halfblocks, que está limitado a 2 píxeles por celda. Se detecta por entorno (`KITTY_WINDOW_ID` / `TERM` con "kitty" / ghostty / WezTerm) y `CHOZ_KITTY_BG=0` vuelve al camino anterior, que sigue intacto para el resto de terminales.
  - La colocación se re-emite solo cuando cambia el archivo, el ajuste o el tamaño; se borra al salir, porque la imagen es del terminal y no se va con la pantalla alternativa.
- Test de regresión LV2: el **último** plugin de un bundle que comparte binario de UI tiene que dar editor.

#### Corregido
- **`descriptor_for` sólo miraba los 64 primeros `lv2ui_descriptor(i)`.** LSP publica **un** binario de UI para sus ~390 plugins, así que todo lo que caía más allá del índice 64 se quedaba sin ventana en silencio: **135 plugins** en el barrido. El recorrido termina ahora donde el plugin dice (primer nulo), con un tope de 4096 sólo para que un binario roto no cuelgue choz.
- `ui_probe` distingue **NOCARGA** (el plugin no instancia) de **NOEDITOR** (instancia pero no ofrece ventana). Mezclarlos fue lo que escondió el bug de arriba.

### 2026-08-07 (sexies) — cuarentena a tres sondas, y agujeros en el fondo

#### Añadido
- **`nothing_punches_a_hole_in_the_desktop`**: renderiza la UI a 170x45 con un escritorio puesto y falla si queda **una sola celda en `Color::Reset`**. Cubre pantalla normal, cajones abiertos, modal ADD FX, ABOUT y el menú desplegado.
- `views::theme::overlay_style()`: relleno **siempre opaco** para lo que se dibuja *encima* del cuerpo (modales, menú, ABOUT). Van precedidos de `Clear`, que resetea las celdas, y un wallpaper asomando por debajo de un modal es un agujero, no una gracia.
- `CHOZ_PROBE_RUNS` para ajustar cuántas veces se sonda un plugin antes de darlo por bueno.

#### Corregido
- **La cuarentena muestreaba una sola vez un crash que es una carrera.** Medido en esta máquina: padthv1 revienta al destruirse **14 de cada 15** corridas, así que una sonda única lo declaraba `Ok` cada ~7 intentos, cacheaba ese veredicto, dejaba el plugin sin sandbox y cerrar esa tab tumbaba la app. Ahora `check()` sondea hasta 3 veces y se queda con el **peor** veredicto (`Ok` < `CrashesOnTeardown` < `CrashesOnLoad`), cortando en cuanto sale el peor posible. Coste: solo la primera vez por plugin y cacheado; 0,21 s por sonda con el instrumento más pesado instalado (Surge XT).
- **Los modales dejaban un rectángulo del color del terminal sobre el wallpaper**: `Clear` + `panel_style()` (que con escritorio no fija fondo) = 612 celdas en `Color::Reset`. Ahora usan `overlay_style()`.
- El `UiRestore` de los tests también devuelve la bandera global de escritorio a su sitio.

#### Cambiado
- `examples/ui_probe` (LV2) crea su ventana padre **sin mapear** por defecto, para poder barrer las ~340 UIs instaladas sin llenar la sesión del usuario; `--mapped` reproduce la condición real de choz.
- **La cifra de "~212 secuencias SGR 49" del roadmap no medía agujeros**: son el epílogo que ratatui emite al final de *cada frame* (`ESC[39m ESC[49m ESC[59m ESC[0m`), así que crecía con los fps y no con los defectos. Lo que sí mide es inspeccionar el buffer, que es lo que hace el test nuevo.

### 2026-08-07 (quinquies) — ventana nativa de los plugins VST3

#### Añadido
- **Editor X11 para VST3** (`choz-plugin-vst3/src/editor.rs`): `Vst3Editor` implementa `PluginEditor` (`IPlugView`: `setFrame` → `attached(X11EmbedWindowID)` → `getSize`), así que los botones `GUI` del RACK funcionan sin tocar la UI. Era el último formato hosteado sin ventana.
- **`HostFrame` = `IPlugFrame` + `Steinberg::Linux::IRunLoop`**: un plugin VST3 no tiene callback de idle — registra timers y descriptores en el run loop que obtiene consultando su frame. `PluginEditor::idle` es quien los dispara (los fds se comprueban con `poll(2)` antes de llamar `onFDIsSet`). Sin run loop, un editor JUCE ni se engancha.
- **`IHostApplication::createInstance` construye `IMessage`/`IAttributeList` de verdad** (antes devolvía `kNotImplemented`): es el canal por el que la mitad UI de un plugin habla con su mitad DSP. Sin él, la UI de cualquier plugin DPF abortaba con `assertion failure: "message != nullptr"` nada más abrirse.
- `examples/gui_probe`: abre cada VST3 instalado en una ventana X real y **cuenta los hijos X11** en vez de creer en los valores de retorno. La ventana padre se crea sin mapear, así que un barrido no llena el escritorio del usuario. Medido: **20 de 21 abren ventana real** con el tamaño que piden (Surge XT, los Zam, Pianoteq); el que no es el bundle `arm-64bit` de Pianoteq, que en x86 tampoco carga.
- Nueva dependencia `libc` en el crate VST3 (solo para `poll`), y `x11rb` como dev-dependency del probe.

#### Corregido
- **Los handlers del run loop se envolvían en `ComPtr`, que libera al soltarse**: cada tick le quitaba una referencia a un objeto que es del plugin. DPF lo cantó (`Host run loop did not give away timer (refcount -29)`). Ahora se usa `ComRef`, que no toma posesión.
- El `Drop` de `Vst3RealInstance` vacía la celda compartida antes de terminar nada, y **no** llama a `removed()`: hacerlo sobre una vista nunca enganchada dispara un assert duro en DPF. El desenganche lo hace `close()`, que es lo que llama el hilo del editor al salir.

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
