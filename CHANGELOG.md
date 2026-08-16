# Changelog

Todos los cambios notables de choz. Formato basado en
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/).

El historial va agrupado por día de trabajo. **Éste es el historial**: los
diagnósticos, las medidas y los callejones sin salida se cuentan aquí.
[docs/roadmap.md](docs/roadmap.md) sólo lleva lo que falta, y
[docs/architecture.md](docs/architecture.md) cómo encajan las piezas.

## [1.0.0] — 2026-08-09

Primera versión etiquetada. Artefactos de x86_64 construidos desde este árbol
(binario, `.deb` con las dependencias resueltas solas, `.rpm`); el aarch64 se
cruza con `cross` y `.github/workflows/release.yml` los genera todos en un tag.

Todo lo de abajo — desde el commit inicial hasta hoy — es lo que lleva:

- los seis formatos de plugin hosteados, con su ventana nativa;
- rack multi-slot con mixer, FX de inserto, ruteo de entradas/salidas y proyectos en YAML;
- cuatro capas contra el código ajeno que revienta: escaneo fuera de proceso, cuarentena, sandbox por plugin, y tener ventana como motivo suficiente para aislarlo;
- parámetros dibujados según lo que el plugin dice que son (interruptor, enumerado, fader, banco vertical, knob), MIDI-learnables uno por uno;
- transporte propio que leen VST2, VST3 y CLAP;
- 372 esquemas de color, fondo de escritorio y nueve idiomas;
- empaquetado `.deb`/`.rpm`/`install.sh` con entrada de escritorio, y una superficie de control de ejemplo para ESP32-S3 táctil.


## [1.1.0] — 2026-08-16

Lo que trae, sobre la 1.0.0:

- **Pure Data hosteado**: un `.pd` con `adc~` y `dac~` es un efecto más de la
  cadena, corriendo en su propio proceso (`choz-pd-host`, el único binario que
  enlaza libpd), y **todos sus sliders son knobs en el rack**. Los que el patch
  no nombra (`empty empty`, que es como Pd guarda un slider salvo que alguien
  escriba el símbolo a mano — o sea, casi todos) los nombra choz en una **copia**
  que es lo que suena; el archivo del usuario no se toca. Medido con
  `delay.pd`: cinco sliders sin nombre, patch mudo antes, 3.0 de pico ahora.
  Los knobs además arrancan en la unidad cuando el rango la contiene, y no en el
  mínimo: una cadena de multiplicaciones que empieza en cero es silencio sin un
  solo error.
- **Los 45 efectos propios, publicados como un `.clap`** para Bitwig, Reaper,
  Carla o cualquier host CLAP, siguiendo el transporte del anfitrión. El
  instalador y los paquetes lo ponen donde el host lo busca.
- **Guardia de acople** en la entrada: baja 18 dB cuando la señal crece y sigue
  creciendo, y suelta al segundo. Interruptor en Settings → AUDIO.
- **`A→M` y AutoTune, endurecidos**: anti-alias y paso-alto de sala en el
  detector, mediana de tres, y el trim de entrada dejó de ser también el nivel
  de la mezcla — que es lo que sonaba saturado.
- **Harmonizer**: suena al nivel que debe (era 7 dB más bajo) y puede **seguir
  el acorde de un teclado MIDI**, con interruptor y canal propios.
- **Importador de Max/MSP**: lee un `.maxpat`, conserva lo que tiene
  equivalente y **nombra lo que no**.
- Los jacks de captura se guardan por nombre, así que un proyecto reabierto sin
  la tarjeta no escucha el micrófono de otra cosa; los wallpapers viajan con el
  programa y una instalación nueva abre con el suyo; Pure Data entra en la
  instalación por defecto y CI construye ese camino.
- **Quitado**: la sección de algoritmos de entrada, que existió durante una
  tarde. Queda el arpegiador.

530 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-16 (ter) — CI decía la verdad: el paquete es `libpd-dev`

#### Corregido
- **`puredata-dev` no trae `libpd.so`.** Trae las cabeceras para escribir
  externals de Pd; la biblioteca que `-lpd` necesita la trae **`libpd-dev`**
  (comprobado con `dpkg -S`: `libpd-dev` da `/usr/lib/*/libpd.so` y
  `/usr/include/pd/z_libpd.h`). Con el nombre equivocado, CI compilaba los once
  crates y **fallaba al enlazar**, que es la forma más cara de equivocarse.
  Cambiado en `ci.yml`, en `release.yml`, en `install.sh` y en el README.
- CI comprueba ahora que `libpd.so` existe **antes** de construir nada con la
  feature, así que la próxima vez el mensaje dirá lo que pasa en vez de dejar un
  error del enlazador.
- El tarball de ARM lleva también los wallpapers: son ficheros y viajan a
  cualquier arquitectura, mientras que el `.clap` y el hijo de Pd son nativos y
  se construyen sólo en x86_64 (`install.sh` se salta lo que no esté a su lado).

### 2026-08-16 (bis) — Enter apaga el arpegiador

#### Corregido
- **Enter no podía apagarlo.** Su interruptor es el primer knob de la caja, y
  Enter *empujaba* ese knob hacia arriba — que en algo de dos posiciones
  significa "encendido", y otra vez "encendido". Un interruptor se pulsa, no se
  empuja: ahora Enter lo cambia en los dos sentidos.
- Lo demás que hace Enter en esa caja no se movió: sobre un knob cuyas
  posiciones tienen nombre (modo, división, octavas) sigue abriendo la lista.

### 2026-08-16 — Fuera la sección de algoritmos: queda el arpegiador

Decisión del usuario, dicha a mitad de construirla: la sección de algoritmos de
entrada se va entera y la pestaña vuelve a tener **un arpegiador**, con su
interruptor donde siempre estuvo.

#### Quitado
- La caja `ALGO` y todo lo que colgaba de ella: la lista por tab, el `+ ADD`,
  los interruptores por fila, el `DEL`, su modal, sus acciones de ratón, sus
  teclas y sus destinos de MIDI learn.
- **`choz-ui/src/algo.rs`** — con el trait `InputAlgorithm`, que se había
  escrito precisamente cuando apareció un segundo implementador y se queda sin
  ninguno ahora que el arpegiador es el único.
- **`choz-engine/src/note_algo.rs`**, el driver que movía un patch de Pd desde
  el bucle de interfaz.
- **La vuelta de notas del puente del sandbox** (`out_midi`, `OutMidiLink`,
  `take_notes`) y el camino MIDI del hijo de Pure Data. Existían para eso y
  para nada más; código sin quien lo llame es peor que código que no está.
- `PatchRole::InputAlgorithm`: un `.pd` con `adc~` y `dac~` es un **efecto**, y
  las notas que tenga son asunto suyo.

#### Sin tocar
- El arpegiador, entero: modos, divisiones, `SYNC`, `TAP`, latch, acorde y su
  ruteo.
- Los patches de Pure Data **como efectos**, con sus sliders convertidos en
  knobs — que es lo que se arregló ayer y sigue funcionando igual.
- `A→M`, que nunca estuvo en la lista, y el acorde MIDI del Harmonizer, que usa
  su propio canal (`chord.rs`) y no tiene que ver con esto.

529 tests, clippy limpio con y sin `--features pd`.

### 2026-08-15 (unvicies) — Por qué un patch de Pd no sonaba: tres causas, las tres reales

Reportado: "añadí `/home/jorge/Pd` al escaneo y no funcionó", con dos patches de
ejemplo. Ninguna de las tres causas era la que parecía.

#### Corregido
- **Un formato que el `plugin-paths.json` guardado no conoce se quedaba sin
  directorios.** Todo fichero de rutas es anterior al último formato añadido, así
  que `PD` llegó con **ningún sitio donde buscar** — y eso se vive como "choz no
  encuentra mis patches" y se contesta escribiendo una ruta a mano (la del
  usuario acabó siendo `/home/jorge/Pd/.pd`, que no existe). Ahora, al cargar, se
  rellenan con sus valores por defecto **sólo** los formatos que el fichero no
  menciona: lo que el usuario editó manda siempre.
- **libpd no es Pure Data**: arranca sin ruta de búsqueda, así que todo objeto
  que Pd trae como abstracción —`rev1~`, `rev2~`, `hilbert~`, todo `extra`—
  fallaba al crearse. El patch abría con un agujero y el efecto no hacía nada.
  `choz-pd-host` añade ahora la carpeta del patch, un `externals/` al lado, los
  `extra` del sistema y `$PD_PATH`.
- **Pd hablaba solo**: sus mensajes —incluido "couldn't create"— iban a una
  consola que nadie lee. Ahora pasan por el log de choz, y el hook se instala
  **antes** de `libpd_init` porque las primeras quejas salen durante el arranque.

#### Añadido
- **Los controles de un patch son knobs en choz.** `read_patch` lee ahora los
  `hsl`, `vsl`, `nbx`, `tgl` y `radio` de la ventana: nombre, rango, y —lo que
  decide todo— **si tienen símbolo de recepción**. Los que lo tienen se publican
  como parámetros (`read_plugin_params` contesta para `PD`) y moverlos en el rack
  mueve el control dentro de Pd, escalado a su propio rango.
- **Los que no lo tienen se nombran en voz alta.** Es la causa de que los dos
  patches de ejemplo sonaran a nada: sus sliders son `empty empty`, y un
  `hsl` recién puesto vale **cero** — el patch entero multiplicado por cero, sin
  error en ninguna parte. El log dice ahora cuáles son y qué hacer:
  ponerles un símbolo de recepción en las propiedades del slider.
- Verificado con los patches reales: con `gain` y `room` nombrados, el reverb
  pasa de un pico de 0.000 a 1.52 movido desde choz, a través del proceso hijo.

#### Nota
- **El `hsl` de un patch tiene que estar escrito entero.** Pd no crea uno al que
  le faltan campos, aunque el lector de choz sea tolerante. Costó un test.

### 2026-08-15 (vicies) — El Harmonizer sigue el teclado, y su `Wet` dejaba de existir al reconstruir

#### Añadido
- **Entrada MIDI en el Harmonizer, y sólo en él.** Dos parámetros nuevos al
  final de su lista (el orden de los anteriores está congelado: un CC aprendido
  en `Wet` sigue en `Wet`): **`MIDI`**, un interruptor, y **`Ch`**, la lista de
  los dieciséis canales — que por ser una lista se abre como un modal en vez de
  recorrerse con la flecha.
  - Con el interruptor puesto, **la armonía es el acorde que se toca**: la nota
    más grave es la raíz y las de encima son los intervalos, sin escala ni
    tonalidad de por medio, porque la mano ya eligió las notas. Sin nada
    pulsado se queda el último acorde: un armonizador que se calla al levantar
    las manos no se puede tocar.
  - **La referencia es el tab activo**, y sólo él.
  - **En MULTI se deshabilita**: allí cada tab responde a su canal, y un acorde
    global sería el teclado de otro decidiendo esta armonía.
- El acorde viaja por `choz-engine/src/chord.rs`: un singleton de nueve
  atómicos, publicado por la interfaz y leído en el callback, igual que el
  transporte. **`FxProcessor` sigue siendo audio y sólo audio** — esta es la
  puerta más pequeña que deja entrar lo pedido sin ponerle un puerto de notas a
  los cuarenta y cinco efectos.

#### Corregido
- **`Harmonizer::with_params` no leía su propio `Wet`** (índice 8), así que
  cada reconstrucción de la cadena —añadir otro efecto, reabrir un proyecto—
  devolvía la mezcla a la mitad por debajo del knob.
- **`intervals()` mentía**: recalculaba desde la forma y la tonalidad en vez de
  decir lo que las voces hacen. Con un acorde mandando, eso era una respuesta
  de otro efecto. Ahora cada voz guarda su transposición y eso es lo que se
  reporta.

#### Verificado
- Test nuevo en el motor: **la captura llega a la cadena de FX de su tab y
  vuelve a la mezcla**. Escrito porque "conecté el micro al Harmonizer y no
  pasa nada" se estaba contestando midiendo el efecto suelto, que sólo sabía
  decir que el efecto estaba bien.

### 2026-08-15 (undevicies) — El Harmonizer sonaba 7 dB por debajo de lo que entraba

Reportado: micrófono del headset al Harmonizer, **ninguna respuesta**. Estaba
funcionando; salía tan por debajo que no se oía.

#### Corregido
- **Las voces se dividían por su número y no por su raíz.** Cantan notas
  distintas, así que son incoherentes y lo que se suma es la potencia: `1/√n`,
  no `1/n`. Eran 3 dB de menos con dos voces y 9 con ocho.
- **El seguidor de envolvente abría contra un nivel absoluto.** Un micrófono de
  headset vive sobre -40 dBFS y el umbral estaba puesto en una constante, así
  que las voces no pasaban de medio abiertas por fuerte que se cantara. Ahora la
  envolvente rápida se lee **contra el pico reciente de la propia señal**, con
  lo que abre igual con una línea caliente que con un micro flojo.
- Medido: a pleno wet la salida pasa de **-7,2 dB bajo la entrada a -4,7 dB**, y
  —lo que importaba— ese número **ya no depende del nivel de entrada**. Los
  -4,7 restantes son el panorama: con dos voces cada canal lleva una, y la suma
  de las dos tiene la potencia de la entrada.
- Test nuevo `the_harmony_is_as_loud_as_the_input_at_any_level`, con una línea
  caliente y un micro 30 dB por debajo. Comprobado que **falla con el código
  anterior**.

#### Notas para el que mida esto otra vez
- **Un desplazador de línea de retardo hace warble, y el warble reparte la
  energía en bandas laterales.** Midiendo un solo bin de frecuencia el tercero
  salía a -9,7 dB y la octava a -39, y por ahí se llega a "el desplazador está
  roto" — que es donde casi se arregla lo que no estaba mal. Con ruido (que no
  tiene fase que cancelar) y con RMS total: **la entrada y la salida miden lo
  mismo a cualquier intervalo**. La medida correcta es la potencia, no el bin.
- Se probó y se **descartó** cambiar `VoiceShifter` a cabezas alternas: la
  premisa era esa medición equivocada, y no se pudo demostrar que sonara mejor.

### 2026-08-15 (duodevicies) — `A→M` vuelve a ser independiente

Corrección del usuario: `ftom` no va dentro de la sección de algoritmos.

#### Cambiado
- **`A→M` sale de la lista `ALGO`** y vuelve a su propio interruptor en la línea
  de entrada, con sus knobs `IN`/`SENS`/`MIX`. La lista queda en `OFF`, `ARP` y
  `PD`.
- **Y deja de ser excluyente**: el arpegiador y un patch siguen turnándose entre
  ellos —los dos deciden qué notas llegan al instrumento, y dos a la vez
  tendrían que ponerse de acuerdo en el orden— pero ninguno de los dos apaga ya
  el conversor, ni él a ellos. Una tab puede convertir su guitarra en notas
  **y** arpegiar el resultado, que es lo que meterlo en la lista había
  prohibido sin que nadie lo pidiera.
- El razonamiento está escrito donde vive: `A→M` lee audio, y el audio sólo
  existe en el callback; la lista decide qué le pasa a las **notas** camino del
  instrumento. Son preguntas distintas y ahora se responden por separado.

### 2026-08-15 (septendecies) — El roadmap queda en una sola cosa

Decisión del usuario: cerrar todo lo pendiente salvo el plugin CLAP que genera
notas, que resuelve él porque la parte difícil es una definición y no código.

#### Cambiado
- **`docs/roadmap.md` queda en 166 líneas y un único punto abierto.** Se
  cerraron las tres secciones que quedaban: la voz (ya estaba decidida), **las
  comprobaciones con hardware delante** y las deudas conocidas.
- **Lo que se cerró sin hacerse queda dicho donde se va a leer**, no borrado: el
  primer gotcha del documento dice ahora que **todo el DSP está verificado
  contra señales sintéticas y no contra una habitación**, y que cuando algo
  suene raro con el equipo delante ésa es la primera hipótesis — con los
  nombres de las constantes que hay que mover (`GROWTH_CHECKS`, `DUCK`,
  `STEADY_ANALYSES`, `MEDIAN_ANALYSES`, `SENS`/`IN`). Las decisiones cerradas
  (un algoritmo por tab, `A→M` fuera del trait, el `.pd` con `adc~` y `dac~`, el
  dispositivo que no cambia solo, JSFX inexistente) pasan a la misma sección
  como "no se vuelven a discutir sin una razón nueva".
- **README**: libpd entra en la tabla de dependencias de ejecución (opcional, y
  decide qué se construye), y una tabla nueva dice qué pone un install además
  del binario — el `.clap` de los efectos, los wallpapers y `choz-pd-host` —
  con `--no-clap` documentado.

#### Pendiente, y sólo esto
- **Un plugin CLAP que genere notas** en la sección de algoritmos de entrada.
  Lo que falta antes del código es la definición que el usuario se reservó: si
  la lista se limita a plugins "de algoritmos compositivos" o se ofrece
  cualquier CLAP con salida de notas. **Un CLAP no declara ser compositivo**; lo
  más cercano que da el formato es "puerto de notas de salida y ninguno de
  audio", que es una heurística y deja fuera a los que también suenan.

### 2026-08-15 (sedecies) — Ocho decisiones del usuario, aplicadas

El usuario contestó las ocho preguntas que bloqueaban el roadmap. Lo que
cambió en el código:

#### Cambiado
- **Nada de cambiar de dispositivo solo.** Cuando el sink elegido no tenía
  puertos, choz se pasaba a otro y lo decía en el log: una interfaz apagada
  movía el equipo entero a los altavoces del portátil, en mitad de lo que
  fuera. Ahora se queda **desconectado**, TRANSPORT escribe `NOT CONNECTED` y
  el mensaje dice dónde elegir otro. El dispositivo lo cambia el usuario, con
  `r` en el cajón OUT, y nadie más. (`any_sink_with_ports`, borrada.)
- **Un `.pd` necesita `adc~` **y** `dac~`** para que choz lo hostee. Los
  sliders y el MIDI son opcionales; `noteout` es lo que además lo convierte en
  algoritmo de entrada. Un patch que sólo tiene notas ya no se ofrece: no hay
  forma de la ranura para algo que ni toma ni devuelve audio.
- **El `.clap` de los 45 efectos se instala con choz**, no bajo bandera:
  `~/.clap/choz.clap` desde el instalador, `/usr/lib/clap/choz.clap` desde el
  `.deb`, el `.rpm` y el PKGBUILD de Arch. `--no-clap` para quien no lo quiera.
- **Los wallpapers viajan con el programa** (`share/choz/wallpapers`) y una
  instalación nueva **abre con el que trae** — `settings::shipped_wallpaper`,
  que sólo se consulta cuando no hay `ui.json` todavía. El selector de imagen
  arranca ahí, y en `assets/` cuando se corre desde el repositorio.
- **Pure Data entra en la instalación por defecto**: `install.sh` pide libpd por
  nombre (con el paquete de cada distro), construye `choz-pd-host` con
  `--features pd` y lo instala junto a choz. Sin libpd **no falla**: instala sin
  esa mitad y lo dice. El `.deb` lo recomienda, Arch lo lleva en `optdepends`, y
  **CI instala `puredata-dev`** y construye y prueba ese camino, tests de punta
  a punta incluidos.

#### Cerrado sin código
- **Sinte de 8 voces controlado por voz**: no. La tab con `A→M` + un plugin ya
  lo hace.
- **Pitch shifting polifónico**: no; lo cubre el `Harmonizer`.
- **JSFX**: confirmado que se borró en 2026-08-06 y no queda nada en el código.
  La línea del roadmap que lo llamaba deuda estaba mal.
- **Encadenar algoritmos por tab**: no. Lo que se quiere en su lugar es poder
  **insertar un plugin CLAP que genere notas** en esa sección, y eso es lo único
  que queda como código pendiente en el roadmap.

#### Corregido
- Una línea del roadmap decía que los LV2 con `worker#schedule` se rechazaban.
  **Está soportado** desde que se portó el host; lo que se rechaza se dice
  plugin a plugin, con el nombre de la feature que falta.

### 2026-08-15 (quindecies) — Los jacks por nombre, y el flake que llevaba cuatro sesiones

#### Corregido
- **Un proyecto guarda ahora el nombre de sus jacks de captura**
  (`Mixer.in_ports`), no sólo el índice. La lista de puertos de captura es
  plana y global: desenchufa una interfaz y todos los índices posteriores se
  corren, así que un proyecto reabierto sin la tarjeta estaba escuchando el
  micrófono de otra cosa y no lo decía. Ahora manda el nombre; y si el jack ya
  no está, la tab se abre **sin entrada de audio** —que se ve— en vez de con la
  equivocada, que no. Un proyecto viejo, que sólo tiene el índice, sigue
  cargando como cargaba.
- **`arp::tests::a_synced_step_is_scheduled_ahead_of_the_sample_it_is_for`
  fallaba una de cada cuatro corridas** desde que existe, y no era un bug de
  timing: el transporte es uno por proceso y ese test medía un paso contra un
  playhead que otro acababa de rebobinar. Ahora los tests que tocan el
  transporte toman el mismo cerrojo que los que tocan idioma y color —una sola
  cola para todo lo global es más simple que dos que no se ordenan entre sí.

#### Cambiado
- **`docs/roadmap.md` podado**: 660 líneas a 216. Los puntos cerrados
  (arpegiador, algoritmos de entrada, Pure Data / Max, export CLAP) y los
  bloques "Hecho" salen del documento — están en este CHANGELOG, día por día y
  con los porqués. Lo que queda es lo que queda: la voz (aplazada a propósito),
  lo que sólo se comprueba con hardware delante, y una sección nueva de **deudas
  conocidas decididas a propósito**, para que ninguna decisión que estaba
  enterrada en un punto cerrado se pierda al borrarlo.

### 2026-08-15 (quaterdecies) — Max/MSP: importar lo que se pueda, y decir el resto

Punto 3.4 del roadmap, el último sin empezar. **No hay runtime de Max
empotrable** —no existe un libpd para Max y no va a existir—, así que prometer
compatibilidad sería mentir. Lo que sí se puede es leer el patch y ser claro.

#### Añadido
- **`choz-engine/src/maxpat.rs`**: un `.maxpat` es JSON, así que se lee sin Max
  instalado. Sigue los cables desde `adc~`/`plugin~`, convierte los objetos que
  tienen equivalente real entre los efectos propios de choz, y **nombra uno por
  uno los que no**. Sin `adc~` del que tirar, lee en orden de fichero y lo dice.
  - La tabla de equivalencias es corta a propósito: sólo donde la equivalencia
    es de verdad (`overdrive~`→saturador, `freeverb~`→reverb, `lores~`→filtro,
    `comb~`/`tapout~`→delay, `limi~`→limitador…). Adivinar produciría un patch
    que suena parecido y no es el que se escribió, y nadie sabría en qué parte.
  - Los argumentos de un objeto de Max están en sus unidades y **no se
    convierten**: cada efecto entra con sus knobs al medio.
  - Un bucle de realimentación en el patch no cuelga el recorrido; el comentario
    y los botones no salen en la lista de descartados (enterrarían lo que
    importa).
- **FILE → Import Max patch…**: elige el `.maxpat`, mete en la cadena de la
  pestaña activa lo que quepa (`MAX_FX`) y **abre un informe** con lo que entró
  y lo que no. El informe no es un `eprintln` en un log: lo que un import *no*
  pudo hacer es la mitad que hay que leer.

#### Notas
- Sólo se sigue el **primer** camino de audio. Un patch que se abre en tres
  ramas paralelas y las suma es un mezclador, y un mezclador no es una cadena de
  inserto; coger la primera y nombrar el resto es la respuesta que se puede
  comprobar de oído.

### 2026-08-15 (terdecies) — El trim era del detector, y un guardia contra el acople

Dos cosas que reportó el usuario tocando: seguía sonando saturado con `A→M` o
AutoTune activos, y el acople de la entrada molesta en cuanto entra una
distorsión o una reverb sumada a un delay.

#### Corregido
- **El trim de entrada era también el nivel de la mezcla.** Con `A→M`, `IN` es
  lo que oye el detector —una guitarra necesita mucho— y esa misma ganancia se
  colaba en lo que el jugador escucha por `MIX`. Subir uno estropeaba el otro
  por construcción. Ahora la señal que vuelve por `MIX` es la del jack, sin
  tocar; el nivel de la pestaña es su `VOL`, que siempre lo fue.
