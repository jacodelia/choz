# El reverb de choz — arquitectura

Reemplazo del banco Schroeder derivado de Freeverb que vivía en
`fx/reverb.rs`. Escrito el 2026-08-27. El código es original; lo que se toma de
la literatura son principios publicados (Schroeder 1962, Jot & Chaigne 1991),
no coeficientes ni estructuras de ningún producto.

Cómo encaja en el resto está en [architecture.md](architecture.md).

---

## Por qué no alcanzaba con más combs

El motor anterior eran 8 combs en paralelo y 4 allpass en serie, por canal.

Un comb es un **resonador**: su cola es un acorde de las frecuencias múltiplo de
`1/T`. Ocho combs son ocho acordes. Eso *es* el timbre metálico, y agregar más
combs agrega más acordes — el problema no se diluye, se apila. Los otros cuatro
síntomas venían del mismo sitio:

| Síntoma | Causa |
|---|---|
| `input * 0.015` | Ocho resonadores en paralelo tienen una ganancia resonante que nadie calculó, así que se bajaba la entrada hasta que dejaba de explotar. |
| `room_size → feedback` | El decay no era un tiempo; era un número de realimentación que sonaba largo. `room_size = 1.0` quedaba a 0.98, al borde. |
| Estéreo | `tuning + 23 samples` en el canal derecho. Dos veces la misma señal, corrida. |
| Sin early reflections | Lo primero que salía era el primer eco de los combs. Un comb no es una pared. |

Lo que **no** estaba mal y se conserva: vivir en la cadena de FX, y la mezcla
mid/side de ancho.

---

## La cadena

```text
in ─▶ send mono ─▶ pre-delay ─┬─▶ early reflections ──────────────────┐
                              │                                       │
                              └─▶ difusión de entrada ─▶ FDN ─▶ late ─┤
                                                                      ▼
 dry ────────────────────────────────────────────▶ + ◀── ancho ◀ tono/cortes
```

El dry sale exactamente como entró. Lo único que se le suma es el wet.

### 1 · Send mono

Un campo reverberante es difuso cuando llega al oído: dónde estaba la fuente en
la imagen estéreo sobrevive en el dry, no en la cola. Mandar mono es lo que hace
que el estéreo del wet sea **el de la sala**, decorrelacionado por la red, y no
el que venía de la entrada.

### 2 · Pre-delay

0–250 ms, **fuera** del lazo de realimentación. Un solo búfer: el tap del
pre-delay y cada reflexión leen de él a distancias distintas, así que una
reflexión cuesta una lectura y no una línea de retardo.

### 3 · Early reflections

Ocho taps por lado (cuatro en Economy), entre 9 y 91 ms escalados por `Size`.

Una primera reflexión llega a `2d/c` para una superficie a `d` metros; una sala
de unos pocos metros pone sus primeras llegadas entre ~8 y ~90 ms. Ese rango
**es** el tamaño de la sala y es lo que el oído lee como tal.

- Separados de a más de ~4 ms: más cerca se funden en una sola llegada con un
  notch de comb en el medio.
- **Los dos lados nunca coinciden.** Ninguna reflexión llega a los dos oídos a
  la vez; ahí está toda la imagen estéreo de esta etapa.
- Ganancia `(t₀/t_k)^0.65`: un poco menos que el `1/r` de la expansión esférica,
  para que las llegadas tardías sigan contando.
- Polaridades dispersas: un set de llegadas todas del mismo signo hace comb.

### 4 · Difusión

Cadena de allpass de Schroeder — magnitud exactamente plana, fase dispersada.
Es lo que convierte llegadas discretas en un manchón **sin colorear**: por
muchos que se encadenen el espectro no cambia, sólo se emborrona el tiempo.

Cuatro etapas a la entrada (2 en Economy) y dos a la salida por lado (1). El
coeficiente es el control `Diffusion`, entre 0.35 y 0.77 — arriba de ~0.8 la
cadena empieza a resonar en su propio retardo y deja de ser transparente.

### 5 · La FDN

`N` líneas de retardo cuyas salidas se mezclan con una matriz **ortogonal** y se
realimentan.

Ortogonal es todo el punto: es una rotación, mueve energía entre las líneas sin
crearla ni destruirla. El decay queda entonces determinado **sólo** por las
ganancias por línea — que es lo que hace posible un RT60 explícito y lo que hace
la red estable por construcción, en vez de por recortar la entrada hasta que se
porta bien.

**La matriz** es una reflexión de Householder, `I − (2/N)·11ᵀ`, compuesta con una
diagonal de ±1 y una rotación del orden de las líneas. Cada factor es ortogonal,
así que el producto lo es. La reflexión sola es una involución (`H² = I`); los
otros dos factores son lo que impide que la red deshaga su propia mezcla cada
dos pasadas. Cuesta una suma y un multiply-add por línea, no `N²`.

