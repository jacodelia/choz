# choz — Pendiente

Qué falta. Lo hecho está en [CHANGELOG.md](../CHANGELOG.md), día por día; cómo
encajan las piezas, en [architecture.md](architecture.md).

Última actualización: 2026-08-15.

## Estado en una línea

Los seis formatos de plugin (CLAP, LV2, LADSPA, DSSI, VST2, VST3) se escanean,
se hostean y abren su ventana nativa; el rack es multi-slot con mixer, FX,
ruteo canal por canal y proyectos en YAML; entra audio en vivo por JACK (cada
jack del grafo) y por ALSA/PulseAudio/PipeWire (un dispositivo de captura
elegible en Settings), así que también es un multiefecto; hay tres capas contra el código ajeno
que revienta —escaneo fuera de proceso, cuarentena y sandbox—; hay transporte
propio con compás, automatización contra ese reloj, `A→M` (audio a notas),
AutoTune, `A→M` como traductor de guitarra/micro a notas para un plugin,
44 efectos propios (**la suite está completa**: dinámica, EQ,
saturación, modulación, frecuencia, espacio y medida), arpegiador por tab (esclavo de un reloj MIDI
externo, y capaz de tocar hacia fuera por un puerto MIDI) y fondo de escritorio
configurable; la dinámica y el EQ ya son ajustables de verdad (detección
Peak/RMS, stereo link, sidechain HPF, lookahead, histéresis, mid/side, solo y
curva de respuesta dibujada), hay un LFO compartido detrás de las modulaciones,
un par de Hilbert detrás del desplazador de frecuencia y un analizador de
espectro por FFT en la pestaña `SPEC`. **La 1.0.0
está publicada y sus paquetes verificados** (ver "Hecho" abajo). 462 tests,
`clippy --workspace --all-targets -D warnings` limpio.

---

## Pendiente

### 1. El arpegiador

Completo (ocho modos, ocho divisiones, octavas, gate, swing, latch, acorde,
`SYNC` al transporte y `TAP`) y su ruteo también: `A→M`, otra tab y MIDI OUT.
Queda una sola cosa, **aplazada a propósito**:

- **Timing en el engine.** Es **resolución, no deriva**: con `SYNC` el número
  del paso viene de la posición del transporte (que avanza el callback de
  audio), así que la rejilla no se desplaza; pero `Arp::tick` sigue en el bucle
  de UI, que despierta cada 5 ms, y un paso puede sonar hasta 5 ms tarde.
  - **Lo que costaría** (mirado, para no volver a mirarlo): programar las notas
    por adelantado con sello de tiempo no basta —aplicarlas al principio del
    bloque que las contiene deja el error en un bloque (1–3 ms) y no en cero—.
    Para que sea exacto hay que **partir el render por slot** en los puntos
    donde caen las notas, o sea llamar a `source.render` varias veces por bloque
    con trozos pequeños: legal, pero es otra cosa para un plugin hosteado.
    Además saca la generación de notas de donde vive el ruteo (la UI), y
    `MIDI OUT` no se puede programar por adelantado (ALSA manda cuando se le
    dice), así que una nota programada llegaría antes fuera que dentro.
  - Decidido con el usuario: **se queda así**.
- **Lo que no se hace**: una matriz de ruteo general. Un `Vec` de "esta fuente
  va a esta tab con este arpegiador" cubre todo lo pedido; una matriz N×M es la
  abstracción que se escribe hoy y se depura durante meses.

### 2. Algoritmos de entrada: una sección propia, antes del source y de los FX

Hoy el arpegiador es un caso especial cableado en la tab: vive en `arp.rs`, se
dibuja en su caja del RACK y se le llama a mano desde el bucle de eventos. Y
`A→M` es otro caso especial, cableado en el callback. Los dos son **lo mismo**:
algo que se pone **entre la entrada y el instrumento** y decide qué notas llegan.

Lo que pide el usuario: que eso sea una **clasificación**, no dos excepciones —
una sección del rack, antes del source y de los FX, donde se elige un algoritmo
de entrada como hoy se elige un efecto.

- **La cadena, como queda**:

  ```
  entrada (MIDI / OSC / QWERTY / audio) ──► [ ALGORITMO ] ──► INSTR ──► FX ──► OUT
  ```

- **Los que ya existen** y pasarían a ser dos entradas de la lista, no dos
  cableados: el **arpegiador** (notas → notas) y **`A→M`/`ftom`** (audio →
  notas). Que los dos quepan en la misma lista es la prueba de que la
  abstracción vale: hoy uno vive en la UI y el otro en el callback, y eso es
  exactamente lo que hace que añadir un tercero sea otro caso especial.