- **Lo mismo dentro de AutoTune**: su `InGain` multiplicaba el audio *y* el
  análisis. Ahora sólo levanta el análisis y el knob se llama **`Sens`**, como
  el de `A→M`; `OutGain` es el único control de nivel. Test: 18 dB de `Sens` no
  mueven el pico de salida, y 24 dB sí meten una voz por debajo de la puerta
  dentro del detector.

#### Añadido
- **Guardia de acople** (`choz-engine/src/feedback.rs`): mira la entrada y busca
  lo único que un acople siempre hace —**crecer, y seguir creciendo**— y baja la
  entrada 18 dB cuando lo ve, en 20 ms, soltándola en algo más de un segundo
  cuando la sala se calla. Va **antes del trim**, porque ahí es donde se cierra
  el lazo; bajar la salida dejaría la reverb de la pestaña alimentando el micro.
  - **No es un supresor de realimentación**: uno de verdad busca la frecuencia
    y la nota; eso es un banco de filtros y otro proyecto. Éste compra los
    segundos que se tarda en llegar a un fader.
  - Lo que **no** hace, y hay test de cada uno: una nota tenida fuerte no se
    toca, una que decae tampoco, y algo bajo que sube (un arco, un fundido) está
    por debajo del suelo (-26 dBFS) y ni se mira.
  - Interruptor en EDIT → Settings → AUDIO → Engine (`Feedback guard`,
    encendido por defecto y guardado en `ui.json`), y **se ve mientras actúa**:
    la fila dice `ON (holding -18 dB)` y el cajón IN escribe `GUARD -18 dB`.
- **`packaging/install.sh --with-clap`**: instala los 45 efectos de choz como
  `~/.clap/choz.clap` para usarlos en otro host, y `--uninstall` se lo lleva.
  Opcional a propósito: escribe fuera de `--prefix`, en un directorio que es del
  usuario.

### 2026-08-15 (duodecies) — AutoTune tenía los mismos agujeros que `A→M`

Reportado por el usuario: "el efecto autotune tiene el mismo problema de ruidos
que tenía ftom". Lo tenía, literalmente: su detector es el gemelo del de `A→M` y
se quedó sin las tres cosas que aquél aprendió con una guitarra y un micro
delante.

#### Corregido
- **La diezmación era un promedio**, es decir su propio filtro anti-alias, y un
  promedio es un filtro que fuga. Un siseo de 9,5 kHz —sibilancia, aire, la sala—
  se doblaba encima de la nota: medido, el detector reportaba **+91 cents** sobre
  un tono de 220 Hz. Ahora se pasa-baja a 3,5 kHz **antes** de promediar.
- **Nada quitaba la sala por debajo.** Con un retumbe de 41 Hz más fuerte que la
  voz, el detector **no encontraba nota ninguna**. Ahora hay un paso-alto a 55 Hz
  de dos secciones (24 dB/oct) sobre la señal diezmada.
- **Cada ventana salía tal cual**: una sola mala —una consonante, una puerta—
  movía el ratio de corrección de ese bloque, y eso se oye como warble. Ahora la
  frecuencia que sale es la **mediana de las tres últimas**, y una ventana que
  pierde confianza **sostiene** la nota dos análisis en vez de soltar la
  corrección de golpe (soltarla es un clic).
- **El ratio escalonaba por bloque.** El suavizador da un paso por bloque y ese
  valor se usaba para las 512 muestras enteras: plano, salto, plano. Ahora el
  shifter recibe el ratio **en los dos extremos del bloque** y lo camina muestra
  a muestra (un `add` por muestra).
- **La puerta baja a -56 dBFS** (era -50): el nivel se mide *después* de los
  filtros nuevos, que quitan energía real de una voz. La misma lección que `A→M`
  aprendió a -61.
- **El análisis va por el camino rápido de YIN**: la ventana se endereza antes de
  llamar, que es lo que evita dos `%` por muestra en el bucle interno.

#### Añadido
- **La nota de AutoTune se ve en `MIDI IN`**: el panel del monitor enciende la
  nota a la que está corrigiendo, junto a la de `A→M`. `KeyboardState` lleva
  ahora una nota por fuente (`Converted::{PitchToMidi, AutoTune}`) y cada una
  apaga sólo la suya. Esas notas se deciden en el callback y no viajan como
  MIDI: este panel es el único sitio donde se pueden mirar.
- Cuatro tests que **fallan sin los arreglos** (comprobado quitándolos): retumbe
  bajo la voz, siseo encima, una ventana mala aislada, y el efecto entero con
  voz + retumbe + siseo a la vez (sin filtros oía 433,8 Hz donde se cantaba 445).
  Más uno de que el ratio se camina y no salta, y otro del monitor.

#### Notas
- **AutoTune no manda MIDI a ningún puerto**, y esto no lo cambia: lo que se ve
  es la nota a la que apunta. Sacarla por un puerto MIDI es otra cosa — lo que
  hace `A→M` — y sigue sin pedirse.
- El medidor de AutoTune es uno por proceso: con dos en el rack, el último en
  correr es el que se ve, igual que en su propia lectura del RACK.

### 2026-08-15 (undecies) — El tercer algoritmo, y el trait que se lo ganó

Punto 3.3 del roadmap, y con él la mitad de dentro del punto 2: un patch de
Pure Data que saca notas es ahora una entrada más de la caja `ALGO`, al lado
del arpegiador.

#### Añadido
- **Notas de vuelta por el puente**: `out_midi` en la cabecera de
  `choz-plugin-sandbox`, con `Sandbox::out_midi_link()` (el hijo, desde dentro
  de su closure de proceso) y `Host::take_out_midi()` (el anfitrión, después de
  cada bloque). Mismo tamaño fijo y misma regla que el camino de ida: lleno =
  se tira lo nuevo, porque ninguno de los dos lados reserva memoria.
- **`choz-pd-host` habla notas en los dos sentidos**: `libpd_noteon` para lo
  que entra, `libpd_set_noteonhook` para lo que sale.
- **`choz_engine::note_algo::PatchAlgorithm`**: el patch corriendo en su
  proceso, movido por `tick(now)`. El reloj de Pd avanza con los bloques que se
  le dan, así que un tick pide **tantos bloques como tiempo real ha pasado** —
  y si la interfaz se quedó parada, se recorta en vez de disparar una ráfaga de
  notas atrasadas.
- **`InputAlgorithm`** (`choz-ui/src/algo.rs`): notas entran, notas salen, más
  un tick. Lo implementan el arpegiador y el patch. **`A→M` no**, y está
  escrito por qué: lee audio, y el audio sólo existe en el callback — el
  argumento `audio: &[f32]` que el roadmap dibujaba no lo podría rellenar nadie
  desde este lado. Sigue en la misma lista y sigue siendo excluyente.
- **La caja `ALGO` ofrece `PD`** cuando hay patches con `noteout` en las rutas
  de búsqueda, con su modal para elegir cuál. El patch se guarda en el proyecto
  (`algo_patch`), y uno que ya no está en la máquina se avisa y se salta, como
  un plugin que falta.

#### Notas
- **El proceso se sincroniza solo**: cada tick compara el patch de la pestaña
  con el que su proceso arrancó. Así no hay una segunda lista que acordarse de
  tocar cada vez que una pestaña se añade, se borra o se mueve.
- **Un patch que no arranca se dice una vez** y la pestaña se queda sin
  algoritmo — si no, se intentaría arrancar en cada tick.
- **PANIC se lleva el proceso**, porque un patch sigue a su propio reloj y
  pedirle que pare no es lo mismo que quitarlo.
- El timing de las notas de un patch es el del bucle de interfaz (unos
  milisegundos), no el de muestra exacta que el arpegiador ganó en el engine.
  La misma discusión aplazada, con la misma respuesta cuando toque: programar
  por adelantado con una muestra de transporte.

### 2026-08-15 (decies) — Los 45 efectos de choz, fuera de choz

Punto 5 del roadmap. Un `.clap` que publica **los 45 efectos**, cargable en
Bitwig, Reaper, Carla o cualquier host CLAP — verificado cargándolo con el host
CLAP de choz, por dlopen, como haría un DAW.

#### Añadido
- **`choz-plugin-clap-export`** (`cdylib` + `rlib`): entry point, una factory y
  dos extensiones (`audio-ports`, `params`) contra **`clap-sys`** en crudo, que
  es sobre lo que ya se apoya el `clack-host` que choz usa para hostear. No hay
  framework de plugin de por medio, igual que en el lado LV2.
  - Instalar: `cargo build --release -p choz-plugin-clap-export` y copiar
    `libchoz_plugin_clap_export.so` a `~/.clap/choz.clap`.
- **`fx_chain::BUILT_IN_KINDS`**: la lista de efectos propios (id + nombre), al
  lado del `match` que los construye. La copia que vivía en los tests se ha
  borrado — era exactamente la deriva que debía cazar — y hay un test en la
  interfaz que compara su lista con ésta en los dos sentidos.
- **El dry/wet viaja como último parámetro**: dentro de choz lo aplica la
  cadena, y fuera no hay cadena que lo aplique.
- Los valores se publican como posiciones 0..1 y `value_to_text` los escribe en
  las unidades del efecto (`480.00 ms`, no `0.24`).

#### Notas
- **El transporte del host manda**: `follow_host_transport` lleva tempo, compás,
  posición en negras y play/stop del DAW al reloj global de choz en cada bloque,
  así que `BeatRepeat` exportado sigue al proyecto en el que está. Sin
  transporte, el reloj avanza con el bloque. (`Transport::set_position_beats`
  es la única puerta para una línea de tiempo ajena.)
- **Verificación sin DAW**: cuatro tests llaman al ABI en el propio proceso
  (factory, descriptores, extensiones, un bloque de audio) y uno más
  (`tests/real_host.rs`) copia el `.so` a un `.clap`, lo escanea con
  `choz_plugin_clap::scan_directory`, lo instancia con `ClapEffect::build` y le
  mete un bloque. Los 45 aparecen desde un solo fichero.
- **Un efecto exportado es el DSP, no el panel**: el medidor, los presets, la
  curva del EQ y el banco de puntos del waveshaper se quedan en la interfaz de
  choz. Darles ventana propia es otro proyecto.
- **La automatización de muestra exacta se aplana** a propósito: los efectos de
  choz toman un parámetro como "a partir de ahora", así que respetar el sello
  de tiempo obligaría a partir cada bloque para una diferencia inaudible.

### 2026-08-15 (nonies) — Pure Data suena: un patch es un efecto más

Puntos 3.1 y 3.2 del roadmap. Un `.pd` se escanea, se elige en ADD FX y procesa
audio **en su propio proceso**, medido de punta a punta contra la libpd 0.56.2
de Debian.

#### Añadido
- **`choz-pd-host`** (`crates/choz-plugin-pd/src/bin/`, sólo con
  `--features pd`): el único binario que enlaza libpd. Se engancha a la misma
  región compartida que el sandbox de plugins y sirve bloques. Así la LGPL de
  libpd se queda de este lado de la frontera de procesos, y la regla de "una Pd
  por proceso" —que no es un detalle, es la arquitectura— se cumple sola.
- **`PluginFormat::Pd`**: extensión `.pd`, `$PD_PATH`, y los sitios donde la
  gente guarda patches (`~/pd`, `~/.local/share/pd`, `~/Documents/Pd`,
  `/usr/share/pd/patches`). Con eso el escaneo, el cache, la lista de rutas de
  Settings y el chip `PD` de ADD FX salen del código que ya existía.
- **El escaneo lee el patch**: un `.pd` es un documento, no un plugin, así que
  sólo se ofrecen como efecto los que conectan audio. Los que sacan notas son
  algoritmos de entrada y esperan a su sección; los que no conectan nada no se
  listan. Todo eso sin Pure Data instalado — el fichero es texto.
- **Test de punta a punta** (`crates/choz-engine/tests/pd_patch.rs`): un patch
  de ganancia cargado como efecto de la cadena, 51 bloques, y el 0.4 vuelve
  como 0.2. Se salta **diciéndolo** cuando `choz-pd-host` no está construido.

#### Notas
- **Sin `choz-pd-host` no hay silencio raro**: el efecto no se crea y el log
  dice cómo construirlo. `CHOZ_PD_HOST` apunta a otro sitio si hace falta.
- Un patch no tiene ventana que choz pueda empotrar (el canvas de Pd es otro
  programa), así que el hijo declara `editor_present = false` y no aparece
  botón `GUI`.

### 2026-08-15 (octies) — El algoritmo de entrada es una elección, no dos interruptores

El punto 2 del roadmap, en su primera mitad: el arpegiador y `A→M` dejan de ser
dos excepciones cableadas y pasan a ser **dos entradas de la misma lista**.

#### Añadido
- **`crates/choz-ui/src/algo.rs`**: `InputAlgo::{Off, Arp, PitchToMidi}`, qué
  algoritmo son las dos banderas de una tab (`of`), cuáles puede elegir
  (`options` — `A→M` sólo con audio entrando) y los knobs de la caja
  (`knobs`, que construyen la lista una sola vez para el panel y para la
  interfaz: una caja cuyos knobs no son los que se editan mueve otro mando).
- **La caja ARP del RACK es ahora la caja `ALGO`**, siempre presente: su primer
  knob es el algoritmo, y debajo van los mandos del que esté corriendo. El
  botón de la fila corta —el que había para encender el arpegiador— nombra el
  algoritmo y camina al siguiente.

#### Cambiado
- **Uno por tab, y exclusivo.** Elegir uno retira el otro, por la puerta que
  sea: el knob `ALGO`, el botón de la fila, el botón `A→M` de la línea de
  entrada o un CC aprendido. La exclusión vive en los dos interruptores que ya
  existían (`edit_arp` y `toggle_pitch_to_midi`, que se llaman entre sí y
  terminan porque el segundo siempre está *apagando*), así que todo lo que un
  cambio arrastra —parar las notas que el arpegiador tenía cogidas, avisar al
  motor de la conversión— sigue pasando donde pasaba.
- **Las implementaciones no se movieron**: el arpegiador sigue en el bucle de
  interfaz y `A→M` en el callback de audio. Lo que cambia es que elegir uno es
  una elección y no dos interruptores que podían contradecirse. El trait común
  (`process(notas, audio, ahora) -> notas`) espera al tercer algoritmo, que es
  quien tiene que decidir su forma.

### 2026-08-15 (septies) — El arpegiador en muestras, y el punto 1 cerrado

El ítem que llevaba aplazado desde el principio, hecho como pidió el usuario:
partiendo el render por slot.

#### Añadido
- **Notas con hora.** `EngineCommand::NoteOn/NoteOff` llevan `at`, una muestra
  absoluta del transporte. `0` es "ahora" y es lo que manda todo lo que no
  tiene reloj propio —una tecla, un puerto MIDI, OSC—, así que ese camino no
  cambia en nada.
- **El render se parte por el slot.** Un slot con notas programadas se
  renderiza en segmentos: se aplica lo que vence en una muestra, se renderiza
  hasta la siguiente, y así. La nota empieza en la muestra para la que se
  escribió y no al principio del bloque que se enteró.
  - Cola fija de 8 por slot, y **el desbordamiento toca, no descarta**: una
    nota un poco pronto sigue siendo la nota; una perdida es silencio, o peor,
    una nota que no para nunca.
- **El arpegiador programa hacia delante** en lugar de reaccionar. Cuando el
  siguiente compás entra en la ventana de anticipación (25 ms, cinco veces lo
  que tarda el bucle en despertar), se manda ya con su muestra — swing incluido,
  porque la muestra es la que la nota merece. El cierre de puerta también lleva
  muestra: un ataque exacto seguido de una suelta en el siguiente tic de
  interfaz es media mejora.

#### Corregido
- **El mismo compás se programaba una y otra vez.** Cuando el siguiente paso
  estaba todavía lejos, el camino sincronizado caía al reloj libre, que borra
  `grid` — y en el tic siguiente el compás volvía a parecer nuevo. "Todavía no
  toca" no es "no hay rejilla", y ahora se decide una sola vez cuál de los dos
  relojes manda. Lo cazó el test.
- **`0` significaba dos cosas** en la muestra cero de un transporte rebobinado:
  "ahora" y "el downbeat". Una muestra de diferencia (20 µs) desambigua el
  protocolo.
- **Dos candados distintos para el mismo transporte global** no se serializan
  entre sí, y `render` lo avanza — así que cualquier test que renderizara movía
  el reloj de los demás. Un candado por global, en `test_locks`, y lo cogen los
  catorce tests que renderizan. Tres pasadas seguidas limpias.

#### Notas
- **`MIDI OUT` sigue saliendo inmediato**, y tiene que ser así: ALSA manda
  cuando se le dice, así que una nota programada para dentro de 20 ms saldría
  fuera antes de sonar dentro. El instrumento de la tab recibe la precisa.
- **El reloj libre no cambió**: sin transporte no hay línea de tiempo contra la
  que ser exacto, y sus notas siguen siendo "ahora".
- El coste que se temía es real y acotado: un plugin hosteado recibe varias
  llamadas pequeñas en vez de una cuando hay notas en medio del bloque. Es
  legal, y es lo que hace cualquier host con automatización de muestra exacta.
- 5 tests nuevos: la nota empezando en su muestra exacta, varias notas en un
  bloque (empujadas en desorden a propósito), la nota sin hora tocando al
  instante, el paso sincronizado programado por delante y no dos veces, y el
  reloj libre siguiendo inmediato.

### 2026-08-15 (sexies) — Vocoder, y el talkbox que salió del mismo código

Lo que el roadmap decía que era el más barato de los cuatro que faltaban de la
lista de voz, y el que más suena a lo pedido.

#### Añadido
- **`Vocoder`**: la voz se parte en bandas, se mide **cuánto** hay en cada una,
  y el portador se parte en las mismas bandas y se sube o baja con esos
  números. No pasa nada del sonido de la voz — sólo su forma — y por eso lo que
  se oye es el portador hablando.
  - **El portador es todo el carácter**: `SAW` y `PULSE` son la voz de
    ordenador (`Pitch` es la nota a la que habla, y mantenerla quieta es lo que
    la hace robot y no persona), `NOISE` es un susurro, y **`INPUT R` es un
    talkbox** — porque un talkbox *es* un vocoder cuyo portador es un
    instrumento de verdad. Voz por la izquierda, guitarra por la derecha, `Res`
    arriba para la respuesta puntiaguda que tiene un tubo en la boca. **No hay
    un segundo efecto para eso**: era el mismo código con otro portador.
  - 8, 16 o 24 bandas (16 es donde está el trato: menos es más basto y más de
    24 son bandas más estrechas de lo que se mueve un formante), `Res`,
    `Speed` de las envolventes, y `Shift` — que desplaza las bandas del
    portador contra las de la voz, o sea las mismas palabras saliendo de una
    cabeza de otro tamaño.
  - `Biquad::bandpass` en `fx/utility.rs`, al lado del `lowpass` y el
    `highpass` que ya estaban.
- 5 tests: el portador tomando la forma de la vocal (una vocal grave abre las
  bandas graves y una aguda las agudas), **silencio dentro = silencio fuera**
  aunque el portador siga corriendo (el fallo que todo el mundo ha oído de un
  vocoder mal hecho), el talkbox dejando oír la guitarra y no la voz, el número
  de bandas sin ser un knob de volumen, y los extremos.

### 2026-08-15 (quinquies) — Harmonizer, y lo que AutoTune no es

#### Añadido
- **`Harmonizer`**: hasta ocho voces transpuestas, en la tonalidad.
  - **Diatónico, no paralelo** — y esto es lo que lo hace musical: con clave y
    escala puestas, un intervalo es un **número de grados de la escala**, no un
    número fijo de semitonos. Una tercera sobre un Do en Do mayor es Mi (cuatro
    semitonos); sobre un Re es Fa (tres). Desplazar todo una distancia
    constante es el sonido de un pitch shifter barato, y se equivoca justo
    donde se nota. `Chromatic` **es** "sin tonalidad", así que la armonía
    paralela sigue estando, como una opción y no como un fallo.
  - **Formas con nombre** en vez de ocho knobs de intervalo (ocho knobs es una
    matriz): `3rds`, `5ths`, `OCT`, `ABOVE`, `BELOW`, `CLUSTER`, truncadas al
    número de voces (1, 2, 4 u 8).
  - **Micro-afinación** (`Detune`, hasta 25 cents repartidos entre las voces —
    dos copias exactamente al mismo tono son una copia más fuerte), **delay
    escalonado** por voz (llegar a la vez es un chorus; llegar 20 ms después es
    otro cantante), **seguidor de envolvente** (las voces abren con la entrada,
    así que un armonizador sobre un micro no canta en los huecos) y **anchura**
    estéreo.
  - Ocho voces **no** suenan ocho veces más fuerte: las copias están
    correlacionadas, y hay un test que lo fija.
- **`fx/shift.rs`**: el pitch shifter de dos cabezas, extraído de `shimmer.rs`,
  que ahora comparten el shimmer y el armonizador. Estaba a punto de haber una
  copia por efecto. 3 tests propios: la octava, la quinta, la octava abajo, la
  unidad como cable, y acotado a cualquier ratio.

#### Revisado: qué es y qué no es AutoTune
Contra la lista pedida, y hay que decirlo claro porque los nombres se parecen:
`AutoTune` es un **corrector de afinación monofónico** y eso es lo que hace
bien. **No** tiene motor sinte de 8 voces, ni voz de ordenador, ni talkbox, ni
pitch shifting polifónico — y no son ajustes que le falten, son otros efectos.
El corrector *sigue* un tono; un motor de síntesis *genera* uno. Están en el
roadmap con lo que costaría cada uno y en qué orden salen a cuenta (el vocoder
primero: el banco de filtros ya está).
- Lo único de esa lista que sí sale hoy —armonías orgánicas— es el
  `Harmonizer`. Sus intervalos **no vienen de MIDI**: un `FxProcessor` recibe
  audio y nada más, por diseño. La armonía dirigida por acordes vive en la
  sección de algoritmos de entrada, donde hay notas.

#### Roadmap
- Sección nueva: **exportar los 44 efectos propios como plugins CLAP**.
  `FxProcessor` ya tiene la forma de un plugin y choz ya hostea CLAP, así que
  falta la otra mitad. CLAP encaja mejor que LV2 para esto por una razón
  concreta: **no hay TTL** —los metadatos viven en el código, no en cientos de
  líneas de Turtle que se desincronizan— y la *factory* publica los 44 desde un
  solo binario. Lo que no viaja está anotado: el medidor y los presets son del
  panel, no del DSP.

### 2026-08-15 (quater) — Pure Data: medido primero, y la medida decidió la arquitectura

El roadmap decía cuál era la primera prueba y por qué: *"cargar libpd, correr un
patch de dos objetos sobre un bloque de audio, y medir qué cuesta. Si eso no
sale, lo demás sobra."* Salió, y además contestó una pregunta que no se había
hecho.

#### Medido (libpd 0.56.2 de Debian)
- Un patch de ganancia (`adc~ → *~ 0.5 → dac~`) cuesta **0.03 % del callback** a
  128, 256 y 512 frames. Pure Data no es la parte cara de nada.
- **Cero reservas de memoria por bloque.** Pd reserva cuando el grafo *cambia*,
  no mientras corre.
- **`libpd_new_instance()` devuelve null**: sin `PDINSTANCE`, hay **una Pd por
  proceso**. Eso decide la arquitectura entera — dos efectos de Pd no pueden
  convivir en un choz, así que **cada patch es un proceso**, que es justo lo que
  `choz-plugin-sandbox` ya hace. La licencia sale gratis de ahí: el hijo enlaza
  libpd (LGPL), el binario de choz no lo enlaza.
- Detalle que costó una medida falsa: **sin `pd dsp 1` libpd no calcula nada** y
  devuelve ceros muy rápido. La primera pasada de la sonda dio un coste
  maravilloso y una salida de exactamente cero.

#### Añadido
- **`crates/choz-plugin-pd`**: lector de `.pd` (texto plano, **sin necesidad de
  Pd instalado** — es lo que permite listar patches y decir por qué uno no
  aparece), clasificación `Effect` / `InputAlgorithm` / `Unusable` según lo que
  el patch conecta, y el camino real con libpd tras la feature `pd`, apagada por
  defecto: un host que no compila sin Pure Data instalado es un host que nadie
  puede compilar.
- El parser separa por **puntos y coma sin escapar**, no por líneas: Pd parte
  las líneas largas y escapa los `;` de verdad como `\;`, así que un parser por
  líneas leería medio nombre de objeto. Con test.
- 4 tests, uno de ellos el camino completo con libpd: patch abierto, bloque
  procesado, `0.4 → 0.2`, y el segundo `open` rechazado porque hay una Pd por
  proceso.

