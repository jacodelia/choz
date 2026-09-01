# Auditoría DSP de los efectos de choz

Escrito el 2026-08-27 contra el árbol de trabajo —45 efectos entonces, **56**
hoy, en `crates/choz-engine/src/fx/`— y ampliado después: la sección 6 son las
mediciones del 28, la 7 la auditoría de guardado del 29 (cerrada el 30) y la 8
los diez efectos que entraron el 2026-09-01.

**Está cerrado entero.** No queda ningún hallazgo sin resolver; lo único que se
midió y se decidió no arreglar está en la sección 6, con el porqué.

Esto **no** es una lista de deseos: cada hallazgo cita archivo y línea y dice qué
se oye. El roadmap de la sección 3 está ordenado por *daño audible por línea de
diff*, no por lo interesante que sea el DSP.

Restricciones respetadas en todo lo que sigue: sin samples, sin IR, sin archivos
de audio, sin circuitos propietarios, sin dependencias nuevas, sin romper la API
pública, sin allocations en `process_block`.

---

## 0 · Lo que ya está bien

Vale decirlo primero, porque marca el listón y porque tres de estos son los
patrones que hay que copiar al resto.

| | |
|---|---|
| `compressor.rs` | Knee cuadrática real, lookahead, detector RMS/peak conmutable, HPF de sidechain, stereo link continuo. Coeficientes por bloque, no por sample. Es calidad de plugin. |
| `parametric_eq.rs` | Biquads RBJ completos, con respuesta calculada para dibujar la curva. |
| `saturator.rs` | Oversampling 2×/4×/8× con `Oversampler`, DC blocker después de la curva asimétrica, tone. Hace lo correcto. |
| `pedal.rs` | Igual: `Oversampler2x` alrededor de cada `tanh`. |
| `filter.rs` | SVF topology-preserving (Simper). Estable en todo el rango, sin el colapso del Chamberlin. |
| `reverb.rs` | Reescrito el 2026-08-27. Es el patrón de referencia para smoothing, interpolación en lazo y decorrelación estéreo. |
| `oversample.rs`, `smooth.rs`, `lfo.rs` | Infraestructura compartida, ya escrita y ya probada. **El problema es que la mitad de los efectos no la usa.** |

Y una comprobación que pasa en todo el árbol: **ningún `process_block` alloca**,
con una excepción (F6). Se rompió una vez —tres de los efectos nuevos del
2026-09-01 copiaban el bloque para filtrarlo— y la auditoría del diff antes de
publicar la 1.3.6 la restauró; ver la sección 8.

---

## 1 · Hallazgos

### F1 · Búferes dimensionados en samples, no en tiempo — dependencia del sample rate

**Severidad: alta. El mismo proyecto suena distinto en otro dispositivo.**

```rust
// chorus.rs:4
const MAX_DELAY_SAMPLES: usize = 4096;
// flanger.rs:2
const MAX_FLANGER_SAMPLES: usize = 2048; // ~46ms at 44.1kHz
```

El comentario del flanger lo dice solo: 2048 samples son 46 ms **a 44.1 kHz**. A
96 kHz son 21 ms y a 192 kHz, 10.7 ms. Como los clamps de `delay_ms` y `depth`
se calculan contra esa capacidad:

```rust
// chorus.rs:56
let depth_s = (self.depth * sr / 1000.0).clamp(1.0, MAX_DELAY_SAMPLES as f32 / 2.0);
let base_s  = (self.delay_ms * sr / 1000.0).clamp(1.0, MAX_DELAY_SAMPLES as f32 - depth_s - 2.0);
```

…un chorus con `delay_ms = 30` y `depth = 10` a 44.1 kHz es lo que el usuario
pidió, y a 192 kHz es un flanger: el retardo base queda recortado a la mitad de
lo pedido y la modulación con él.

Lo mismo, con otra forma, en el bitcrusher: `hold` es una **cuenta de samples**,
así que la frecuencia de decimación —que es *todo* el efecto— escala con el
dispositivo. `hold = 8` es 6 kHz a 48 kHz y 24 kHz a 192 kHz: a rate alto el
efecto desaparece.