- **La forma del trait**: `process(&mut self, entrada: &[NoteMsg], audio: &[f32],
  ahora) -> Vec<NoteMsg>`. Audio y notas entran, notas salen. El arpegiador
  ignora el audio; `A→M` ignora las notas. Un tercero puede usar los dos.
- **Dónde vive**: es una decisión de ruteo, así que en la UI —donde ya se
  resuelve el ruteo— salvo que necesite el reloj de muestra a muestra, que es
  la misma discusión aplazada del punto 1. Un algoritmo que genere notas desde
  el callback tiene el mismo coste que se midió allí.
- **Lo que no se hace**: una cadena de varios algoritmos por tab. Uno por tab
  cubre todo lo pedido; encadenar dos pide decidir qué le pasa el primero al
  segundo, y eso es una matriz de ruteo con otro nombre.

### 3. Efectos y algoritmos escritos en Pure Data / Max MSP

**La pregunta que decidía todo está contestada.** Medido con la libpd 0.56.2 de
Debian (`crates/choz-plugin-pd`, y el módulo lo lleva escrito):

- Un patch de ganancia (`adc~ → *~ 0.5 → dac~`) cuesta **0.03 % del callback**
  a 128, 256 y 512 frames. Pure Data no es la parte cara de nada.
- **No reserva memoria por bloque.** Pd reserva cuando el grafo de DSP
  *cambia* —abrir un patch, encender el DSP, crear un objeto— y no mientras
  corre. O sea la regla de siempre: construir fuera del hilo de audio, procesar
  dentro.
- **`libpd_new_instance()` devuelve null**: la build de Debian no lleva
  `PDINSTANCE`, así que hay **una Pd por proceso**. Eso no es un detalle, es la
  arquitectura: dos efectos de Pd no pueden convivir en un choz.

Y con eso la forma queda decidida sola: **cada patch es un proceso**, que es
exactamente lo que `choz-plugin-sandbox` ya hace con los plugins —audio por
memoria compartida, supervisor que lo reinicia si muere—. Dos cosas salen
gratis: la licencia queda limpia (el hijo enlaza libpd LGPL, el binario de choz
no lo enlaza) y un patch que cuelgue Pd se lleva un patch, no la sesión.

**Hecho**: `choz-plugin-pd` con el lector de `.pd` (formato de texto, no
necesita Pd instalado: es lo que permite listar patches y decir por qué uno no
aparece), la clasificación `Effect` / `InputAlgorithm` / `Unusable` según lo que
el patch conecta, y el camino real con libpd detrás de la feature `pd` —
apagada por defecto, porque un host que no compila sin Pure Data instalado es un
host que nadie puede compilar.

**Lo que falta**, en orden:

1. **El hijo**: un binario como el del sandbox que abra un patch y procese
   bloques por memoria compartida. Casi todo está en `choz-plugin-sandbox`; lo
   nuevo es cambiar "carga este plugin" por "abre este patch".
2. **Entrarlo en la UI**: `PluginFormat::Pd` con su directorio de escaneo, y
   los patches con rol `Effect` en el modal de ADD FX como un formato más.
   `AudioFxEntry.plugin` ya es la ruta para algo que no es un `AudioFxKind`.
3. **Los de rol `InputAlgorithm`** esperan a la sección 2 — y su forma es la
   que debe decidir la del trait, no al revés.
4. **Max/MSP**: `.maxpat` es JSON y se puede leer igual, pero **no hay runtime
   empotrable**. Lo honesto es importar lo que se pueda y decir claramente qué
   no; prometer compatibilidad sería mentir.

### 4. Voz: lo que AutoTune **no** es, y lo que haría falta

Revisado contra la lista pedida. Hay que ser claro, porque los nombres se
parecen y las cosas no:

**Lo que `AutoTune` es hoy**: un **corrector de afinación monofónico**. Detecta
el tono, lo lleva a la nota más cercana de una tonalidad y escala, con
`Correct`, `Retune`, `Human`, referencia `A4`, límites de Hz y ganancias. Una
voz, una nota, en tono. Eso funciona y está medido.

**Lo que la lista pide, y no está**:

| Pedido | Estado |
|---|---|
| Motor sinte de 8 voces, doble motor, controlado por voz | **no existe**; es otro efecto, no un ajuste de éste |
| Voz de ordenador (vocoder / talkbox) | **no existe** |
| Pitch shifting polifónico | **no existe** — el detector es monofónico por diseño |
| Armonías orgánicas | **hecho**, en el `Harmonizer` nuevo (ver abajo) |
| Tono clásico de talkbox de guitarra/voz | **no existe** |

- **Por qué no es un ajuste de AutoTune**: el corrector *sigue* un tono y lo
  mueve; un motor de síntesis *genera* uno y lo toca. Meterlo en el mismo efecto
  sería un efecto con dos mitades que no comparten nada salvo el nombre.
