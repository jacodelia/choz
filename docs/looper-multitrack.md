# Looper multipista — plan

Qué hay hoy, qué se pide, y en qué orden hacerlo. Cómo encajan las piezas está
en [architecture.md](architecture.md); lo que queda abierto del resto del
programa, en [roadmap.md](roadmap.md).

Escrito el 2026-08-25. **Implementado el 2026-08-26** — las seis fases están en
el árbol; lo que quedó afuera está al final, en *Lo que no se hizo*.

---

## Lo que se pide

Un looper multipista que reemplace al de una sola pista.

**Multipista quiere decir varias tomas sobre una misma fuente**, no varias
fuentes. Un micrófono o una guitarra entrando por un tab graban pista tras
pista, y esas pistas se pueden tocar juntas o no. Es el looper de pedalera
—una entrada, tomas apiladas— y no un grabador de mixer.

**El looper vive dentro del tab.** Recibe el audio de ese tab y nada más:
sigue siendo un efecto en la cadena, en el punto de la cadena donde el usuario
lo puso.

De ahí sale el resto:

- `REC` / `PLAY` / `STOP` por pista;
- las pistas suenan simultáneamente o no, cada una con su estado;
- un botón de cuantizar audio, con un mínimo de **1 segundo o 1 compás**;
- un toggle que prenda el metrónomo al empezar a grabar;
- **5 minutos** por pista como techo;
- exportar cada pista a WAV;
- el búfer en memoria acotado, para no matar a choz;
- **la primera pista define el límite de tiempo**;
- la frecuencia de muestreo, la que ya tiene choz.

## Dónde queda, exactamente

```
guitarra / micrófono
        │
        ▼
  [IN trim + gate]        ← lo que el tab ya hace con una entrada de captura
        │
   [FX 1] ─▶ [FX 2] ─▶ ┌───── LOOP ─────┐ ─▶ [FX 4] ─▶ mixer del tab ─▶ master
                       │ pista 1  ▶ ○ ● │
                       │ pista 2  ▶ ○ ● │
                       │ pista 3  ▶ ○ ● │
                       └────────────────┘
```

El deck graba lo que **llega a su slot** y suma sus pistas a lo que **sale** de
él. Dos consecuencias que hay que decidir a propósito y no por accidente:

- Los efectos **antes** del loop quedan grabados en la toma. Los de **después**
  se aplican a las pistas cada vez que suenan. Poner el looper primero graba
  seco; ponerlo último graba todo mojado. Es el usuario quien elige, moviendo el
  slot — y hay que decirlo en la ayuda, porque es la diferencia entre poder
  cambiar el reverb después y no poder.
- Un tab por fuente. Dos guitarras son dos tabs con un looper cada uno, que es
  lo que MULTI ya hace con todo lo demás.

## Estado antes de esto: el looper no se podía grabar

Lo que sigue describe el punto de partida. **Los cinco puntos están cerrados**;
quedan escritos porque explican por qué el reemplazo tiene la forma que tiene.

| | Qué | Dónde |
|---|-----|-------|
| **Inalcanzable** | `record()`, `stop_record()`, `play()`, `stop()`, `overdub()` y `clear()` **no tienen ningún llamador** fuera de los tests del propio módulo. El Looper aparece en la lista de ADD FX y se puede meter en una cadena, donde se queda en `Idle` para siempre. No hay botón, tecla ni target de MIDI-learn que llegue a su transporte. | `fx/looper.rs` |
| **Mandos muertos** | El panel dibuja `LENGTH`, `FEEDBACK` y `WET`. `Looper` sobreescribe únicamente `set_mix`; `set_param` es el no-op del trait, así que los dos primeros no mueven nada. `params()` tampoco está sobreescrito, de modo que el `org.choz.fx.looper` exportado publica una sola perilla — y, sin transporte, tampoco se puede grabar desde un host. | `source.rs:740`, `fx/looper.rs` |
| **Violación de RT** | `Cmd::Record` hace `self.buf = vec![0.0; new_cap * 2]` cuando el sample rate cambió desde la construcción: **11,7 MiB de allocación adentro de `process_block`**, en el hilo de audio. El header del módulo promete "no allocation after `new()`". Es un hueco esperando al primero que arme un loop después de cambiar la frecuencia del dispositivo. | `fx/looper.rs` |
| **Una sola pista** | `buf` es un búfer; `loop_frames` es un largo. No hay dónde poner una segunda toma. | `fx/looper.rs` |
| **32 s fijos** | `MAX_LOOP_SECS = 32`, allocados enteros al construir: 11,7 MiB por instancia a 48 kHz, se grabe un frame o ninguno. | `fx/looper.rs` |

