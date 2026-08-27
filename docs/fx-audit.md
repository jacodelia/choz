# Auditoría DSP de los efectos de choz

Escrito el 2026-08-27, contra el árbol de trabajo. 45 efectos, 19 400 líneas en
`crates/choz-engine/src/fx/`.

Esto **no** es una lista de deseos: cada hallazgo cita archivo y línea y dice qué
se oye. El roadmap al final está ordenado por *daño audible por línea de diff*,
no por lo interesante que sea el DSP.

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
| `reverb.rs` | Reescrito el 2026-08-27 — ver [reverb.md](reverb.md). Es el patrón de referencia para smoothing, interpolación en lazo y decorrelación estéreo. |
| `oversample.rs`, `smooth.rs`, `lfo.rs` | Infraestructura compartida, ya escrita y ya probada. **El problema es que la mitad de los efectos no la usa.** |

Y una comprobación que pasa en todo el árbol: **ningún `process_block` alloca**,
con una excepción (F6).

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
| 7 | DC blocker después de las curvas de cinta y vinilo | `cassette`, `vinyl` | pendiente |
| 8 | Smoothing del cutoff del SVF y del tiempo del delay | `filter`, `delay` | pendiente |
| 9 | `delay_line.rs` en `delay`, `gran_delay`, `beat_repeat` | 3 efectos | pendiente |
| 10 | Auditoría de aliasing en `harmonizer`, `freq_shift`, `vocoder` | 3 efectos | pendiente |
| 11 | Ley de mezcla y ganancia unificada (hoy cada efecto inventa la suya) | todos | pendiente |

Las fases 1–6 son este cambio. Las 7–11 quedan escritas acá porque están
medidas, no porque suenen bien: cada una tiene su hallazgo arriba.

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