- **Lo que haría falta, en orden de dificultad**:
  1. **Vocoder** (voz de ordenador): banco de filtros de la voz aplicado a un
     portador. La infraestructura está toda: `FilterBankFx`, los biquads, la
     envolvente. Es el más barato de los cuatro y el que más suena a lo pedido.
  2. **Talkbox**: es un vocoder con el portador a la entrada y una respuesta
     más resonante — el mismo código con otros ajustes.
  3. **Motor sinte controlado por voz**: `A→M`/`ftom` ya saca la nota; lo que
     falta es el sinte, y choz **hostea sintes**. La versión honesta es una tab
     con `A→M` y un plugin, que ya funciona; un sinte propio de 8 voces dentro
     de un efecto es escribir un sintetizador.
  4. **Pitch shifting polifónico**: requiere un detector polifónico (varios
     tonos a la vez), que es un problema distinto y mucho más caro que el
     monofónico que hay. No antes de que alguien lo pida por segunda vez.

### 5. Los efectos propios como plugins CLAP

Que los 44 efectos de choz se puedan usar **fuera** de choz: en Bitwig, en
Reaper, en Carla, en cualquier host CLAP.

- **Lo que juega a favor**: `FxProcessor` ya es exactamente la forma de un
  plugin —`process_block`, `params`, `set_param`, `reset`— y los efectos no
  dependen del rack: se construyen desde un array de `f32` normalizados y no
  saben nada de tabs. choz ya hostea CLAP (`choz-plugin-clap` sobre
  `clack-host`), así que el formato ya se conoce por dentro; lo que falta es la
  otra mitad, el lado plugin.
- **Por qué CLAP y no LV2**, ahora que está elegido:
  - **No hay TTL.** Los metadatos de un CLAP viven en el código, en la misma
    estructura que declara el plugin. Exportar 44 efectos a LV2 significaba
    generar cientos de líneas de Turtle desde `params()` y mantenerlas en
    sincronía; en CLAP el problema no existe.
  - **Un solo binario.** La *factory* de CLAP publica N plugins desde un
    `.clap`, así que los 44 son un fichero y no cuarenta y cuatro bundles.
  - **La licencia coincide**: CLAP es MIT, igual que choz. LV2 (ISC) tampoco
    daba problemas, pero esto no deja ni la duda.
- **Lo que hay que decidir**:
  - **Con qué se escribe el lado plugin**: `clack-plugin` (el hermano del
    `clack-host` que ya se usa, así que es un crate del mismo autor y la misma
    forma) o el ABI a mano contra `clap-sys`, como se hizo con LV2 al hostear.
    Lo primero, salvo que aparezca una razón.
  - **Qué no se exporta**: lo que necesita el transporte de choz
    (`BeatRepeat`) puede leer el del host — CLAP lo da en cada `process` — pero
    el **medidor** y los **presets** de la UI de choz no viajan. Un efecto
    exportado es el DSP, no el panel. El `WaveShaper` sin su banco de puntos
    dibujado sigue siendo un waveshaper; con una GUI propia sería otro
    proyecto.
  - **Qué crate**: uno nuevo (`choz-plugin-clap-export` o similar) que dependa
    de `choz-engine` y no al revés, porque la dependencia contraria metería el
    ABI de plugin dentro del motor.
- **Lo primero que hay que probar**: **un** efecto (`Gain`, que no tiene estado)
  publicado como `.clap` y cargado en Carla o Bitwig. Si eso carga, los otros 43
  son el mismo molde más un bucle sobre `ALL_FX_KINDS`.

### 6. Lo que sólo se comprueba con el hardware delante

No queda código pendiente aquí: queda mirar.

- **Que se oiga lo que sale.** Con la UMC1820 apagada, un `output_device`
  guardado apunta a un sink sin puertos: ahora choz cae a otro y el panel
  TRANSPORT escribe `NOT CONNECTED` si aun así no llega a ningún sitio. Falta
  confirmarlo con la interfaz apagada y encendida, y ver que al encenderla se
  vuelve a ella (hoy hay que elegirla a mano en el cajón OUT).
- **Las entradas por JACK**: los tests corren sin cliente JACK, así que
  `all_capture_ports()` devuelve cero y las filas de canal no existen para
  ellos. Falta ver que las ocho entradas de la UMC1820 salgan bajo su tarjeta
  junto al micro del portátil y la otra placa, que el jack 5 sea el jack 5
  (`in_order` ordena por el número final del nombre del puerto, y no todos los
  nombres terminan en número), y que registrar ~20 puertos de captura no cueste
  nada en el callback.