**Arreglo:** dimensionar los búferes por tiempo al rate máximo soportado
(192 kHz) en la construcción, y expresar la decimación en Hz. Ninguno de los dos
cambia la firma de `new()`.

### F2 · Interpolación lineal en un retardo modulado **dentro de un lazo de realimentación**

**Severidad: alta. Es la diferencia entre un flanger que resuena y uno que zumba.**

```rust
// chorus.rs:33, flanger.rs:33 — idénticos
buf[p0] * (1.0 - frac) + buf[p1] * frac
```

…y el resultado se realimenta:

```rust
// chorus.rs:74
self.buf_l[self.write_pos] = in_l + self.feedback * wet_l;
```

Una interpolación lineal es un FIR de dos taps, o sea un pasabajos: a medio
sample de offset está **3 dB abajo en fs/4**. Fuera de un lazo eso es un
suavizado; adentro se aplica en cada vuelta. Con el `feedback` de 0.9 que el
flanger permite, la resonancia queda sorda y —peor— *cambia de timbre con la
posición del LFO*, porque la fracción barre de 0 a 1 mientras la muesca se
mueve. Es exactamente el problema que se corrigió en el reverb el mismo día.

**Arreglo:** Catmull-Rom de cuatro puntos en el camino realimentado, que está una
décima de dB abajo en el mismo punto. Ya está escrito en `reverb.rs`; hay que
extraerlo a un módulo compartido.

### F3 · Sin smoothing en controles que mueven un puntero de lectura o un coeficiente

**Severidad: alta. Clicks y saltos de afinación con automatización.**

36 de 45 efectos no usan `Smoothed` nunca. Los que importan:

| Efecto | Parámetro | Qué se oye |
|---|---|---|
| `chorus`, `flanger` | `delay_ms`, `depth` | Salto del cabezal de lectura: click y escalón de afinación |
| `phaser` | `center` | Salto de la muesca |
| `filter.rs` | `set_cutoff` recalcula `g = tan(πf/sr)` de inmediato | Escalón en el coeficiente del SVF |
| `delay.rs` | `delay_ms` | Salto del cabezal |

`Smoothed` existe, está probado, y cuesta un multiply-add por sample.

### F4 · Trascendentales por sample donde alcanza un oscilador recursivo

**Severidad: media. No es un bug; es el presupuesto de CPU que paga la calidad.**

```rust
// chorus.rs:64  — dos sin() por frame
let lfo_l = (self.lfo_phase * TAU).sin();
let lfo_r = ((self.lfo_phase + 0.5) * TAU).sin();
// phaser.rs:66  — un sin() y un tan() por frame
let t = (PI * freq_clamped / sr).tan();
```

96 000 `sin()` por segundo en el chorus, y en el phaser un `tan()` por sample
sólo para mover una muesca a 0.4 Hz. `lfo.rs` ya tiene un LFO compartido, y una
cúbica suave en el wrap cuesta tres multiplicaciones.

### F5 · ~~Estéreo en contrafase → colapso en mono~~ — **retirado, era falso**

Se queda escrito porque el error de razonamiento es instructivo y porque la
medición vale más que el hallazgo.

```rust
// chorus.rs:66
let lfo_r = ((self.lfo_phase + 0.5) * TAU).sin(); // π phase offset
```

Medio ciclo es exactamente `−lfo_l`, así que los dos cabezales se mueven en
espejo. De ahí a "sumado a mono se cancela la modulación y el chorus se apaga"
hay un paso, y el paso está mal: **lo que se cancela son los dos valores del
LFO, no el audio que producen.** Un retardo no es una función lineal de su
tiempo de retardo, así que `d(base+m) + d(base−m)` no es `2·d(base)`; el
fold-down da el promedio de dos copias con retardos distintos, que se mueve
igual que un par en cuadratura.

Medido lado a lado con el mismo ruido, el mismo LFO y el mismo búfer:

```
anti-phase  : movimiento en mono 0.68445, wet rms 0.52123, ratio 1.313
quadrature  : movimiento en mono 0.68498, wet rms 0.52123, ratio 1.314
```