Lo que **no** está mal: vivir en la cadena de FX. Ahí es donde tiene que estar.
Los estados `Idle → Recording → Playing`, los comandos aplicados en borde de
bloque y el `pending_cmd` se quedan como están; es la forma correcta y el
multipista la hereda.

## La restricción que decide el diseño

Cinco minutos de estéreo `f32` a 48 kHz son 14 400 000 frames: **109,9 MiB**.
Ocho de ésos son 879 MiB de audio residente, y a 96 kHz son 1,7 GiB. Allocar
eso por adelantado —como el looper de hoy alloca sus 32 segundos— es cómo el
OOM killer se lleva a choz en una laptop.

Cinco minutos de estéreo, por pista:

| Formato | 44,1 kHz | 48 kHz | 96 kHz | ×8 pistas @ 48k |
|---------|---------:|-------:|-------:|----------------:|
| `f32` intercalado (lo de hoy) | 100,9 MiB | 109,9 MiB | 219,7 MiB | **878,9 MiB** |
| `i16` intercalado | 50,5 MiB | 54,9 MiB | 109,9 MiB | 439,5 MiB |

Y es por tab: dos tabs con looper son dos decks. El presupuesto tiene que ser
**global al programa**, no por deck.

### La idea que lo sostiene

**"La primera pista define el límite" no es sólo una regla de forma musical: es
la estrategia de memoria.** Los cinco minutos son un techo sobre **la pista 1
sola**. En el momento en que la pista 1 deja de grabar, el largo del loop queda
congelado, y toda pista posterior es exactamente esa cantidad de frames: un
tamaño **conocido**, allocado una vez, fuera del hilo de audio, antes de armar
la pista.

Quien loopea una frase de ocho segundos paga 1,5 MiB por pista, no 55. Sólo la
pista 1 necesita crecer, y sólo ella necesita techo.

Así que la pista 1 graba en **trozos** de un segundo —192 KB a 48 kHz en
`i16`— entregados al hilo de audio por adelantado y devueltos cuando se llenan.
No se alloca nada en `process_block`, y no se paga nada hasta usarlo.

### El presupuesto, comprobado antes de armar

| Ajuste | Default | Qué hace |
|--------|--------:|----------|
| `loop_budget_mib` | 512 | Techo duro de **todos los decks juntos**. `REC` se niega y dice por qué. |
| `loop_max_secs` | 300 | Techo de la pista 1. La grabación se corta sola y cierra el loop. |
| `loop_tracks` | 8 | Tope fijo por deck, para que el array de pistas no realloque nunca en el hilo RT. |

## El puente de hilos que el trait no da

Éste es el problema real de que el looper viva en la cadena, y hay que
resolverlo antes de escribir DSP.

`FxProcessor::process_block(&mut self, buf, sample_rate)` es toda la firma. Un
procesador **no tiene handle al host, ni canal, ni forma de pedir memoria ni de
devolver nada**. Un looper que crece necesita las dos cosas.

Pero el patrón ya existe en el árbol. `AudioEngine::set_slot_fx` construye la
cadena en el hilo UI y, antes de mandarla al RT, le saca handles a cada
procesador:

```rust
// engine.rs — "Last chance to reach the processors: after this they
// belong to the RT thread."
self.fx_editors[slot]   = fx.iter().map(|p| p.editor()).collect();
self.fx_meters[slot]    = fx.iter().map(|p| p.meter()).collect();
self.fx_touches[slot]   = fx.iter().map(|p| p.param_touch()).collect();
```

El deck agrega uno más: `p.loopdeck()`, un `Option<LoopHandle>` que el UI se
queda. Adentro, el par de anillos `rtrb` que ya es el idioma de la casa:

```
  hilo UI                          hilo de audio                    hilo UI
┌──────────────────┐  vacíos  ┌────────────────────┐  llenos  ┌────────────────┐
│ alloca trozos si │ ───────▶ │ escribe frames     │ ───────▶ │ los recibe:    │
│ el presupuesto   │          │ nunca alloca,      │          │ los suelta, o  │
│ lo permite       │          │ nunca libera,      │          │ los escribe a  │
│                  │          │ nunca bloquea      │          │ WAV            │
└──────────────────┘          └────────────────────┘          └────────────────┘
```

