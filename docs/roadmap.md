# choz — Pendiente

Qué falta. **Lo cerrado no vive aquí**: está en [CHANGELOG.md](../CHANGELOG.md),
día por día, con los porqués y lo último arriba; cómo encajan las piezas, en
[architecture.md](architecture.md). Este documento se poda cada vez que un punto
se cierra, para que lo que quede sea sólo lo que queda: se podó entero el
2026-08-19 y otra vez el 2026-08-29, y las dos veces lo que decía "hecho" se fue
al changelog.

Última actualización: 2026-08-29.

## Estado en una línea

Los seis formatos de plugin (CLAP, LV2, LADSPA, DSSI, VST2, VST3) se escanean,
se hostean y abren su ventana nativa; el rack es multi-slot con mixer, FX,
ruteo canal por canal y proyectos en YAML; entra audio en vivo por JACK (cada
jack del grafo) y por ALSA/PulseAudio/PipeWire (un dispositivo de captura
elegible en Settings), así que también es un multiefecto; hay tres capas contra
el código ajeno que revienta —escaneo fuera de proceso, cuarentena y sandbox—;
hay transporte propio con compás, automatización contra ese reloj, `A→M` (audio
a notas), AutoTune, un arpegiador y un secuenciador por tab; 46 efectos propios
(**la suite está completa y auditada**: se apilan sin pasarse de escala, y su
dry/wet es una sola ley), que además se publican como un `.clap` —los dos
artifacts incluidos— para usarlos en cualquier otro host; un looper multipista
con sus tiras de canal y exportación a WAV; un patch de Max se importa hasta donde se
puede, diciendo qué no; hay guardia de acople en la entrada; el mixer tiene un
main **estéreo** y cuatro subgrupos; una tab guarda sus sonidos en botones —que
se asignan desde el mismo modal de bank/preset— y puede partir el teclado entre
ellos; y un efecto puede abrirse con el bombo de otra tab —por su nivel o por
las notas que se le tocan—, con el clock, o con el tap del metrónomo. El MIXER
se maneja entero desde el teclado, grupos y main incluidos, y cada strip lleva
su `O M S` bajo el fader. Los mandos de un plugin se agrupan por lo que el
plugin dice —las unidades de VST3, el módulo de CLAP— y los que tienen
posiciones con nombre abren su lista en los cuatro formatos que saben decirlas. Un plugin que guarda sus controles fuera de sus puertos
—ZynAddSubFX— se maneja por su propio servidor OSC: mandos con nombre, los
armónicos del oscilador, su ventana real, y los mandos leyendo lo que el plugin
tiene. **La 1.0.0 está publicada y sus paquetes verificados; la
1.3.4 es este árbol.**
808 tests, `clippy --workspace --all-targets -D warnings` limpio.

Las comprobaciones con hardware delante quedaron dichas en los gotchas, que es
donde se van a leer.

---

## Pendiente

**No hay ningún punto pedido abierto.** Todo lo que se pidió está cerrado y
contado día por día en el changelog; lo que sigue son los bordes que cada cierre
dejó dichos, la pieza que se decidió no hacer, y lo que hay que saber para
retomar. Un punto que se cierra sale de aquí — este documento es lo que queda,
no lo que hubo.

### Fuera de la lista, por decisión del 2026-08-19

La sección **artifact** — arpegiador + piano roll de 128 pasos portado de
seqterm. Es la pieza más grande de las pedidas, no compite con las demás por
tiempo, y se replantea aparte.

Lo que sí existe, y cubre parte de lo que aquella sección iba a traer: cada tab
tiene su **arpegiador** y su **secuenciador de pasos** —hecho como un MMT-8, con
su propio reloj contra el transporte—, y los dos se publican como plugins CLAP.
Lo que no está es la sección como tal: una vista aparte con el piano roll largo.

### Lo que la auditoría de efectos dejó dicho

Las once fases están cerradas —[fx-audit.md](fx-audit.md) tiene el archivo y la
línea de cada hallazgo, el changelog lo que se hizo con cada uno—. Esto no son
tareas: es lo que hay que saber antes de tocar un efecto.

- **La ley de la mezcla vive en el doc de `FxProcessor::set_mix`**, y es
  `out = dry + wet·(procesado − dry)`. La única excepción a propósito es el
  looper, que suma: sus tomas suenan *debajo* de lo que se está tocando.