Una décima de por mil. **No hay bug.** El offset se dejó donde estaba en vez de
cambiarle el carácter estéreo al efecto sobre un diagnóstico equivocado, y el
test que se escribió para "demostrar" el arreglo se quedó como lo que sí es:
una propiedad que conviene fijar —el chorus se sigue moviendo en mono— con el
número de la medición en el comentario.

### F6 · Una allocation en el hilo de audio

```rust
// space_echo.rs:221
if sample_rate != self.sample_rate {
    *self = SpaceEcho::new(sample_rate, ...);   // ← alloca varios búferes
}
```

Pasa una sola vez, en un cambio de dispositivo, pero es el hilo de audio y la
regla del `choz-ports` es explícita. El resto del árbol ya hace lo correcto
(reconfigurar sin reallocar).

### F7 · Denormales sin flush en caminos realimentados

`chorus.rs`, `flanger.rs`, `phaser.rs` no tienen ninguna guarda. Una cola que
decae en un lazo termina en denormales, y un denormal cuesta hasta cien veces un
multiply normal en x86: el efecto se vuelve **más caro cuando se queda en
silencio**, que es cuando el host menos lo espera.

### F8 · Sin bloqueo de DC después de no-linealidades asimétricas

Sólo `saturator.rs` y `utility.rs` tienen bloqueador de DC. `cassette.rs` y
`vinyl.rs` aplican curvas y ruido sin él; el offset que dejan se suma a lo largo
de una cadena de efectos y se come headroom.

### F9 · Realimentación sin límite blando

El chorus permite `feedback` hasta ±0.9 y el flanger ±0.95 sobre un retardo
corto. Con material correlacionado eso es resonancia cerca de la unidad y no hay
nada que la doble: el `soft_limit` del reverb existe y es transparente por debajo
de la rodilla.

---

## 2 · Cambios arquitectónicos

Cuatro, y los cuatro son **infraestructura compartida**: cada uno arregla varios
efectos a la vez, que es la única forma de tocar 45 archivos sin romper nada.

### A · `fx/delay_line.rs` — una línea de retardo, para todos

Nota de memoria: dimensionar por tiempo al rate máximo cuesta RAM en rates
bajos. Un chorus o un flanger pagan 128 KB; el space echo, con sus dos segundos
de cinta estéreo, pasa de 768 KB a 3 MB por instancia. Es el precio de que
`process_block` no alloque nunca y de que el efecto suene igual en todos lados.


Extraer de `reverb.rs` lo que ya funciona:

- búfer potencia de dos (wrap por `AND`), dimensionado **por tiempo al rate
  máximo**;
- lectura fraccionaria lineal (fuera de lazos) y Catmull-Rom (dentro);
- flush de denormales/NaN en la escritura.

Arregla F1, F2 y F7 en chorus, flanger, y queda disponible para delay,
gran_delay, space_echo y beat_repeat.

### B · Un `safe()` compartido

El flush que hoy vive dentro de `reverb.rs` sube a `fx/mod.rs`. Un `if` que cubre
NaN, infinito y denormal a la vez.

### C · Smoothing por defecto en lo que se mueve

Los efectos con campos `pub` (chorus, flanger, phaser) leen sus campos al empezar
el bloque y los empujan a un `Smoothed`. **No cambia la API**: los campos siguen
siendo públicos y `fx_chain.rs` los sigue escribiendo igual.

### D · Osciladores baratos

Una cúbica C¹ en el wrap en lugar de `sin()`, y coeficientes de filtro calculados
por bloque desde un valor suavizado en lugar de un `tan()` por sample.

---

## 3 · Roadmap

Ordenado por daño audible. Cada fase entrega algo probado.