El UI rellena la provisión en cada cuadro mientras se graba, que a 30 fps es de
sobra para un trozo de un segundo. Si la provisión se acaba —disco ocupado,
presupuesto agotado— el deck cierra el loop en vez de perder audio en silencio.

Es el mismo par que `engine.rs` ya tiene entre el rack y el RT (`rtrb` de
comandos hacia allá, `Retired` de vuelta), sólo que propiedad de un procesador.

---

## Las seis fases

Cada fase termina en algo entregable.

### 1 · Que se pueda grabar, y que sea honesto

*Lo más chico que hay, y arregla lo que ya está roto.*

Antes de multiplicar pistas, que la que hay funcione. Sin esto no hay forma de
probar nada de lo que sigue.

- Sacar la allocación de `Cmd::Record`: el tamaño se fija al construir, con el
  sample rate que `set_slot_fx` ya conoce.
- Sobreescribir `params()` y `set_param`, o borrar `LENGTH` y `FEEDBACK` de
  `fx_param_descs`. Un mando que no mueve nada es peor que no tener mando.
- Un `RackButton` de `REC` / `PLAY` / `STOP` en la caja SLOT cuando el efecto
  seleccionado es el looper, y `MouseAction` que lleguen a `EngineCommand`.
- **Comprobación:** armar, grabar, cerrar y oír el loop desde la interfaz, sin
  tocar un test.

Toca: `fx/looper.rs`, `source.rs`, `views/fx_chain_panel.rs`, `main.rs`,
`engine.rs`.

### 2 · El puente de hilos y la memoria acotada

*El diseño de memoria entero, probado todavía sobre una sola pista.*

- `LoopHandle` sacado en `set_slot_fx` como se sacan `meter()` y `editor()`.
- Anillo de trozos vacíos hacia el RT, anillo de llenos de vuelta.
- Grabación en trozos de un segundo, techo de 5 minutos, presupuesto global
  comprobado en el UI —que es el único que alloca, así que es el lugar natural.
- Llegar al techo cierra el loop y lo toca, en vez de envolver o pararse en
  seco.
- **Comprobación:** grabar 5 minutos de una rampa conocida, afirmar la cuenta de
  frames y que no hubo allocación en el callback.

Toca: `fx/looper.rs`, `engine.rs`, `choz-ports` (el handle en el trait).

### 3 · N pistas sobre la misma fuente

*Donde la regla del largo se gana el sueldo.*

El búfer único se vuelve un array de pistas. La pista 1 congela `loop_frames`;
toda pista posterior alloca exactamente eso, una vez.

- **Un solo playhead** para el deck: todas las pistas leen la misma posición, así
  no pueden derivar y no hay resincronización por pista que escribir.
- Cada pista tiene su propio estado —`Idle` / `Recording` / `Playing`— así que
  suenan juntas o no, que es lo pedido.
- Grabar una pista mientras otras suenan es lo normal, no un caso especial: se
  graba lo que **entra al slot**, y lo que sale es la entrada más las pistas que
  estén tocando. Lo que suena no se regraba, salvo que el usuario ponga el
  looper después de sí mismo, que no puede.
- Armar una pista que el presupuesto no cubre se niega y lo dice, antes de tocar
  audio.
- Borrar la pista 1 borra el largo del deck: lo próximo que se grabe lo define
  de nuevo.

Toca: `fx/looper.rs`, `engine.rs` (`EngineCommand`).

### 4 · Cuantizar, y el metrónomo que se arma con eso

*Necesita el transporte, que choz ya tiene.*

Todo lo necesario está en `choz_ports::transport()`: tempo, compás y posición.
Un compás en frames es `60/bpm × 4·num/den × sr` — a 120 BPM en 4/4, 96 000
frames, 2 s. Cuantizar redondea el largo del loop cerrado al múltiplo entero más
cercano, con piso de uno.

- **QUANT** cicla `OFF → 1 COMPÁS → 1 SEG`. El piso es el mínimo que se pide: un
  loop no puede cerrar más corto que una unidad.
- Con cuantización prendida, `REC` también **empieza** en el próximo borde y no
  en la tecla — si no, el final queda en la grilla y el principio no.
- **CLICK** es un toggle que prende el metrónomo cuando se arma la grabación y
  lo apaga al parar, sin tocar el ajuste de metrónomo del usuario.