Hay un test que lo verifica como propiedad: la etapa de mezcla, con las
ganancias en 1, no agrega ni pierde energía.

**Los largos.** Una línea de `T` segundos resuena en cada múltiplo de `1/T`. Dos
líneas en razón simple comparten casi todos esos modos, y un modo compartido es
una frecuencia en la que la cola canta. Las ocho razones están repartidas
geométricamente sobre 1 : 3.23 —geométrico porque eso reparte la *densidad
modal* de forma pareja— y después corridas de la grilla exacta, para que ningún
par quede cerca de un racional chico. Nada viene de la tabla de Freeverb.

El orden **no** es ascendente: las primeras cuatro ya cubren todo el rango, que
es lo que usa Economy. Economy es una red más chica, no más corta.

**Interpolación cúbica.** La lectura fraccionaria de las líneas del lazo usa
Catmull-Rom de cuatro puntos. La interpolación lineal es un promedio de dos taps
—o sea un pasabajos— y a medio sample está **3.0 dB abajo en fs/4** y 0.68 dB en
fs/8. Fuera de un lazo eso es un suavizado; **adentro** se aplica en cada
pasada, así que la parte alta de la cola muere mucho más rápido que la baja y el
decay deja de ser el que se pidió. El cúbico está 1.1 dB y 0.08 dB en esos
mismos puntos — medido en `a_cubic_read_keeps_the_top_end_a_linear_one_loses`.
Las reflexiones, que se leen una vez y no se realimentan, siguen siendo
lineales.

La línea de retardo, el `safe()` que descarta denormales y NaN, el `soft_clip` y
el LFO cúbico viven en [`fx/delay_line.rs`](../crates/choz-engine/src/fx/delay_line.rs),
compartidos con el resto de los efectos — ver [fx-audit.md](fx-audit.md).

### 6 · RT60

```text
g_i = 10^(−3·T_i/RT60)
```

Es la ganancia por pasada exacta para una caída de −60 dB en `RT60` segundos.
`Decay` es un **tiempo**, no un número de realimentación:

```text
RT60 = 0.20 · 60^decay        decay 0.0 → 0.20 s
                              decay 0.5 → 1.55 s
                              decay 1.0 → 12.0 s
```

`Size` mueve `T_i`; las ganancias lo siguen. Por eso cambiar la sala no cambia
cuánto dura — y por eso `Size = 1.0` no puede caminar la red hacia la
inestabilidad: al alargarse la línea, la ganancia baja.

### 7 · Damping, Tone, cortes

El pasabajos de damping y el pasaaltos de low cut están **dentro** del lazo, así
que se aplican una vez por pasada: los agudos pierden un poco cada vuelta y la
cola se va oscureciendo *a medida que* decae, como una sala real. Un filtro a la
salida podría hacer la cola entera más oscura, nunca progresivamente más oscura.

- **Damping** mueve la esquina de esa pérdida, 16 kHz a 800 Hz.
- **Tone** corre esa misma esquina ±2.4 octavas **y** aplica un tilt de ±8 dB a
  la salida alrededor de 900 Hz. No es sólo un filtro final; el comportamiento
  espectral es parte del camino de realimentación.
- **Low Cut** 20 Hz–1 kHz, adentro del lazo (los graves que no se van son lo que
  convierte un reverb largo en barro) y también a la salida.
- **High Cut** 2–20 kHz, sólo a la salida, sólo sobre el wet.

### 8 · Modulación

Un oscilador lento por línea, todos a ritmos distintos por debajo de 0.5 Hz y
ninguno múltiplo de otro, con profundidades distintas. Es una cúbica suave en el
wrap, no un seno: tres multiplicaciones, sin deriva, y lo único que importa es
que sea lenta, acotada y sin esquinas.

Sirve para romper modos, no para ser un efecto. En valores bajos es
imperceptible; alto, la cola se mueve sin sonar a chorus.

**No hay perilla de Rate**, a propósito. Un solo ritmo para todas las líneas es
lo que hace que se muevan juntas, y moverse juntas es exactamente lo que no
decorrelaciona nada: ocho líneas con el mismo LFO son un chorus de ocho voces en
fase. Los ritmos son ocho, fijos y mutuamente inconmensurables, y `Modulation`
es una sola perilla de profundidad — que es la representación simplificada que
la interfaz de choz puede dibujar en un control.

### 9 · Decorrelación estéreo

Las salidas se arman con pesos **positivos en los dos lados y distintos**, no
opuestos.