- **La entrada por cpal** (ALSA / PulseAudio / PipeWire): **dos relojes de
  verdad derivan de verdad**, y ahora la deriva se ve — el cajón IN escribe
  `N late, N dropped` en cuanto se mueven (`meter::capture_health`), y calla
  mientras se porta bien. Lo que queda es **mirar el número**: con un headset
  USB contra la tarjeta interna, cada cuánto sube, si se oye como chasquido o
  no se oye, y si la latencia se queda donde la deja el backlog de dos bloques.
  Si sube más de lo tolerable, el paso siguiente es un resampleador adaptativo
  (hay un `ponytail:` en `drain_capture` que lo dice).
- **`A→M`, ya endurecido**: fuera el retumbe (paso-alto a 60 Hz, 24 dB/oct) y
  un suelo de ~130 ms antes de que la nota pueda cambiar. Falta oírlo: con voz
  y con guitarra, si 130 ms se sienten como pereza tocando rápido o como
  estabilidad, y si el paso-alto se lleva algo de un bajo (la nota más grave que
  reporta es A1, 55 Hz, y el filtro está en 60).
- **`A→M` con un instrumento de verdad delante.** El detector está verificado
  con tonos, con tonos armónicos y con un vibrato de ±35 cents; entra en
  presupuesto (decimado a 16 kHz, un análisis cada 8 ms). Falta mirar la lectura
  del botón (` A→M● E2-14`) con señal real: cuánto hay que subir `IN`, dónde
  queda `SENS` contra el ruido de una single-coil, y si 24 ms de espera
  (`STEADY_ANALYSES`) se sienten como retardo tocando rápido.
- **AutoTune con una voz de verdad por un micrófono de verdad.** El DSP está
  medido (cero allocations, ~10 % del presupuesto de búfer, 33 ms de latencia) y
  probado contra señales sintéticas, que son más amables que una habitación.
  Falta cantarle: si `Retune` a 120 ms se siente natural, si el gate a -50 dBFS
  aguanta el ruido de sala, y si las formantes se sostienen en una corrección de
  más de un tono.
- **La instalación mirada con los ojos**: el `.desktop` bajo multimedia, el
  icono en el menú, el lanzador abriendo kitty al tamaño correcto, un
  `*.choz.yml` con doble clic, y un plugin sincronizado siguiendo la fila
  `Tempo`.
- **La ESP32-S3 táctil** (`examples/esp32s3-touch/`, hecha como superficie de
  control): flashearla, mirar el panel y medir el retardo de un toque a la nota.
  **No existen versiones del S3 con Linux** — la placa manda y choz hostea.
- **En una Pi**: que ALSA/JACK abren con buffers pequeños sin xruns, y que el
  escaneo encuentra algo — **los plugins son binarios nativos**, así que una Pi
  sólo carga plugins compilados para ARM.

---

## Hecho (2026-08-14)

- **MIDI OUT por tab** (destino que faltaba del punto 1.1): `midi::MidiOut`
  abierto por nombre, con la lista de lo que tiene sonando — corta **nota por
  nota**, porque un sinte que ignora "all notes off" zumba hasta que se apaga.
  Conexiones compartidas por nombre (ALSA da un puerto a un cliente).
  `RackSlot.midi_out` guarda el **nombre**, no un índice. Sección `MIDI OUT` en
  el cajón OUT con la gramática de los canales (liga/desliga, y dice qué tabs la
  usan). Todo pasa por un embudo, `App::send_note`, incluido el `PANIC`.

- **Reloj MIDI externo**: `ClockCounter` en el callback del puerto (el último
  sitio con un sello de tiempo honesto), promediando sobre **una negra entera**
  — 24 pulsos, y el que cierra una abre la siguiente. `InputEvent::Clock` lleva
  `Start`/`Continue`/`Stop`/`Tempo(bpm)`, un mensaje por negra. `START` rebobina
  y arranca, `CONTINUE` no. Interruptor `CLK INT/EXT` en el panel TRANSPORT
  guardado en `ui.json`: un puerto que manda reloj todo el día no puede quedarse
  con el tempo por enchufarlo.

- **El secuenciador fuera, y las listas dentro**: `ModalKind::ArpChoice` — los
  controles con nombres (`MODE`, `DIV`, `OCT`) abren su lista con Enter o con
  un segundo clic, y en la forma compacta los botones abren la lista en vez de
  recorrerla. `k` alcanza el arpegiador aunque no haya caja de knobs, y en la
  fila de botones se marca el que tiene las flechas. Botón **`TAP`** con el
  tempo al lado (con `SYNC` mueve el transporte, que es el reloj que cuenta).
  Se fue el secuenciador entero y con él `SeqStep`/`PlayMode`/`Transport` y los
  campos `arp_*` del proyecto; las tres acciones de MIDI learn de su transporte
  quedan como variantes muertas para que un proyecto viejo siga cargando.