| Fase | Qué | Efectos | Estado |
|---|---|---|---|
| **1** | `delay_line.rs` compartido + `safe()` compartido | infraestructura | **hecho** |
| **2** | Chorus: rate-independence, cúbica en el lazo, smoothing, límite blando, flush | `chorus` | **hecho** |
| **3** | Flanger: lo mismo | `flanger` | **hecho** |
| **4** | Phaser: LFO cúbico, smoothing del centro y de la profundidad, flush | `phaser` | **hecho** |
| **5** | Bitcrusher: decimación en Hz | `bitcrusher` | **hecho** |
| **6** | Space Echo: reconfigurar sin allocar | `space_echo` | **hecho** |
| **7** | DC blocker después de las curvas de cinta y vinilo | `cassette`, `vinyl` | **hecho** |
| **8** | Smoothing del cutoff del SVF y del tiempo del delay | `filter`, `delay` | **hecho** |
| **9** | `delay_line.rs` en `delay` y `gran_delay`; `beat_repeat` no lleva línea, se le arregló el techo en samples | 3 efectos | **hecho** |
| **10** | Auditoría de aliasing: portadora del vocoder band-limitada, lectura cúbica y ventana en tiempo en el shifter, oscilador recursivo en el freq shifter | `vocoder`, `shift`, `freq_shift` | **hecho** |
| **11** | Ley de mezcla unificada. **Medido**: 44 de 46 ya hacían lo mismo; el delay sumaba y ahora cruza, el looper suma a propósito | `delay`, y la ley escrita en el trait | **hecho** |

Las fases 1–6 son el cambio del 2026-08-27 y las 7 a 11 el del 28. **La
auditoría está cerrada entera.** Cada fase quedó escrita acá porque está
medida, no porque suene bien: cada una tiene su hallazgo arriba, y lo que se
midió y se decidió **no** cambiar está dicho en el changelog junto a lo que sí.

---

## 4 · Qué se midió

Todo lo de arriba se comprueba con tests, no con el oído:

| Test | Dónde | Qué falla si el bug vuelve |
|---|---|---|
| `the_delay_line_is_the_same_time_at_every_rate` | `delay_line` | F1 |
| `a_cubic_read_keeps_the_top_end_a_linear_one_loses` | `delay_line` | F2, con las cifras en dB/pasada |
| `the_feedback_path_reaches_true_silence` | `delay_line` | F7 |
| `nothing_that_is_not_a_number_survives_a_write` | `delay_line` | NaN/inf circulando |
| `the_soft_clip_is_transparent_below_its_knee` | `delay_line` | F9 sin colorear lo que no hacía falta |
| `the_wobble_is_bounded_and_smooth_across_the_wrap` | `delay_line` | F4 (pendiente continua en el wrap) |
| `the_chorus_still_moves_in_mono` | `chorus` | propiedad, no F5 (ver arriba) |
| `the_chorus_is_the_same_at_every_sample_rate` | `chorus` | F1 |
| `moving_the_delay_does_not_click` | `chorus`, `flanger` | F3 |
| `the_feedback_is_bounded_and_reaches_silence` | `chorus`, `flanger` | F7 + F9 |
| `the_resonance_keeps_its_top_end` | `flanger` | **F2** |
| `the_flanger_still_moves_in_mono` | `flanger` | la misma propiedad |
| `the_flanger_is_the_same_at_every_sample_rate` | `flanger` | F1 |
| `moving_the_sweep_does_not_click` | `phaser` | F3, centro **y** profundidad |
| `the_feedback_reaches_true_silence` | `phaser` | F7 |
| `the_crush_rate_is_the_same_hz_at_every_sample_rate` | `bitcrusher` | F1 |
| `set_hold_still_holds_that_many_frames` | `bitcrusher` | la API pública sigue significando lo mismo |
| `a_rate_change_does_not_allocate` | `space_echo` | **F6** |
| `the_spring_follows_the_sample_rate` | `space_echo` | que `retune` haga lo que hacía reconstruir |

---

## 5 · Lo que **no** cambió, a propósito

- **La API pública.** `Chorus`, `Flanger` y `Phaser` conservan sus campos `pub`
  y sus firmas de `new()`; `fx_chain.rs` los sigue escribiendo igual. El
  smoothing se enganchó leyendo esos campos una vez por bloque, no cambiándolos
  por setters.
- **`Bitcrusher::set_hold`** sigue existiendo y sigue significando "sostener N
  frames" al rate en que se lo llama. Lo que cambió es que ese número se guarda
  como los hertz que representa.
- **El aliasing del bitcrusher.** Es el efecto, no un defecto. Un crusher que
  resolviera sus propias imágenes sería un pasabajos con pasos de más.