Los taps con signo invertido son la forma barata de conseguir un reverb ancho, y
la razón de que tantos se derrumben en mono: lo que ganó el ancho lo cancela la
suma. Acá cada lado se inclina hacia líneas *distintas*, así que las dos salidas
están hechas de audio en buena medida diferente —y por lo tanto
decorrelacionado— pero sumarlas agrega energía en vez de sacarla.

Medido sobre los cinco caracteres, los dos canales salen prácticamente
**descorrelacionados** (|r| < 0.11) y el fold-down cuesta **2.6 a 3.4 dB** —
que es el ideal aritmético de dos canales incorrelados, y es lo que un par de
signos opuestos cambia por un null. El test lo comprueba en los cinco y exige
mejor que −5 dB.

`Width` es mid/side arriba de eso, así que el fold-down a mono es el mid
cualquiera sea el ancho: ensanchar no puede costar nada en mono.

### 10 · Ganancia interna

No hay factor mágico. La entrada llega a la red con norma unitaria
(`b_i = ±1/√N`) y las salidas son una proyección normalizada (`‖w‖ = 1`).

Una sala reverberante **acumula** energía: el nivel de régimen sube como
`1/√(1−g²)`. Eso es físicamente correcto y musicalmente inútil —significa que
subir el decay sube el volumen— así que la salida late se divide por esa
estimación. Está medido en `the_wet_level_does_not_run_away_with_the_decay`: el
nivel se mueve menos de 6 dB entre `Decay` 0.2 y 0.8.

### 11 · Limitador

Un soft limiter en la escritura de cada línea y en la salida wet. Debajo de la
rodilla (0.7 × 4.0 = 2.8) es **exactamente** la identidad, no una tanh que está
3 % baja a media escala: el material normal atraviesa la red intacto y sólo se
dobla lo que habría enrollado el lazo. Un hard clip acá metería armónicos en el
camino de realimentación y no se irían nunca.

### 12 · Freeze

`set_freeze(true)` — todavía no es una perilla; es el gancho que la interfaz va a
usar. Cuatro cosas tienen que estar de acuerdo, y por eso se diseñó desde el
principio:

- ganancia de lazo 0.99999, no 1.0 (en `f32` un lazo exactamente sin pérdidas no
  tiene por qué quedarse acotado; la diferencia son 40 minutos de decay);
- damping fuera del camino, o la cola se apagaría por el filtro;
- low cut bajado a bloqueador de DC y no más — sacarlo dejaría su estado
  restando una constante, que es lo único que un freeze no puede dejar crecer;
- modulación en cero y distancias de lectura redondeadas a sample entero: el
  cúbico devuelve el sample guardado cuando la fracción es cero, y una fracción
  de dB cientos de veces por segundo es un fade;
- entrada silenciada, con su propio smoothing.

### 13 · Smoothing

Todo lo que se mueve es un `Smoothed` avanzado **una vez por sample**, nunca una
vez por bloque. 20 ms para niveles y coeficientes, 60 ms para las distancias de
lectura — mover un cabezal se oye como afinación, así que `Size` tiene que
llegar lento suficiente para que el glissando sea un glissando y no un chirrido.

Consecuencia: no hay nada en el algoritmo que sepa dónde termina un bloque. El
test `the_block_size_does_not_change_a_single_sample` compara bit a bit contra
32, 96, 128, 256, 512 y 1024 frames.

Lo que **no** se suaviza: `Character` y `Quality`. No hay medio camino entre un
hall y una placa; suavizar eso sería suavizar un nombre.

---

## Caracteres

Un solo motor. `Character` mueve el balance entre sus etapas — que es lo que
realmente distingue una sala de un hall: cuánto de lo que se oye son reflexiones
discretas contra cuánto es cola difusa, qué tan lejos están las superficies, y
cuánto tarda la energía en irse.

| | ER | late | spread | size | decay | diff | damp | mod | width |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **Room** | 1.00 | 0.80 | 0.70 | 0.65 | 0.55 | −0.15 | 1.00 | 0.45 | 0.90 |
| **Hall** | 0.45 | 1.00 | 1.25 | 1.25 | 1.50 | +0.15 | 0.85 | 1.00 | 1.15 |
| **Chamber** | 0.70 | 1.00 | 0.55 | 0.85 | 0.90 | +0.08 | 1.10 | 0.65 | 1.00 |
| **Plate** | 0.12 | 1.00 | 0.35 | 0.50 | 1.00 | +0.30 | 1.45 | 0.55 | 1.05 |
| **Ambient** | 0.22 | 1.00 | 1.45 | 1.50 | 2.50 | 0.70 | 0.70 | 1.70 | 1.30 |