- **Comprobación:** a 120 BPM en 4/4, un loop cerrado a 1,9 s cuantiza a 2,0 s;
  uno cerrado a 0,3 s queda en 2,0 s y no en cero.

Toca: `fx/looper.rs`, `metronome.rs`, `choz-ports` (`transport()`).

### 5 · Exportar: un WAV por pista

*Fuera del hilo de audio, con lo que ya está en el árbol.*

`hound` ya es dependencia y ya escribe WAV en `sources.rs`. Exportar toma los
búferes congelados por el anillo de vuelta, los escribe en el hilo UI, y no le
pide nada al callback.

- Un archivo por pista, nombrado por tab, pista y toma; el sample rate escrito
  es el que se grabó.
- Se escribe como se grabó —`i16` adentro, `i16` afuera— así exportar es una
  copia y no una conversión que pueda clipear.
- Un selector de directorio, reusando el browser que ya abre el guardar
  proyecto.
- Exportar con el deck rodando está permitido: los búferes congelados no son los
  que se están escribiendo.

Toca: `file_browser.rs`, `fx/looper.rs`, `hound`.

### 6 · El panel

*Último, porque recién ahora hay algo cierto que dibujar.*

Cuando el slot seleccionado es un looper, la caja SLOT crece a una fila por
pista: el transporte, el largo, y un medidor de llenado que es la respuesta
honesta a "cuánto presupuesto queda".

- Por pista: `REC` / `PLAY` / `STOP` / `CLR`, más mute y nivel.
- Del deck: `QUANT`, `CLICK`, `EXPORT`, y el largo que fijó la pista 1.
- Cada botón, target de MIDI-learn — un looper que necesita mouse es un looper
  que no se puede tocar. Es lo que más importa acá: el que graba tiene las dos
  manos ocupadas con la guitarra.
- Las cadenas nuevas pasan por `t()` con fila en `i18n.rs`; el test de la tabla
  ahora falla si no.

Toca: `views/fx_chain_panel.rs`, `main.rs`, `i18n.rs`.

---

## Cuatro decisiones antes de la fase 1

**¿El looper graba seco o mojado?** Graba lo que llega a su slot, así que
depende de dónde lo pongan. No hay que resolverlo con código —el usuario mueve
el slot— pero sí decirlo: si el reverb va antes, queda impreso en la toma para
siempre.
→ *Sugerido:* dejarlo como está y explicarlo en el hint del panel. Un mando de
"grabar pre/post" sería una segunda forma de decir lo que mover el slot ya dice.

**¿`i16` o `f32` en el búfer?** `i16` parte la memoria al medio y es como se
escribe el WAV igual. `f32` deja headroom para overdub, donde sumar repetidas
veces sobre un búfer de punto fijo acumula redondeo.
→ *Sugerido:* `i16`, y sin overdub en la v1. Si llega el overdub se lleva `f32`
con él y el presupuesto se duplica — que es una decisión de entonces, no de
ahora.

**¿El deck sigue al transporte de choz?** Atarlo al transporte hace que el loop
arranque con todo lo demás y que el secuenciador quede en fase. Correr libre
hace que el looper funcione con el transporte parado, que es como se comporta un
pedal.
→ *Sugerido:* seguir al transporte cuando rueda, libre cuando no — la regla que
`arp.rs` y `seq.rs` ya usan, así los tres coinciden en qué significa "a tiempo".

**¿Un loop entra al archivo de proyecto?** Una toma estéreo de cinco minutos son
55 MiB de audio que un YAML no tiene por qué cargar.
→ *Sugerido:* no. El proyecto guarda los ajustes del deck; el audio se exporta a
propósito. Cualquier otra cosa convierte "guardar proyecto" en una espera de
varios segundos.

## Fuera de alcance, a propósito

- **Overdub.** El looper de hoy lo tiene y nada puede alcanzarlo. Cambia el
  formato del búfer y duplica el presupuesto; merece su propia decisión. Con N
  pistas, además, apilar tomas es la respuesta a casi todo lo que el overdub
  resolvía.
- **Varispeed, reversa, media velocidad.** Divertido, y nada de eso es lo que se
  pidió.
- **Deshacer una toma.** Quiere un segundo búfer por pista, que es el problema de
  memoria otra vez con un nombre más lindo. Con N pistas se puede borrar una y
  grabarla de nuevo, que cubre el caso.