- **Modo acorde y longitud de patrón** (tercera pasada del clon): `CHORD`
  memoriza lo que esté pisado al encenderlo (con nada pisado conserva el
  anterior) y luego una tecla toca esa forma desde donde se toque — las notas
  entran en `held`, así que modos, octavas y latch siguen valiendo; soltar la
  tecla se lleva lo que trajo. `LEN` recorta lo que suena **sin borrar** los
  pasos de más, se guarda (`arp_length`) y se olvida al cambiar de patrón o de
  secuencia. Del clon sólo queda el reloj MIDI externo.

## Hecho (2026-08-13)

- **Ligaduras y grabación al vuelo** (segunda pasada del clon): `SeqStep.tie` —
  el paso no suelta en su gate y la nota que continúa no se re-ataca; la que no
  continúa sí se suelta. Botón `TIE` (con paso elegido liga ése, sin ninguno el
  último grabado) y marca `‿` **sobre el chip que sostiene**, no entre dos,
  porque la tira envuelve. `REC` armado mientras rueda **graba encima** sin
  tirar el patrón, cuantizando al paso **más cercano** (pasada la mitad, la
  tecla iba dirigida al siguiente: redondear siempre hacia abajo atrasa a quien
  toca adelantado).

- **Primera pasada del clon de KeyStep** (punto 1.0): los **ocho modos** en el
  orden del hardware — `INCL` repite las notas de los extremos y `EXCL` (el
  antiguo `UP-DN`) no, `×2` toca cada nota dos veces, `ORDER` es el antiguo
  `PLAYED` —, las **ocho divisiones** (faltaban `1/4T` y `1/32T`),
  **transposición tocando** (una tecla mientras corre mueve el patrón; do
  central lo devuelve, y el transporte escribe `+7 st`) y **ocho secuencias por
  tab** con knob `SEQ`, botón en la forma compacta y persistencia
  (`arp_patterns`/`arp_seq`; un proyecto viejo carga su patrón único como el
  primero). Cambiar de secuencia **suelta lo que la anterior tenía sonando**.

- **Editar un paso grabado**: tira de chips (uno por paso, con sus notas) bajo el
  transporte del secuenciador. Clic elige, otro clic suelta, la rueda camina;
  con un paso elegido la tecla tocada lo **sustituye** (acorde dentro de 40 ms,
  la misma ventana que grabando) sin mover el cursor, `REST` lo silencia y
  `DEL STEP` lo borra. El teclado del ordenador pasa ahora por el arpegiador
  como una tecla MIDI — era la única entrada que no podía escribir un paso. La
  tira se limita a dos filas (`strip_window`), con la ventana **medida** para
  que el paso en el que se trabaja tenga rect: un chip recortado se lleva su
  rect y deja un paso que no se puede arreglar.

- **`SYNC`: el arpegiador enganchado al transporte**: `ArpSettings.sync` +
  `ArpSettings::tempo` (una sola fuente para lo que se imprime y lo que cuenta)
  + `Arp::grid_step`, que saca el índice del paso de `transport().ppq()` en vez
  de sumar duraciones — sin deriva, dos tabs en fase, y el knob de BPM mueve el
  transporte porque hay un solo reloj. Transporte parado = corre libre a ese
  tempo (un acorde sostenido tiene que sonar).

- **Knobs del arpegiador, con la forma que la pantalla aguanta**:
  `ArpSettings::{knobs, norm, set_norm}` + `ArpParam` (ON, PLAY, MODE, DIV, BPM,
  GATE, SWING, OCT, LATCH) dibujados por `draw_knob_box` — el diseño existente
  manda, el arpegiador se adapta a él. `ArpShape::{Off, Boxed, Strip, Buttons}`
  elige por filas libres, no por un ajuste; `draw_knob_box` aprendió `bordered`
  (sin marco = 3 filas). `RackFocus::Arp` y `k` cicla sólo por las cajas que
  están en pantalla. Los knobs se direccionan **por lo que son**, no por su
  índice (en SEQ no hay MODE ni LATCH), y escriben por `ArpEdit::Knob`, que ya
  sabía parar lo que sonaba.

- **Filas de botones que envuelven + cantidad de `mtof`**: `ButtonRow` en
  `fx_chain_panel` (fila `INSTR`, arpegiador, transporte del secuenciador y
  cadena de FX, que ya envolvía y ahora no tiene copia propia). **No es la caja
  de knobs que decía el roadmap a propósito**: una caja con borde cuesta cinco
  filas que el RACK no tiene. Botón `AMT` de `mtof`: aparece sólo con destino
  bound, el clic recorre los cuartos y la rueda acota. Test: el panel a 80
  columnas con el arpegiador entero encendido — diez interruptores, ninguno
  fuera del panel, más de una fila, y el envuelto sigue respondiendo al clic.