- **Los efectos se apilan sin saturar, y hay un test que lo sostiene**
  (2026-08-28). `no_built_in_effect_is_a_gain_stage` recorre los 46 con los
  mandos que el rack les da: ninguno suma más de 4,5 dB y los ocho más fuertes
  apilados no pasan de 6. Antes de eso, protocosmos sumaba 9,1 dB solo —clipeaba
  por sí mismo desde una entrada a −8,7 dBFS— y 46 de los 2 070 pares pasaban de
  escala; los 46 lo tenían a él adentro. `measure_stacking` (ignorado, en
  `choz-ui`) imprime la tabla entera.
- Con la entrada a −2,7 dBFS, 57 pares siguen pasando de escala, el peor a 1,34.
  Eso ya no es un efecto que amplifique: son 2,7 dB de headroom y cualquier
  cadena se los come. Se resuelve en el fader del tab.
- La dispersión de nivel que queda al mover `Wet` a media posición —de +2,9 dB
  en protocosmos a −9,0 en el shimmer— **no es la ley, es el nivel del wet de
  cada efecto**, y emparejarlos querría medir cada uno contra un programa real y
  no contra ruido. `examples/mix_probe` es la tabla.
- Lo que la fase 10 midió y **no** arregló: el shifter de voces suma dos
  cabezales a media ventana de distancia, y eso peina una nota aguda —2,1 dB a
  14 kHz— haga lo que haga el interpolador. Sacarlo pide otro shifter, no otra
  lectura.

### Bordes que quedaron abiertos

| # | Qué falta | Dónde | Por qué no se hizo |
|---|-----------|-------|--------------------|
| 1 | **Los mandos del editor SF2 no son por zona** | SOUNDS | El editor escribe sus offsets en todos los canales, así que da forma al instrumento entero. Una envolvente distinta por zona del split querría un juego de mandos por zona, y eso es un panel nuevo. |
| 2 | **LADSPA y DSSI no listan los nombres de los pasos** | RACK | Un parámetro con nombres abre su lista en LV2 enumerado, CLAP, VST3 y —desde el 2026-08-29— VST2. LADSPA sí dice **cuántas** posiciones tiene un puerto y choz ya lo respeta (`steps_of`, con `HINT_TOGGLED` e `HINT_INTEGER`): lo que no hay es de dónde sacar los **nombres**, porque el ABI no tiene ninguna llamada que diga cómo se lee un valor. Un interruptor se dibuja como interruptor; un puerto de cuatro posiciones sigue siendo cuatro números. |

### Y lo de siempre

**Mirar con el equipo delante.** Léase el primer punto de los gotchas antes de
dudar de un número: todo el DSP está verificado contra señales sintéticas, no
contra una habitación.

## Notas / gotchas para el que retome

- **La ventana del shifter de voces está escrita como `2048 / 48` a propósito**
  (2026-08-28). Es tiempo, no samples, pero el número es el viejo conteo sobre
  el rate común: la segunda octava del shimmer es una resonancia de esa longitud
  contra el reverb que lleva adentro, y moverla dos samples le baja la tercera
  aserción de su test de 0,74 a 0,14. Mismo sonido a 48 kHz, y ahora el mismo en
  todos lados. `examples/alias_probe` es la herramienta con la que se midió todo
  esto.

- **`space_echo` sigue con su propia línea** (2026-08-28). Es el único que
  queda: se le arregló la allocation y el dimensionado por tiempo en la fase 6,
  así que no tiene ninguno de los dos bugs por los que existía la línea
  compartida, y portarlo sería diff sin sonido. `beat_repeat` tampoco la lleva
  —captura lineal, lectura entera hacia adelante, sin realimentación— y no es un
  olvido: lo que tenía era el techo del grano medido en samples, que ahora está
  en segundos.