- **El `tan()` por sample del phaser.** Se evaluó moverlo a por bloque y se
  descartó: a diferencia de la distancia de lectura del chorus, la posición de
  la muesca *es* el sonido, y un coeficiente por bloque escalona el barrido al
  ritmo del búfer — audible en barridos rápidos, y haría que el efecto dependa
  del tamaño de bloque del host. Lo que sí se sacó fue el `sin()`.
- **`compressor`, `parametric_eq`, `saturator`, `pedal`, `filter`.** Están bien.
  Tocarlos habría sido cambio sin beneficio audible.
- **El indexado sin chequear.** `delay_line.rs` usó `get_unchecked` un rato: es
  el lazo más caliente del crate y todos los índices están enmascarados por
  construcción. Medido contra el benchmark del reverb —33 lecturas por frame
  sobre ocho líneas realimentadas— compró **0.018 % de un core** (1.340 %
  contra 1.358 %). Cuatro bloques `unsafe` en un camino de realimentación no
  valen una cincuentava parte de un por ciento.
- **El offset de LFO del chorus.** F5 se investigó, se midió y resultó falso.
  Cambiarlo igual habría sido cambiarle el carácter estéreo al efecto para
  arreglar algo que no estaba roto.

---

## 6 · Lo que se midió el 2026-08-28, y lo que no se arregló

**Antes de todo, la ley.** La mezcla de un efecto vive documentada en
`FxProcessor::set_mix` y es `out = dry + wet·(procesado − dry)` — un crossfade,
así que `0` es un cable y `1` es el efecto sin nada de lo que entró. No es
`dry + wet·procesado`, que es un nivel de envío: con eso, subir el mando sólo
puede hacer el tab más fuerte y nunca saca el dry, y el mismo mando significaría
dos cosas distintas según el efecto. **La única excepción a propósito es el
looper**, que suma: sus tomas suenan *debajo* de lo que se está tocando, y un
crossfade bajaría el instrumento en vivo a medida que suben los loops.


Las fases 10 y 11 midieron dos cosas que no son hallazgos de un efecto sino de
la **suite entera**, y quedaron fuera de las tablas de arriba porque este
documento se escribió el 27. Van acá porque son lo que hay que saber antes de
tocar un efecto, y porque una de ellas es lo único de toda la auditoría que se
midió y **no** se arregló.

### Los efectos se apilan sin saturar, y hay un test que lo sostiene

`no_built_in_effect_is_a_gain_stage` (en `choz-ui`) recorre los 46 con los
mandos que el rack les da, sobre dos segundos de tono más ruido:

- **ninguno suma más de 4,5 dB solo**, y
- **los ocho más fuertes apilados no pasan de 6**.

Antes de eso, `protocosmos` sumaba **9,1 dB él solo** —clipeaba por sí mismo
desde una entrada a −8,7 dBFS— y 46 de los 2 070 pares posibles pasaban de
escala; los 46 lo tenían a él adentro. `measure_stacking` (ignorado, al lado)
imprime la tabla entera cuando hace falta mirarla.

**Lo que el test no cubre, a propósito**: con la entrada a −2,7 dBFS, 57 pares
siguen pasando de escala, el peor a 1,34. Eso ya no es un efecto que amplifique
—son 2,7 dB de headroom y cualquier cadena se los come—, así que se resuelve en
el fader del tab y no en el DSP.

### La dispersión de nivel del wet no es la ley de mezcla

Con `Wet` a media posición el nivel va de **+2,9 dB** en protocosmos a
**−9,0** en el shimmer. Eso **no** es la ley de `set_mix` fallando —los 46 hacen
`out = dry + wet·(procesado − dry)`, medido— sino el nivel del wet de cada
efecto. Emparejarlos querría medir cada uno contra un programa real y no contra
ruido, que es otra auditoría. `examples/mix_probe` imprime la tabla.

### Lo único que se midió y no se arregló: el peine del shifter de voces