### 2026-08-15 (ter) — Dónde se satura, dicho en pantalla

Queda algo de saturación después de arreglar el callback. Las dos formas de
saturar que quedaban no las decía nadie.

#### Añadido
- **Techo suave en el trim de entrada, y un `CLIP` que lo dice.** Un trim que
  llega a +24 dB llega más allá de fondo de escala, y una señal pasada de ahí
  es una onda cuadrada: satura lo que sale **y** le da al detector de tono una
  forma de onda cuyo periodo ya no es el que se tocó. El techo es `tanh` —
  degrada en compresión y no en una esquina — y se **cuenta**, porque un trim
  que limita en silencio es un trim mal puesto. El knob `IN` escribe `CLIP` en
  amarillo mientras ocurra.
- **`CLIP` en la línea del transporte** cuando la mezcla pasa de fondo de
  escala. Ahí clipa el dispositivo, y un clip duro es el peor sonido que puede
  hacer una mesa. choz no lo limita —es un nivel, y el mixer es donde se pone—
  pero deja de ser algo que hay que adivinar.

#### Notas
- Con esto el camino entero dice dónde se está pasando: el jack de entrada
  tiene su nivel en el cajón IN, el trim avisa si recorta, cada efecto tiene su
  medidor IN/OUT en la caja `SLOT`, y la salida avisa si clipa.
- 2 tests: el trim limitando y contándolo (y no contando cuando cabe), y la
  salida contando lo que pasa de fondo de escala.

### 2026-08-15 (bis) — El detector se comía el callback, y con él el grafo entero

Corrección de lo que dije ayer. Achaqué la degradación del audio del sistema al
quantum forzado, y el usuario señaló lo que no encajaba: **pasa sólo al activar
`A→M`**, y el quantum se fuerza al arrancar. Tenía razón, y el mecanismo real es
peor.

#### Corregido
- **El detector se llevaba entre el 27 % y el 41 % del plazo del callback**
  (medido, en debug; 15.7 % en release), *antes* del instrumento y de la cadena
  de efectos. Un callback que se pasa de su plazo no glitchea a choz: glitchea
  **el grafo**, y toda aplicación de la máquina se entrecorta con él. Eso es lo
  que se oía como "encender ftom estropea el sonido de YouTube", y era cierto.
  - **La causa**: el bucle de la función de diferencia leía la ventana a través
    del anillo — **dos `%` por muestra, unas 300 000 por análisis** — y ensanchaba
    a `f64` dos veces por iteración. Ambas cosas, dentro del hilo de audio.
  - **El arreglo**: la ventana se endereza una vez por análisis (1024 copias) y
    el bucle recorre dos slices planos, sin envolvente y vectorizable; la suma
    interna se acumula en `f32` y se ensancha una sola vez al final (`running`
    sigue en `f64`, que es la que crece de verdad).
  - Medido después: **1.6 % del callback** a 128 frames. Diez veces menos.
- Test permanente (`#[ignore]`, porque es un reloj) que falla si el análisis
  vuelve a pasar del 8 % del callback en release. Se corre a mano:
  `cargo test --release -p choz-engine --lib -- --ignored what_one_block`.

#### Notas
- El arreglo del quantum de ayer **sigue siendo correcto** y sigue haciendo
  falta —forzar el reloj del grafo no es de choz— pero no era esto.
- En **debug** el detector todavía cuesta un 20 % del callback a 128 frames.
  choz se ejecuta en release (`cargo run --release`, o el paquete instalado);
  en debug, con un plugin hosteado además, el presupuesto sigue siendo justo.

### 2026-08-15 — choz le estaba moviendo el reloj a toda la máquina

El reporte que lo destapó: **el sonido de YouTube también se oía saturado y sin
definición mientras choz usaba esa entrada**. YouTube no pasa por choz, así que
lo que fallaba no era el DSP.

#### Corregido
- **choz forzaba `node.force-quantum` y `node.force-rate` en el grafo entero**,
  en cada arranque, siempre que el búfer fuera ≥ 128 — es decir, siempre. Y eso
  no mueve el nodo de choz: mueve **PipeWire**. Todas las demás aplicaciones
  pasan a resamplearse a lo que choz pidió, y un navegador reproduciendo 44.1
  kHz por un headset que estaba contento a su ritmo sale fino y distorsionado.
  - Ahora se respeta el ajuste que ya existía: `PW quantum: system` (0, el
    valor por defecto) **pide** un periodo y no fuerza nada. Cualquier otro
    valor fuerza ese quantum, con el suelo de 128 frames intacto porque un
    quantum de 64 tira los endpoints de una interfaz USB.
  - El ajuste llevaba desde que se escribió mostrándose en pantalla,
    guardándose en `ui.json` y **sin aplicarse nunca**. Choz hacía justo lo
    contrario de lo que decía la fila.
- **La velocity se clavaba en 127.** Era `sqrt(rms) * 3`, que satura con
  cualquier cosa por encima de −19 dBFS; y en cuanto el trim está lo bastante
  alto para que el detector oiga un micrófono, **todas** las notas son 127. Un
  mda ePiano a 127 durante toda una toma es exactamente la saturación
  reportada. Ahora la escala va **del gate a fondo de escala**: lo más flojo que
  cuenta como nota es velocity 1 y 0 dBFS es 127, así que sigue a `SENS` y no
  necesita constante propia. Subir el trim ya no convierte cada nota en un
  fortissimo.

#### Añadido
- **La nota que `A→M` está traduciendo se ve en el visor MIDI IN**, pedido: se
  enciende en el teclado y en el ROLL como cualquier otra. Esas notas nacen en
  el callback de audio y nunca viajan como MIDI, así que ésta era la única
  vista donde no se podían ver — y un conversor que no se puede mirar sólo se
  puede creer o no. Se apaga sola al cambiar de nota o al callarse, porque
  nadie le manda un note-off.

#### Notas
- 3 tests: el quantum no forzado con el ajuste por defecto, la velocity leída
  desde el gate (flojo < normal < fuerte, y "normal" con margen por encima), y
  la nota convertida encendiendo su tecla y apagándose sola.
- **Lo que queda de tu reporte y no es de choz**: la estática y el ruido de
  fondo del H340 (incluidas las teclas) es lo que el micro capta con la
  ganancia del previo donde esté puesta. Con el `PW quantum` ya sin forzar,
  vuelve a probar: si YouTube suena bien otra vez, ése era el problema.

### 2026-08-14 (septendecies) — La sibilancia se plegaba encima de la nota, y el `A→M` gana su dry/wet

Reporte clave: **como multiefecto directo del micro suena perfecto**, y sólo
`A→M` falla — cuesta que entre en dB negativos, y cuando entra "convierte el
ruido en una frecuencia pero no en una nota".

#### Corregido
- **La diezmación era un filtro de caja.** Bajar de 48 kHz a 16 se hacía
  promediando 3 muestras, y un promedio de 3 muestras deja pasar casi todo lo
  que hay por encima de 8 kHz — que en una voz es **mucho**: sibilancia, aire,
  el siseo de la sala. Todo eso se plegaba (alias) justo encima de la banda
  donde vive la nota, así que el detector encontraba *un* periodo, pero no el
  que se había cantado. Ahora hay un paso-bajo de verdad a **3.5 kHz** (dos
  secciones, al rate de entrada) **antes** de promediar: por encima de la nota
  más aguda que reporta y muy por debajo del Nyquist del rate de trabajo, así
  que no quita nada de las notas y quita todo lo que se plegaría sobre ellas.
  - Medido: un tono de 220 Hz con un siseo de 9.5 kHz encima **nunca** se
    asentaba en la nota; ahora sí. Es el par simétrico del paso-alto contra el
    retumbe: uno limpia por debajo de las notas y el otro por encima.
- **El gate por defecto baja a −61 dBFS.** El nivel con el que se compara se
  mide **después** de esos dos filtros, y los filtros quitan energía real de una
  voz: lo que en el medidor de entrada marca −50 dBFS llega al detector más
  bajo. Un gate por encima de la señal se lee como "no hace nada", que es el
  peor de los dos fallos. `SENS` sigue yendo de −70 a −20 para subirlo.

#### Añadido
- **Dry/wet del `A→M`**, pedido: aparece **al encender el conversor** y no
  antes (apagado no hay nada que mezclar), empieza en 100 % —el instrumento
  solo, que es lo que hacía cuando no había elección— y el clic lo baja por
  cuartos y da la vuelta; la rueda lo empuja y se para en los topes. Con él a
  0 % se oye la entrada tal cual, y a la mitad la guitarra debajo del sinte que
  está tocando, que es el sonido que casi todo el mundo busca de un conversor.
  - `Slot.pitch_mix` en el motor con su comando, `RackSlot.pitch_mix` en la UI,
    guardado en el proyecto (`None` en un fichero viejo = todo instrumento).
  - Un segundo scratch (`RtState.dry`) sostiene la entrada mientras el
    instrumento escribe sobre el primero. Reservado una vez, como el resto.

#### Notas
- 4 tests nuevos: la sibilancia sin tapar la nota, la mezcla del conversor en
  sus tres puntos (todo instrumento, todo entrada, mitad y mitad), el control
  apareciendo sólo con el conversor encendido y su vuelta al dar la vuelta.
- Roadmap: dos secciones nuevas — **algoritmos de entrada como clasificación**
  (el arpegiador y `A→M` son el mismo tipo de cosa y hoy son dos casos
  especiales cableados en sitios distintos) y **Pure Data / Max MSP**, con las
  decisiones que hay que tomar antes de escribir nada: hostear libpd en vez de
  escribir un intérprete, la licencia LGPL contra el MIT de choz, el sandbox
  que ya existe como sitio para correrlo, y que de Max lo honesto es importar
  lo que se pueda y decir qué no.

### 2026-08-14 (sedecies) — El knob que cortaba el sonido, el micro que se colaba y un trim que no llegaba

Tres del mismo reporte con la UMC1820 delante.

#### Corregido
- **Mover un knob cortaba el sonido** en delay, space echo, Z5, Protocosmos y
  cualquier cosa con cola. Había **dos caminos** para mover un parámetro y sólo
  uno miraba `takes_live_params`: el del ratón y las flechas reconstruía la
  cadena entera en **cada paso**, y una cadena reconstruida son procesadores
  nuevos — el delay pierde sus ecos, el space echo su cinta, el granular sus
  granos. Ahora los dos caminos hacen lo mismo: el valor va al procesador vivo,
  y sólo se reconstruye lo que de verdad se construye una vez. Además se marca
  en vez de hacerse, así que arrastrar un knob por el panel es **una**
  reconstrucción al final del drenado y no una por paso.
- **El micrófono se colaba junto al instrumento con `A→M` encendido.** El búfer
  que se le pasa al instrumento todavía lleva la entrada, y `render` **no está
  obligado a sobrescribir** lo que le dan: un plugin sin notas que tocar puede
  sumar o puede no tocarlo. Se limpia antes de que el instrumento toque, que es
  justo lo que este modo existe para garantizar. Test que falla sin la línea.
- **El trim de entrada llegaba a +6 dB**, que para un micro dinámico es nada:
  había que cantarle a dos centímetros para que `A→M` oyera una nota. Ahora
  tiene su propio techo, **+24 dB** (`MAX_IN_GAIN`, separado del de la salida
  del slot, que es otra cosa), y el knob se lee **en dB** en vez de como un
  multiplicador — "×8.30" no es un número con el que nadie ajusta un micrófono.

#### Notas
- 2 tests nuevos: los cinco efectos que el usuario nombró sin reconstruir la
  cadena al mover un knob (y `FilterBank`, que sí la necesita, todavía
  pidiéndola), y el `A→M` sin filtrar la entrada hacia la mezcla.

### 2026-08-14 (quindecies) — `A→M` estable: el retumbe fuera y un suelo de duración

Pedido con la UMC1820 delante: que la nota se estabilice lo máximo posible, que
el MIDI se mueva poco, sin microtonalidad, y una sola voz de polifonía.

#### Corregido
- **El retumbe de la sala volvía sordo al detector.** Un micro en una habitación
  siempre trae algo por debajo de la nota más grave —una mesa, un ventilador, un
  previo, pisadas— y a un detector de periodo eso le da un periodo. Medido: un
  tono de 220 Hz con un retumbe de 41 Hz **5 dB más fuerte** hacía que el
  tracker no reportara **nada en absoluto**, porque la mezcla no es periódica en
  ninguno de los dos periodos. Ahora la entrada se pasa por un paso-alto a
  60 Hz, justo bajo `MIN_NOTE`, de **24 dB/octava** (dos secciones): el retumbe
  suele ser más fuerte que la nota, y una pendiente suave lo deja más fuerte
  todavía. La diezmación que ya había es el otro extremo del filtro.
  - `Biquad::highpass` en `fx/utility.rs`, al lado del `lowpass` que ya estaba.

#### Añadido
- **`MIN_NOTE_ANALYSES`: un suelo de ~130 ms antes de que nada pueda sustituir
  a la nota que suena.** Los controles que había deciden *si una lectura es una
  nota*; éste decide **cada cuánto se permite que la salida cambie**, que es lo
  que se oye: un sinte re-disparado cada 30 ms es un zumbido fueran cuales
  fueran las notas. Medido con dos notas alternándose cada 85 ms: **24
  note-ons sin el suelo, 14 con él**.
  - Cuesta lo que dice: una escala más rápida que eso sale con menos notas. Es
    el trato de un conversor monofónico —una voz, sostenida— y por eso es una
    constante y no un knob: quien quiera cada semitono de una escala rápida
    quiere un teclado.

#### Notas
- **Sin microtonalidad, y estructuralmente**: `PitchEvent` no tiene ninguna
  variante que lleve una fracción, así que no hay por dónde mandar pitch bend.
  Un cuarto de tono sale como **una** nota entera y la fracción se queda en la
  pantalla. Test nuevo de punta a punta.
- **Una sola voz**, también con test: sobre un arpegio, un glissando, un
  silencio y una vuelta, en ningún momento hay dos notas sonando a la vez.
- Dos cambios que **no** se quedaron porque no se ganaron su sitio: sustituir
  "N análisis seguidos" por una votación por mayoría, y un guardián de saltos de
  octava. Los dos pasaban sus tests igual con la regla vieja —las lecturas malas
  ya las rechaza el umbral de claridad antes de llegar ahí—, así que se
  revirtieron en vez de quedarse como complejidad sin efecto medible.

### 2026-08-14 (quaterdecies) — Una mezcla que no llega a ningún sitio lo dice

Siguiendo el mismo reporte: el nombre del dispositivo de salida se guarda, y un
**nombre sobrevive a la caja que lo llevaba**. Con la UMC1820 apagada, choz
arrancaba pidiendo un sink que ya no tiene puertos, `connect()` se rendía con un
`eprintln!` a un fichero de log que nadie abre, y la mezcla se calculaba entera
para no ir a ninguna parte. Desde fuera: el WAVE se mueve, los medidores de FX
marcan, y no se oye nada — indistinguible de un efecto roto.

#### Añadido
- **Caída a un sink que existe**: si el guardado no tiene puertos de playback,
  `connect()` busca otro en el grafo (prefiriendo un `alsa_output` real sobre un
  loopback) y dice cuál usó en su lugar. Devuelve además **a qué sink acabó
  yendo y cuántos de nuestros puertos llegaron**, así que `output_device` deja de
  ser lo que se pidió y pasa a ser lo que se consiguió.
- **`AudioEngine::output_wired()`** y, con él, `NOT CONNECTED` en amarillo en la
  línea `OUT` del panel TRANSPORT. Sin motor abierto también avisa: un stream
  que nunca abrió es la versión más ruidosa de "no sale nada".
- Los fallos de conexión de salida ya no se descartan uno a uno: cada
  `connect_ports_by_name` que falla se nombra.

#### Notas
- Es el mismo bug de forma que el de la captura de ayer: **un fallo que sólo va
  al log es un fallo invisible**. Los dos sitios donde choz se cablea al grafo
  lo tenían, y los dos hablan ahora.
- 1 test: la línea del transporte avisando cuando nada está cableado.

### 2026-08-14 (terdecies) — El nivel de cada jack, `mtof` fuera, `TAP` en su sitio

Tres cosas del reporte de hardware: el audio entrante sigue sin oírse, y las
otras dos son decisiones del usuario.

#### Añadido
- **`meter::capture_levels`: el pico de cada jack de entrada**, publicado en el
  callback antes de que ningún slot decida nada, y escrito en la fila de ese
  jack en el cajón IN (`3  capture_1   -12dB`, o `--` cuando no llega nada).
  Es la lectura que separa las tres formas en que el audio en vivo desaparece —
  **no llega** (la conexión no se hizo o el dispositivo no está abierto), **llega
  y no se rutea**, y **se rutea y el problema es el efecto**— que sin ella se
  ven exactamente igual. Dos rondas de este reporte se han ido en distinguirlas
  a ciegas.
- **Las conexiones de captura de JACK ya no se hacen con el error descartado.**
  Eran un `let _ = connect_ports_by_name(...)`: si PipeWire se negaba, choz se
  quedaba con puertos de entrada cableados a nada y **nadie se enteraba** —
  idéntico a un efecto roto. Ahora dice cuáles fallaron y por qué.

#### Eliminado
- **`mtof` entero** (`crates/choz-ui/src/mtof.rs`, el botón `M→P`, `AMT`, los
  campos del slot y del proyecto, el modo de puntero y sus siete tests).
  Decisión del usuario: choz lleva **`ftom` y nada más** — audio a notas, para
  meter una guitarra o un micro en un plugin como Surge XT sin una cápsula MIDI
  tipo GI-20. La dirección contraria (una nota moviendo un parámetro) no es lo
  que este programa hace.

#### Cambiado
- **`TAP` se dibuja sobre el borde superior de la caja del arpegiador**, a la
  derecha, en vez de flotar en la fila de encima. Es un gesto del arpegiador y
  ahora está donde vive; en las formas que *son* una fila de botones sigue en la
  fila, porque allí no hay caja sobre la que ponerlo.

#### Notas
- El timing del arpegiador queda **aplazado por decisión explícita**, con el
  coste ya medido escrito en el roadmap.

### 2026-08-14 (duodecies) — La deriva de la captura, en pantalla

