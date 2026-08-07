# Latencia de audio: por qué choz se clavaba en 21 ms

Incidente 2026-08-06. choz suena, pero la latencia es inaceptable para tocar en
vivo: ~21 ms entre la tecla y el sonido, con `buffer_size: 256` guardado en
`ui.json`. El ajuste de la UI no tenía ningún efecto.

## Síntoma

`pw-top` con choz sonando y el grafo supuestamente en 256:

```
S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME
R  106   1024  48000 383,5us  80,8us  0,02  0,00    0   S24LE 12 48000 alsa_output...UMC1820
R   96   1024  48000  43,6us 293,9us  0,00  0,01    0                   + choz
```

`QUANT 1024` a 48000 = **21,3 ms**. `ERR 0` en todos los nodos: no hay xruns, el
problema es puramente el tamaño del período.

Forzar el quantum global mueve el device pero **no** a choz:

```
$ pw-metadata -n settings 0 clock.force-quantum 256
alsa_output...UMC1820   QUANT 256     ✅
 + choz                 QUANT 1024    ❌
```

Eso descarta la configuración de PipeWire como causa: el techo lo pone el nodo
de choz.

## Causa raíz

Los props reales del nodo lo dicen todo:

```
$ pw-dump | python3 -c "..."   # ver scripts al final
node.force-quantum = 1024
node.force-rate    = 48000
node.lock-quantum  = True
node.latency       = 256/48000   ← lo que choz pedía, ignorado
```

choz exportaba `PIPEWIRE_LATENCY` antes de abrir el cliente JACK. Pero
`node.latency` es solo un **pedido**: pipewire-jack abre todo cliente JACK con
`node.lock-quantum = true` y un `node.force-quantum` heredado del quantum que el
grafo estuviera corriendo en ese momento, y **force gana sobre latency**.

El grafo estaba en 1024 (su `max-quantum`) cuando choz arrancó, así que el
cliente quedó clavado ahí de por vida, sin importar qué dijera `ui.json`.

## Solución

`request_pipewire_period()` en `crates/choz-engine/src/engine.rs`, llamada desde
las dos rutas de arranque (`start_jack_native` y `pick_backend`). Exporta
`PIPEWIRE_QUANTUM` además de `PIPEWIRE_LATENCY`: `PIPEWIRE_QUANTUM` escribe
`node.force-quantum` / `node.force-rate`, que es lo único que le gana al valor
heredado.

Ambas variables tienen que estar puestas **antes** de crear el cliente JACK: la
colocación del nodo se lee una sola vez, en `jack_client_open`.

### El piso de 128 frames

`MIN_FORCED_QUANTUM = 128`. Por debajo choz pide (`PIPEWIRE_LATENCY`) pero no
fuerza (`PIPEWIRE_QUANTUM`), y deja que el grafo decida.

No es cosmético: 64 frames sobre una interfaz USB class-compliant stallea sus
endpoints (`urb status -32`), y en el xHCI de AMD Renoir un endpoint stalleado
se lleva puesto el host controller entero — ver [usb-xhci-crash.md](usb-xhci-crash.md).
El piso existe para que ningún valor de la UI pueda volver a disparar eso.

## Configuración que quedó

| Capa | Valor | Latencia |
|---|---|---|
| choz `ui.json` | `buffer_size: 128`, `sample_rate: 48000` | **2,7 ms** |
| PipeWire graph | `default.clock.quantum 256`, min 32, max 1024 | — |
| ALSA UMC1820 | `period-size 256 × period-num 8 = 2048` | — |

El buffer ALSA tiene que contener el `max-quantum` del grafo o hay xrun en cada
ciclo. Subir `period-num` **no** agrega latencia: la latencia la fija el
quantum, esto solo da lugar dónde escribirlo.

Confirmación en el log de arranque (`~/.local/state/choz/choz.log`):

```
choz: PipeWire period forced to 128/48000 (2.7 ms)
choz: using native JACK client via PipeWire (sr=48000, buf=128, out=12 ch, in=10 ch, dev=UMC1820)
```

### Archivos de configuración del sistema

Ninguno necesitó cambios — ya estaban bien afinados:

- `~/.config/pipewire/pipewire.conf.d/99-lowlatency.conf` — quantum del grafo
- `~/.config/pipewire/jack.conf.d/99-lowlatency.conf` — `node.latency` de clientes JACK
- `~/.config/wireplumber/wireplumber.conf.d/99-lowlatency.conf` — buffers ALSA, perfil `pro-audio`, `htimestamp = false`

Recargar: `systemctl --user restart pipewire pipewire-pulse wireplumber`

### Lo que sí falta del lado del SO

**Governor de CPU en `powersave`.** Fuente clásica de xruns esporádicos en vivo:
el CPU baja de frecuencia entre notas y el pico de un ataque llega tarde.

```
sudo cpupower frequency-set -g performance
```

Permanente: `governor='performance'` en `/etc/default/cpupower` +
`sudo systemctl enable --now cpupower`.

Los límites RT ya están: `rtprio 95`, `memlock unlimited`, rtkit activo, usuario
en el grupo `audio`.

**Correr el binario release.** `target/debug/choz` no tiene headroom de CPU para
DSP a 128 frames.

## Diagnóstico

Quantum real y xruns del nodo:

```bash
pw-top -b -n 4 | grep -E "QUANT|choz"
```

- `QUANT 0` = el nodo nunca ejecuta un ciclo (ver el `htimestamp` en
  [usb-xhci-crash.md](usb-xhci-crash.md)); silencio absoluto.
- `ERR` creciendo = xruns → subir el buffer.
- `ERR 0` con audio que igual "clipea" = **no es latencia**, es saturación de
  nivel; bajar el gain del slot.

Props de colocación del nodo:

```bash
pw-dump | python3 -c "
import json,sys
for o in json.load(sys.stdin):
    p=(o.get('info') or {}).get('props') or {}
    if p.get('node.name')=='choz':
        for k in ('node.latency','node.force-quantum','node.force-rate','node.lock-quantum'):
            print(f'{k} = {p.get(k)}')
"
```

Si `node.force-quantum` no coincide con el `buffer_size` de `ui.json`, el
arranque no pasó por `request_pipewire_period()`.

## Notas

- **96 kHz no conviene acá.** Con 12 canales sobre USB 2.0 duplica el ancho de
  banda para no ganar nada perceptible. 48000/128 con el governor en
  `performance` rinde mejor que 96000/256.
- **Cambiar el buffer requiere reiniciar choz.** La pestaña Engine muestra
  `(restart: running N)` mientras el valor esté pendiente.
- **Pendiente:** `main.rs:2212` guarda el `buffer_size` del engine corriendo y no
  el pendiente, así que un proyecto guardado tras cambiar el buffer se lleva el
  valor viejo.