`fx::shift::VoiceShifter` suma dos cabezales de lectura a **media ventana** de
distancia. Sumar una nota aguda con una copia retardada de sí misma la peina, y
eso pasa haga lo que haga el interpolador: la fase 10 pasó la lectura a cúbica y
una nota de 14 kHz mejoró de 3,6 dB a **2,1 dB** abajo, que es el peine, no la
interpolación. `a_high_note_comes_through_the_read` (en `fx/shift.rs`) lo fija en
−2,8 dB para que no empeore.

Sacarlo pide **otro shifter**, no otra lectura — uno de fase vocoder, o uno que
sincronice los cabezales con el período detectado como hace
`autotune::shifter::RetuneShifter`. Es un efecto nuevo, no un arreglo, y por eso
no se hizo: el shifter de voces es el que usan el shimmer y el harmonizador, y
ahí la copia retardada es parte del sonido.

---

## 7 · Auditoría de guardado (2026-08-29)

Pregunta distinta de la del resto del documento: no *cómo suena* un efecto sino
**si vuelve a sonar igual** cuando se reabre el proyecto. Recorrida efecto por
efecto, con el formato de proyecto delante.

### Cómo se guarda un efecto

Un efecto en el YAML es `kind`, `enabled`, `wet`, `params`, y —si es un plugin
hosteado— `plugin_path` + `plugin_id` + `state` en base64. Los `params` son el
vector que dibuja el rack (`fx_param_descs`), aplicados por **índice**. De ahí
sale la regla: *todo lo que el rack dibuja se guarda; nada más se guarda*.

### Qué encontró la recorrida

| | |
|---|---|
| **44 de los 46 efectos: completos** | Todo su estado editable son parámetros. No hay un solo efecto propio que cargue archivos (ni IR, ni samples, ni presets en disco), así que no hay rutas que perder. |
| **Los que tienen preset interno** (`autotune`, `graphiceq`, `waveshaper`, `pedal`) | El preset **es** un parámetro más y se guarda como tal; y como los mandos se aplican en orden de índice, un knob debajo del preset lo pisa — por eso los descs del auto-tune escriben los valores del preset explícitamente. |
| **Los de tiempo** (`delay`, `grandelay`, `space_echo`, `reverse`, `shimmer`, `beat_repeat`) | Sin estado oculto: tiempo, realimentación, damping, ping-pong, cruce y modulación son parámetros. Lo que hay dentro de la línea de retardo es **audio en vuelo**, no ajustes: se pierde al reabrir y así tiene que ser (un delay que reabre con la cola de la sesión anterior es un delay con ruido de otra persona). |
| **El looper: era el único agujero real** | Sus 50 parámetros llevan estado, mute, solo, pan, volumen, cuantización y cantidad de canales — todo eso se guardaba. Lo que no se guardaba eran **las tomas**: minutos de audio que no caben en ningún parámetro. Cerrado el 2026-08-29. |

### Cómo se guarda un looper ahora

- Al guardar, cada canal con audio se escribe como WAV estéreo `i16` en
  `<proyecto>.loops/tab<N>-fx<M>-ch<K>.wav` — el mismo `export_track` que ya
  usaba el botón EXPORT, así que no hay un segundo camino que mantener. Los
  archivos que el proyecto ya no nombra se borran en el mismo guardado.
- El YAML nombra cada toma en `fx.loops` con su ruta **relativa al proyecto**,
  la longitud del loop en frames y la frecuencia a la que se grabó. Mover un
  proyecto es mover el `.yml` y su directorio.
- Al abrir, `looper::import_track` vuelve a cortar el WAV en chunks del deck
  actual y `FxProcessor::load_loops` los mete en el deck **antes** de que salga
  al hilo de audio — el único instante en que alguien que no sea el callback
  puede tocarlo. La otra punta (`LoopHandle::adopt`) recibe los mismos `Arc`,
  que es lo que hace que EXPORT siga escribiendo lo que hay y que el presupuesto
  de memoria los cuente.
- **Vuelven en PAUSE, no en PLAY.** El estado del canal es un parámetro y viene
  con los demás, así que un deck que estaba rodando arrancaría solo al abrir el
  proyecto: un rack que hace ruido antes de que nadie se lo pida. PAUSE conserva
  la toma y la longitud del loop, y espera.