Lo añadido ayer abrió una pregunta que sólo el hardware podía contestar ("¿cuánto
deriva en mi máquina?") y ninguna forma de contestarla salvo tocar una hora y
ver. Ahora es un número.

#### Añadido
- **`meter::capture_health`**: dos contadores relajados que `drain_capture`
  mueve sólo cuando algo va mal — **late** (un bloque en el que la entrada no
  había producido bastante, rellenado con silencio) y **dropped** (muestras
  tiradas porque la entrada iba por delante y el backlog se estaba volviendo
  latencia). Se ponen a cero al reabrir el stream, así que los números son del
  dispositivo que está abierto ahora.
- **El cajón IN los escribe** en su cabecera —`AUDIO IN (2) · 3 late, 512
  dropped`— y **calla mientras se porta bien**: un contador a cero es ruido en
  un panel de este ancho. Es la diferencia entre "mi micro chasquea a veces" y
  algo que se puede señalar y medir.
- Sólo se mueven en los backends cpal: el cliente JACK nativo entrega captura y
  reproducción al mismo callback, así que no hay contra qué derivar.

#### Notas
- 2 tests: el anillo contando sus dos derivas (un bloque corto = un `late`, y el
  backlog recortado contado en muestras), y la cabecera del cajón callando en
  reposo y hablando en cuanto los contadores se mueven.

### 2026-08-14 (undecies) — choz como multiefecto sin JACK: entrada de audio elegible

Reportado desde el hardware: micrófono de un headset H340 seleccionado, el visor
WAVE mostrando señal, y los efectos sin hacer nada. Diagnóstico: **fuera del
cliente JACK nativo no existía ningún camino de captura**. `build_input_stream`
no aparecía en el código; `rescan_inputs` respondía literalmente "audio input
needs the native JACK client"; y `all_capture_ports()` abre su *propio* cliente
de sondeo, así que el cajón IN ofrecía micrófonos que el backend en marcha no
podía entregar.

#### Añadido
- **Captura por cpal** (ALSA, PulseAudio, PipeWire). Un stream de entrada propio
  y un anillo sin locks (`rtrb`) hasta el callback de salida: JACK entrega
  reproducción y captura al **mismo** callback, cpal les da callbacks distintos
  con relojes distintos, y entre dos hilos de audio lo único que puede pasar es
  un anillo.
- **`RtState::drain_capture`**, que responde por las dos derivas: **corto** (la
  entrada aún no ha producido) rellena con silencio en vez de repetir audio
  viejo; **largo** (la entrada va por delante) tira todo lo que pase de dos
  bloques de reserva, porque un anillo al que se le deja llenarse es latencia
  que crece toda la noche y no vuelve.
- **Fila `Input` en `EDIT → Settings → AUDIO → Engine`**, debajo de `Device`:
  cicla `(off)` y los dispositivos de captura del sistema, **aplica al momento**
  igual que la salida, y se guarda en `ui.json` (`audio.input_device`). Empieza
  apagada y un `ui.json` viejo abre apagado: un host que se queda con el
  micrófono al arrancar es un host que nadie pidió.
- **El rate no se negocia**: la captura se abre al rate al que ya corre el motor
  o no se abre, y el error dice que se cambie en Settings. Un micrófono que sale
  un tono más agudo es peor que uno que explica por qué no abre.
- **El cajón IN dice dónde encenderla** cuando no hay entrada:
  `AUDIO IN (0) — EDIT > Settings > AUDIO > Input`. "Aquí no hay nada" y "esto
  no funciona" se ven igual, y esa confusión es exactamente la de este reporte.
- `rescan_inputs` ya no falla fuera de JACK: reabre el dispositivo, así que un
  headset enchufado después de arrancar aparece.

#### Notas
- El camino motor→FX **ya era correcto** y ahora hay dos tests que lo fijan: un
  efecto sobre una tab alimentada por captura procesa la entrada, y lo hace
  **con el transporte parado** — un multiefecto no es un secuenciador, un
  micrófono no espera al botón de play. Ninguno de los dos existía; por eso el
  fallo pudo llegar hasta el hardware.
- 3 tests nuevos: los dos de arriba más el del anillo en sus dos derivas
  (bloque exacto, anillo vacío = silencio, y 100 bloques encolados recortados a
  la reserva quedándose con lo **reciente**, no con lo viejo).
- Cambiar de dispositivo de entrada reconstruye el stream, y un stream nuevo no
  tiene slots: pasa por `rebuild_rack()`, el mismo camino que el cambio de
  salida. Se extrajo `AudioEngine::restart_cpal()` para que las dos puntas del
  audio compartan esa operación en vez de tener cada una su copia.

### 2026-08-14 (decies) — Un knob que se desliga vuelve a su sitio, y 563 líneas menos

#### Añadido
- **`mtof` devuelve el parámetro al desligarlo.** Ligar apunta las notas a un
  knob *y se acuerda de dónde estaba*; desligar lo pone de vuelta. Mientras
  estuvo ligado el valor era de la nota, no del usuario, así que dejarlo donde
  cayó la última es dejar un knob en un sitio que no eligió nadie. Re-apuntar a
  otro knob devuelve el primero y **lee el "antes" del segundo después de la
  devolución**, para no grabar como suyo el valor que dejó una nota.
  - `RackSlot.mtof_prev` **no se guarda en el proyecto**: es el valor del knob
    de *esta* sesión, y un número de otra se restauraría sobre el plugin que
    hoy esté en ese slot.
  - Lo que no se puede leer no se puede devolver: `target_value` sólo ve la tab
    activa y su FX seleccionado, así que ligar en una unidad y desligar desde
    otra deja el parámetro donde lo dejaron las notas. Es el mismo límite que
    tienen las automatizaciones, y por el mismo motivo.

#### Eliminado
- **`registry.rs`, `scanner.rs` y `plugin_types.rs`** — 563 líneas de infra
  vieja, casi toda `#[allow(dead_code)]`, a la que sólo llegaba un campo
  (`App.registry`) que nadie leía nunca. El roadmap llevaba desde julio con
  "decidir si se borra"; decidido. El camino real son los crates
  `choz-plugin-*` más `paths.rs`, y ahora es el único.

#### Notas
- **El timing del arpegiador se queda pendiente a propósito**, y el roadmap
  lleva ahora por qué: programar las notas con sello de tiempo deja el error en
  un bloque (1–3 ms) en vez de en cero, y para que sea exacto hay que partir el
  render por slot en los puntos donde caen las notas — o sea llamar a
  `source.render` varias veces por bloque con trozos pequeños, que para un
  plugin hosteado es otra cosa. Más sacar la generación de notas de donde vive
  el ruteo, y `MIDI OUT`, que no se puede programar por adelantado. 5 ms de
  retraso contra tocar todo eso: no sale a cuenta hoy.
- 2 tests nuevos de `mtof` (el knob volviendo al desligar, y re-apuntar
  devolviendo el primero sin heredar su valor).

### 2026-08-14 (nonies) — Analizador de espectro (fase 6, y con ella el punto 2 entero)

#### Añadido
- **Anillo sin diezmar en el meter** (`meter::SPECTRUM_POINTS`, 2048 muestras):
  el anillo `wave` que ya existía guarda **un punto por rebanada de bloque**,
  que es un dibujo de la envolvente y nada sobre lo que se pueda correr una
  FFT. El nuevo guarda cada frame — un store relajado por frame, que es lo que
  cuesta un bloque de 256 — y a 48 kHz son 43 ms de sonido y 23 Hz por bin.
- **`choz-ui/src/spectrum.rs`**: ventana de Hann, FFT radix-2 iterativa escrita
  a mano (40 líneas; una dependencia cuyo planificador SIMD no vale nada a este
  tamaño), magnitudes en dB con suelo a −78, y **peak hold por bin** que baja
  1.4 dB por redibujo. Por bin y no por columna: redimensionar el panel no tira
  los picos. Corre **en el hilo de UI y sólo con su pestaña en pantalla** — una
  FFT que nadie mira es una FFT que nadie debe pagar.
- **Pestaña `SPEC`** en el panel MIDI IN (`MIDI │ KEYS │ ROLL │ WAVE │ SPEC │
  ACTIVITY`): media-cuadros, así que un panel de ocho filas lleva dieciséis
  escalones; escala **logarítmica** de 20 Hz a 20 kHz, el pico sostenido
  dibujado **encima** de la barra y en otro color, y una fila de eje con
  `100 / 1k / 10k` — sin eso una escala logarítmica es un dibujo de nada en
  particular. Cada columna se queda con **el bin más alto** que cubre, no con
  la media: en el agudo una columna cubre cientos de bins, y promediar esconde
  justo el pico que el analizador existe para enseñar.
- La escala está calibrada: un seno a fondo de escala lee 0 dB y la mitad de
  amplitud lee 6 dB menos, que es la única forma de saber que es una escala y
  no una forma.

#### Notas
- 10 tests: el tono en su bin y a su nivel, los −6 dB al partir la amplitud por
  dos, el silencio en el suelo con el peak hold todavía bajando, el peak hold
  llegando al suelo, las octavas del mismo ancho, el tono en su columna, el
  ancho cero, la transformada contra un impulso y contra DC, el dibujo con el
  tono en la columna que le toca y el silencio vacío, y el panel sin sitio.
- Con esto la **fase 6 y el punto 2 del roadmap quedan cerrados**. Las reglas
  de DSP y el patrón de tests por efecto se mudan del roadmap a
  `docs/architecture.md`, que es donde vive lo que sigue siendo ley.

### 2026-08-14 (octies) — Frecuencia y espacio (fase 5 del punto 2)

#### Añadido
- **`FreqShifter` y `RingMod`**, un procesador con dos usos de la misma
  portadora. Multiplicar por un seno da **las dos** bandas laterales — eso es el
  ring mod, y es una línea. Quedarse con una sola necesita la señal analítica:
  el par de cadenas all-pass (Hilbert polifásico) que mantienen 90° de
  diferencia en toda la banda, y entonces `out = re·cos(θ) + im·sin(θ)`.
  **No es un pitch shifter**: mueve cada parcial los mismos Hz, así que un
  sonido armónico deja de serlo — que es justo para lo que se usa.
- **`ShimmerReverb`**: pre-delay → reverb → pitch shift **dentro del lazo** →
  paso-bajo de amortiguación → realimentación. Dentro, no después: uno detrás
  daría una sola copia transpuesta y ningún ascenso. Reutiliza el `Reverb`
  Freeverb que ya existía; el shifter es de dos cabezas cruzadas con `sin²/cos²`
  (que suman exactamente uno, así que el salto de cada cabeza cae donde esa
  cabeza está callada). Desplazamiento **cuantizado a semitonos** de −12 a +24:
  un shimmer a 11.6 semitonos está desafinado con lo que sea que suene.
- Medido: 43 dB de rechazo de la banda lateral no deseada en el shifter, y el
  segundo octavado del shimmer se ve en la cola a los dos segundos.

#### Corregido / decidido
- **La cadena que lleva el retardo de una muestra del par de Hilbert era la
  equivocada.** Con el retardo en la cadena B el rechazo era de 16 dB —
  audible como "el shifter produce las dos bandas". Con él en la A, 43 dB.
  Medido con un Goertzel, que es la única forma honesta de elegir el convenio
  de signos de un par de Hilbert.
- **El shimmer realimentaba un escalar por chunk**, así que el resultado
  dependía del tamaño de bloque (0.33 de diferencia entre 512 y 97 muestras).
  Ahora el reverb se procesa **frame a frame** y el lazo tiene exactamente una
  muestra de retardo, venga el bloque que venga. Se fue el búfer de scratch.
- **El shifter leía hacia atrás**: `read` se usaba como distancia y a la vez se
  incrementaba, así que la cabeza retrocedía. Ahora la distancia a la cabeza de
  escritura se **cierra** a `ratio − 1` por muestra y envuelve en la ventana.
- **El lazo no se estabiliza con una constante.** El reverb de dentro es un
  banco de peines *resonantes*: su ganancia en sus resonancias está muy por
  encima del 1.17 que enseña con ruido de banda ancha, y se mueve con el tamaño
  de sala. Medido, a 0.26 de realimentación todavía crecía sin límite a los
  diez segundos. La solución es estructural — un **saturador en el lazo**
  (`tanh` escalado, techo 1/3) —, no un número: con poca realimentación se
  apaga, y al máximo se asienta en un wash en vez de explotar.
- **La "reverb de sala barata" no se escribe**: el Freeverb que ya existe *es*
  una reverb de sala con `Room` bajo, y su preset de 0.30 ya está ahí. Una
  segunda sería el mismo algoritmo con otro nombre.

#### Notas
- 11 tests nuevos: el tono desplazado 200 Hz aterrizando en 1200 y no en 800,
  el desplazamiento negativo, el shift a cero sin mover el nivel, el ring mod
  dando las dos bandas iguales, el shifter subiendo la octava por su cuenta, la
  cola del shimmer trepando, sin realimentación no trepando, el lazo apagándose
  con poca y acotado a tope, el tamaño de bloque irrelevante, y los extremos.

### 2026-08-14 (septies) — Modulación y tiempo (fase 4 del punto 2)

Cuatro efectos nuevos y un delay que ya no lee en enteros. Todo cuelga de un
LFO único, porque siete formas copiadas en cuatro ficheros son siete formas que
acaban siendo distintas.

#### Añadido
- **`fx/lfo.rs`** — el LFO que usan todos: siete formas (seno, triángulo,
  sierra, rampa, cuadrada, S&H y random suave), **desfase estéreo** y estado
  aleatorio **por canal**, redibujado cuando envuelve *su* fase — un offset que
  escalona los dos canales a la vez no es un offset. xorshift de semilla fija:
  la misma sesión da el mismo temblor, que es lo único que hace testeable una
  modulación aleatoria.
- **`Tremolo` y `AutoPan`**: un procesador, dos efectos (como `Saturator` y
  `WaveShaper`). El trémolo modula **hacia abajo desde la unidad**, así que
  encenderlo nunca sube el nivel de la tab; el auto-pan es de potencia
  constante, así que barrer la imagen no hace un bache en el centro. El `Pan`
  estático se queda como está: "esta tab un poco a la izquierda" no es una
  modulación.
- **`AutoFilter`**: SVF de Simper con **coeficientes por canal** (dos cutoffs
  distintos es lo que significa un desfase estéreo), LFO con las siete formas,
  seguidor de envolvente con cantidad **con signo** (negativo = se cierra al
  tocar más fuerte) y modos LP/BP/HP. La envolvente escucha **la entrada, no la
  salida**: si oyera su propio filtrado, un filtro que se cierra se cerraría
  más y el efecto se comería a sí mismo.
- **`BeatRepeat`** sincronizado al transporte de choz. Cada `Interval` negras
  tira un dado contra `Chance`; si gana, **captura pasando la señal** (un beat
  repeat que se calla mientras escucha es un agujero en el compás) y luego
  repite el grano con `Decay` por vuelta. **Transporte parado = pasa de largo**:
  la posición no avanza, así que no hay rejilla, e inventarla lo pondría donde
  el resto del rack no está.
- **Delay modulado, con interpolación**: lectura fraccionaria (`read_frac`) y
  knobs `ModRate`/`ModDepth`. Las dos cabezas van a media fase, así que las
  repeticiones se ensanchan en vez de sólo desafinarse. El ping-pong ya estaba.

#### Corregido
- **El knob `Wet` de un built-in se perdía en cada rebuild.** `is_mix_param`
  sólo decía "sí" para plugins, así que la mezcla de un efecto propio vivía en
  `params` y no en `entry.wet` — que es lo que la cadena vuelve a aplicar.
  Bajabas el Wet de un delay, añadías otro efecto, y el delay volvía a estar al
  100 %. Ahora un desc llamado `Wet` **es** el dry/wet, venga de un plugin o no.
- **`BeatRepeat` cruzaba de compás con un click de 0.375** cuando un repeat
  daba paso a una captura nueva: el fundido de salida sólo se armaba al volver
  al silencio. Salir de un repeat siempre lo arma, vaya a donde vaya.
- **La rejilla derivaba en `f32`**: sumar 4e-5 a una posición que crece, 48 000
  veces, movía el límite de compás 30 muestras antes de tiempo. La posición se
  cuenta ahora en `f64` desde el principio del bloque, no acumulando.

#### Notas
- Tremolo, auto-pan, auto-filter y beat repeat toman valores **en vivo**
  (`takes_live_params`), así que mover un knob no reconstruye la cadena — que
  en el beat repeat significaría tirar el grano capturado.
- Los coeficientes del auto-filter se recalculan **cada 16 frames**, no cada
  muestra: a 20 Hz de LFO eso es un tercio de milisegundo de retraso, por la
  dieciseisava parte de las `tan()`.
- 22 tests nuevos (5 del LFO, 6 del trémolo/auto-pan, 5 del auto-filter,
  6 del beat repeat) + 2 del delay modulado.

### 2026-08-14 (sexies) — `WaveShaper`: la curva se dibuja (fase 3 del punto 2)

El roadmap decía "el DSP es el mismo waveshaper con una tabla en vez de una
fórmula". Lo es, literalmente: **no hay procesador nuevo**.

#### Añadido
- **`AudioFxKind::WaveShaper`** — el mismo `Saturator`, construido por
  `Saturator::waveshaper()`. El oversampler, el bloqueo de DC, el filtro de tono
  y el medidor son lo que hace usable a un waveshaper y ya estaban ahí; lo único
  que cambia es de dónde sale la curva. `Shape::{Curve, Table}` sustituye al
  campo `curve`, y `name()` responde `WaveShaper` o `Saturator` según cuál sea.
- **`saturator::Table`**: ocho puntos (`TABLE_POINTS`) repartidos sobre −1…+1,
  rectas entre ellos y **plano fuera** — no extrapolado, porque una recta que
  sigue subiendo es un waveshaper que no puede prometer salida acotada, y todas
  las demás curvas de este fichero sí pueden. Por defecto es la identidad: meter
  el efecto y no tocar nada no cambia nada.
- **La curva se edita dibujándola**: los ocho puntos se pintan con
  `draw_eq_bank` — la misma función que dibuja el EQ gráfico, que ya trae sus
  rects de clic y su cursor. **No hay editor de curvas que escribir**: el dibujo
  *es* el editor. Las etiquetas de las columnas son el nivel de entrada
  (`-1.0 … +1.0`), no `P1…P8`, porque "P3" no dice dónde cae el punto.
- Oversampling por defecto **4x**: una tabla con esquinas hace mucho más
  aliasing que un `tanh`, y la esquina es justo lo que se ha dibujado a mano.

#### Corregido
- El test `every_preset_names_knobs_that_exist` tenía **su propia copia** de la
  lista de efectos; ahora usa `ALL_FX_KINDS`. La copia se habría quedado atrás
  justo el día en que se añade un efecto, que es el día en que el test sirve.

#### Notas
- 3 tests de DSP (la identidad es un cable, cualquier curva dibujada acotada y
  finita —incluida la de dientes de sierra entre raíles a 8x—, y una curva
  invertida invierte la señal) + 1 de UI (los ocho puntos con su rect, la
  identidad subiendo de izquierda a derecha y el eje de entrada escrito).
- Una identidad **no** sale idéntica muestra a muestra: el bloqueo de DC a 10 Hz
  y el tono a 18 kHz dejan un par de grados de fase a 220 Hz. Lo que el test
  exige es que el nivel no se mueva.

### 2026-08-14 (quinquies) — Dinámica y EQ de verdad (fase 2 del punto 2)

Los cuatro procesadores de dinámica y EQ que ya existían tenían la topología
correcta y los controles de una demo. Esto les pone lo que decide si sirven.

#### Añadido
- **Detector del compresor**: `Detect::{Peak, Rms, RmsFast}` — ventana de
  30 ms y de 3 ms, calculada como un polo sobre `x²`. Es la diferencia entre
  tratar una caja como un pico o como parte del nivel, y se elige por su nombre
  (`ParamShape::Named` sacada del enum del DSP, no una etiqueta a mano).
- **Stereo link** (0..1) en compresor y limitador: con 1 los dos canales siguen
  al más alto, con 0 cada uno se comprime solo. Hay **dos envolventes y dos
  suavizadores de ganancia**, no uno; el link mezcla el detector, que es donde
  se decide si un canal duro arrastra al otro y mueve la imagen.
- **Paso-alto de sidechain** (20–500 Hz) sobre el detector, no sobre el audio:
  un bombo deja de mandar en la ganancia sin que el bombo se vaya de la mezcla.
  A 20 Hz está apagado de verdad, porque ahí no hay nada que quitar.
- **Lookahead real en el limitador**: anillo entrelazado dimensionado una vez
  (10 ms a 192 kHz), el detector mira la muestra que llega y la ganancia se
  aplica a la que ya pasó, así que el ataque llega con la ganancia abajo. Se
  reporta por `FxProcessor::latency_samples`, o sea que la caja `SLOT` ya lo
  escribe en ms. Knob `Look` de 0 a 10 ms.
- **Histéresis y máquina de estados en el gate**: `GateState::{Closed, Attack,
  Open, Hold, Release}` sustituye a `is_open` + contador (dos cosas que podían
  contradecirse). Abre en `Thresh` y cierra en `Thresh − Hyst`: con un solo
  umbral, una señal parada encima castañetea varias veces por segundo.
- **EQ paramétrico apuntable**: `EqMode::{Stereo, L only, R only, Mid, Side}`.
  Un juego de bandas con destino, no dos curvas que editar — cubre "quita la
  sibilancia sólo de los lados" y "la suciedad está en la izquierda" por el
  precio de un knob. El componente que no se procesa sale **byte por byte** como
  entró.
- **Solo por banda** (`Solo`: off + las cuatro bandas): un paso-banda a la
  frecuencia y Q de la banda elegida, con su biquad reservado en el constructor
  — escuchar una banda no reserva memoria.
- **Curva de respuesta dibujada** bajo los knobs del EQ (6 filas, 20 Hz–20 kHz
  en log, ±18 dB), en verde lo que sube y en rojo lo que baja, con marcas en
  100/1k/10k. Sale de `ParametricEq::response_db`, que calcula |H(e^jw)| **de
  los mismos coeficientes que procesan el audio**: un test comprueba contra un
  seno que lo dibujado y lo medido no se separan más de 0.5 dB. Si no quedan
  filas, no se dibuja y no consume ninguna.

#### Corregido
- **El knob `HiMid` del EQ paramétrico no movía nada**: el constructor escribía
  `bands[3].gain_db` dos veces y la segunda (`High`) pisaba la primera. Y
  `bands[0]` era un paso-alto fijo a 80 Hz que ningún knob alcanzaba y por el
  que pasaba todo. Ahora son **cuatro knobs y cuatro bandas**: shelf grave,
  peak a 250, peak a 2k y shelf agudo, con `MidQ` sobre los dos peaks. El
  preset "Presence", que movía el knob muerto, ahora hace lo que dice.
- El mapeo de parámetros del EQ vive en `ParametricEq::from_params`, y lo
  llaman tanto la cadena como el dibujo de la curva: dos copias serían una
  curva que miente el día que se toque una de ellas.

#### Notas
- El orden de los knobs de compresor, limitador y gate **es** el orden de
  `set_param` del procesador, porque los tres toman valores en vivo
  (`takes_live_params`) y el índice del knob es el índice que se escribe.
- 10 tests nuevos: lookahead contra el primer pico, RMS dejando pasar una
  espiga de 1 ms que el pico sí caza, el link decidiendo si un canal agacha al
  otro, el sidechain sordo a 40 Hz, la histéresis sosteniendo una señal parada
  entre los dos umbrales, el hold contando, mid/side dejando intacto lo que no
  procesa, el solo dejando sólo su banda, la curva contra un seno, y la curva
  rindiéndose cuando no hay filas.

### 2026-08-14 (quater) — Una tab puede tocar hacia fuera (MIDI OUT)

Punto 1.1 del roadmap: el destino que faltaba de la cadena `fuente → ARP →
destino`. El arpegiador ya no termina forzosamente en el plugin de su propia tab.

#### Añadido
- **`midi::MidiOut`**: puerto de salida abierto por nombre, con la lista de lo
  que tiene sonando. Corta **nota por nota**, no con CC 123: un sintetizador que
  ignora "all notes off" queda zumbando hasta que se apaga, y la lista de lo que
  está pisado está justo ahí. Se comparte por nombre (`App.midi_outs`) porque
  ALSA le da un puerto a un cliente y dos tabs apuntando al mismo serían dos
  conexiones que no se pueden abrir.
- **`RackSlot.midi_out: Option<String>`** — el **nombre**, no un índice: los
  puertos van y vienen, y un índice a una lista que cambió con choz cerrado
  apunta al sintetizador de otro. Se guarda en el proyecto.
- **Sección `MIDI OUT` en el cajón OUT**, con la misma gramática que los canales:
  Enter/clic liga, otra vez desliga, y la fila dice qué tabs ya la usan
  (`← tab 1,2`).
- **Un solo embudo**: `App::send_note` es por donde pasan las teclas, lo que
  toca el arpegiador y los note-offs del `PANIC`. Un segundo destino que unos
  caminos conocen y otros no es un sintetizador zumbando por el camino que se
  olvidó.

#### Notas
- `PANIC` alcanza los puertos abiertos, que es lo mínimo que se le pide al botón
  cuando hay un cable de por medio.
- 395 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-14 (ter) — Reloj MIDI externo

Lo último que le faltaba al arpegiador para tocar con otra máquina: esclavizarse
a un reloj de fuera.

#### Añadido
- **`ClockCounter` en `midi.rs`**, dentro del callback del puerto — que es el
  último sitio donde el sello de tiempo es honesto; un pulso leído desde el
  bucle de UI trae la jitter de ese bucle. Cuenta los 0xF8 y **promedia sobre
  una negra entera** (24 pulsos): un solo intervalo carga con toda la jitter del
  cable y del emisor. El pulso que cierra una negra **abre la siguiente**, o
  cada medición perdería un tiempo.
- **`InputEvent::Clock(ClockMsg)`** con `Start`, `Continue`, `Stop` y
  `Tempo(bpm)` — un mensaje por negra, no veinticuatro. Sin puerto de origen:
  hay un reloj, y dos puertos mandándolo es un problema de cableado, no algo que
  promediar.
- **`START` rebobina y arranca, `CONTINUE` arranca donde quedó, `STOP` para** —
  esa es la diferencia entre los dos primeros, y es la que hace útil el
  `CONTINUE`. El tempo se escribe directo al transporte: el emisor **es** el
  reloj, y suavizarlo aquí pondría a choz un tiempo por detrás de lo que está
  acompañando.
- **Interruptor `CLK INT ○ / CLK EXT ●`** en el panel TRANSPORT, guardado en
  `ui.json`. Es un interruptor a propósito: un puerto que manda reloj todo el
  día se quedaría con el tempo en cuanto se enchufa, y eso no es algo que
  descubrir a mitad de un tema.
- El monitor MIDI muestra las líneas de `CLOCK`, así que se ve llegar el reloj
  antes de encender nada.

#### Notas
- Con esto **el arpegiador no tiene pendientes** en el roadmap.
- 394 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-14 (bis) — Fuera el secuenciador; el arpegiador se elige con listas

Dos decisiones del usuario en la misma sesión: **las opciones con nombre se
eligen de una lista**, alcanzable con teclado, y **el secuenciador se va** — la
tab lleva un arpegiador y nada más.

#### Añadido
- **`ModalKind::ArpChoice`**: los knobs con nombres (`MODE`, `DIV`, `OCT`)
  abren su lista con **Enter** desde el teclado y con un **segundo clic** sobre
  el que ya tiene el cursor — la misma regla que siguen los knobs de los FX. Los
  interruptores no abren nada: dos posiciones no son un menú, Enter los cambia.
  En la forma compacta, los botones con nombres **abren la lista** en vez de
  recorrerla: pulsar ocho veces para llegar a `RANDOM` no es una forma de
  elegir.
- **El teclado llega al arpegiador aunque no haya caja de knobs**: `k` lo
  alcanza siempre que esté encendido, y en la fila de botones **se marca el
  control que tiene las flechas**. Antes, en una pantalla corta, la fila era
  sólo para el ratón.
- **Botón `TAP`**, siempre visible con el arpegiador encendido, con el tempo al
  lado (`TAP 120`). Nunca es un knob: un tap es un gesto, y un gesto no tiene
  posición a la que girar. Con `SYNC` puesto **mueve el transporte**, que es el
  reloj que se está contando.

#### Quitado
- **El secuenciador entero**: modo `SEQ`, `REC`, `REST`, `CLR`, `TIE`, el
  transporte `▶ ‖ ■`, la tira de pasos y su edición, las ocho secuencias, la
  longitud de patrón y la transposición tocando. Con ellos se van
  `SeqStep`/`PlayMode`/`Transport`, los campos `arp_*` del proyecto (un archivo
  viejo sigue cargando: serde ignora lo que no conoce) y ~40 % de `arp.rs`.
- Las tres acciones de MIDI learn del transporte (`▶/‖`, `■`, `REC`) **se
  conservan como variantes muertas** de `TriggerAction`: una variante
  desconocida es un error de parseo, y un error de parseo es un rack perdido.
  No las ofrece el picker y no hacen nada.

#### Notas
- 391 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-14 — Modo acorde y longitud de patrón

Tercera pasada del clon de KeyStep. De la lista original queda una sola cosa: el
reloj MIDI externo.

#### Añadido
- **Modo acorde** (`CHORD`). Encenderlo **con un acorde pisado lo memoriza**;
  encenderlo con las manos quietas conserva el que hubiera — usar el que ya
  tienes es el caso normal, y borrarlo haría del interruptor un gesto
  destructivo. Después, una tecla toca esa forma **desde donde se toque**.
  - Las notas que la tecla trae entran en `held` como cualquier otra, así que
    los ocho modos, las octavas y el latch siguen funcionando sobre ellas sin
    tocar nada.
  - Soltar la tecla **se lleva lo que trajo**: esas notas nunca se pulsaron, y
    nadie más mandaría sus note-offs.
  - El botón dice cuántas notas hay memorizadas (`CHORD ●3`): un modo acorde sin
    acorde se ve igual que uno que funciona hasta que pulsas una tecla.
- **Longitud de patrón** (`LEN`), independiente de lo grabado: un riff de dos
  compases recortado a uno está **a un knob** de volver a ser de dos, mientras
  que borrar los pasos es volver a grabar. Los pasos de más se conservan; poner
  la longitud igual al patrón es no tener longitud.
  - Se guarda (`arp_length`), y se olvida al cambiar de secuencia o de patrón:
    una longitud pertenece al patrón sobre el que se midió.

#### Notas
- 419 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (terdecies) — Ligaduras y grabación al vuelo

Segunda pasada del clon de KeyStep: las dos cosas que separaban "escribir notas"
de "grabar una parte".

#### Añadido
- **Ligadura (`TIE`) por paso.** `SeqStep.tie` (`#[serde(default)]`): el paso
  **no suelta en su gate** y la nota que continúa en el siguiente **no se vuelve
  a atacar** — se queda pisada, que es lo que significa ligar. La que no
  continúa sí se suelta en el límite, porque ahí es donde termina.
  - Botón `TIE` junto a `REST`: con un paso elegido liga ése; sin ninguno, el
    **último grabado**, que es como se escribe una ligadura metiendo una parte
    paso a paso — tocas la nota, pulsas TIE, y dura dos.
  - La marca va **sobre el chip que sostiene** (`2:D#4‿`), no entre dos: la
    tira envuelve, y una marca en el hueco caería al otro lado del salto de
    línea.
- **Grabar al vuelo.** Armar `REC` mientras la secuencia corre **graba encima**
  en vez de empezar de cero: cada tecla cae en el paso sobre el que se tocó, y
  el patrón que está sonando no se tira. Parado, `REC` sigue empezando uno
  nuevo — las dos mitades de lo que graba un KeyStep.
  - **Cuantización al paso más cercano, no al que suena**: una tecla pasada la
    mitad del paso iba dirigida al siguiente. Redondear siempre hacia abajo
    atrasa un paso a cualquiera que toque ligeramente adelantado, y un
    secuenciador que castiga eso no lo usa nadie.

#### Notas
- Del clon quedan: modo acorde, longitud de patrón independiente de lo grabado
  (+`SKIP`), y reloj MIDI externo.
- 417 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (duodecies) — El arpegiador, más cerca de un KeyStep

El usuario fijó el objetivo: el arpegiador/secuenciador es un clon del **Arturia
KeyStep**. Esto es la primera pasada contra esa lista.

#### Añadido
- **Los ocho modos del KeyStep, en su orden**: `UP`, `DOWN`, `INCL`, `EXCL`,
  `RANDOM`, `ORDER`, `UP×2`, `DN×2`.
  - `INCL` (Inclusive) **repite las notas de los extremos** y `EXCL` (lo que
    antes se llamaba `UP-DN`) no: es la única diferencia entre los dos modos, y
    la que decide si una tríada suena pareja o tartamudea en las puntas.
  - `×2` toca cada nota dos veces; la dirección ya la puso el orden.
  - `ORDER` es el antiguo `PLAYED` (mismo comportamiento, el nombre del
    hardware).
- **Las ocho divisiones**: `1/4`, `1/4T`, `1/8`, `1/8T`, `1/16`, `1/16T`,
  `1/32`, `1/32T` — faltaban el tresillo de negra y el de fusa.
- **Transposición tocando** (el gesto del KeyStep): una tecla mientras el
  secuenciador corre **mueve el patrón entero** en vez de añadir una nota suya.
  Do central lo devuelve a como se grabó, y el transporte lo escribe (`+7 st`)
  para que una secuencia que vuelve en otro tono no parezca un bug. Es una
  ejecución, no parte del patrón: no se guarda.
- **Ocho secuencias por tab** (SEQ 1–8), con knob `SEQ` y botón en la forma
  compacta. Cambiar de una a otra **suelta lo que la anterior tenía sonando** —
  esas notas son suyas y nadie más mandaría sus note-offs.
  - Se guardan las ocho (`arp_patterns` + `arp_seq`); un proyecto de antes trae
    una sola (`arp_pattern`) y **carga como la primera**.

#### Notas
- Lo que falta para el clon está anotado en el roadmap: ligaduras (TIE) por
  paso, modo acorde, grabación en tiempo real contra el reloj, longitud de
  patrón independiente de lo grabado, y reloj MIDI externo (24/48 ppqn).
- 413 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (undecies) — Editar un paso ya grabado

Lo que faltaba del secuenciador: se grababa, se borraba y se añadían silencios,
pero cambiar la nota del paso 3 era volver a grabar la parte entera.

#### Añadido
- **Tira de pasos** bajo el transporte del secuenciador: un chip por paso con
  sus notas (`2:D#4`, `5:—` para un silencio), la cabeza lectora marcada y el
  paso elegido resaltado. Clic elige, otro clic en el mismo devuelve las teclas
  a tocar — no hay otro gesto para "deja de editar" —, y la rueda camina la
  selección.
- **Con un paso elegido, la tecla que se toca lo reescribe**: las teclas dentro
  de 40 ms siguen siendo un acorde (la misma ventana que grabando, porque es el
  mismo gesto), y una tecla después de esa ventana **sustituye** lo que había.
  Un paso que sólo pudiera ganar notas no se podría corregir.
  - **El cursor no avanza solo**: escribir una tirada es para lo que está `REC`;
    esto es para la que salió mal, y un cursor que corriera pondría la
    corrección en el paso siguiente.
- `REST` silencia el paso elegido (y sigue añadiendo uno cuando no hay ninguno);
  **`DEL STEP`** aparece sólo con un paso elegido y lo borra, dejando el cursor
  donde estaba — borrar dos veces borra hacia abajo, como una lista.
- **El teclado del ordenador pasa por el arpegiador** cuando la tab lo tiene
  encendido, igual que una tecla MIDI. Era la única entrada que ni arpegiaba ni
  podía escribir un paso, que es justo para lo que sirve elegir uno.

#### Notas
- La tira **no puede pasar de dos filas**: un patrón de sesenta y cuatro pasos
  echaría la cadena de FX fuera del panel. Se dibuja una ventana calculada
  (`strip_window`) que siempre incluye el paso en el que se está trabajando —
  medida, no recortada: un chip recortado se lleva su rect, y un paso que no se
  puede clicar es un paso que no se puede arreglar.
- 409 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (decies) — El arpegiador se engancha al transporte

Primer pendiente del punto 1 del roadmap: el arpegiador tenía su propio BPM y
nada más.

#### Añadido
- **`ArpSettings.sync`** (knob `SYNC`, y botón en la forma compacta). Con él
  puesto:
  - **el tempo es el del transporte** (`ArpSettings::tempo`), así que lo que la
    fila imprime y lo que el reloj usa salen del mismo sitio y no pueden
    discrepar;
  - **el paso lo dice la posición de la canción, no una cuenta de duraciones**
    (`Arp::grid_step`): el índice se saca de `transport().ppq()` dividido por la
    división, y un paso vence cuando ese número cambia. Nada se acumula, dos
    tabs quedan en fase entre sí y con un plugin sincronizado, y un hilo de UI
    ocupado ya no puede arrastrar la rejilla — el reloj lo lleva el callback de
    audio, que es quien avanza el transporte.
  - el swing se aplica sobre la rejilla: la off-beat no llega hasta que ha
    pasado su parte del paso.
  - **el knob de BPM mueve el transporte** en vez de un número que no suena:
    hay un reloj, y eso es la tab pidiendo que vaya más rápido.
- **Con el transporte parado, sigue sonando**: no hay rejilla a la que
  engancharse, así que corre libre al tempo del transporte. Alguien sosteniendo
  un acorde quiere oírlo, no que le expliquen por qué no.
- `#[serde(default)]`, así que los proyectos guardados siguen cargando.

#### Notas
- Lo que queda del punto de timing es **resolución**, no deriva: `Arp::tick`
  sigue viviendo en el bucle de UI (5 ms), de modo que un paso puede sonar hasta
  5 ms tarde — pero ya no *desplaza* la rejilla, porque el número del paso viene
  de la posición y no de la suma de los anteriores.
- 407 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (nonies) — El arpegiador tiene knobs, y toma la forma que la pantalla aguanta

El requerimiento original pedía knobs para el arpegiador; la sesión anterior los
había cambiado por una fila de botones que envuelve, argumentando altura. Ahora
están los dos, y **la altura disponible decide cuál se dibuja** — no un ajuste,
no una preferencia: las filas que quedan.

#### Añadido
- **`ArpSettings::{knobs, norm, set_norm}` + `ArpParam`**: el arpegiador
  expuesto como parámetros normalizados (ON, PLAY, MODE, DIV, BPM, GATE, SWING,
  OCT, LATCH) con su `ParamShape` — interruptor, lista de nombres o continuo.
  Se dibujan con **`draw_knob_box`, la caja que ya existía** para los FX y el
  instrumento: el diseño no se toca, el arpegiador se adapta a él.
  - **Los knobs se direccionan por lo que son, nunca por su posición**: en modo
    secuenciador no hay `MODE` ni `LATCH`, así que un índice significa otro
    control según el modo. Misma lección que los presets de fábrica.
  - Escriben por `ArpEdit::Knob`, o sea por `edit_arp`, que ya sabía parar lo
    que sonaba al cambiar de modo y soltar el acorde al desenganchar el latch.
- **Tres formas, elegidas por las filas libres** (`ArpShape`):
  - **caja con borde y título** donde el RACK tiene sitio (≥ 5 + 7 filas);
  - **los mismos knobs sin marco** dos filas más baratos — que es la diferencia
    entre tener knobs o no en una pantalla de 5" (un panel de 800×480 con una
    fuente legible deja al rack unas trece filas);
  - **la fila de botones que envuelve** cuando ni eso cabe.
  Nada desaparece en ninguna de las tres; sólo cambia la forma. Y cuando sólo
  entra una fila de knobs, la caja **hace scroll con el cursor** como las otras
  dos, así que los de abajo se alcanzan.