- **Importar WAV a una pista.** Primero exportar; importar es la misma cañería
  al revés y puede seguir después.
- **Un looper de mixer.** Grabar la suma de todo el rack es otro aparato, en otra
  altura, y no es esto.
- **El deck como plugin CLAP.** Los artifacts salieron así y esto también podría,
  pero un grabador sin extensión de estado perdería su audio en cada recarga.

---

## Fase 7 · Las tiras de canal (2026-08-26)

La fila por pista se volvió una **tira por canal**, que es como se lee un
looper de pedalera:

```
┌ CHANNEL 1 ─────────────────┐
│ IN LEVEL: -3.2 dB          │   ← medidor de lo que entra, no un mando
│────────────────────────────│
│ METRO   QUANTIZE 1 BAR     │
│────────────────────────────│
│ ■ STOP   ▶ PLAY   ● REC    │
└────────────────────────────┘
 ◀ 1/2 ▶   +   CLEAR   EXPORT   2.00s · 3 MiB / 512
```

- **Cuatro canales por defecto**, ocho como techo. El `+` agrega uno. Las tiras
  se **reparten el ancho**: entran las que quepan a 18 columnas mínimo y después
  se ensanchan para llenar la fila; si no entran todas aparecen `◀` `▶`. Con
  lugar escriben las palabras, apretadas caen a símbolos.
- **IN LEVEL es un monitor**, no un mando: el pico de lo que **llega** al deck,
  medido antes de sumarle lo que el deck toca —si no, el número subiría solo
  porque hay una toma sonando. Verde arriba de −12 dB, rojo arriba de −1.
- **METRO acciona el metrónomo general de choz.** No es un ajuste del deck: es
  el mismo clic que oye el resto del programa, contando el mismo
  `choz_ports::transport()` al que `QUANTIZE 1 COMPÁS` redondea. Un solo estado,
  mostrado en todas las tiras porque eso es lo que es.
- **STOP, PLAY y REC son tres botones.** `REC` arma y, en la segunda apretada,
  cierra la toma y la deja sonando —la pedalera de siempre.
- **PLAY es también PAUSE.** El deck tiene un solo cabezal, así que la
  diferencia está en qué pasa con él: una toma en pausa se calla y el loop sigue
  corriendo abajo, de modo que volver a apretar PLAY la devuelve **en tiempo con
  las demás**. STOP la entrega, y cuando no queda nada sosteniendo el cabezal el
  deck vuelve al principio. Eso es `LoopTrackState::Paused`, la cuarta posición
  de la perilla de transporte.
- **Todo es alcanzable de las tres formas.** Transporte y cuantización son
  **parámetros**, así que MIDI learn y un host llegan por `SetFxParam` como a
  cualquier otro efecto. Los que no son parámetro —`+`, las flechas de página,
  CLEAR y EXPORT— tienen su `TriggerAction`. Y el teclado camina la grilla
  entera: `←→` los canales, `↑↓` los botones de la tira y de la fila de abajo,
  Enter aprieta.

Los índices de los dos primeros bloques de parámetros no se movieron, así que un
proyecto guardado antes de esto abre con su transporte y sus mutes.

### El bug que impedía grabar

Nada de esto se podía probar hasta acá, y la causa estaba dos capas más abajo.

`build_chain_from_specs` envuelve **cada** efecto en `Metered`, y en `Gated` si
tiene gate. Los dos wrappers reenvían `editor()`, `meter()`, `state()`,
`param_touch()`, `sandbox()`… y **ninguno reenviaba `loopdeck()`**. Así que
`set_slot_fx` siempre recibía el `None` del trait: el extremo del anillo nunca
llegaba a la interfaz. El deck se podía agregar a una cadena, dibujar y apretar,
y no grababa nada, porque el lado que alloca los trozos no tenía con qué
alimentarlo — y el panel, sin estado publicado, caía a la grilla genérica de
ocho perillas `T1`…`T8`.

Al mirarlo apareció el segundo, de la misma familia: `build_chain_from_specs`
**filtraba** los efectos apagados y los que no cargaban, así que la cadena
construida quedaba más corta que la lista de specs. Pero todo lo que la interfaz
guarda está direccionado por posición —`fx_editors[slot][i]`, `fx_loopers`, el
`fx` de un `SetFxParam`—, que es el índice del efecto **como lo dibuja el rack**.
Apagar un efecto temprano corría un lugar a todos los que venían después, en
silencio. Ahora un spec apagado o que no carga deja un `Bypass` en su lugar: un
procesador por spec, siempre, y los índices no se mueven.