- **Una toma grabada a otra frecuencia se resamplea** (2026-08-30).
  `import_track` lee la cabecera del WAV y saca la frecuencia del deck del
  propio `chunk_frames` —que es `LOOP_CHUNK_SECS` segundos a esa frecuencia—,
  así que una toma de 44,1 kHz abierta en un equipo a 48 conserva su afinación
  y su duración en segundos en vez de sonar casi un tono arriba y un 8 % más
  corta. `read_loops` escala también el largo del loop, que se guardó en frames
  de la frecuencia de grabación: compararlo tal cual cortaba el loop antes de
  tiempo, cada vuelta. La interpolación es lineal —sirve para 44,1 → 48, que es
  el caso real; una toma traída de 8 kHz va a sonar apagada—.

### El otro agujero: los efectos como plugin (cerrado el 2026-08-30)

`FxProcessor::params()` es lo que publica el `.clap` exportado, y **22 de los 46
efectos devolvían una lista vacía**: en un DAW aparecían como cajas sin mandos
que mover, automatizar ni guardar. Dentro de choz estaban completos, porque el
rack usa su propia tabla de descriptores. `graphiceq` y `autotune` publicaban
listas *incompletas* (les faltaban PRESET/WET), que es peor que vacías porque
los índices dejan de coincidir con los del rack.

**Los 46 publican su lista entera, en el orden que dibuja el rack.** Escribir
esas listas obligó a escribir el `set_param` que faltaba, y ahí aparecieron
cinco mandos que no llegaban a ninguna parte —cuatro efectos que
`build_processor` construía con `::new()` sin leer sus `params` (`filterbank`,
`isolator`, `cassette`, `sidechain`), y el `Wet` de `gain`, que se guardaba y no
se usaba— y dos mandos del `grandelay` con el nombre cambiado. Tres más
publicaban una constante en vez de su posición (`Preset` del graphic EQ, `Size`
y `Width` del shimmer): el mando andaba, lo que no andaba era preguntarle dónde
estaba, que es lo que un host hace para guardar.

Dos tests lo sostienen, y hay que mantener los dos verdes al tocar un efecto:

- `a_published_parameter_list_matches_the_knobs_the_rack_draws` — ya sin techo:
  cero efectos mudos, y la cuenta sólo puede bajar.
- `every_live_knob_reaches_the_processor` — lleva cada mando de cada efecto que
  toma valores en vivo a los dos extremos de su recorrido y exige que lo
  publicado se mueva. Es lo que habría cazado el `Drive` de la cassette el día
  que se escribió. Se saltan dos nombres: `Wet`, que entra por `FX_MIX_PARAM`, y
  `Preset`, que `AudioFxEntry::apply_preset` resuelve en los mandos de abajo.

**Compatibilidad, dicha una vez**: `filterbank`, `isolator`, `cassette` y
`sidechain` se construían descartando sus `params`, así que sonaban siempre en
los valores de su `new()`. Ahora leen lo que el proyecto guardó. En los dos
primeros el defecto es neutro y no cambia nada; en los otros dos se corrieron los
defectos de los descriptores para que un efecto agregado hoy suene como sonaba
—`Drive` 0,40 → 0,20 y `Release` 0,30 → 0,14—, pero **un proyecto guardado antes
del 2026-08-30 con una cassette o un ducker adentro suena distinto al
reabrirlo**. No hay forma de evitarlo: el mando que guardó nunca hizo nada, y
ahora hace lo que dice.

Como consecuencia, **doce efectos dejaron de reconstruir la cadena** en cada
giro de mando (`takes_live_params`): una reconstrucción reemplaza todos los
procesadores del slot, así que mover el `Drive` de un saturador tiraba la cola
del reverb que estaba detrás. El único que sigue reconstruyéndose es el LOOPER,
que es un deck y no un juego de mandos.


---

## 8 · Los diez efectos del 2026-09-01