- `draw_knob_box` acepta `bordered`: sin marco, tres filas en vez de cinco.
- **`RackFocus::Arp`**: `k` cicla FX → INSTRUMENT → ARP → FX, saltándose las
  cajas que no están en pantalla. Flechas mueven el cursor, `w`/`s` giran,
  Enter/segundo clic pasan a la siguiente posición de un knob con nombres, y la
  rueda gira el que esté debajo del ratón.
- El interruptor `ARP` es el primer knob de la caja: la fila de cabecera que lo
  llevaba **es una fila**, y una fila es lo que escasea.

#### Notas
- La caja de FX ya no se dibuja "activa" cuando las flechas están en otra: con
  tres cajas en el panel, "no es la del instrumento" dejó de significar "es la
  de los FX".
- 406 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (octies) — Las filas de botones del RACK envuelven, y `mtof` tiene cantidad

Dos pendientes del punto 1 del roadmap.

#### Añadido
- **`ButtonRow`** en `fx_chain_panel`: una fila de botones que **pasa a la línea
  siguiente** cuando se acaba el panel, y devuelve el rect de todo lo que
  dibuja. La usan la fila `INSTR`, la del arpegiador, la del transporte del
  secuenciador y la cadena de FX — que ya envolvía por su cuenta y ahora no
  tiene copia propia.
  - **Por qué no la caja de knobs** que proponía el roadmap: una caja con borde
    cuesta cinco filas, y el RACK no las tiene — es exactamente el motivo por el
    que la fila `ARP` se colapsa a un interruptor cuando está apagada. Envolver
    cuesta una fila y sólo cuando hace falta.
  - Cada botón se pinta donde se mide, así que una etiqueta traducida corre a
    los siguientes en vez de dejar sus rects atrás. Ese bug se arregló dos veces
    sobre offsets a mano; ésta es la forma que no puede tenerlo.
- **Botón `AMT` de `mtof`**, al lado del destino y sólo cuando hay destino: el
  clic recorre 25 → 50 → 75 → 100 → 25 (el mismo gesto que `GATE` del
  arpegiador) y **la rueda acota en vez de dar la vuelta** — una rueda que salta
  del tope a un cuarto no se usa dos veces.

#### Notas
- El test del arpegiador aprieta el panel a 80 columnas con todo encendido:
  los diez interruptores tienen rect, ninguno se sale del panel, ocupan más de
  una fila, y un clic en el que quedó envuelto sigue llegando.
- 404 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (septies) — Los cajones IN y OUT hacen scroll

El primer gotcha de la lista del roadmap ("es lo primero que va a doler"):
ningún cajón se desplazaba, y el cajón IN lista **todos** los puertos de captura
del sistema (una UMC1820 son ocho, más el micro del portátil, más la otra
tarjeta). En una terminal baja las últimas filas se pintaban fuera del panel y
no había forma de llegar a ellas.

#### Añadido
- **`drawer::{list_height, list_scroll}`** — la ventana visible de una lista.
  **No hay offset de scroll guardado en ninguna parte**: la ventana es función
  del cursor, igual que la caja de knobs del RACK. Sin segundo estado que
  mantener en sincronía con una lista que cambia debajo, sin nada que
  reinicializar, y —lo que importa— el dibujo y los rects de clic **no pueden
  desviarse**, porque los dos llaman a la misma función.
- `source_panel::input_window` envuelve las dos para el panel IN, que además
  tiene el banner de MIDI learn clavado abajo y por tanto menos filas.
- **Los títulos dicen dónde está la ventana** (`INPUTS ↕ 8-14/21`) en vez de
  gastar una fila en un indicador.
- **La rueda del ratón sobre un cajón mueve su cursor**, que es lo que desplaza
  la vista. En una lista donde cada fila es algo sobre lo que se actúa, eso es
  para lo que sirve la rueda.

#### Notas
- El test nuevo comprueba lo que se rompía: con 20 puertos y 7 filas de alto, la
  última fila **tiene rect**, ese rect está en la última línea de la lista, y el
  panel pinta ahí ese puerto — dibujo y hit-test contrastados contra el mismo
  buffer.
- Con esto el punto 1.2 del roadmap (filas de ruteo nuevas en el cajón IN) deja
  de estar bloqueado.
- 402 tests, `clippy --workspace --all-targets -D warnings` limpio.

### 2026-08-13 (sexies) — Medidores, latencia y presets de fábrica

Lo que quedaba de la fase 1 del punto 2 del roadmap: las tres piezas de
infraestructura que faltaban.

#### Añadido
- **`choz_ports::FxMeter`** — pico de entrada y de salida como dos atómicos
  compartidos, escritos con `Relaxed` desde el callback y leídos cuando la UI
  redibuja. El mismo camino que `SandboxStatus`, y por la misma razón: el
  procesador es del hilo de audio en cuanto se entrega.
- **Todo efecto de una cadena está medido**, sin que ninguno lo sepa:
  `fx_chain::Metered` envuelve cada procesador al construir la cadena y toma el
  pico a la ida y a la vuelta de un `process_block` que no se entera. La
  alternativa era un par de campos dentro de cada efecto — treinta copias de las
  mismas dos líneas, y **cero medidores para un plugin hosteado**, que es
  justo donde se pregunta "¿está llegando algo a esto?". Coste: dos pasadas por
  bloque y por efecto, un `max` por muestra sobre unos cientos.
- **`FxProcessor::latency_samples()`** — lo que un efecto retrasa la señal.
  `AutoTune` reporta la ventana de su shifter; el resto, cero. La cadena lo suma
  (`AudioEngine::slot_latency`) y la caja `SLOT` lo escribe en milisegundos. No
  hay compensación: no hay arreglo contra el que alinear, pero el número es la
  diferencia entre "el rack va pesado" y "el limitador cuesta 5 ms".
- **La caja `SLOT` lleva el medidor en su título** (`IN -6.0 OUT -7.2 dB`) y la
  latencia al lado. En el título porque la caja es una fila de botones: una
  segunda fila para dos números costaba una fila de knobs.
- **Presets de fábrica** (`choz-ui/src/fx_presets.rs`): 17 efectos con tres o
  cuatro cada uno — Slapback/Eighth Note/Dub, Room/Hall/Plate/Cathedral, Vocal
  Glue/Drum Punch/Bus Glue/Squash, Tape Warmth/Tube Drive/Fuzz Fold… Botón
  `PRESET` en la caja `SLOT` y tecla `P`.
  - **Van en la UI, no en el DSP**: un preset son posiciones de knob, y el orden
    de los knobs lo define `fx_param_descs` de este crate; una tabla junto al DSP
    sería una segunda descripción de ese orden, libre de desviarse.
  - **Se indexan por nombre de parámetro, no por posición**: el día que se
    inserte un knob en medio, un índice apunta a otra cosa en silencio. Un
    nombre que ya no existe lo caza el test.
  - **Se aplican por `set_fx_param`**, la misma puerta que un knob, un CC y el
    picker: el procesador vivo se entera, la copia de trabajo se queda con el
    valor y el rebuild se marca cuando hace falta. Un preset que escribiera
    `params` a mano sería un cuarto camino que equivocarse.
  - Lo que ya tenía **knob** de preset (el EQ gráfico con la lista de Winamp,
    AutoTune, la curva del saturador) se queda como está: eso son posiciones
    automatizables de un parámetro, no un menú aparte.

#### Notas
- Un preset que toca `Wet` escribe también `entry.wet`, que es de donde lee el
  rebuild: si no, el preset dura hasta que algo reconstruya la cadena.
- 400 tests en el workspace, `clippy --workspace --all-targets -D warnings`
  limpio.

### 2026-08-13 (quinquies) — Infraestructura de DSP y un saturador general

Fase 1 del punto 2 del roadmap, más el efecto que la justifica.

#### Añadido
- **`fx/oversample.rs`** — `Oversampler` a 1x/2x/4x/8x, cascada de etapas de 2x
  con lowpass Butterworth de **4º orden** por etapa, cada una a su propia
  frecuencia. Más `Tone` (lowpass logarítmico 400 Hz–18 kHz), que todos los
  no-lineales quieren y ninguno quiere tener.
- **`fx/smooth.rs`** — `Smoothed`: un polo, consciente del sample rate, para que
  un knob movido entre bloques no meta un escalón en la onda.
- **`fx/saturator.rs`** — **`Saturator`**: ocho curvas (soft, hard, tubo, cinta,
  foldback, wavefolder, diodo, polinómica), drive exponencial, bias, tono
  después de la curva, bloqueo de DC, ganancia de salida, oversampling elegible
  y medidores de pico de entrada y salida.

#### Lo que se midió y lo que se aprendió
- **El orden del filtro decide si el oversampling sirve.** Con 2 polos la
  primera reflexión a 23 kHz baja apenas 10 dB, así que encadenar etapas topa
  contra el filtro y no contra el factor: 8x quedaba en un 15 % de 1x. Con 4
  polos por etapa, **por debajo del 10 %**. Medido con Goertzel sobre el 7º
  armónico de 5 kHz, que se refleja a 13 kHz — el sitio donde más se oye y donde
  no hay nada armónico que lo tape.
- **`Smoothed` tiene que saltar al destino, no sólo acercarse.** En `f32`,
  `target - diff·coeff` llega a un punto fijo con el hueco todavía en ~1e-5
  cerca de 0.75: el paso cae por debajo del ulp del resultado y el valor se
  queda ahí para siempre. Inaudible, pero deja "¿llegó el parámetro?" sin
  respuesta. Salta bajo 1e-5.
- **El `Foldback` es forma cerrada, no un bucle.** Reflejar `while |y| > 1` es
  trabajo ilimitado para una entrada sin acotar, que es justo lo que no puede
  haber en el hilo de audio. El triángulo al que converge esa reflexión se
  calcula de una vez y vale `x` dentro de ±1.
- **Las curvas van normalizadas.** Sin compensar la ganancia del drive,
  "qué curva suena mejor" es sólo "qué curva suena más fuerte".
- **El bloqueador de DC no es opcional** con bias: una curva asimétrica devuelve
  offset, y un offset cuesta headroom sin sonar. Tarda ~50 ms en asentarse (polo
  a 10 Hz), que es lo que mide el test de silencio.
- `Curve` y `Oversamp` se dibujan como **listas de nombres**, sacadas de los
  enums del DSP: no hay nada entre dos curvas, y la etiqueta no puede desviarse
  de lo que hace el procesador.

396 tests.

### 2026-08-13 (quater) — `mtof`: una nota como control

La otra salida del ruteo: que una nota mueva un **parámetro** en vez de tocar un
instrumento. `crates/choz-ui/src/mtof.rs`.

#### Añadido
- Botón **`M→P`** en la línea INSTR: arma el puntero, un clic en cualquier knob
  liga el destino, un clic fuera lo desliga. La etiqueta dice a dónde apunta.
- Cada nota que suena en esa tab —de las teclas **o del arpegiador**— escribe el
  valor por `apply_target`, la misma función que usa un CC de MIDI learn. Un
  sitio decide qué significa cada destino; una nota es una cosa más que lo mueve.
- Destino y cantidad se guardan en el proyecto.

#### La decisión que importa: dos conversiones, no una
- **Destino con unidad `Hz` y rango declarado** (un parámetro de plugin): se
  escribe la frecuencia real, **en escala logarítmica**. Una octava mide siempre
  lo mismo; lineal, un rango 20 Hz–20 kHz gasta tres cuartos del recorrido por
  encima de donde se toca.
- **Cualquier otro destino**: key tracking sobre `pitch::{MIN_NOTE, MAX_NOTE}`.
  Un `FxParamDesc` de choz no declara ni rango ni unidad —sólo nombre, default y
  forma—, así que "escribir 440 Hz" ahí no es una conversión, es un invento. Y
  key tracking es lo que se quiere casi siempre: que el filtro siga al teclado.
- La nota MIDI más alta son 12.5 kHz, no 20: con un rango que llega a 20 kHz el
  teclado **no** alcanza el tope, y fingir que sí mentiría sobre la frecuencia.

377 tests.

### 2026-08-13 (ter) — Secuenciador de pasos

La otra mitad del punto: el mismo `Arp` gana `PlayMode::{Arp, Seq}`. Mismo
reloj, mismo gate, mismo swing; lo único que cambia es de dónde salen las notas,
que es la razón de que sean una máquina y no dos.

#### Añadido
- **Grabación en vivo** (`REC`): cada tecla es un paso, y **las teclas pisadas a
  la vez —dentro de 40 ms— son un acorde en un solo paso**. Sin rejilla contra
  la que cuantizar, esa ventana es la única pista que distingue un acorde de
  cuatro pasos. Armar el grabador **vacía** el patrón: sobregrabar encima de una
  secuencia que no se ve es como un grabador en vivo deja de ser usable.
- **Transporte propio**: `▶ ‖` y `■`. Son dos botones porque hacen cosas
  distintas — `■` rebobina, `‖` conserva la posición.
- **`TAP`**: media de los últimos 4 golpes, y un hueco de más de 2 s empieza una
  cuenta nueva en vez de promediar la espera. Los golpes son negras, sea cual sea
  la división, que es lo que significa un tap tempo en cualquier caja.
- **`REST`** (silencio con duración) y **`CLR`**. Un secuenciador que no puede
  escribir silencio toca un solo ritmo.
- Botón de **`SWING`**, que estaba implementado y probado desde la mañana y no
  tenía forma de moverse desde la UI.
- **Seis acciones nuevas de MIDI learn**: `ARP ON/OFF`, `▶/‖`, `■`, `TAP`, `REC`
  y `LATCH` — lo que hace falta con las dos manos ocupadas. El resto de la línea
  son ajustes, y un ajuste no merece un pedal.
- El patrón se guarda en el proyecto (`project::Slot.arp_pattern`), **aparte** de
  los ajustes: `ArpSettings` es `Copy` porque el panel toma una copia cada frame,
  y un `Vec` dentro le quitaría esa propiedad.

#### Detalles
- `MAX_STEPS = 64`. Se graba en vivo: sin tope, una tecla trabada bajo un repeat
  escribe hasta quedarse sin memoria.
- Un paso es un acorde, así que hay **varias** notas que soltar: `release`
  vacía la lista en vez de limpiarla, o se quedan colgadas.
- `PLAY` no tiene reloj con el que agendar, así que el primer paso vence **ya**;
  esperar al siguiente tick hacía que el botón pareciera fallar.
- La línea `ARP` ya no cabe en 120 columnas con todo encendido — el transporte
  bajó a una segunda fila, y sólo aparece en modo `SEQ`: en `ARP` las teclas
  *son* el transporte, y dibujar botones muertos diría lo contrario.

372 tests.

### 2026-08-13 (bis) — Arpegiador por tab

Primera mitad del secuenciador MIDI del roadmap. `crates/choz-ui/src/arp.rs`.

#### Añadido
- **Un arpegiador por tab**, apagado por defecto: modos `UP` / `DOWN` /
  `UP-DN` / `PLAYED` / `RANDOM`, divisiones de `1/4` a `1/32` con tresillos,
  BPM propio (20–300), gate, swing, hasta 4 octavas apiladas y latch.
- Línea `ARP` en el RACK: apagado es **un interruptor**, encendido despliega sus
  ajustes en la misma fila — una caja de seis filas para algo que la mayoría de
  las tabs no enciende es alto que el RACK no tiene. `A` lo enciende; los
  botones se pulsan con el ratón.