- **Un tab de plugin partido lleva dos instancias, y sólo mientras las usa**
  (2026-08-29). `choz_engine::layered::Layered` es el envoltorio; la segunda se
  construye **al pintar la segunda zona** y se suelta al quitarla, porque un
  plugin cuesta lo que cuesta y casi ningún tab parte el teclado. El techo es
  dos a propósito: cuatro sonidos a la vez son cuatro tabs, que es lo que hace
  MULTI. Un octava pintada con un tercer botón suena el sonido del propio tab,
  no el de la segunda zona — la lectura honesta de "no hay tercera instancia",
  y lo que hace que un proyecto escrito sobre un SoundFont (donde cuatro zonas
  son gratis) se abra en un plugin sin sonar cualquier cosa. El sonido de una
  zona es un **blob**, no un número de programa: va por `set_slot_zone_state`,
  y `set_zone_program` sigue siendo la puerta del SoundFont.

- **El orden de render no es el de las pestañas** (2026-08-29). Las que manejan
  un gate van primero: un gate lee el nivel de su fuente para el bloque en el
  que está, y una fuente renderizada después se leía un bloque tarde — hasta
  5 ms de un gate rítmico llegando detrás de su propio bombo. La máscara la
  publica `fx_chain::set_gate_sources` cuando se reconstruye una cadena, y el
  callback la lee con dos rangos filtrados: no aloca, y lo que no es fuente
  conserva el orden que tenía. La suma no distingue en qué orden se le sumó.

- **Un puerto de captura que nadie escucha se desconecta, no se desregistra**
  (2026-08-29). Registrar no cuesta —34 puertos callados quedan bajo el ruido
  del propio grafo— y conectar sí: ~0,19 puntos de un núcleo por conexión,
  medido con la UMC1820 delante. Con el rack leyendo un par de veintiuno, el
  grafo pasa de **8,22 % a 4,61 %**. Lo que decide la máscara es
  `App::capture_mask`, y **con el cajón IN abierto se conecta todo**: sus filas
  muestran el nivel de cada jack, y un jack desconectado no tiene nivel que
  mostrar — que es justo el diagnóstico por el que se abre ese cajón.

- **Un sondeo de etiquetas se hace en la instancia de lectura, nunca en la que
  suena** (2026-08-29). VST2 no tiene con qué preguntar cómo se *leería* un
  valor: la única lectura es `effGetParamDisplay`, que contesta por el valor en
  el que el parámetro está. Por eso el barrido lo hace `read_params`, que carga
  una instancia suya y la tira, y por eso deja cada parámetro donde estaba —
  `probing_the_positions_leaves_every_parameter_where_it_was` lo sostiene, y
  falla si se le quita la restauración. Hacer lo mismo sobre un plugin que está
  sonando se oiría.

- **El flake de los tests de UI sigue ahí, y un intento de arreglarlo lo
  empeoró** (2026-08-29). `XDG_STATE_HOME` es global al proceso y
  `sandbox_state_dir()` borra `ui.json` sin sostener el `ui_guard`, así que un
  test que guarda ajustes y otro que los borra se cruzan y algo sin relación
  falla una vez cada cuatro corridas. Hacer que `sandbox_state_dir()` devolviera
  el guard **no** lo arregló: serializó media suite (23 s → 45 s) y cambió el
  orden lo bastante como para destapar otro test que deja el idioma en español
  sin `UiRestore`, que entonces fallaba en las tres corridas de tres. Se
  revirtió. Lo que hace falta es que **todo** test que llame a `App::new()`
  sostenga el guard, no sólo los que tocan el estado a propósito.

- **El cutoff del SVF ya no obedece en el acto** (2026-08-28). Se suaviza en
  octavas con 15 ms de constante y los coeficientes se rehacen cada 16 samples
  mientras se mueve — un `tan()` por sample sería pagar el hallazgo F4 para
  arreglar el F3. Si algún día un efecto quiere que el filtro salte (un mode
  switch, un preset), lo que hace falta es `Smoothed::snap`, que es lo que
  `reset()` usa. Lo mismo con el tiempo del delay: camina en 80 ms, a propósito,
  porque así glisa como una cinta.