- **Scroll en los cajones IN y OUT** (el primer gotcha de la lista de abajo):
  `drawer::{list_height, list_scroll}` + `source_panel::input_window`. **No hay
  offset de scroll guardado**: la ventana es función del cursor, como la caja de
  knobs del RACK — un estado menos que mantener, y el dibujo y los rects de clic
  no pueden desviarse porque los dos llaman a la misma función. El título dice
  dónde está la ventana (`INPUTS ↕ 8-14/21`) en vez de gastar una fila. La rueda
  sobre un cajón mueve su cursor. El test contrasta rect y pintado contra el
  mismo buffer con 20 puertos en 7 filas.

- **Medidores, latencia y presets de fábrica** (lo que faltaba de la fase 1 del
  punto 2):
  - **`choz_ports::FxMeter`**: pico de entrada y salida en dos atómicos
    compartidos — el mismo camino que `SandboxStatus`, porque el procesador es
    del hilo de audio en cuanto se entrega.
  - **`fx_chain::Metered` envuelve cada efecto de la cadena**, así que todo está
    medido sin que ningún efecto lo sepa —**incluido un plugin hosteado**, que es
    donde de verdad se pregunta si está llegando algo. La alternativa (dos
    campos dentro de cada procesador) eran treinta copias de las mismas dos
    líneas y ningún medidor para lo ajeno.
  - **`FxProcessor::latency_samples()`**: `AutoTune` reporta la ventana de su
    shifter, la cadena la suma (`AudioEngine::slot_latency`) y la caja `SLOT`
    la escribe en ms. **No hay compensación** — no hay arreglo contra el que
    alinear —, pero el número explica por qué el rack va pesado.
  - **Presets de fábrica**: `choz-ui/src/fx_presets.rs`, 17 efectos con 3–4 cada
    uno, botón `PRESET` en la caja `SLOT` y tecla `P`. Van en la UI porque un
    preset son posiciones de knob y el orden de los knobs lo define
    `fx_param_descs`; se indexan **por nombre** de parámetro (un índice apunta a
    otra cosa el día que se inserte un knob, y en silencio); y se aplican por
    `set_fx_param`, la misma puerta que un knob, un CC y el picker. Lo que ya
    tenía knob de preset (EQ gráfico, AutoTune, curva del saturador) se queda:
    eso son posiciones automatizables, no un menú.
  - Un preset que toca `Wet` escribe además `entry.wet`, que es de donde lee el
    rebuild. 3 tests nuevos.

- **Infraestructura de DSP + `Saturator`** (fase 1 y la mitad de la fase 3 del
  punto 2):
  - `fx/oversample.rs`: `Oversampler` a **1x/2x/4x/8x** como cascada de etapas
    de 2x, cada una con su lowpass Butterworth de **4º orden** a su propia
    frecuencia. El orden importa: con 2 polos la primera reflexión a 23 kHz
    baja apenas 10 dB y encadenar etapas topa contra el filtro, no contra el
    factor. Medido con Goertzel sobre el 7º armónico de 5 kHz reflejado a
    13 kHz: 8x deja **menos del 10 %** de lo que deja 1x. Incluye `Tone`
    (lowpass logarítmico 400 Hz–18 kHz) y un offset alterno de -260 dBFS que
    mantiene los filtros fuera del rango denormal en silencio.
  - `fx/smooth.rs`: `Smoothed`, un polo, consciente del sample rate. **Salta al
    destino cuando el hueco baja de 1e-5**: en `f32`, `target - diff·coeff`
    llega a un punto fijo con el hueco todavía en ~1e-5 cerca de 0.75, porque el
    paso cae bajo el ulp del resultado — sin eso el valor se queda para siempre
    a un redondeo del destino. Se llama `tick()` y no `next()` a propósito: no
    es un iterador.
  - `fx/saturator.rs`: **`Saturator`**, el waveshaper general — ocho curvas
    (soft/hard/tubo/cinta/foldback/wavefolder/diodo/polinómica), *drive*
    exponencial, *bias*, tono **después** de la curva, bloqueo de DC, ganancia
    de salida y oversampling elegible. Las curvas están normalizadas para que
    comparar dos compare su forma y no su volumen; el `Foldback` es **forma
    cerrada, no un bucle** (un `while` sobre una entrada sin acotar es trabajo
    ilimitado en el hilo de audio). `Curve` y `Oversamp` se dibujan como listas
    de nombres (`ParamShape::Named`) sacadas de los enums del DSP, así que una
    etiqueta no puede desviarse de lo que hace el procesador. 10 tests: cada
    curva acotada y finita, silencio, mezcla seca idéntica al dry, el bias sin
    dejar DC, aliasing medido, el tamaño de bloque sin cambiar el resultado, y
    bloques vacíos / cambios de sample rate.