- Se guarda en el proyecto (`project::Slot.arp`, `#[serde(default)]`, así que un
  proyecto viejo carga igual y suena igual).

#### Por qué está donde está
- **No es un `FxProcessor` y no puede serlo.** `process_block` recibe audio
  interleaved y devuelve audio: no hay por dónde emitir una nota. Vive donde se
  resuelve el ruteo, que hoy es el hilo de UI. `Arp::tick` **recibe el
  instante** en lugar de leer el reloj, de forma que moverlo al engine (timing
  exacto contra el transporte) no toca la lógica ni los tests.
- El bucle de eventos pasa a despertar cada **5 ms** mientras algún patrón
  suena, 50 ms en reposo. Un paso que cae dentro de 50 ms se oye tarde.

#### Detalles que costaron pensarse
- Un tick tardío **no corre la rejilla** (el siguiente paso se cuenta desde el
  anterior, no desde ahora) **ni dispara una ráfaga** para recuperar el tiempo
  perdido: si el siguiente ya quedó atrás, se re-ancla al presente. Lo primero
  arrastraría el tempo en un hilo ocupado; lo segundo sonaría a metralleta.
- Una nota que al apilar octavas se pasaría de 127 se **descarta**; envolverla
  metería un bajo en medio de una subida.
- `RANDOM` es determinista (xorshift con semilla fija): dos arpegiadores con los
  mismos ajustes tocan lo mismo, que es lo que hace reproducible un informe.
- Cada `note_on` que emite tiene su `note_off`: `PANIC` y apagar el arpegiador
  sueltan lo que estuviera sonando. Nadie más va a mandar esos note-off.

12 tests del arpegiador, 364 en total.

### 2026-08-13 — Visualizador de teclado MIDI y paquete de Arch

Los dos primeros puntos del roadmap, implementados.

#### Añadido
- **Pestañas `KEYS` y `ROLL` en el panel `MIDI IN`.** `KEYS` dibuja un piano de
  dos filas (negras arriba con `▄`, cuerpos blancos debajo, etiquetas de octava
  cuando hay alto) que se enciende con lo que entra; `ROLL` deja caer las mismas
  notas hacia ese teclado en una ventana de 4 s. Estado en `KeyboardState`
  (`views/midi_monitor.rs`): mapa de 128 teclas + anillo de 256 notas de
  **presupuesto fijo** — las viejas se pisan, nada crece con el tiempo.
- **Tres modos de color**, `C` para rotarlos, guardado en `ui.json`:
  `CHANNEL` (un tono por canal, sacado de la misma rueda HSV que el logo),
  `INSTRUMENT` (la tab que realmente lo está tocando) y `VELOCITY` (el color del
  tema escalado por la fuerza).
- Los CC **no encienden teclas**: fila propia bajo el piano, con `BEND` y `MOD`
  siempre visibles y los últimos controladores vistos detrás.

#### Decisiones que cuestan explicar y no repetir
- Se alimenta en `drain_midi` **después** de resolver el ruteo, que es lo único
  que permite pintar una tecla del color de su tab. No reutiliza `App.sounding`:
  ésa indexa slots y existe para que un note-off llegue donde fue su note-on.
- **Un note-on con velocity 0 es un note-off** — hay hardware que sólo lo dice
  así, y leerlo literal deja la tecla encendida el resto de la sesión.
- **No hay timeout de notas colgadas.** Un pad sostenido un minuto es una nota
  sostenida; apagarla sola sería mentir sobre el caso fácil para acertar el
  raro. `PANIC` limpia el teclado a la vez que manda los note-off de verdad.
- La tecla es `C` mayúscula: `c` minúscula conecta una entrada en el cajón IN, y
  el teclado no tiene foco propio con el que desambiguar.

#### Empaquetado
- **Arch Linux**: `packaging/arch/PKGBUILD.in` (`choz-bin`, x86_64 + aarch64 +
  armv7h) y `mkpkgbuild.sh`, que rellena versión y los tres `sha256` desde los
  tarballs publicados. Un tarball que falte es **error**, no un placeholder que
  reviente en la máquina de otro — y los sumas se asignan a variables antes del
  `sed` porque un fallo dentro de `$( )` usado como argumento se traga el
  código de salida.
- Job `arch` en `release.yml`: dentro de `archlinux:base-devel`, genera
  `.SRCINFO` con `makepkg --printsrcinfo`, pasa `namcap`, adjunta ambos al
  release y empuja al AUR **sólo si existe** el secreto `AUR_SSH_KEY`.
- `depends=(alsa-lib …)` con **JACK en `optdepends`**: se `dlopen`ea, así que
  declararlo dependencia bloquearía la instalación en una máquina ALSA perfecta.
  Sin hooks de caché en `package()` — pacman ya los dispara por su cuenta.

352 tests, clippy `--workspace --all-targets -D warnings` limpio.

### 2026-08-12 — La 1.0.0 verificada desde fuera, y dos bugs que salieron de mirarla

El tag `v1.0.0` ya apuntaba a los arreglos de empaquetado y el workflow había
reconstruido los artefactos, así que lo que quedaba del punto 1 del roadmap era
**comprobarlo sobre los paquetes publicados**, no sobre el árbol. Sin `gh` en
esta máquina: `curl` contra `api.github.com` y `dpkg-deb` sobre lo descargado.

#### Verificado
- El `.deb` publicado lleva binario, `choz-launcher`, `choz.desktop`, los siete
  PNG más el SVG, el `.xml` del MIME y el copyright.
- **Cero rutas `/home/jorge`** dentro del binario (`strings | grep -c`), que era
  el otro síntoma del 2026-08-10.
- `SHA256SUMS.txt` cuadra con lo descargado, y los tarballs de `armv7` y
  `aarch64` traen ELF de su arquitectura (`ARM EABI5` y `ARM aarch64`). El
  armv7, que en local no se pudo cruzar por la red del contenedor, sí lo
  construyó el CI.

#### Arreglado
- **`install.sh` dentro de un tarball de release llamaba a `cargo build`.** Sin
  `--binary` iba directo a compilar **con el binario ejecutable a su lado**: el
  usuario que baja un `.tar.gz` es justo el que no tiene toolchain, y lo que
  veía era `rustup could not choose a version of cargo to run`. Ahora, si
  `$HERE/choz` es ejecutable, ése es el binario y lo dice. Test:
  `the_installer_uses_the_binary_shipped_beside_it`.
- **Los botones `◀ ▶` del banco sólo respondían en parte.** Los rects de clic de
  la línea `BANK` (y los de la línea `INSTR`) partían de un desplazamiento fijo
  —`inner.x + 2 + 8`, o sea `"  BANK  "` en inglés— mientras el texto se dibuja
  traducido: con `BANCO` el rect queda corrido una columna, media flecha no
  reacciona y la celda de al lado sí. Ahora la posición se acumula de las
  anchuras reales de los spans (`line_width`, con `Span::width`, que a
  diferencia de `chars().count()` no miente con CJK). Test:
  `the_whole_bank_arrow_is_clickable_after_the_label_is_translated`, que pulsa
  **cada columna** del botón con el idioma en español.

#### Documentación
- `docs/roadmap.md` reescrito con los pendientes nuevos: visualizador de teclado
  MIDI, paquete de Arch Linux, secuenciador/arpegiador MIDI con su ruteo y
  `mtof`, y la ampliación de la suite de efectos.

341 tests.

### 2026-08-10 — El `.deb` llevaba el binario y nada más

Reportado: choz instalado no aparece en el menú de inicio de Linux. Y aparte:
sacar del repo y de los binarios las rutas con el usuario de quien compila.

#### Arreglado
- **El paquete no llevaba ni entrada de escritorio, ni icono, ni lanzador.** La
  lista `assets` había quedado dentro de `[package.metadata.deb.variants.arm]`,
  y **una variante hereda de la tabla base, no al revés**: el paquete x86_64
  —el que se instala en un PC— salía con `usr/bin/choz` y el `copyright`, y nada
  más. No hay aviso de esto: cargo-deb construye contento, el paquete instala
  limpio, y la aplicación simplemente no existe para el menú.
  - Se ve sólo con `dpkg-deb -c` sobre el paquete construido, así que ahora hay
    cuatro tests (`crates/choz-ui/tests/packaging_assets.rs`) que leen el
    manifiesto y exigen que cada destino esté declarado en la tabla base y que
    cada fuente exista, para `.deb` y para `.rpm`.
  - Verificado sobre el paquete: los siete archivos están, en las dos variantes,
    y la ARM sigue declarando sus dependencias a mano.
- **Nadie refrescaba las cachés del escritorio.** `dpkg-deb -I` no mostraba
  postinst: Debian trae triggers que suelen hacerlo, y «suelen» es como una
  aplicación se instala y nunca aparece. Van `packaging/debian/{postinst,postrm}`
  con `update-desktop-database`, `update-mime-database` y
  `gtk-update-icon-cache`, los mismos que ya corría `install.sh` —al que le
  faltaba el de iconos, que es la diferencia entre una entrada de menú y una
  entrada de menú con un cuadrado en blanco al lado— y los mismos que ahora
  corre el `.rpm` en sus scriptlets.
- **415 rutas `/home/jorge` dentro del binario de release.** Un build de release
  mete la ruta del fuente en cada mensaje de pánico, y esas rutas vienen del
  `$HOME` de quien compila. `strip = true` no toca ninguna; cuatro
  `--remap-path-prefix` (registro de cargo, git, sysroot y el árbol) las llevan
  a **cero**, contadas con `strings`. Están en el workflow, apuntando a las
  rutas del runner. `trim-paths` haría lo mismo desde `Cargo.toml` y sigue
  siendo inestable en rustc 1.97.1.
- **`crates/choz-plugin-vst3/examples/gui_probe.rs`** tenía `/home/jorge/repo` en
  una constante. Ahora busca en `/usr/lib/vst3`, `/usr/local/lib/vst3`,
  `$HOME/.vst3` y `VST3_PATH`. `git grep /home/jorge` no devuelve nada.

#### Cambiado
- **El icono es un teclado con una onda encima**, dibujado en el repo y no
  tomado de un set —así no hay licencia que seguir, es MIT con el resto. Una
  octava de verdad (siete blancas, cinco negras) a 16 px es papilla gris:
  renderizado y mirado, no supuesto. Van tres negras gordas y ninguna línea
  entre blancas. Mal como piano, bien como silueta de 16 px, que es el tamaño
  donde este icono vive.
- **`Categories=AudioVideo;Audio;Midi;Sequencer;Music;`** — lo que lo pone bajo
  multimedia, que es donde se pidió. Con `Keywords` para synth, sampler, los
  seis formatos de plugin y autotune.

### 2026-08-09 (septendecies) — AutoTune con el método de zita-at1, y los knobs dejan de cortar el sonido

Reportado: el AutoTune seguía saturando la entrada; los presets del EQ no movían los sliders; y mover un parámetro de cualquier built-in cortaba el sonido.