- **El cazador de acoples estaba calibrado al revés** (2026-08-20). Reportado
  como "el harmonizer empieza bien pero no sostiene una nota larga", y no era
  el harmonizer: `FeedbackGuard` está en la **entrada**, antes del trim, así que
  se lleva la voz y la armonía juntas. Medido con `examples/guard_probe`, con
  las constantes viejas (`GROWTH` 1,5× **por chequeo de 64 ms**, 3 chequeos):
  un swell normal de 600 ms hacia una nota larga bajaba a **0,126 (-18 dB)** y
  se quedaba ahí, porque lo único que soltaba el duck era que la sala se
  callara — o sea, que el cantante dejara de cantar. Y al mismo tiempo un acople
  real de +6 dB/s **no se detectaba**: sube 0,4 dB entre dos chequeos y nunca
  llegaba al 1,5×. Cazaba cantantes y dejaba pasar acoples.
  Lo que se hizo: el crecimiento se mide contra la lectura **más vieja del
  historial** (medio segundo atrás), o sea una tasa y no un salto; hacen falta
  `GROWTH_CHECKS = 16` ventanas seguidas (~1,5 s de crecimiento continuo), que
  es lo único que separa a un cantante de un lazo — los dos crecen, pero una
  nota deja de crecer cuando llega; y el duck se suelta cuando **deja de
  crecer** (`CALM_CHECKS`), no sólo con silencio. Medido después: los tres casos
  cantados quedan en 1,00, el acople de +6 dB/s se caza y se sostiene.
  Lo que **no** resuelve: un crescendo largo de verdad (2 s subiendo) sigue
  siendo indistinguible de un lazo lento mientras sube — dispara, pero ahora se
  suelta solo en cuanto para. El interruptor sigue en Settings → AUDIO.