- **`mtof`: una nota como control** (cierra el punto): `crates/choz-ui/src/mtof.rs`
  + `RackSlot.{mtof, mtof_amount}`. El botón `M→P` de la línea INSTR arma el
  puntero; un clic en cualquier knob lo liga, un clic fuera lo desliga, y la
  etiqueta del botón dice a dónde apunta. Cada nota que suena en esa tab —de las
  teclas **o del arpegiador**— escribe el valor por `apply_target`, o sea por la
  misma puerta que un CC.
  - **Dos conversiones, no una.** Si el destino es un parámetro de plugin que
    declara unidad `Hz` y rango, se escribe la frecuencia real **en escala
    logarítmica** (una octava = la misma distancia siempre; lineal, 20 Hz–20 kHz
    gasta tres cuartos del recorrido por encima de lo que se toca). Cualquier
    otro destino se **key-trackea** sobre `pitch::{MIN_NOTE, MAX_NOTE}`: un
    `FxParamDesc` de choz no declara ni rango ni unidad, así que "escribir 440"
    ahí sería un invento, no una conversión.
  - Se guarda en el proyecto. 5 tests de la conversión + 1 de la UI.

- **Secuenciador de pasos** (segunda mitad del punto): el mismo `Arp` con
  `PlayMode::{Arp, Seq}` — mismo reloj, mismo gate, mismo swing, sólo cambia de
  dónde salen las notas. `REC` graba pisando teclas (**las teclas pisadas a
  la vez, dentro de 40 ms, son un acorde en un paso**, que es lo único que
  distingue un acorde de cuatro pasos sin rejilla contra la que cuantizar),
  `REST` escribe silencio, `CLR` borra, `▶ ‖` y `■` son transporte de verdad
  (`■` rebobina, `‖` conserva la posición — por eso son dos botones) y `TAP` fija
  el tempo con la media de los últimos 4 golpes, descartando el hueco si pasan
  más de 2 s. Tope de `MAX_STEPS = 64`: se graba en vivo y una tecla trabada
  escribiría hasta quedarse sin memoria. El patrón viaja en el proyecto
  (`project::Slot.arp_pattern`, aparte de los ajustes porque éstos son `Copy`).
  Botón de `SWING` (que estaba implementado y sin UI) y seis acciones nuevas de
  MIDI learn: `ARP ON/OFF`, `▶/‖`, `■`, `TAP`, `REC` y `LATCH` — los controles
  que se necesitan con las dos manos ocupadas; un ajuste no merece un pedal.
  8 tests más.

- **Arpegiador por tab** (la mitad del punto 1): `crates/choz-ui/src/arp.rs`.
  Modos `UP` / `DOWN` / `UP-DN` / `PLAYED` / `RANDOM` (xorshift con semilla fija:
  aleatorio reproducible), divisiones `1/4 … 1/32` con **tresillos de verdad**
  (tres en el espacio de dos), BPM propio 20–300, gate, swing, hasta 4 octavas
  apiladas —una nota que se pasaría de 127 se **descarta**, no da la vuelta— y
  latch. Línea `ARP` en el RACK: apagado es un solo interruptor, encendido
  despliega sus ajustes en la misma fila; `A` lo enciende, los botones se pulsan
  con el ratón. Se guarda en `project::Slot.arp` con `#[serde(default)]`.
  - **No es un `FxProcessor`** y no puede serlo: `process_block` recibe audio
    interleaved y no tiene por dónde sacar notas. Vive donde se resuelve el
    ruteo — el hilo de UI —, y `Arp::tick` **recibe el instante** en vez de leer
    el reloj, que es lo que permitirá moverlo al engine sin tocar la lógica.
  - El bucle de eventos despierta cada **5 ms** mientras hay un patrón sonando
    (50 ms en reposo): un paso que cae dentro de 50 ms se oye tarde.
  - Un tick tardío **no corre la rejilla ni dispara una ráfaga** para
    recuperarse: el siguiente paso se cuenta desde el anterior, y si eso ya
    quedó atrás se re-ancla al presente.
  - Cada nota que empieza, la termina: `PANIC` y apagar el arpegiador sueltan lo
    que estuviera sonando. Un generador que se olvida de un note-off es una nota
    colgada que ya no depende de soltar ninguna tecla. 12 tests.