#### Cambiado
- **El pitch shifter es ahora el de zita-at1** (Fons Adriaensen; x42 lo porta en [`fat1.lv2`](https://github.com/x42/fat1.lv2)): **un lector de delay a velocidad variable** que salta **períodos enteros** con un crossfade de coseno elevado, e interpolación cúbica. Fuera el overlap-add.
  - **Por qué**: una suma de copias enventanadas **tiene ganancia**, y esa ganancia depende del espaciado de granos, del largo de ventana y de cuánto se alinean granos consecutivos. Con una señal cuyo período se mueve, los tres bailan, y la salida sale sucia **y más fuerte que la entrada**. Más fuerte que la entrada se oye como que el efecto satura, y ninguna corrección de ventana arregla la clase de problema.
  - Dos lectores mezclados no pueden: la salida es una combinación convexa de dos muestras de la entrada, así que **`|out| ≤ max |in|`** para cualquier relación y cualquier altura. Es una propiedad del método, no un ajuste — y hay un test que lo comprueba a cinco relaciones distintas.
  - **Las formantes se mueven con la altura** (es un resampler). A las relaciones en las que vive un *corrector* — un semitono es 6 % — no se oye, y es lo que hace zita-at1. **Se fue el parámetro `Formant`**: con este método no hacía nada, y un interruptor que no hace nada es peor que no tenerlo. Un camino que preserve formantes sería otra implementación de `PitchShifter`, que es para lo que está ese trait.

#### Arreglado
- **Mover un parámetro de un built-in cortaba el sonido.** La UI marcaba la cadena para reconstruir en **cada** cambio de knob de un efecto nativo, y una reconstrucción reemplaza **todos** los procesadores del slot: mover el knob de un Gain tiraba la cola de la reverb, el búfer del delay y la grabación del looper. Ahora sólo se reconstruye si el procesador **no** puede tomar el valor en vivo (`AudioFxEntry::takes_live_params`).
  - `space_echo`, `protocosmos` y `z5_texture` — los tres que se reportaron — ya tenían `set_param`: nunca fue su culpa, era la reconstrucción.
  - Se añadió `set_param` a los que faltaban y tienen estado: `gran_delay`, `vinyl`, `bitcrusher`, `expander`, `filter`, `widener`, `pan`.
- **Los presets del GRAPHIC EQ no movían los sliders.** El preset llegaba al procesador pero no al array de parámetros — y ese array es lo que dibuja el panel y lo que guarda el proyecto, así que el preset se veía como si no hubiera pasado nada y desaparecía en la siguiente reconstrucción. `apply_preset` ahora escribe las diez bandas.

#### Verificado
- El shifter: la frecuencia objetivo aparece y la original desaparece; y **el pico de salida nunca supera el de entrada** con armónicos a fondo de escala, a relaciones de 0.6 a 1.5. El preset del EQ deja cada slider exactamente donde dice el preset (curva de sonrisa en "Rock"). Y que un knob de space echo **no** ensucia la bandera de reconstrucción, mientras que uno de cassette — que no tiene camino en vivo — sí. Coste sin cambios: cero allocations, ~10 % del presupuesto de búfer, 33.3 ms de latencia. **335 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (sexdecies) — AutoTune limpio, el EQ como el de tanu, y doce efectos

Probado con voz: sonaba sucio, con clip, y en pentatónica sólo ruido. Tres causas distintas.

#### Arreglado
- **El PSOLA reanclaba la rejilla de análisis en cada grano.** La marca de análisis se calculaba como `anal_ref + round((centro − anal_ref)/P)·P`, que es de manual para un `P` **fijo** y está mal para una voz: `P` cambia con cada análisis, así que la rejilla que define se mueve, y la fuente del grano salta hasta medio período cada vez que el cantante mueve la altura. Ese salto es una discontinuidad **dentro** de un grano, y una discontinuidad cada 8 ms es exactamente la suciedad que se oía. Ahora la marca **avanza un período** desde la anterior, y la deriva contra el reloj de salida se corrige sumando o quitando períodos enteros — que es lo que significa "duplicar o saltar una marca".
- **El overlap-add no estaba normalizado.** Un Hann de `2P` solapado con salto `P/ratio` suma `ratio`: corregir hacia arriba **subía el volumen**, y eso se oye como que el efecto satura cuando en realidad está subido. Cada grano se escala ahora por `1/ratio`.
- **Sonoro y sordo se conmutaban muestra a muestra**, así que cada consonante era un borde entre la señal desplazada y la seca — y con una voz real, que es medio sorda, se oía la fuente todo el rato. Ahora hay un crossfade de ~8 ms.
- **Una corrección enorme es un error de detección, no un cantante.** Más de tres semitonos de error se **ignora** (no se recorta: recortar seguiría doblando la voz tres semitonos hacia una nota que nadie cantó). Es lo que convertía la escala pentatónica en ruido: los objetivos más lejanos pedían relaciones que el shifter no puede hacer limpias, y obedecía.
- El período que corta los granos se sigue **suavizado**: la respuesta del detector tiembla una fracción de muestra entre análisis, y un largo de grano que tiembla es una ventana que deja de solapar a uno.

#### Añadido
- **Los parámetros con nombre se eligen en un modal** — Enter o clic sobre el knob. Vale para el `Preset` del GRAPHIC EQ (los 18 de Winamp, como en tanu), y para `Preset`, `Key`, `Scale` y `Mode` del AutoTune. Recorrer dieciocho presets con una flecha es un knob haciéndose pasar por un menú.
  - De paso salió `App::set_fx_param`: mover un parámetro de FX tiene tres llamadores — un CC, un clic y ahora el modal — y que cada uno se equivoque a su manera es cómo un knob acaba moviéndose en la interfaz y no en el audio.
- **El GRAPHIC EQ se dibuja como el ecualizador de tanu**: una columna por banda, el knob sobre la pista y la línea de cero recta por el medio, con las frecuencias debajo. Diez arcos no se leen como una curva, y la curva es la única pregunta que se le hace a un ecualizador. Los knobs que no son bandas (preamp, preset, wet) siguen debajo.
- **Hasta doce efectos por cadena** en vez de cinco. Una cadena de guitarra es afinador, puerta, compresor, drive, modulación y dos delays antes de que nadie piense en la reverb. El botón `+ ADD` ya envolvía a la línea siguiente solo; lo único que faltaba era el número.

#### Verificado
- El EQ dibujado de verdad sobre un `TestBackend`: los knobs, las pistas, las etiquetas de frecuencia (`70`, `180`, `1k`, `16k`) y un rectángulo de clic por banda, lado a lado y altos como una corredera. El picker: una banda **no** abre lista (no hay nada que listar), el preset sí y trae los dieciocho de Winamp, elegir "Rock" deja el parámetro en su posición, y `Preset`/`Key`/`Scale`/`Mode` del AutoTune abren la suya. Y en el DSP: la nota corregida es **limpia** — la frecuencia objetivo domina seis a uno sobre todo lo que una rejilla saltarina pondría alrededor — y **no sube de nivel**, que era el clip. **335 tests**, clippy `--all-targets -D warnings` limpio.

#### Arreglado (herramienta)
- **`ui_guard` es reentrante.** Un `std::sync::Mutex` no lo es, y los ayudantes que dibujan un panel toman ese candado por su cuenta: un test que lo toma y luego dibuja se bloquea contra sí mismo y arrastra a todos los demás. Se ve como una suite lenta, no como un fallo — todos los hilos en `futex_do_wait` al 0 % de CPU — y me costó encontrarlo **dos veces**. Un flag por hilo hace que tomarlo dos veces salga gratis, y dos hilos siguen turnándose.

### 2026-08-09 (quindecies) — AutoTune: corrección de altura en tiempo real, built-in

Un efecto de pitch correction nativo, no un envoltorio de nada. Documentación entera en [docs/autotune.md](docs/autotune.md).

#### Añadido
- **`AUTO-TUNE` es un built-in más** (`a → PITCH → AUTO-TUNE`), con su propia categoría porque un corrector de altura no es una textura ni un filtro. Efecto 34 de la casa.
- **Detección**: YIN — la misma función que usa `A→M`, extraída a `pitch::yin` y llamada dos veces en vez de escrita dos veces. Con gate por RMS, confianza, decisión de sonoro/sordo y una comprobación de octava (si la lectura está a unos cents del doble o la mitad de la anterior, gana la respuesta continua: nadie salta una octava entre dos análisis de 8 ms y cae afinado).
  - **Decimado a 16 kHz, y no es opcional.** A 48 kHz con ventana para 60 Hz, un análisis son ~2.2 millones de operaciones y un salto de 256 muestras pide 187 por segundo: **410 millones de operaciones por segundo por una sola voz**. Eso no es "va lento", son xruns y un plugin sin CPU — exactamente como fallaba `A→M` antes de reescribirlo. Promediar `round(sr/16000)` muestras en una (el downsample *y* su filtro anti-alias) lo deja en ~117 k: **treinta veces menos**. La precisión de frecuencia sobrevive porque el valle se interpola.
- **Nota objetivo**: `ftom`, con `A4` configurable (430–450 Hz), y seis escalas — cromática, mayor, menor, pentatónicas y blues. La nota más cercana se busca contra el número de nota **fraccionario**: alguien 40 cents por encima de F en Do mayor está más cerca de F que de G, y redondear primero tiraría justo el dato que lo dice.
- **Corrección en el dominio logarítmico** — semitonos, no hertz. Suavizar una frecuencia linealmente pasa por las notas equivocadas de camino. `Retune` (0–1000 ms) es la constante de tiempo de un polo, nunca cero porque un escalón en la relación de altura es un clic; `Correction` cuánto del error se toma; `Humanize` **mueve el tiempo de retune, no la altura** (modular la altura sería un vibrato que nadie cantó); `Mode` es Natural o Hard Tune, y Hard ignora las dos perillas a propósito.
- **Pitch shifting por PSOLA**, no por resampling: leer una línea de retardo más rápido sube la altura *y acorta el sonido*. PSOLA cambia sólo el espaciado de los granos, así que la salida siempre tiene tantas muestras como la entrada. Y preserva las formantes gratis — un grano son dos períodos del original sin tocar, su envolvente espectral es la del cantante. `Formant Preserve` es entonces un interruptor sobre **cómo se lee un grano**, y apagarlo da la ardilla, que es un sonido que la gente pide.
- **Latencia: 1600 muestras, 33.3 ms a 48 kHz** — dos veces el período más largo (60 Hz) **al rate que corre**, no al máximo para el que están dimensionados los búferes; ese error costaba 67 ms a 48 kHz para nada. La señal seca se retrasa lo mismo antes de mezclarse: mezclar un seco sin retrasar con un mojado 33 ms tarde es un filtro peine, no una mezcla.
- **Cinco presets** (Natural Vocal, Fast Vocal, Hard Auto-Tune, Subtle Correction, Robot Voice) que **escriben** sus valores en el array de parámetros del efecto, que es lo que guarda el proyecto y lo que reconstruye la cadena. Decírselo sólo al procesador haría que el preset durara hasta la próxima perilla que se tocara.
- **Lectura bajo las perillas**: nivel, la nota que oye, la nota a la que apunta, el error en cents y una traza de por dónde ha ido ese error. Sale de un medidor sin locks que publica el callback — seis stores relajados y nada más.

#### Arreglado (dentro de la propia feature, y vale la pena recordarlo)
- **El PSOLA no desplazaba nada.** El reloj de síntesis se reancla cuando se queda atrás — así se reengancha tras un tramo sordo. La primera versión preguntaba "¿está atrasado?" a secas, y como la marca es fraccionaria cae entre dos muestras y **siempre** lo está por una fracción: la rejilla de análisis se reanclaba en cada grano, cada grano salía de su propia posición de síntesis, y la suma solapada reconstruía la entrada. Espectro puro, espaciado de granos perfecto, y cero desplazamiento — una relación de 1.5 daba 220 Hz a partir de 220 Hz. Ahora la pregunta es "¿atrasado más de un período?".

#### Verificado
- **31 tests** propios: el detector a 44.1/48/96 kHz sobre seis notas y sobre un fundamental más flojo que sus armónicos (donde un detector ingenuo canta la octava); silencio y ruido como sordos; NaN e infinitos que entran y no salen. Escalas, la nota más cercana con sus empates, el A4 configurable. El glide que no salta, `Correction` al 50 % dando medio semitono, Hard Tune llegando diez veces antes, `Humanize` cambiando la curva y **no** la nota. El shifter midiendo con un DFT de un bin — ni cruces por cero ni autocorrelación, que las dos mienten aquí — y comprobando que la frecuencia objetivo está y la original **no**. Y el efecto entero: 445 Hz que se va a 440, la escala decidiendo el destino, mezcla a 0 % devolviendo la entrada retrasada, silencio que sigue en silencio, tamaños de bloque de 37 a 1024, cambio de sample rate a mitad de camino y ningún clic al cambiar de nota.
- **Coste medido, no prometido**: `examples/autotune_bench.rs` instala un asignador global que cuenta y toma la cuenta alrededor de `process_block` sólo. **Cero allocations** a 44.1/48/96 kHz con bloques de 64 a 512, y entre un 6 % y un 11 % del presupuesto del búfer. Alrededor de una décima de núcleo por voz.
- **Sin probar con una voz de verdad**: todas las señales de test son sintéticas, y una señal sintética es más amable que una habitación.

### 2026-08-09 (quaterdecies) — un solo color traslúcido para todas las secciones

#### Añadido
- **Todas las secciones — IN/OUT, RACK, FX, TRANSPORT y el monitor — comparten un mismo fondo de color traslúcido**, con la opacidad y el color elegidos en `Settings → THEME`. Antes el lavado existía sólo sobre una imagen de fondo; sobre un color plano los paneles no pintaban nada y no se distinguían del escritorio.
  - **`Panel colour`**: el color del lavado. Por defecto es *"theme's own"* — el color de escritorio del esquema activo — así que una interfaz lavada sigue pareciendo el tema y no un filtro encima. `←`/`→` lo pasean por toda la paleta y vuelven al del tema.
  - **`Panel opacity`**: 0 % deja el escritorio intacto, 100 % lo tapa. Ahora aparece con **cualquier** escritorio, no sólo con una imagen.
  - Las dos filas desaparecen con el fondo por defecto de la terminal: choz no puede leer ese color, así que no tiene con qué mezclarse — y una traslucidez que no se puede calcular es una casilla que miente.
- **La traslucidez se resuelve a un color de verdad**, porque el fondo de una celda de terminal no tiene alfa: `theme::blend(base, tint, alpha)` es el único sitio donde vive lo "semi transparente". Sobre una imagen el lavado sigue siendo celda a celda (cada una mezcla con lo que la foto muestra ahí); sobre un color plano se calcula una vez por cuadro (`theme::panel_fill`).

#### Verificado
- El camino entero por la app: color plano al 50 % da exactamente la mitad, 0 % deja el escritorio y 100 % lo tapa; el mismo color sale por `panel_style()`, que es por donde pasan **todos** los paneles; el selector de color recorre la paleta y vuelve al del tema; y con el fondo de la terminal no se ofrece ninguna de las dos filas. Más el render real: un panel sobre un escritorio plano pinta la mezcla, y sin lavado deja pasar el color de abajo. **335 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (terdecies) — `A→M`: una nota exacta, como el `ftom` de Csound

Probado con el micro de unos auriculares H340 y un LV2: no sonaba bien. La entrada es **un jack mono** y lo que tiene que salir es **una nota**, ni más ni menos.

#### Cambiado
- **La conversión es `ftom` de Csound con `irnd` distinto de cero**: *"if non-zero the result is rounded to the nearest integer"* ([docs](https://csound.com/docs/manual/ftom.html)). `freq_to_note_exact` es `ftom` con el `irnd = 0` por defecto (la nota fraccionaria) y `freq_to_note` es la redondeada, que es la que se toca. Un jack entra, una nota sale, como un teclado.
  - **Redondear solo no alcanza, y ésta es la parte que hay que acertar**: una altura apoyada en el límite entre dos semitonos redondea arriba y abajo según tiembla, y cada salto sería un note-on. Una nota sólo cambia cuando la nueva es **claramente** la más cercana (`HYSTERESIS`, 20 cents pasado el punto medio) **y** se ha sostenido tres análisis. Un vibrato dentro del semitono es una nota sostenida, que es lo que habría mandado un teclado.
  - Se fue el pitch bend continuo del intento anterior: la petición era una nota exacta, no una cinta de altura.
- **La entrada es un jack, no una mezcla.** Un tab alimentado por un solo canal tiene la misma señal en los dos lados; uno alimentado por dos canales distintos tiene **dos micrófonos distintos**, y sumarlos es cancelación de fase y dos alturas a la vez, que no es ninguna nota. El detector escucha el lado izquierdo, que es el canal que el usuario asignó primero.
- **El gate por defecto baja a -55 dBFS.** Un micro de auriculares por el previo de un portátil está mucho más flojo que una guitarra por una DI, y un gate por encima de la señal se lee como "no hace absolutamente nada", que es el peor de los dos fallos. `SENS` está en la tira del mixer justo para volver a subirlo donde una pastilla ruidosa lo necesita.

#### Añadido
- **El botón `A→M` dice lo que está oyendo**: ` A→M● E2-14` con la nota y los cents de desvío, o el nivel de entrada en dB cuando no suena nada. Sin esto, un tracker que no toca nada y uno que toca la nota equivocada se ven exactamente igual desde fuera, y `SENS` no tiene a qué apuntar. Los cents son **sólo display**: al plugin le llega la nota, exacta. Sale de un medidor sin locks que publica el callback (`meter::pitch_meter()`), igual que el de la salida.
- **El rack avisa cuando no hay a quién tocar.** `A→M` toca el **instrumento del tab**, no su cadena de FX: un tab sin instrumento sigue la altura perfectamente y no tiene con qué sonar, que desde fuera es idéntico a un detector roto. Ahora la línea del instrumento lo dice: `AUDIO IN 5 → A→M needs an instrument [1]`.

#### Verificado
- `ftom` en sus dos formas (440 Hz son 69 exactos; un cuarto de tono es 69.5 con `irnd=0` y **70** redondeado; 0.49 semitonos redondea a 69). Un vibrato de ±35 cents que sale como **una nota sostenida y cero eventos**, y un semitono de verdad que sí es nota nueva. Y la entrada mono: con un G en el canal izquierdo y una quinta invertida en el derecho, sale el G — sumar habría dado cualquier otra cosa. **335 tests**, clippy `--all-targets -D warnings` limpio.
- **Sigue sin probarse con el micro delante.** El modelo ya es el correcto y el gate no puede tapar una señal floja, pero cuánto hay que subir `IN` para unos H340 concretos sólo se ve mirando la lectura del botón.

### 2026-08-09 (duodecies) — el A→M no llegaba a tiempo, y el resto del roadmap

#### Arreglado
- **`A→M` mandaba notas al azar, y no era el detector: era el presupuesto.** YIN es O(ventana × lags), y corría **a la frecuencia del dispositivo en cada bloque**: 872 lags sobre 2048 muestras, 187 veces por segundo, dentro del callback de audio. Son ~340 millones de operaciones por segundo por una sola guitarra. El resultado no era "va un poco tarde" — eran xruns, el plugin sin CPU, y notas que parecían aleatorias porque el callback perdía su plazo. Dos cambios, los dos son hacer menos:
  - **Decimar a ~16 kHz.** La nota más aguda de una guitarra son 1.3 kHz; nada por encima de 8 kHz dice nada sobre su período. Promediar `D` muestras en una es a la vez el downsample y su filtro anti-alias.
  - **Analizar por salto, no por bloque.** Una nota no puede empezar dos veces en 8 ms, así que ésa es la frecuencia con la que se mira la ventana, sea cual sea el tamaño de bloque.
  - Juntos: ~150k operaciones cada 8 ms, unas **30 veces menos trabajo**, y el callback llega. La interpolación parabólica pasa de ser un pulido a ser obligatoria: a 16 kHz un lag entero es un *tono* en los trastes altos.
  - Test nuevo con **armónicos más fuertes que el fundamental**, que es como oye una pastilla de puente y donde un detector ingenuo canta la octava.

#### Añadido
- **Trim de entrada y sensibilidad, por tab** (`IN` y `SENS` en la tira del mixer, sólo donde hay audio entrando). Una guitarra por un previo no está ni cerca del nivel de un sintetizador, y sin esto los dos quedaban donde los dejó la interfaz. `SENS` es el gate del `A→M` en dB (-70 a -20 dBFS), que desde el lado del que toca es la misma perilla: cuánto hay que pegarle para que suene una nota. Rueda del ratón, `<`/`>` y `;`/`:`, MIDI-learnables y automatizables como todo lo demás, y guardados en el proyecto. Cierra el punto del roadmap que decía que el gate era fijo y sin control.
- **Posición de compás para VST3 y CLAP** — lo que le faltaba al transporte. `Transport::bar_position()` sale del compás leído en negras (4/4 son cuatro, 6/8 son tres, 7/8 son tres y media), así que VST3 declara `kBarPositionValid` con `barPositionMusic` y CLAP manda `bar_number` y `bar_start`. **Es la fase, no un sitio en una canción**: choz no tiene arreglo, cuenta compases desde el último reset del transporte. Eso es exactamente lo que necesita un plugin que sincroniza un patrón al inicio de compás, y es verdad — "compás 1 para siempre" no lo era.
- **La longitud del bucle de automatización se ajusta desde la interfaz**: botón `◀ LOOP n ▶` en el TRANSPORT, flechas o clic (mitad izquierda acorta, derecha alarga). Se muestra **en compases**, porque "16 pulsos" no significa nada en 6/8.

#### Verificado
- El detector contra seis notas de guitarra y bajo, y contra tonos con armónicos más fuertes que el fundamental. El trim y `SENS` de punta a punta: no existen sin audio entrando, se recortan en los extremos, el gate va en dB y la perilla ida y vuelta, los dos son targets de learn y automatización, y los dos sobreviven al proyecto. El compás en negras (4/4 son cuatro, 6/8 tres, 7/8 tres y media) y la posición de compás llegando a VST3 y a CLAP. El bucle de automatización en compases del compás vigente. **290 tests**, clippy `--all-targets -D warnings` limpio.
- **Lo que no se puede simular**: nada de esto se ha tocado con una guitarra por un amplificador. El presupuesto de CPU está calculado y acotado por un test, no medido con un `xrun` contador delante. Queda en el roadmap.

### 2026-08-09 (undecies) — faltaban entradas, y el gesto de asignar

#### Arreglado
- **Con las entradas ya visibles, no se podían asignar.** Un tab del rack sólo nacía al enlazar un **puerto de notas** (`bind_selected_input` llama a `add_silent` y `push_slot`). Quien entra por una guitarra no tiene ningún puerto MIDI que enlazar, así que el rack se quedaba vacío y `set_active_capture` volvía sin hacer nada: filas dibujadas, clic sin efecto. Asignar un canal de audio ahora arranca el tab si no hay ninguno, igual que enlazar un puerto.
- **La UMC1820 con ocho entradas mostraba `AUDIO IN (0)`.** choz pedía los puertos de captura **del sink**, y en PipeWire una interfaz son *dos* nodos: `alsa_output…` no tiene ninguno (los `monitor_*` se descartan a propósito, o el rack se realimentaría). Ahora `jack_backend::all_capture_ports()` devuelve **todos** los puertos de captura del grafo, de todas las tarjetas, y el cliente registra un puerto de entrada por cada uno y los cablea uno a uno. El cajón IN los lista agrupados por la tarjeta que los publica.
  - **Se fue la elección de "dispositivo de entrada"**, que ya no significa nada: están todos conectados y lo que se elige es un canal. Con ella se van `set_input_device`, `input_devices`, `capture_channels` y el ajuste `input_device` de `ui.json`. `r` en el cajón IN vuelve a leer el grafo, para una tarjeta enchufada después de arrancar.
  - Cambiar de salida ya no reconstruye el cliente por culpa de las entradas: dependen del grafo entero, no del sink.

#### Cambiado
- **Asignar y desasignar, en vez de mover lados.** Como pidió el usuario: `Enter` o `Espacio` ponen y quitan un canal del tab activo, el botón izquierdo del ratón pone y el derecho quita. Un tab tiene hasta dos canales — el primero es su izquierda, el segundo su derecha.
  - **Asignar es una cola de dos**: el nuevo entra por la derecha y el más viejo se cae por la izquierda. No es un detalle: un tab nace en 1 y 2, así que fijar la izquierda dejaría el canal 1 dentro por más clics que se hicieran. Así, clic en 3 y clic en 9 da exactamente 3 y 9.
  - Quitar el último canal de una **entrada** devuelve el tab a su instrumento (el mismo estado que la fila `(instrument)`); una **salida** siempre conserva uno, porque el motor necesita un canal donde mezclar.
  - El botón derecho no significa nada fuera de las filas de canal — ni sobre un puerto MIDI ni sobre un dispositivo de salida.

#### Verificado
- `assign_channel` y `unassign_channel` como funciones puras (la cola de dos, quitar el que no está, quitar el último), el recorrido entero por la app — nace en 1 y 2, clic en 3, clic en 9, queda en 3 y 9; el derecho lo deja en mono; el último no se puede quitar de una salida — y el reparto del ratón: izquierdo asigna, derecho desasigna, y fuera de las filas de canal el derecho no hace nada. **290 tests**, clippy `--all-targets -D warnings` limpio.
- **Sin la interfaz de audio delante no se puede comprobar lo que importa**: que las ocho entradas de la UMC1820 aparezcan de verdad y que el jack 5 sea el jack 5. Los tests corren sin cliente JACK, así que ven cero puertos de captura. Queda en el roadmap.

### 2026-08-09 (decies) — el ruteo es por canal, no por pares

#### Cambiado
- **Los dos cajones listan un canal por fila, no pares.** Los jacks de una interfaz no vienen pegados de a dos, y choz dejaba elegir sólo `2n`/`2n+1`: la salida 3 con la 9 no se podía pedir aunque el motor la soportara desde siempre (`mix[l]` y `mix[r]` son índices sueltos, y ya se recortaban al último canal). Ahora:
  - Cada fila dice qué es para el tab activo — `L`, `R`, `L+R` — así que el ruteo se lee del panel en vez de recordarse.
  - El gesto para poner y quitar canales cambió el mismo día; está en la entrada de arriba, que es la que vale.
- La etiqueta del RACK dice `AUDIO IN 5` cuando un tab entra por un jack, no `5/5`.

#### Verificado
- El caso entero por la app: entrada 5 en mono (la etiqueta dice `AUDIO IN 5`), salida por el 3 y el 9, y las filas del cajón etiquetadas `L`, `R`, `L+R`. Elegir un canal mueve **sólo** el tab activo.
- **El motor no hubo que tocarlo**: `SetSlotOut { left, right }` y `SetSlotIn { pair }` ya eran canales sueltos, y sus tests (`slots_land_on_the_output_pair_they_are_routed_to`, `an_out_of_range_pair_folds_onto_the_last_channel`) ya cubrían un par arbitrario y el recorte al último canal. Lo que sobraba era la interfaz.

### 2026-08-09 (nonies) — la guitarra toca el VST, el EQ de tanu, y el monitor mira el audio

#### Añadido
- **`A→M`: la entrada de audio de un tab se convierte en notas para su propio instrumento** (`choz-engine/src/pitch.rs`). Un botón en la línea del instrumento del rack, que **sólo aparece donde hay una entrada** — sin par de captura no hay nada que escuchar. Con él encendido el audio ya no pasa por los FX: se escucha, y lo que suena es el instrumento del tab tocando lo que oyó, así que una guitarra toca Surge XT.
  - **Monofónico, y no es un atajo que arreglar luego.** Un período es una frecuencia; un acorde tiene varias y elegir una es adivinar. Los sintetizadores de guitarra funcionan así — una cuerda, un conversor — desde hace cuarenta años.
  - **YIN, no autocorrelación pelada.** La primera versión usaba una diferencia cuadrada sola, que en una señal suave baja en *todos* los lags cortos: el mi grave de una guitarra salía una octava y media arriba. Dividir cada lag por la media acumulada de los anteriores deja el período real como el primer valle bajo el umbral.
  - **Un tono tiene que sostenerse para ser nota** (tres análisis, ~16 ms a 256 frames). Sin eso, arrastrar un dedo hasta la nota dispara una nota-on por semitono: medido, ocho notas donde debía haber dos. Cambiar de nota exige además una lectura más limpia que empezar una (0.95 contra 0.85), porque mientras la ventana aún tiene la nota anterior la mezcla ensucia el valle.
  - Se corre sobre el bloque que el callback ya tiene: sin latencia extra, sin reservar memoria, sin locks. Apagarlo suelta la nota que estuviera sonando.
- **GRAPHIC EQ: el ecualizador de diez bandas de tanu, con sus 18 presets de Winamp** (`fx/graphic_eq.rs`). Las mismas frecuencias, el mismo rango de ±12 dB y los mismos presets copiados de tanu (`src/audio/eq.rs`), así que un preset significa aquí lo mismo que allí. La diferencia: cada banda es un parámetro de choz, o sea **MIDI-learnable y automatizable una por una**, y el selector de preset es un knob más.
- **El monitor MIDI tiene pestañas: MIDI / WAVE / ACTIVITY** (`F5`, o clic en la tira). Las dos visualizaciones vienen de seqterm. Se alimentan de un medidor sin locks (`choz-engine/src/meter.rs`): el callback publica pico, RMS y una ventana de onda diezmada en atómicas relajadas, y la interfaz las lee al redibujar — un medidor un bloque atrasado es un medidor correcto.
- **La splash screen llena su caja**: reflejo del logo, versión, los seis formatos encendiéndose por turno y una línea de onda que viaja. Antes eran 24 filas con once de contenido. La onda es determinista en `(tick, width)`, así que hay test: una animación que nadie puede comprobar es una animación que se rompe en silencio.

#### Arreglado
- **El knob de preset del GRAPHIC EQ no hacía nada en caliente.** `set_param` atendía las diez bandas y el preamp y dejaba caer el índice 11, así que elegir "Rock" sólo surtía efecto la próxima vez que se reconstruía la cadena (recargar el proyecto): un knob muerto.
- **Y al reconstruirla, el preset anulaba las bandas.** Quien elegía un preset y luego movía una banda no movía nada. Ahora una banda en el centro se queda con lo que puso el preset y una movida gana, que es la decisión más reciente.
- **`cargo test --workspace` se colgaba para siempre.** El test nuevo de `A→M` tomaba `ui_guard()` y llamaba a `render_rack`, que lo vuelve a tomar: un `std::sync::Mutex` no es reentrante, así que el hilo se bloqueaba contra sí mismo — y con él todos los demás tests que esperan ese mismo candado. Se veía como una suite lenta (todos los hilos en `futex_do_wait` al 0% de CPU) y no como un fallo. `render_rack` ya toma el candado; el test no debe.

#### Verificado
- Bandas y presets contra tonos: la banda de 1 kHz sube 1 kHz y deja 70 Hz en paz, y los 18 presets llegan enteros y en dB. El detector, contra seis notas de guitarra y bajo (mi grave 82 Hz hasta el mi 1318 Hz), el silencio, el gate y el cambio de nota. Las tres pestañas del monitor, incluida la de que la tira devuelve sus rectángulos para el clic. **281 tests**.
- **Los paquetes se plantan de verdad sin ALSA, comprobado sobre los paquetes construidos y no sobre la intención**: el `.deb` sale con `Depends: libasound2t64 (>= 1.0.29), libc6 (>= 2.43)` (`dpkg-deb -f`), o sea que apt no lo instala sin ALSA; el `.rpm` pide `libasound.so.2()(64bit)` con sus versiones de símbolo (`ALSA_0.9`, `0.9.0rc4`, `0.9.0rc8`) además de `libc`, `libm`, `libgcc_s` y el cargador, así que `rpm -i` falla con "Failed dependencies". JACK es débil en los dos (`Recommends` / `recommends`), que es correcto: se abre con `dlopen`. `install.sh` sale con error 1 sin copiar nada.
- **Lo que no se puede simular**: nada de esto se ha tocado con una guitarra de verdad por un amplificador. Queda en el roadmap.

### 2026-08-09 (octies) — los nice-to-have: canal por tab, compás, LV2 y automatización

Cierra el último punto de Pendiente que no dependía de hardware ni de decisiones del usuario.

#### Añadido
- **Un puerto MIDI se puede partir entre tabs por canal, en modo LIVE.** Es **opt-in**, y por una razón concreta: pulsar `+` da otro patch sobre el mismo controlador, y si ese tab reclamara un canal que el teclado ya manda, el patch que está en pantalla se quedaría mudo. Así que un tab responde a **cualquier** canal (`CH ANY`) hasta que se le da un número; entonces ese canal le llega aunque el activo sea otro. Al entrar en MULTI los tabs en ANY se numeran solos, porque allí un tab *es* un canal.
- **Compás distinto de 4/4** en el transporte (`Settings → AUDIO → Time signature`, ocho compases usuales), y viaja a los tres formatos que ya leían el reloj: `timeSigNumerator/Denominator` en VST2 y VST3, `time_signature_*` en CLAP.
- **LV2 recibe el transporte.** Era el que faltaba, y el más trabajoso: LV2 no tiene callback de reloj — el host **escribe un objeto `time:Position`** en el puerto de atoms del plugin, junto al MIDI. Se manda `frame`, `speed`, `beatsPerMinute`, `bar`, `barBeat`, `beatsPerBar` y `beatUnit`, en un objeto de atoms construido a mano y alineado a ocho bytes.
- **Automatización** (`choz-ui/src/automation.rs`): se graba lo que el usuario mueve y se vuelve a mover en la pasada siguiente. `R` arma (y arranca el transporte, porque grabar contra un reloj parado graba un instante), `X` borra, y el botón `REC` del TRANSPORT dice en qué estado está.
  - **Las direcciones son las de MIDI learn** (`LearnTarget`): un carril es "este control, a lo largo de un bucle". Nada nuevo hubo que hacer automatizable, y un carril significa lo mismo en el proyecto que un binding de CC.
  - **Graba muestreando, no interceptando.** La alternativa era un gancho en cada setter — el mixer, los knobs del instrumento, los del FX, la ventana del plugin, el camino del CC: cinco sitios que nadie puede olvidarse. El bucle de la interfaz ya late más rápido que una mano, así que cada vuelta pregunta los valores y anota los que cambiaron.
  - **Escalón, no rampa**: se reproduce dónde *estuvo* el control, sin inventar el movimiento entre dos muestras. Un valor repetido no escribe punto, y una segunda pasada reemplaza a la primera en vez de entrelazarse.
  - Se guarda en el proyecto (`automation:`), y un proyecto anterior simplemente no trae carriles.

#### Arreglado
- **La posición del transporte avanzaba con el transporte parado.** Un delay sincronizado leyendo una posición que se mueve con el STOP pulsado es peor que uno que no lee nada.

#### Verificado
- El bucle entero por la app: armar, rodar, mover un fader un pulso adentro, desarmar, adelantar el reloj a la misma posición de la pasada siguiente y ver el valor volver solo. Más el objeto `time:Position` recorrido como lo recorre un plugin (evento → objeto → propiedades por URID: tres segundos a 120 BPM son el compás 1, pulso 2), el split por canal con sus tres casos, y el compás llegando a VST2, VST3 y CLAP. **265 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (septies) — documentación al día, y el instalador dice qué falta

#### Añadido
- **`install.sh` comprueba las dependencias de ejecución antes de copiar nada, y con ALSA ausente se planta.** Antes no miraba: se instalaba un binario que arrancaba y no abría ningún dispositivo, que es un informe de bug esperando a pasar y no una instalación. Ahora sale con error 1, sin copiar nada, diciendo el paquete de cada familia de distro. `--skip-deps-check` instala igual, para el único caso en que seguir es lo correcto: preparar la instalación para una máquina que no es ésta.
  - **ALSA (`libasound.so.2`) es obligatoria; JACK no.** Comprobado sobre el binario, no supuesto: `ldd` sólo lista `libasound`, `libc`, `libm` y `libgcc_s`; **`libjack.so.0` se abre con `dlopen`**, así que sin ella choz funciona por ALSA y lo único que se pierde es el ruteo JACK/PipeWire. Por eso el `.deb` declara sólo `libasound2t64` y `libc6`, que es correcto.
  - X11 no aparece por ningún lado: las ventanas de plugin van por `x11rb`, que habla el protocolo y no enlaza ninguna librería C. La instrucción de instalar `libx11-dev` para compilar sobraba y se ha quitado.

#### Documentación
- **README**: estado en 1.0.0; qué necesita para *compilar* frente a qué necesita para *ejecutar* (tabla nueva); la política de "tener ventana basta para ir al sandbox"; la tabla de qué control dibuja cada tipo de parámetro; el transporte y la fila `Tempo`; `packaging/` y `examples/esp32s3-touch/` en el árbol; `CHOZ_SANDBOX_GUI` en las variables de entorno; y los conteos de tests por crate corregidos uno a uno.
- **architecture.md**: secciones nuevas de transporte y de empaquetado/escritorio; la sección de parámetros reescrita con de dónde sale cada control y las dos invariantes que hay que mantener (un paso por pulsación, y un rect por control); la deny-list de UIs explicada como propiedad del proceso.
- **roadmap.md**: el punto de la release dice ahora exactamente qué queda (commit, tag `v1.0.0`, push, binario aarch64) y qué ya está construido.

#### Verificado
- Los tres caminos, con un `ldconfig` falso: sin ALSA **falla y no instala nada**; con `--skip-deps-check` instala y dice que se saltó la comprobación; con ALSA pero sin JACK instala y deja la nota. Sin `ldconfig` en la máquina (contenedor, musl) tampoco se planta: dice que no pudo comprobarlo. **259 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (sexies) — superficie de control en un ESP32-S3 con pantalla táctil

#### Añadido
- **`examples/esp32s3-touch/`**: cuatro faders de canal con mute y un teclado de una octava, mandando OSC por WiFi al puerto que choz ya escucha. **No hace falta tocar nada de choz**: el servidor OSC es el de 2026-07-29 y el firmware sólo le habla.
- **El firmware no lleva librería de OSC**: un mensaje es dirección, tag de tipos y argumentos big-endian, todo alineado a cuatro bytes — treinta líneas de codificador frente a una dependencia que habría que fijar por cada variante de placa.

#### Corregido (premisa del requisito)
- **No existen versiones de ESP32-S3 con Linux.** El S3 es un Xtensa LX7 sin MMU, con cientos de kilobytes de RAM y sin `dlopen`; las placas S3 con pantalla táctil (ESP32-S3-BOX-3, LilyGO T-Display-S3 Touch, Waveshare ESP32-S3-Touch-LCD) corren ESP-IDF/FreeRTOS con LVGL. Hostear plugins *es* cargar código nativo en tiempo de ejecución, así que el ejemplo apunta a esas placas haciendo lo que sí pueden: ser la superficie mientras choz corre en la máquina que tiene la interfaz de audio.

#### Verificado
- `the_bytes_an_esp32_control_surface_sends_are_understood`: los bytes exactos que arma el sketch — `/mix/2/gain ,f`, `/mix/3/mute ,i`, `/note ,ii` con velocidad 0 como note-off — decodificados y parseados por el propio choz. **Sin comprobar en la placa**: flashear, mirar el panel y medir el retardo del toque necesita el hardware delante. **257 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (quinquies) — el banco de faders verticales (cierra el rediseño de parámetros)

#### Añadido
- **Tres o más faders seguidos con la misma unidad se dibujan como un banco de barras verticales**: un ADSR (cuatro tiempos), las ganancias de un EQ por bandas, un juego de envíos. Es el caso que el roadmap describía — ver el perfil de un vistazo vale más que leer cuatro números — y ahora el perfil se lee de corrido.
  - **La agrupación también sale del plugin, no de los nombres**: la unidad dice que son la misma clase de cosa y el orden es el suyo. Un plugin que llama a su envolvente `A/D/S/R` y otro que la llama `Attack/Decay/…` se agrupan igual, y un knob en medio parte el grupo.
  - Dos barras no son un banco (el mínimo es tres) y una unidad distinta corta la racha.
  - Medido: **58 de los 647 plugins LV2 instalados** dibujarían un banco — MVerb, ZaMultiCompX2, los Dragonfly, LSP Beat Breather…
- Cada barra **conserva su rect**, así que el ratón y el MIDI learn siguen funcionando sobre ella: hay un test que hace clic en la tercera barra y comprueba que selecciona el tercer parámetro.

#### Verificado
- El agrupador entero sobre casos sintéticos (racha corta, unidad distinta, knob en medio), la barra creciendo hacia arriba y llenando primero la celda de abajo, y el render real del RACK con una envolvente de cuatro tiempos. **256 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (quater) — el fader y el knob fino; el punto 1 dado por cerrado

El usuario dio por buena la pasada de mirar y oír: **el fondo se ve bien, el modo MULTI le
sirve como está, y los plugins responden al MIDI con latencia correcta.** Eso desbloquea la
mitad visual de los parámetros, que estaba detrás de ella a propósito.

#### Añadido
- **Fader horizontal para lo que se lee como recorrido** — mezcla, tiempos, porcentajes: la
  pista entera dibujada y sólo el mango moviéndose (`─────▮────`), en la caja del RACK y en
  la lista larga. **Qué es un recorrido lo dice el plugin, no su nombre**: la unidad
  (`units:unit` en LV2, `units` en VST3). `s`, `ms`, `%`/`pc` y los centésimos son faderes;
  un hercio o un decibelio siguen siendo knob. Un plugin que no declara unidad no cambia.
  - Medido aquí: **21 291 puertos de control con unidad** entre los 261 bundles instalados
    (`units:pc` y `units:ms` son las dos más comunes) — p. ej. Dragonfly Plate, cuyo dry/wet
    pasa a fader.
- **El arco del knob resuelve ocho veces más fino.** Una celda de terminal es la unidad más
  gruesa que hay, así que ocho celdas sólo podían mostrar ocho posiciones y un corte movido
  por poco se veía idéntico. Con los bloques de octavo (`▏▎▍▌▋▊▉█`) son **65 imágenes
  distintas en el mismo ancho** — lo más cerca de la resolución angular de un knob real que
  da un terminal.

#### Verificado
- El arco dibuja 65 imágenes distintas donde antes dibujaba 9, el mango del fader recorre la pista de punta a punta, y ambos salen igual en la caja del RACK y en la lista larga. **254 tests**, clippy `--all-targets -D warnings` limpio.

#### Cerrado sin código
- **"Mirar con audio y ojos reales"**, que era el punto 1 del Pendiente y lo único que los
  tests no dan. Confirmado por el usuario: fondo correcto, MULTI suficiente por ahora,
  plugins sonando por MIDI con latencia razonable. Lo que no se llegó a mirar —
  el escritorio recién empaquetado y un plugin siguiendo la fila `Tempo` — queda
  anotado con la release, que es cuando se instala de verdad.

#### Pendiente del mismo punto
- El fader **vertical** para grupos que se comparan entre sí (un ADSR, un EQ por bandas) y
  el layout no uniforme que hace falta para él: `param_grid` sigue repartiendo celdas
  iguales y `RackLayout.instr_knobs` necesita un rect por control para el ratón y el MIDI
  learn.

### 2026-08-09 (ter) — el reloj también para VST3 y CLAP

El transporte lo leía sólo VST2, que es donde el problema apareció. Ahora los tres
formatos que preguntan la hora reciben la misma respuesta.

#### Añadido
- **VST3: `processContext` deja de ser `NULL`.** Un puntero nulo ahí significa "el host no sabe qué hora es", y un delay sincronizado se inventa el tempo. Se rellena por bloque desde `choz_ports::transport`: `sampleRate`, `projectTimeSamples`, `projectTimeMusic` (en negras), `tempo`, 4/4 y el flag de reproducción.
- **CLAP: el bloque lleva su `clap_event_transport`** (era `None` = "libre"), con la posición en beats y en segundos, el tempo y el estado.
- **Sólo se marca válido lo que choz sabe de verdad.** Nada de compás distinto de 4/4, ni posición de compás, ni ciclo, ni SMPTE: un plugin lee un campo cuando su flag dice que está, e inventar un número de compás es peor que no ofrecerlo.

#### Verificado
- Los tres hosts, con el mismo reloj y la misma comprobación: 90 BPM y un segundo de audio dan 1.5 negras (`ppqPos` en VST2, `projectTimeMusic` en VST3, `song_pos_beats` en CLAP), y parado se cae el flag de reproducción. Los tests de runtime con plugins reales siguen en verde con el contexto ya adjunto. **252 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-09 (bis) — la lista larga de parámetros, y un transporte de verdad

#### Añadido
- **La lista del modal INSTRUMENT dibuja lo que cada parámetro es**, igual que la caja del RACK: **checkbox** `[x] ON` / `[ ] OFF` para lo binario (una columna, no un botón — es donde el roadmap lo pedía: en una lista de cuarenta filas), `◀ Nombre ▶ 3/8` para los pasos con nombre, y el arco de siempre para lo continuo — **con la unidad del plugin al lado del valor** (`20.000 Hz`), que era el hueco que quedaba abierto.
- **La rueda del ratón sobre un interruptor lo cambia de posición**, no de 0.03: pasaba por un delta crudo mientras las flechas ya iban por `ParamShape::nudge`.
- **choz tiene transporte propio** (`choz_ports::Transport`, punto de "nice-to-have"): posición en frames, BPM, sample rate y play/stop en atómicas, avanzado por el callback de audio y por nadie más. Es global al proceso a propósito: hay un solo reloj, y el sitio que más lo necesita — `audioMasterGetTime` de VST2 — es un callback en C al que le pasan un puntero al plugin y ningún contexto del host.
  - **El host VST2 ya no responde 120 BPM fijos** (se va el `ponytail:` que lo marcaba): rellena `VstTimeInfo` con el reloj real en cada llamada — tempo, `samplePos`, `ppqPos` y el flag de reproducción. Un delay o un arpegiador sincronizado sigue ahora a choz. Sigue fijo el compás: 4/4, porque nada en choz lo elige todavía.
  - **Settings → AUDIO → Engine gana una fila `Tempo`**, con ←/→ (20–300 BPM, clamped), aplicada al instante — no hay stream que reconstruir — y guardada en `ui.json`.

#### Verificado
- `get_time_follows_the_host_clock`: a 90 BPM y un segundo de audio, el plugin lee `ppqPos = 1.5` y `samplePos = 48000`; parado, el flag de reproducción se cae; un tempo fuera de rango se recorta antes de llegarle. Más el checkbox/nombre/unidad de la lista y la fila de Settings de punta a punta (se mueve, se ve, se guarda). **250 tests**, clippy `--all-targets -D warnings` limpio.

#### Pendiente del mismo punto
- Faders (horizontal y vertical), checkbox agrupado, más resolución angular en el knob y el layout no uniforme que hace falta para ellos. **Va detrás del punto 1**: son decisiones que se juzgan mirando, y la mitad del modelo — de dónde sale el tipo — ya está hecha, así que probarlas es barato cuando haya ojos delante.

### 2026-08-09 — instalable y en el menú del escritorio (puntos 3 y 4 de Pendiente)

#### Añadido
- **`choz --version` y `choz --help`** — la bandera que faltaba para que un instalador pueda preguntarle a la copia que ya está en disco qué versión es. Responden **antes** del redirect del log, porque si no la respuesta se va al fichero: fd 1 deja de ser la terminal en cuanto choz arranca de verdad.
- **`packaging/install.sh`**: construye (o toma un `--binary`), **busca la instalación vieja en `~/.local/bin`, `/usr/local/bin` y `/usr/bin`, le pregunta `--version` y la quita antes de copiar la nueva**, e instala también el lanzador, el `.desktop`, el icono y el tipo MIME. `--prefix`, `--uninstall` y `CHOZ_SEARCH_BINS` (lo único que sale del prefijo es esa búsqueda, así que el que llama puede apagarla — la suite de tests la apaga).
  - **Lo que ningún desinstalado toca: `~/.local/state/choz`.** Proyectos, rutas de plugins y ajustes son del usuario, no del paquete. Hay un test que lo comprueba con un proyecto escrito a mano.
- **`.deb` y `.rpm`** desde el mismo material (`cargo deb -p choz-ui --no-build`, `cargo generate-rpm -p crates/choz-ui`). Los dos reemplazan la versión anterior por nombre de paquete, que es la mitad del "detecta y desinstala" hecha por el gestor. El `.deb` sale con `Depends: libasound2t64, libc6` resueltos solos — ningún formato de plugin es dependencia, todos se cargan con `dlopen`.
- **choz en el menú del sistema** (punto 4): `choz.desktop` (`Categories=AudioVideo;Audio;`), icono SVG en `hicolor`, tipo MIME `application/x-choz-project` para `*.choz.yml` — el binario ya aceptaba la ruta como argumento, así que abrir un proyecto desde el gestor de archivos lo lanza con él.
  - **La decisión que el roadmap dejaba abierta**: no `Terminal=true`, sino un lanzador propio. `choz-launcher` prueba **kitty primero** (es donde el fondo se dibuja por protocolo gráfico, a resolución de píxel real) y luego ghostty, wezterm, alacritty y xterm, pidiendo 120×40 celdas — por debajo de ~100×30 el RACK no cabe.
- **`.github/workflows/release.yml`**: binario por arquitectura (x86_64, aarch64, armv7) más `.deb` y `.rpm`, en tag o a mano.
- **`Cross.toml`**: las imágenes de `cross` no traen ALSA ni JACK del target y el build moría en `alsa-sys`/`jack-sys` antes de compilar una línea de choz. Con el `pre-build` que los instala, **aarch64 compila y da un ELF ARM de verdad** (verificado aquí). armv7 no se pudo verificar en esta máquina: `cross` intenta bajarse el toolchain y la red del contenedor está cortada.

#### Arreglado
- **Un test que fallaba una vez de cada cinco**, y no por lo que decía: el directorio de estado de prueba es **por proceso, no por test**, así que el que deja un fondo de imagen guardado en `ui.json` se lo pasa al siguiente por `App::new()`. `sandbox_state_dir()` borra ahora ese `ui.json` (un sandbox que arrastra estado no es un sandbox) y el test afectado fija su fondo de partida en vez de suponerlo. **248 tests**, clippy `--all-targets -D warnings` limpio, y 12 corridas seguidas limpias del binario de UI (falla ~1 de cada 4 sin el arreglo).

### 2026-08-08 (duodecies) — el barrido de editores LV2, con Xvfb y con la ventana mapeada

Cierra el punto 2 de Pendiente. El usuario instaló Xvfb, que era lo que faltaba para
correrlo sin llenar el escritorio de ventanas.

#### Arreglado
- **El probe preguntaba demasiado pronto si la UI había creado su ventana.** `query_tree` justo después de `open()` es una moneda al aire: varios toolkits crean la ventana en la primera vuelta de su propio bucle. El mismo plugin daba `ok` y `SINVENTANA` en corridas consecutivas — medido, cinco veces cada uno. Ahora el probe bombea `idle` y espera al hijo hasta 500 ms. **Las cinco "UIs sin ventana" del barrido anterior eran esto**, no plugins.
- **El probe levanta la deny-list de UIs** (`allow_denied_uis(true)`): es un proceso que existe para que lo mate un plugin, así que esconderle justo los editores peligrosos es esconder lo que el barrido busca.

#### Medido (259 UIs X11 instaladas, Xvfb `:99`, `--mapped`)
- **252 abren una ventana hija de verdad. 0 sin ventana. 0 crashes achacables a choz.**
- **5 no cargan** y es correcto: QMidiArp (Arp/Seq/LFO), MIDI Step Sequencer8x8 y B.SEQuencer no tienen salida de audio, así que no se instancian como efecto ni como instrumento.
- **Mapear la ventana padre no cambia nada**: las listas de resultados con y sin `--mapped` salen **idénticas**, línea por línea. Era la duda que dejaba abierta el barrido anterior.
- **Primer barrido de los editores de guitarix**, que hasta ahora la deny-list escondía también del probe: **nueve rebanadas murieron con SIGSEGV, las nueve en un `gx_*`** (MultiBandCompressor, Studio Preamp Stereo, digital_delay, Alembic, BigMuffPi, Chorus-Stereo, duck_delay_st, w20…). La lista no era una sospecha: es lo que hacen. Y es exactamente lo que la política nueva resuelve — esos plugines tienen ventana, así que van a su propio proceso y el crash cuesta un hijo.
- **2 falsos positivos de Xvfb**: LSP Room Builder Mono y Stereo mueren con `BadMatch` en `MIT-SHM X_ShmPutImage` bajo Xvfb, mapeada o no. En el servidor X real abren perfectamente en los dos modos — se comprobó con esos dos plugines, un plugin, una ventana. **Un barrido bajo Xvfb no es prueba de nada sobre memoria compartida**: el que la usa para pintar necesita el servidor de verdad.

### 2026-08-08 (undecies) — el control que el parámetro pide (primera mitad del punto 3)

Un plugin no tiene sólo knobs. La caja `INSTRUMENT` y la de FX dibujaban **todos** los
parámetros igual — arco, valor, nombre — así que un `bypass` era un arco a 0.00 y un
selector de forma de onda un número sin sentido.

#### Añadido
- **`choz_ports::PluginParam` lleva ahora la pista del plugin**: `steps` (0 continuo, 2 interruptor, n enumerado), `unit` y `points` — los pasos con nombre. **Nunca se adivina por el nombre**: un host que no dice nada deja `steps: 0` y sigue saliendo un knob (la lección de `FxCategory::guess`, donde equivocarse sólo descoloca una fila de una lista).
  - **LV2**: `lv2:portProperty` (`toggled`/`enumeration`/`integer`), `lv2:scalePoint` (valor + `rdfs:label`, ordenados) y `units:unit`. De 261 bundles instalados, **78 declaran `toggled` y 49 `enumeration`**.
  - **LADSPA/DSSI**: `HINT_TOGGLED` y `HINT_INTEGER`, lo único que el ABI dice de un puerto.
  - **CLAP**: el flag `IS_STEPPED`, más `value_to_text` para nombrar los pasos cuando son pocos (≤32).
  - **VST3**: `ParameterInfo.stepCount` — que cuenta *intervalos*, así que `1` es un interruptor de dos posiciones — más `units` y `getParamStringByValue` para los nombres.
  - **VST2** no da nada y se queda en continuo, como decía el plan.
- **La caja de knobs dibuja tres controles según lo que el parámetro es**: interruptor (`[  ON  ]` verde / `[ OFF ]` apagado), enumerado (`◀ Sine ▶` con `1/3` debajo) y el arco de siempre. Las dos cajas salen de `draw_knob_box`, así que el FX lo hereda.
- **Las flechas mueven una posición, no 0.05**: un interruptor necesitaba veinte pulsaciones para cambiar y se quedaba en sitios que no existen.
- **Las posiciones con nombre viajan con su sitio en el rango, no con un índice.** El caso que lo obliga es real: el divisor de `a-delay` (Ardour) nombra diez figuras — 1, 2, 4, 6, 8, 12, 16, 24, 32, 48 — sobre un rango 1..48. Suponer una rejilla uniforme ahí muestra el nombre equivocado y salta a valores que el plugin no ofrece.

#### Pendiente del mismo punto
- Faders horizontales y verticales, más resolución angular en el knob, y el layout no uniforme que hace falta para ellos (`param_grid` sigue repartiendo celdas iguales). Los tres controles nuevos caben en la rejilla actual a propósito: es lo que se podía hacer sin tocar `RackLayout.instr_knobs`, del que dependen el ratón y el MIDI learn.

#### Verificado
- Con `a-delay` de verdad: puerto `enumeration` + `integer`, diez puntos ordenados, y el parámetro sale con `steps: 10` y las posiciones normalizadas donde el plugin las puso. Más el render de las tres formas sobre un `TestBackend` y el paso de flechas. **246 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-08 (decies) — los 361 esquemas de Gogh (pedido)

#### Añadido
- **Settings → THEME pasa de 11 esquemas a 372**: los 11 propios más los 361 de [Gogh](https://github.com/Gogh-Co/Gogh) (`data/themes.json`), en orden alfabético y sin repetir los que choz ya traía.
- **La tabla viaja como texto, no como código**: `crates/choz-ui/src/gogh_themes.txt` (10 KB, `nombre|texto RRGGBB|escritorio RRGGBB`) va por `include_str!` y se parsea una vez en un `LazyLock<Vec<Theme>>`. 361 literales `Theme` habrían sido 2000 líneas de fuente que nadie revisa; regenerarlo desde upstream es un `curl` y un script.
- **El color de marco lo pone choz**: un esquema de terminal no tiene uno. Es el punto medio entre el texto y el escritorio — más apagado que el texto, visible sobre el fondo — y la misma fórmula funciona para los esquemas claros. Una línea que no parsea se salta en vez de tirar la lista entera.

#### Cambiado
- **La pestaña THEME pone primero los controles de escritorio y la lista de esquemas al final.** Con 11 esquemas daba igual; con 372, todo lo que estuviera debajo de la lista era inalcanzable.
- `theme_rows`/`theme_row` salen ahora de una sola tabla (`theme_layout`), etiqueta y significado juntos. La aritmética de índices duplicada en dos funciones ya era un bug esperando la siguiente fila.

#### Verificado
- La tabla parsea a temas usables (Gruvbox Dark byte a byte, marco derivado, sin nombres vacíos ni repetidos) y la pestaña THEME sigue aplicando esquema, marco y escritorio a la vez. **243 tests**, clippy `--all-targets -D warnings` limpio.

### 2026-08-08 (nonies) — `state:mapPath` para LV2

#### Añadido
- **Las rutas de archivo que un plugin guarda ya no se pierden.** Un sampler o un convolutor tiene que pasar sus rutas por `abstract_path` antes de almacenarlas, y muchos no guardan **nada** si el host no ofrece la feature. `save`/`restore` pasan ahora `state:mapPath` y `state:freePath` (antes iba una lista de features vacía), y `Features::supported` los declara para el plugin que los exija al instanciar.
- **Las cadenas salen de `malloc`, no del asignador de Rust**: el plugin las libera con `free()` salvo que use `state:freePath`, así que se ofrecen las dos vías. Nueva dep `libc` en `choz-plugin-lv2` (ya estaba en el workspace).
- El mapeo es **la identidad**: se guarda la ruta absoluta. Un host de verdad copia el archivo dentro del directorio del proyecto para que sea autocontenido; el proyecto de choz es un solo YAML sin sitio donde meter una librería de samples, y una ruta que funciona en esta máquina vale más que un estado que el plugin se negó a guardar. Marcado `ponytail:` en el único sitio que habría que cambiar.

#### Verificado
- Round-trip de las dos funciones (copia propia, `free` por las dos vías, null→null) y la lista de features terminada en NULL. Con plugins reales, `plugins_with_a_state_interface_round_trip_their_patch` sigue en verde con las features nuevas. **No medido**: cuántos de los 611 LV2 instalados guardan ahora una propiedad `atom:Path` — el barrido tarda más de lo que aguanta una corrida en primer plano.

### 2026-08-08 (octies) — tener ventana es motivo suficiente para aislar el plugin

Cierra el punto 1 de Pendiente: la cuarentena medía si el plugin *sonaba* bien, y la GUI —
el código ajeno menos fiable que choz ejecuta — se quedaba fuera de la política.

#### Añadido
- **Un plugin con ventana se hostea en su propio proceso, aunque el probe lo haya visto sanísimo.** Procesar audio dos bloques no dice nada de la GUI: guitarix toca perfecto y su editor segfaultea. Ahora el veredicto de la cuarentena viaja con un segundo dato — *tiene ventana* — y `wants_sandbox` lo trata como razón por sí sola. `CHOZ_SANDBOX_GUI=0` apaga esa mitad para quien prefiera pagar el crash antes que el proceso extra.
- **El probe pregunta por la ventana, no la abre.** Tras instanciar (instrumento o efecto) llama a `editor()`, que sólo construye el mango, y lo anota en el fichero de etapa: `done gui` / `loaded gui`. Un plugin que muere antes de cargar no deja marca, que es la verdad — nadie llegó a mirar.
- **El probe mira por encima de la deny-list de UIs** (`allow_denied_uis(true)`): esconderlas ahí dejaría fuera del sandbox justo a los plugins por los que la lista existe.

#### Cambiado
- `quarantine::check` devuelve un `Report { verdict, editor }` en vez de un `Verdict` suelto. Un `plugin-verdicts.json` viejo (veredictos pelados) ya no parsea, se descarta entero y se vuelve a probar cada plugin — que es lo que hacía falta de todos modos para averiguar lo de la ventana.

#### Verificado
- Con plugins reales: `gxts9` (guitarix, UI en la deny-list) pasa a `wants_sandbox` por la ventana, y `ZamComp` (VST2 con GUI de DPF) también — el test de "sandbox a petición del usuario" sólo ve la mitad manual con `CHOZ_SANDBOX_GUI=0`. **240 tests**, clippy limpio.

### 2026-08-08 (septies) — la política de la ventana sandboxeada

Cierra el punto 1 de Pendiente: el mecanismo estaba hecho la sesión anterior, faltaba usarlo.

#### Añadido
- **Los editores de la deny-list vuelven a ofrecerse cuando el plugin va sandboxeado.** Las UIs de guitarix segfaultean lo que las cargue, así que `choz-plugin-lv2` las esconde — pero esconderlas es lo correcto sólo en el proceso de choz. `allow_denied_uis(true)` levanta la lista y **el único que lo llama es el hijo del sandbox**, donde el crash cuesta un proceso que el supervisor repone. La lista pasa a ser propiedad del proceso, no del plugin.
- **El hijo publica si su plugin tiene ventana** (`editor_present` en la cabecera compartida: desconocido / no / sí), **antes de servir su primer bloque** — que es antes de que `SandboxedPlugin::build` devuelva, y por tanto antes de que el host capture el mango. `editor()` sólo ofrece botón `GUI` cuando la respuesta es sí; antes lo ofrecía siempre y un plugin sin GUI abría un marco vacío. El host no tiene otra forma de saberlo: cuando pregunta, el hijo todavía está cargando.

#### Verificado
- Con plugins reales, en `tests/sandboxed_plugin.rs`: `gxts9` (guitarix) **no** tiene editor visible desde el proceso de choz y **sí** desde su sandbox; `a-delay` (Ardour, sin UI X11) no ofrece botón ni sandboxeado. Más el cruce del flag sobre un `Vec<u8>`, sin mapear ni forkear.

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