## Lo que no se hizo

- **Un fader por canal.** Estuvo un rato y se fue: lo que se pidió arriba de la
  tira es un **medidor** de entrada, no un mando. El mute sigue siendo un
  parámetro (`P_MUTE`), alcanzable por MIDI learn y por el host, pero ya no
  tiene botón en la tira.
- **Un banner de estado.** El resultado de EXPORT va al log (`choz: exported N
  take(s) to …`) porque el RACK no tiene línea de estado donde escribirlo.
  Marcado con `ponytail:` en `export_loops`.
- **Overdub, varispeed, deshacer, importar WAV.** Como estaba dicho abajo.

## Cómo quedó, en una línea por fase

1. **Grabable y honesto** — la allocación salió de `Cmd::Record` (el tamaño se
   fija al construir); `params()` y `set_param` son reales; el transporte se
   toca desde la caja LOOP del panel.
2. **Puente de hilos** — `FxProcessor::loopdeck()` entrega un `LoopHandle` en el
   mismo instante que `meter()` y `editor()`. Anillos `rtrb` de trozos vacíos
   hacia el RT y llenos de vuelta; `App::pump_loopers` los rellena por cuadro
   contra `loop_budget_mib` (512 por defecto, global al programa).
3. **N pistas** — ocho tomas, un solo playhead, la pista 1 congela el largo y
   las demás son exactamente eso.
4. **Cuantizar** — `Quantise::{Off, Bar, Second}` con piso de una unidad, como
   parámetro para que el proyecto lo guarde. `CLICK` prende el metrónomo al
   armar y devuelve el ajuste del usuario al parar.
5. **Exportar** — `export_track` escribe `i16` a `i16` con `hound`, en el hilo
   de la interfaz, desde los trozos que volvieron.
6. **Panel** — caja LOOP con `REC` / `PLAY` / `CLR` / `M` por pista y
   `QUANT` / `CLICK` / `EXPORT` abajo, en lugar de la grilla de mandos.

---

Cifras a estéreo, 300 s, 1 MiB = 1 048 576 bytes. El estado se leyó contra el
árbol de trabajo: `fx/looper.rs`, `fx_chain.rs`, `engine.rs`, `source.rs`,
`choz-ports/src/lib.rs`.

---

## Fase 8 · La tira como tira de canal (2026-08-27)

Lo que faltaba para que la tira fuera una tira y no una fila de botones, y el
bug que hacía que grabar no se oyera.

### El bug: REC no grababa nada audible

`REC` apretado por segunda vez manda `P_PLAY`, que llegaba a `Cmd::Play` →
`start_play`, y eso **ponía el estado en Playing sin cerrar la toma**. Cerrar es
lo que congela `loop_frames`, manda a casa el trozo a medio escribir y deja la
pista sonando; sin eso `loop_frames` quedaba en cero, la rama de reproducción
nunca corría, y apretar REC dos veces era silencio.

Arreglado donde entran todos los llamadores: `start_play` sobre una pista que
está grabando la cierra, venga de la interfaz, de un CC aprendido o de un host.

### Lo que la tira tiene ahora

```
┌ CHANNEL 1 ──────────────[×]┐
│ ▓▓▓▓█░░░  -6.2             │   ← RMS lleno, pico montado encima
│────────────────────────────│
│ ♩ METRO   QUANTIZE 1 BAR   │
│ MUTE   SOLO  L───●───R     │
│ LEVEL ████████ 100%        │
│────────────────────────────│
│ ■ STOP   ▶ PLAY   ● REC    │
└────────────────────────────┘
 + CHANNEL   CLEAR   EXPORT   2.00s · 3 MiB / 512
```

- **REC es rojo siempre** — apagado mientras espera, encendido mientras graba.
  Rojo es lo que el botón *es*, no lo que está haciendo.
- **`[×]` en la esquina** tira el canal: el deck corre los de arriba hacia abajo
  —audio, estado y ajustes juntos— con un `rotate_left`, que en el hilo de audio
  no pide un byte. `P_DEL` es el camino: un gesto escrito y devuelto a cero,
  porque los parámetros son la única ruta de la interfaz al procesador.
- **`+` está encendido**, no teñido. Un `+` gris verdoso sobre un botón gris era
  un botón que nadie veía.