Pedido: auditar los efectos de choz contra [`oximedia-effects`](https://docs.rs/oximedia-effects/)
e implementar como built-in lo que faltara. De los 56 que hay hoy, éstos son los
que entraron ese día, con lo que hace falta saber de cada uno.

| Efecto | id | Topología | Reusa |
|---|---|---|---|
| Pitch Shifter | `pitchshifter` | Dos cabezas cruzadas, una por canal | `shift::VoiceShifter` |
| Vibrato | `vibrato` | Línea leída por una cabeza que el LFO mueve | `delay_line`, `lfo` |
| Multi-tap Delay | `multitap` | Cuatro tomas de una línea; la última realimenta | `delay_line` |
| Plate Reverb | `platereverb` | Tanque de Dattorro (JAES 1997) | `delay_line` |
| Moog Ladder | `moogladder` | Cuatro polos ZDF, realimentación con `tanh` | `smooth` |
| De-esser | `deesser` | Detector sobre la banda alta, ganancia sobre esa banda | `filter::Svf` |
| Transient Shaper | `transient` | Dos seguidores a distinta velocidad | — |
| Multiband Comp | `multiband` | Linkwitz-Riley de 4º orden, tres bandas | `filter::Svf` |
| Exciter | `exciter` | Armónicos de la banda alta, sumados debajo | `filter::Svf` |
| Bass Enhancer | `bassenhance` | Armónicos del grave **sin** el fundamental | `filter::Svf` |

### Lo que las mediciones corrigieron

**Un pasa-altos y un pasa-bajos al mismo corte no reconstruyen la señal.** El
de-esser dividía así y un tono de 8 kHz salía **más fuerte** que como entró: la
diferencia de fase entre las dos ramas suma en vez de restar. La división por
sustracción (`alto := x − lowpass(x)`) reconstruye exacto por construcción, y es
lo que usa ahora. Donde hace falta separación de verdad —el exciter, el guard
del bass enhancer— va un paso real de 24 dB/octava.

**`Svf::new(mode, hz, 0.0)` no es Butterworth.** `k = 2 − 2·resonance` y
`Q = 1/k`, así que en 0 la sección está críticamente amortiguada (Q 0,5). Dos de
ésas en cascada **no** son un Linkwitz-Riley: medido, las bandas del multiband
volvían 2,4 dB abajo en el cruce. La constante es
`BUTTERWORTH = 1 − √2/2`, y está escrita en los dos archivos que la usan.

**El tanque de Dattorro tiene dos líneas por mitad, y las tomas van en la
primera.** Tapeadas en la segunda, el plate quedaba mudo 150 ms —lo que tarda la
primera en llenarse— y después llegaba todo junto. Un plate es denso desde el
primer milisegundo; eso es lo que lo distingue de una sala.

**El `SvfMode::Allpass` es nuevo y existe por el crossover.** La banda que se
salta un cruce necesita el all-pass del otro para volver en fase; sin él las
tres bandas recombinan con error de fase, que es un peine en medio de la mezcla.
Nadie pide un all-pass como efecto, y por eso no está en la lista de mandos de
`filter.rs`.

### La regla que se rompió y se restauró

Los tres efectos que necesitan una copia del bloque para filtrarla —de-esser,
multiband, exciter/bass— la hacían con `buf.to_vec()` **dentro de
`process_block`**: un `malloc` por bloque en el hilo de audio. Lo cazó la
auditoría del diff antes de publicar la 1.3.6, con la suite entera en verde: no
hay test que lo cubra, y escribir uno pide un allocador propio. El patrón que
quedó, y el que hay que copiar:

```rust
const SCRATCH: usize = 8192;       // frames estéreo, reservados en el `new()`
if self.low.len() < buf.len() {    // sólo si el host agranda el bloque
    self.low.resize(buf.len(), 0.0);
}
```

### Lo que se decidió no implementar

| | Por qué |
|---|---|
| Reverb por convolución | Es una función, no un efecto: cargar IRs de disco, un picker y convolución particionada por FFT. |
| Time stretch | No es un insert en vivo — cambiar el tempo sin cambiar el tono necesita material con principio y fin. Su lugar es el looper. |
| Medidor LUFS | Es un medidor. Los mandos de un `FxProcessor` son entradas, no lecturas. |
| Formantes en el armonizador | Cambiaría el sonido de algo que ya funciona. Iría como modo nuevo, no como cambio. |
| Ping-pong, lookahead limiter | Ya existen: el 4º mando del DELAY y `Compressor::limiter`. |