- **"Se satura al pisar el sustain" son huecos, no clipping** (2026-08-20). El
  log del usuario lo dice entero: el grafo corre a **96 kHz con 128 frames**, o
  sea **1,33 ms por bloque**, y el tab de SoundFont solo ya cuesta 0,66–1,04 ms
  de eso (532 de 716 avisos lo nombran a él; Surge XT es el segundo con 91). El
  peor bloque **no crece** durante la sesión: arranca en 1,4 ms y se queda ahí,
  o sea el rack vive al borde desde el primer minuto. Pisar el sustain no baja
  ninguna voz, así que el pool de oxisynth se llena y se queda lleno: medido con
  `examples/sustain_probe`, un bloque pasaba de 233 µs al minuto a 330 µs una
  vez lleno (+40 % en un solo tab). Un bloque que se pasa del presupuesto es un
  hueco en la salida, y un hueco suena a distorsión, no a nivel — de ahí
  "satura". Lo que se hizo: **polifonía 64** en `Sf2Synth` (el default de
  oxisynth es 256), que deja el coste plano en ~124 µs con el pedal abajo, 2,7×
  más barato, sin tocar el pico de nivel (0,115 medido antes y después. Y sí,
  parte del problema es el sistema: hay líneas de "el grafo sólo corrió 380 de
  750 ciclos" que son otro cliente xruneando. Las dos cosas son ciertas.
- **El DSP % del menú era un solo bloque** (2026-08-20). `Load::last()` leía
  `last_us`, uno de los ~190 bloques por segundo, y `elapsed()` es reloj de
  pared: un bloque que se comió una expropiación se lee como un rack que cuesta
  el 40 % cuando el tiempo de CPU del hilo dice 4 %. Eso es lo que parecía "el
  número sube cuanto más tiempo lleva choz abierto" — medido con
  `/proc/PID/task/*/stat`, ni la CPU del `data-loop` ni el RSS crecen en 3
  minutos. Ahora publica una media exponencial (1/16, ~100 ms); el pico sigue
  aparte, porque un deadline se pierde por picos y no por medias. Descartado por
  medición: denormales (probé los 14 FX con cola larga contra 30 s de silencio,
  ninguno pasa de 1,14×).
- **`take_worst_slot` no se vaciaba en un segundo sano**: sólo se leía camino a
  un aviso, así que la primera línea tras una hora nombraba el tab que alguna
  vez fue caro, no el que está pasándose ahora. Se lee siempre.

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
- **Lo que se instala con choz**: sus 46 efectos **y los dos artifacts** —el
  arpegiador y el secuenciador— como un solo `.clap` de 48 plugins (`~/.clap`
  desde el instalador, `/usr/lib/clap` desde los paquetes), los wallpapers en
  `share/choz/wallpapers` —una instalación nueva abre con el que trae— y
  `choz-pd-host` cuando hay libpd. `--no-clap` para quien no quiera el plugin.

- **Un patch de Pd sin símbolos de recepción en sus sliders no suena, y no es
  un fallo de choz.** Un `hsl` recién puesto en Pd vale cero y no se puede
  direccionar desde fuera: el patch queda multiplicado por cero. choz nombra
  esos controles en el log al cargarlo. Los que sí tienen símbolo salen como
  knobs. Y ojo: **Pd no crea un `hsl` con campos de menos**, aunque el lector de
  choz lo acepte.
- **Una lista larga en un panel se hace con `drawer::{list_height, list_scroll}`**: calculan la ventana visible **a partir del cursor**, sin offset guardado, y las llaman tanto el dibujo como los rects de clic — que es lo que impide que se desvíen.
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
- **Los probes de editores abren ventanas de verdad.** `examples/ui_probe` (LV2) y `examples/clap_gui_probe` (CLAP) instancian plugins y abren su GUI: usar `Xvfb` (o la ventana padre sin mapear) y **matarlos al terminar**. Ningún test abre ventanas, y así debe seguir.
- **En VST3, la GUI no habla con el procesador.** El edit controller reporta al host (`IComponentHandler::performEdit`) y es el host quien lleva el valor al procesador por `inputParameterChanges`. Y `getParameterInfo` toma un **índice** y devuelve un **id arbitrario**: confundirlos mueve otro parámetro.
- **Un valor que no se guarda tampoco se puede editar.** El cap de 7 parámetros truncaba la lista *al construirla*, no sólo al dibujarla.
- **Una nota-off tiene que ir a donde fue su nota-on.** `App.sounding` es la memoria; `PANIC` es la salida de emergencia.
- **Un fondo de celda es opaco, y va por encima de la imagen del protocolo gráfico.** En halfblocks la transparencia se mezcla en las celdas (fg *y* bg); bajo kitty el lavado es una segunda imagen con alfa.
- **Un binario de plugin puede publicar cientos de descriptores.** LSP tiene ~390 UIs en un `.so`: recorrer hasta el primer nulo, nunca con un N fijo.
- **Un test que carga el `.so` del propio workspace puede estar leyendo uno viejo.** `real_host` compara lo que el bundle CLAP exporta contra la lista de built-ins, y toma el `.so` de `target/debug`: con uno de antes de los artifacts pasaba en local y fallaba en el runner, que compila limpio. Un fallo de esos se reproduce construyendo el crate primero.
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

# Las sondas de medición, que son con lo que se decide antes de tocar DSP.
cargo run --release -p choz-engine --example alias_probe    # artefactos que no estaban en la entrada
cargo run --release -p choz-engine --example mix_probe      # la ley del dry/wet, efecto por efecto
cargo run --release -p choz-engine --example port_cost      # lo que cuesta un puerto JACK
cargo run --release -p choz-plugin-vst2 --example steps_probe   # posiciones con nombre en VST2
cargo run --release -p choz-plugin-vst3 --example units_probe   # secciones que declara un VST3
cargo test --release -p choz-ui -- --ignored --nocapture measure_stacking  # los 46 apilados

# barridos largos: hostear TODOS los plugins instalados de un formato
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored

# Probes de editores: INSTANCIAN PLUGINS Y ABREN SU GUI.
cargo run -p choz-plugin-lv2  --example ui_probe            # --limit N, --skip N
cargo run -p choz-plugin-vst3 --example vst3_gui_probe
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run -p choz-plugin-clap --example clap_gui_probe

# Tests de runtime con los instrumentos VST2 del usuario.
CHOZ_VST2_DIR=/ruta/a/tus/vst cargo test -p choz-plugin-vst2

# Los efectos de choz como plugin CLAP: construir, probar y usar fuera.
# `real_host` lee el `.so` de `target/debug`: construir antes, o se está
# probando el de la vez pasada.
cargo test -p choz-plugin-clap-export          # ABI en proceso + carga por dlopen
cargo build --release -p choz-plugin-clap-export
cp target/release/libchoz_plugin_clap_export.so ~/.clap/choz.clap

# Comprobar un paquete publicado sin instalarlo.
dpkg-deb -c choz_1.3.4-1_amd64.deb
strings usr/bin/choz | grep -c /home/       # tiene que dar 0
```