Cada campo es un multiplicador o un offset sobre lo que puso el usuario, nunca
un reemplazo: `Decay` sigue significando decay en una placa, sólo que significa
otra cantidad de segundos.

Un test comprueba que los nombres dicen la verdad: la relación
energía-temprana / energía-tardía de Room es más de 1.5× la de Hall, y la de
Plate es menor que la de Room.

---

## Calidad

Los búferes son siempre los de la red grande, así que cambiar de modo **no
alloca**: sólo cambia cuántas líneas, difusores y reflexiones se leen.

| | líneas | difusión in / out | ER por lado | rotación |
|---|---:|---:|---:|---:|
| Economy | 4 | 2 / 1 | 4 | 1 |
| High | 8 | 4 / 2 | 8 | 3 |

La rotación es coprima con el número de líneas en los dos casos (3 con 8, 1 con
4), así que es un ciclo único y cada línea llega a todas las demás.

Al pasar a Economy se vacía lo que queda estacionado — búferes, estados de
filtro y **los allpass**. Un allpass que no se procesa no decae: *retiene*,
exacto, todo el tiempo que esté estacionado, y al volver a High entregaría una
rebanada de audio de cuando se cambió el modo, que sería lo más fuerte en un
reverb por lo demás silencioso.

---

## Parámetros

| # | Nombre | Default | |
|---|---|---:|---|
| 0 | Size | 0.50 | 13–92 ms la línea más corta |
| 1 | Damping | 0.50 | 16 kHz → 800 Hz, en el lazo |
| 2 | Width | 1.00 | mid/side |
| 3 | Wet | 0.35 | ley de send, `mix^1.4` |
| 4 | Decay | 0.45 | 0.2–12 s |
| 5 | PreDelay | 0.08 | 0–250 ms |
| 6 | Diffusion | 0.70 | coeficiente de los allpass |
| 7 | Tone | 0.50 | corre el damping ±2.4 oct + tilt ±8 dB |
| 8 | Modulation | 0.25 | 0–4 ms |
| 9 | LowCut | 0.15 | 20 Hz–1 kHz |
| 10 | HighCut | 0.80 | 2–20 kHz |
| 11 | Character | Hall | lista de nombres |
| 12 | Quality | High | lista de nombres |

**Los primeros cuatro índices no se movieron.** `Room` ahora se llama `Size` y
significa lo mismo; un proyecto guardado contra el motor viejo abre con su
tamaño, su damping, su ancho y su wet donde los dejó, y los nueve que siguen en
sus defaults. `build_processor` usa defaults por índice justamente para eso:
leerlos como cero abriría el reverb sin decay, sin difusión y sin cola.

---

## Costo

`cargo run --release --example reverb_bench` — cuenta las allocations con un
allocator envolvente en vez de afirmar que no hay.

```
Economy (4×4 FDN)     0.86% de un core   ×116 realtime   0 allocations
High    (8×8 FDN)     1.33% de un core   × 75 realtime   0 allocations

por tamaño de bloque (32 / 128 / 512 / 2048 frames): 1.34 – 1.35%
```

21 allocations en `new()`, ninguna después. Una instancia a 48 kHz ocupa unos
600 KB de búferes; a 192 kHz, unos 2.4 MB.

El motor viejo era más barato —24 lecturas de retardo por frame sin
interpolación— pero 1.4 % de un core por instancia es lo que cuesta que la cola
no cante.

---

## Qué comprueban los tests

`fx/reverb.rs`, 24 tests:

| | |
|---|---|
| Respuesta al impulso | llegadas discretas primero, cola que espesa (crest factor), decay, dos canales distintos |
| RT60 | T30 medido contra `rt60()` en tres posiciones del knob, y a 44.1 / 48 / 96 / 192 kHz |
| Estabilidad | 4 s de ruido a 0.8 en cada decay × cada calidad: finito y acotado |
| Silencio | la cola llega a cero **exacto**, no a un denormal |
| Entrada caliente | ×12 de fondo de escala: doblada, no explotada |
| Nivel | el decay mueve el wet menos de 6 dB |
| Dry | con `Mix = 0` el búfer sale idéntico, comparado bit a bit |
| Estéreo | `Width = 0` es mono; fold-down mejor que −5 dB en los cinco caracteres |
| Bloque | idéntico bit a bit entre 32 y 1024 frames |
| Sample rate | cambio en caliente sin NaN y sin allocar |
| Automatización | 600 pasos sobre diez parámetros con audio sonando: sin salto, sin infinito |
| Freeze | mantiene ±6 dB durante 8 s y después suelta |
| Matriz | la mezcla conserva energía |
| Contratos | orden y nombres de los parámetros; `reset` no deja nada |