- **QUANTIZE abre un modal** en vez de ciclar. El valor en la etiqueta es lo que
  crecía más allá de un panel angosto y rompía la fila; y una lista dice cuáles
  son las otras dos opciones, cosa que el ciclo nunca hizo. Angosto, **todos**
  los botones caen a símbolos a la vez.
- **METRO es `♩`**, el mismo glifo de la barra de arriba, porque es el mismo
  metrónomo.
- **MUTE y SOLO** por canal. Solo es un mute de todo lo demás, y sólo mientras
  hay algo en solo.
- **PAN y LEVEL** son sliders: donde cae el clic *es* el valor, como el pan del
  mixer. No están en el paseo del teclado —se agarran— pero son parámetros, así
  que MIDI learn y un host llegan igual.
- **El monitor es por canal**: RMS relleno y el pico como una celda encima. Un
  canal grabando muestra lo que entra; uno tocando, lo que sale. Con cuatro
  tiras al lado, eso contesta cuál es la que está haciendo ruido.

Los índices de `P_STATE`, `P_MUTE`, `P_QUANT` y `P_CHANS` no se movieron: los
bloques nuevos (`P_SOLO`, `P_PAN`, `P_DEL`, `P_VOL`) van después, así que un
proyecto guardado antes de esto abre con todo lo que escribió. Un `P_VOL` que no
está se lee como unidad, no como silencio.

La tira pasó de 7 filas a 9 (6 sin las reglas).

---

## Fase 9 · Símbolos, colores, y el deck que sobrevive al rebuild (2026-08-27)

### El bug: agregar un efecto borraba las tomas

Agregar un reverb a la cadena llama `set_slot_fx`, que **reconstruye todos los
procesadores desde sus specs**. Un deck construido desde un spec es un deck
vacío: todo lo grabado estaba en el procesador que se iba a tirar. Un delay que
pierde su cola en un rebuild es un sonido; un looper que pierde sus tomas son
minutos de tocar, perdidos porque alguien buscó otro efecto.

Ahora el deck **se lleva** de una cadena a la otra, emparejado por su posición
entre los decks de cada una — un `std::mem::swap` de dos `Box`, dos punteros, sin
allocar ni liberar nada, así que es seguro en el hilo de audio
(`RtState::carry_loop_decks`). El deck vacío se va con la cadena vieja y se
libera en el hilo de la interfaz, como todo lo que sale. Del lado de la interfaz
`set_slot_fx` se queda con el handle **viejo** donde hubo carry: es el que tiene
los trozos grabados y los anillos que ese procesador sigue sosteniendo.

`FxProcessor::is_loop_deck()` es lo que lo hace posible — `loopdeck()` no sirve,
porque el handle ya se entregó. Los wrappers `Metered` y `Gated` lo reenvían,
como con `loopdeck()`.

*ponytail:* sólo decks. El resto se sigue reconstruyendo, porque lo que pierden
es una cola y no el trabajo del que toca.

### La tira, sin una sola palabra adentro de un botón

```
┌ CHANNEL 1 ──────────────[×]┐
│ ▓▓▓▓█░░░  -6.2             │
│────────────────────────────│
│ ♩   Q   M   S              │
│ LEVEL ████████ 100%        │
│ L───●───R  C               │
│────────────────────────────│
│ ■   ▶   ●                  │
└────────────────────────────┘
 +   CLEAR   EXPORT
```

Las palabras eran lo que hacía que las filas dependieran del idioma y del ancho
que el panel tuviera. Un transporte de tres símbolos se lee más rápido que uno
de tres palabras, igual.

**El color es cómo un símbolo dice qué botón es** — `M` y `S` tienen la misma
forma; blanco y ámbar no. Cada uno con su gemelo apagado, así la tira se lee
igual haya algo activado o no:

| Botón | Color |
|-------|-------|
| `♩` metrónomo, `S` solo | ámbar |
| `Q` cuantizar | azul |
| `M` mute | blanco |
| `▶` play (`⏸` pausa en ámbar) | verde |
| `●` rec | rojo |
| `■` stop | gris |

El **paneo quedó debajo del volumen**, que es como se lee una tira: cuánto, y
después dónde. Los dos siguen siendo sliders — donde cae el clic *es* el valor.
`+` volvió a ser `+`, encendido.

Las cuatro llaves comparten una fila ahora que son símbolos, así que la tira
sigue midiendo 9 filas (7 sin reglas).