- **Visualizador de teclado MIDI** (punto 1 del roadmap anterior): dos pestañas
  nuevas en el panel `MIDI IN` — `KEYS` (piano de dos filas, negras arriba,
  etiquetas de octava debajo) y `ROLL` (las mismas notas cayendo hacia el
  teclado, ventana de 4 s, anillo de 256 notas de presupuesto fijo).
  `KeyboardState` en `views/midi_monitor.rs`, alimentado en `drain_midi`
  **después** de resolver el ruteo, así que una tecla puede pintarse del color
  de la tab que la está tocando. Tres modos de color (`CHANNEL` / `INSTRUMENT` /
  `VELOCITY`), tecla `C` para rotarlos, guardado en `ui.json`. Los CC no
  encienden teclas: fila propia con `BEND` y `MOD` siempre visibles. `PANIC`
  limpia el teclado. **Sin timeout de notas colgadas**: una nota sostenida un
  minuto es una nota sostenida, y apagarla sola sería mentir sobre el caso fácil
  para acertar el raro. 9 tests.
- **Paquete de Arch Linux** (punto 2): `packaging/arch/PKGBUILD.in` (`choz-bin`,
  x86_64 + aarch64 + armv7h) + `mkpkgbuild.sh`, que rellena versión y los tres
  `sha256` desde los tarballs publicados y **falla si falta una arquitectura**
  en vez de dejar el placeholder. Job `arch` en `release.yml`: corre dentro de
  `archlinux:base-devel`, genera `.SRCINFO` con `makepkg --printsrcinfo`, pasa
  `namcap`, adjunta ambos al release y empuja al AUR sólo si existe el secreto
  `AUR_SSH_KEY`. `depends=(alsa-lib …)` y **JACK en `optdepends`** — se
  `dlopen`ea, declararlo dependencia bloquearía la instalación en una máquina
  ALSA. 2 tests (`packaging_assets.rs`) que comprueban que instala el mismo
  conjunto que el `.deb` y que el generador no deja placeholders.

## Hecho (2026-08-12)

- **1.0.0 re-etiquetada y verificada**. El tag ya apunta a `25ad738` (los
  arreglos de empaquetado) y el workflow reconstruyó los artefactos. Comprobado
  sobre los paquetes publicados: el `.deb` lleva binario, `choz-launcher`,
  `.desktop`, los siete PNG + SVG, el MIME y el copyright; **cero rutas
  `/home/jorge`** dentro del binario; `SHA256SUMS.txt` cuadra; y los tarballs de
  `armv7` y `aarch64` traen ELF de la arquitectura correcta (el `armv7` que no
  se pudo verificar en local sí lo construyó el CI).
- **`install.sh` dentro del tarball ya no pide un compilador.** Sin `--binary`
  llamaba a `cargo build` **estando el binario a su lado** — un usuario que baja
  el `.tar.gz` no tiene toolchain. Ahora usa `$HERE/choz` si es ejecutable. Test:
  `the_installer_uses_the_binary_shipped_beside_it`.
- **Los botones `◀ ▶` del banco se pulsan enteros, en cualquier idioma.** Los
  rects de clic partían de un desplazamiento fijo (`inner.x + 2 + 8`) que sólo
  valía para la etiqueta inglesa: con `BANCO` (o cualquier traducción de otra
  anchura) el rect quedaba corrido y media flecha no respondía. Ahora la
  posición sale de lo que los spans ocupan de verdad (`line_width`), tanto en la
  línea `BANK` como en la de `INSTR`. Test:
  `the_whole_bank_arrow_is_clickable_after_the_label_is_translated`.

---

## Notas / gotchas para el que retome

- **Los cajones IN y OUT hacen scroll** (ya no es el gotcha que era): `drawer::{list_height, list_scroll}` calculan la ventana visible **a partir del cursor**, sin offset guardado, y las llaman tanto el dibujo como los rects de clic — que es lo que impide que se desvíen. La rueda del ratón mueve el cursor. Si aparece otra lista larga en un panel, ésa es la pieza a reusar.
- **Un rect de clic no se calcula con offsets a mano.** Es la raíz del bug de los botones de banco: cualquier texto anterior en la línea puede estar traducido, y entonces el rect apunta a otra columna. Los rects salen de las anchuras reales de los spans (`Span::width`, no `chars().count()`, que miente con CJK).
- **`in_pair` es un índice en esa lista plana de puertos.** Si se desenchufa una tarjeta, los índices se corren y un proyecto guardado apunta a otro jack. Guardar el nombre del puerto lo arreglaría; hoy la respuesta honesta es volver a asignar.
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

# Comprobar un paquete publicado sin instalarlo.
dpkg-deb -c choz_1.0.0-1_amd64.deb
strings usr/bin/choz | grep -c /home/       # tiene que dar 0
```
