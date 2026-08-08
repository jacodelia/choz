# choz — Pendiente

Qué falta. Lo hecho está en [CHANGELOG.md](../CHANGELOG.md), día por día; cómo
encajan las piezas, en [architecture.md](architecture.md).

Última actualización: 2026-08-08.

## Estado en una línea

Los seis formatos de plugin (CLAP, LV2, LADSPA, DSSI, VST2, VST3) se escanean,
se hostean y abren su ventana nativa; el rack es multi-slot con mixer, FX,
ruteo de entradas/salidas y proyectos en YAML; y hay tres capas contra el código
ajeno que revienta — escaneo fuera de proceso, cuarentena y sandbox. **236
tests**, `clippy --workspace --all-targets -D warnings` limpio.

## Pendiente (en orden de ROI)

1. **Política del editor sandboxeado.** El mecanismo está hecho: un plugin que
   corre en su propio proceso abre ahí su ventana, empotrada en una ventana X11
   de choz. Falta usarlo:
   - **(a)** volver a ofrecer los editores de la deny-list — las 31 UIs de
     guitarix segfaultean — **cuando el plugin va sandboxeado**, que es
     exactamente lo que el aislamiento hace seguro;
   - **(b)** un plugin sandboxeado sin ventana abre un marco vacío: el hijo
     tendría que publicar "tengo editor" al arrancar y el host esperarlo (al
     capturar el mango, el hijo todavía está cargando).

2. **La cuarentena y el sandbox no cubren la GUI en proceso.** Un editor que
   revienta sólo es inofensivo si ese plugin ya iba sandboxeado. Lo natural es
   que abrir una ventana sea, por sí solo, motivo para aislar el plugin.

3. **`state:mapPath` para LV2.** Un plugin que guarda rutas de archivos (un
   sampler) no puede guardar su estado: falta el par de funciones de mapeo de
   rutas, que además devuelven cadenas que el plugin libera con `free`.

4. **Mirar con audio y ojos reales**, que es lo único que los tests no dan:
   - el modal INSTRUMENT y la caja de knobs con Surge XT (CLAP), Yoshimi (LV2),
     WhySynth/hexter (DSSI) y los VST2 de u-he;
   - el fondo por protocolo gráfico en una kitty de verdad (la secuencia está
     verificada byte a byte, la imagen no la ha visto nadie);
   - el modo MULTI con Reaper mandando varios canales a la vez.

5. **Terminar el barrido de editores LV2 con `--mapped`.** El barrido limpio
   (254 de 259 UIs con ventana real, 0 crashes) se hizo con la ventana padre
   **sin mapear**, que es más suave que lo que hace choz de verdad.

6. **Rediseño visual de los parámetros del instrumento** (prompt para la
   próxima sesión, con lo que ya se sabe):

   > La caja `INSTRUMENT` del RACK dibuja hoy **todos** los parámetros igual: un
   > arco `[▁▂▃▄▅▆▇]`, el valor y el nombre, tres filas por knob. Un plugin no
   > tiene sólo knobs. Hay que **elegir el control según lo que el parámetro
   > es**, y dibujarlo con la densidad que el terminal permite:
   >
   > - **Botón / interruptor** para lo binario (bypass, sync, retrigger): se ve
   >   encendido o apagado, no un arco a 0.00 o 1.00.
   > - **Checkbox** para lo binario dentro de una lista larga, donde un botón
   >   ocuparía demasiado.
   > - **Fader horizontal** para lo que se lee mejor como recorrido (mezcla,
   >   paneo, tiempos): ancho de sobra en una fila, y el valor a la derecha.
   > - **Fader vertical** para grupos que se comparan entre sí — un ADSR, un
   >   ecualizador por bandas — donde ver el perfil de un vistazo vale más que
   >   leer cuatro números.
   > - **Knob rotativo** para lo que ya funciona bien así (corte, resonancia,
   >   ganancia), pero con más resolución angular que el arco actual: media
   >   celda con `▘▝▖▗` o un dial de 12 posiciones se lee mucho mejor.
   > - **Enumerado** (`◀ NOMBRE ▶`) cuando el parámetro tiene pasos con nombre:
   >   forma de onda, tipo de filtro, modo. El plugin los da (`getParamStringByValue`
   >   en VST3, `points`/`scalePoints` en LV2, la lista de CLAP).
   >
   > **Lo que hay que resolver, no sólo dibujar:**
   > - **De dónde sale el tipo.** `choz_ports::PluginParam` sólo tiene
   >   `id/name/min/max/default`. Hace falta una pista: `steps` (0 = continuo, 2 =
   >   interruptor, n = enumerado), y `unit`. Cada host la puede llenar — VST3
   >   `ParameterInfo.stepCount` y `units`, LV2 `lv2:portProperty` (`toggled`,
   >   `enumeration`, `integer`) más `units:unit`, CLAP los flags de
   >   `clap_param_info`, VST2 no da nada y se queda en continuo.
   > - **Adivinar por el nombre es tentador y suele fallar** (la lección del
   >   `FxCategory::guess` de ADD FX): usar la pista del plugin cuando la haya y
   >   caer en knob continuo cuando no.
   > - **La distribución deja de ser una rejilla uniforme.** `param_grid` reparte
   >   celdas iguales; un fader vertical ocupa varias filas y un enumerado una
   >   fila ancha. Hace falta un layout que agrupe por tipo y siga cabiendo con
   >   scroll, sin romper `RackLayout.instr_knobs` (cada control sigue necesitando
   >   su rect para el ratón y para MIDI learn).
   > - **Todo lo nuevo tiene que seguir siendo MIDI-learnable y clicable**, y
   >   respetar el lavado del tema (`theme::wash`, nada de `bg` propio salvo un
   >   resaltado deliberado).
   > - Aplicarlo también a la caja de FX: las dos salen de `draw_knob_box`, así
   >   que el trabajo se hereda.
   >
   > Referencia visual: Carla dibuja exactamente esto en su panel genérico.

7. **Empaquetado e instalación** (pedido 2026-08-08). Hoy sólo hay
   `cargo build --release`; falta todo lo que convierte eso en algo instalable:

   - **Un instalador que detecte la versión anterior, la desinstale y ponga la
     nueva.** Concreto: `.deb` y `.rpm` (con `cargo-deb` / `cargo-generate-rpm`,
     que ya hacen el reemplazo por versión con las dependencias declaradas) más
     un `install.sh` para quien no use paquetes. El script tiene que mirar
     `~/.local/bin/choz` y `/usr/local/bin/choz`, comparar `choz --version`
     (**que aún no existe: hace falta la bandera**) y quitar la instalación
     vieja antes de copiar. **Lo que nunca se toca al desinstalar**:
     `~/.local/state/choz/` — los proyectos, las rutas de plugins y los ajustes
     del usuario no son parte del paquete.
   - **Artefactos por arquitectura en cada release**: `x86_64`, y **Raspberry
     Pi** en `aarch64` (Pi 3/4/5, 64-bit) y `armv7` (Pi 2 y Zero 2 W en sistemas
     de 32 bits). Se cruza con `cross`, y hay que comprobar dos cosas que en ARM
     no son gratis: que ALSA/JACK abren con buffers pequeños sin xruns, y que el
     escaneo de plugins encuentra algo — **los plugins son binarios nativos**, así
     que una Pi sólo carga plugins compilados para ARM, no los `.so` de x86.
   - **ESP32: no puede ejecutar choz, y conviene decirlo antes de intentarlo.**
     Es un microcontrolador sin sistema operativo, con cientos de kilobytes de
     RAM y sin `dlopen`; choz necesita las tres cosas (hostear plugins *es*
     cargar código nativo en tiempo de ejecución). Lo que sí encaja, y sería
     útil de verdad, es un ESP32 **como controlador de choz**: MIDI por USB, o
     mandando OSC por WiFi al puerto que choz ya escucha (`/note`, `/mix/…`,
     `/fx/…`). Eso funcionaría hoy sin tocar el código de choz — lo que falta es
     el firmware de ejemplo y documentarlo. Decidir cuál de las dos cosas se
     quiere antes de empezar.

8. **choz como aplicación de escritorio** (pedido 2026-08-08): que salga en el
   menú del sistema y se abra con un clic.

   - Un `choz.desktop` (`Categories=AudioVideo;Audio;`) instalado en
     `/usr/share/applications` o `~/.local/share/applications`, más un icono en
     `hicolor`. choz es una TUI, así que hay dos caminos y hay que elegir:
     `Terminal=true` (el escritorio abre su terminal por defecto, que puede no
     ser la que el usuario quiere) o un lanzador propio que arranque una terminal
     buena — **kitty primero**, porque es donde el fondo se ve a resolución real,
     con `ghostty`/`wezterm`/`alacritty`/`xterm` como alternativas.
   - El lanzador tiene que fijar un tamaño mínimo de ventana: por debajo de unas
     100×30 celdas el RACK no cabe y la TUI se ve rota.
   - Y asociar los proyectos: `application/x-choz-project` para `*.choz.yml`, de
     modo que abrir un proyecto desde el gestor de archivos lance choz con él —
     el binario ya acepta la ruta como argumento.

9. Nice-to-have: ruteo por canal MIDI *dentro* de un puerto en modo LIVE;
   automatización; transporte propio (hoy el host VST2 responde 120 BPM fijos a
   `audioMasterGetTime`, marcado `ponytail:`).

## Notas / gotchas para el que retome

- **Los probes de editores abren ventanas de verdad.** `examples/ui_probe` (LV2) y `examples/gui_probe` (CLAP) instancian plugins y abren su GUI: usar `Xvfb` (o la ventana padre sin mapear, que es lo que hacen ahora) y **matarlos al terminar**. `sweep.sh` reanuda tras cada segfault por diseño, así que colgado sigue insistiendo indefinidamente. Ningún test abre ventanas, y así debe seguir — `vst2_runtime.rs` lo dice explícitamente donde toca un editor.
- **En VST3, la GUI no habla con el procesador.** El edit controller reporta al host (`IComponentHandler::performEdit`) y es el host quien lleva el valor al procesador por `inputParameterChanges`. Un host que no lo hace tiene knobs que se mueven sin sonar. Lo mismo con los ids: `getParameterInfo` toma un **índice** y devuelve un **id arbitrario**; confundirlos mueve otro parámetro.
- **Un valor que no se guarda tampoco se puede editar.** El cap de 7 parámetros truncaba la lista *al construirla*, no sólo al dibujarla: lo que no está en el `Vec` no viaja al proyecto.
- **Una nota-off tiene que ir a donde fue su nota-on.** El ruteo depende de la pestaña activa, así que resolverlo dos veces (una al pulsar, otra al soltar) deja notas colgadas en cuanto el usuario cambia de tab. `App.sounding` es la memoria; `PANIC` es la salida de emergencia.
- **Un fondo de celda es opaco, y va por encima de la imagen del protocolo gráfico.** De ahí las dos reglas: en halfblocks la transparencia se mezcla en las celdas (fg *y* bg, o se pierde la mitad de la resolución); bajo kitty **no se tocan las celdas** y el lavado es una segunda imagen con alfa.
- **Un binario de plugin puede publicar cientos de descriptores.** LSP tiene ~390 UIs en un `.so`. Cualquier bucle `for i in 0..N` sobre `lv2_descriptor`/`lv2ui_descriptor` con N fijo miente en silencio: se recorre hasta el primer nulo, y el tope existe sólo contra un binario roto.
- **"No carga" y "no tiene ventana" son respuestas distintas.** Un probe que las imprime igual esconde bugs: dos sesiones creyendo que LSP no ofrecía editores cuando en realidad no las estábamos encontrando.
- **La primera explicación de una medición rara suele ser falsa.** "Un proceso que abre cientos de UIs se queda sin recursos" sonaba bien; barrer en rebanadas de procesos frescos dio los mismos números y la tiró en cinco minutos. Medir la hipótesis cuesta menos que escribirla en el roadmap como si fuera un hecho.
- **El fondo por protocolo gráfico depende del `z`**: por debajo de -1073741824 la imagen queda bajo los fondos de celda (lo que choz quiere); con `z=-1` taparía paneles y resaltados. Y `ratatui-image` no vale para esto: coloca por placeholders Unicode, que el texto de encima borra.
- **Un puntero COM prestado no se envuelve en `ComPtr`.** `ComPtr` libera al soltarse, así que cada uso le quita una referencia a un objeto que es del plugin (VST3: los handlers del run loop). Para eso está `ComRef`. Vale para cualquier callback que reciba punteros ajenos.
- **Un probe que consume el objeto bajo prueba mide otra cosa.** `.and_then(|i| i.editor())` dropea el plugin antes de usar el editor, y el `Drop` vacía la celda compartida: las llamadas salen por la rama "instancia muerta" sin decir nada. Costó una conclusión equivocada entera sobre CLAP.
- **stdout a un archivo va en bloques.** Un resultado "impreso" pero no volcado se pierde si el proceso siguiente segfaultea. `ui_probe::say()` imprime y hace flush; sin eso una corrida perdió 74 resultados y el total pareció limpio.
- **El fondo se dibuja antes que nada en `ui()`**, y depende de que los widgets no fijen `bg`. Cualquier panel nuevo debe usar `theme::panel_style()`, no una constante de color ni `Color::Reset`, o abrirá un agujero opaco en el wallpaper.
- **`Color::Reset` no es transparente.** Es SGR 49 — el fondo por defecto del terminal — y pinta encima de lo que haya. Lo único que deja el buffer intacto es no fijar `bg` en absoluto, que es por qué `panel_style()` devuelve un `Style` y no un `Color`.
- **Sandbox por plugin, no por tab**: `quarantine::forced` se guarda por `formato|ruta|id` en `<state dir>/plugin-sandbox.json`; el toggle del RACK sólo se ve al reinstanciar, y por eso el botón dice `(reload)` mientras tanto. `SandboxStatus` se captura junto a `editor()` — si aparece otro sitio que crea slots, hay que capturarlo ahí también.
- **Sync engine↔UI slots**: `AudioEngine.slot_count` y `App.slots` se mantienen en el mismo orden (append/remove espejados). No romper ese invariante.
- **Working copy**: `App.{source, fx_chain, fx_slot, fx_param}` son la copia viva del `active_slot`; se persisten a `App.slots[active]` en `persist_active()` al cambiar de tab / borrar. Los handlers de FX operan sobre la copia y llaman `rebuild_fx()` → `engine.set_slot_fx(active_slot, …)`.
- **`target/` es efímero en el sandbox**: correr build+test en una sola invocación para ver el binario en `target/debug/choz`.
- **Ruteo**: se resuelve en la UI (`note_targets`), no en el engine. Si algún día hace falta que el RT rutee, hay que meterle el binding — hoy no lo tiene a propósito.
- **`midi_connected` vs `midi_ports`**: `InputSource::Midi(i)` indexa `midi_connected` (lo que devolvió `connect_inputs`, en ese orden exacto); `midi_ports` es "todo lo que se vio". No confundirlos.
- **Cambiar de output device pierde los slots del engine** (se dropea el stream); `App::set_output_device` los recrea. Cualquier estado que solo viva en el engine hay que recrearlo ahí también.
- **Canal de notas único**: `App.note_tx/note_rx` se crea una vez al arrancar; `connect_midi()` clona el sender. No volver a crear el canal al reconectar MIDI (dejaría huérfano el hilo OSC).
- **Mixer**: `RackSlot.{gain,pan,mute,solo}` NO están en la working copy; viven solo en `slots[i]` y se empujan con `push_mix()` (que resuelve el solo). El engine no conoce el solo.
- **Teardown de plugins CLAP**: por defecto se filtran a propósito (ver arriba). Si aparece un leak raro o quieres medir memoria, corre con `CHOZ_CLAP_STRICT_TEARDOWN=1` — y prepárate a que un plugin roto tire la app.
- **Verificar la TUI sin terminal**: `(sleep 5; printf '\r') | script -qec "stty rows 45 cols 170; timeout 10 ./target/debug/choz" /dev/null > out`, luego quitar ANSI con `re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]','',...)`. **Ojo**: ratatui hace redibujo incremental, así que las líneas completas del scrape suelen ser de frames viejos.
- **`registry.rs`/`scanner.rs`/`plugin_types.rs`**: infra vieja, en gran parte stub (`#[allow(dead_code)]`). El camino real son los crates `choz-plugin-*`. Decidir si se borra.

## Comandos útiles

```bash
cargo build --workspace                 # todos los hosts van en el build normal
cargo test --workspace
# barridos largos: hostear TODOS los plugins instalados de un formato
cargo test --release -p choz-plugin-lv2 -- --ignored
cargo test --release -p choz-plugin-ladspa -- --ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --bin choz          # necesita una terminal real (tty)
tail -f ~/.local/state/choz/choz.log    # ver errores/log en vivo

# Probes de editores: INSTANCIAN PLUGINS Y ABREN SU GUI. Los de LV2 y VST3
# crean la ventana padre sin mapear (no ensucian el escritorio); `--mapped`
# reproduce lo que hace choz de verdad, y entonces conviene un display aparte.
cargo run -p choz-plugin-lv2  --example ui_probe            # --limit N, --skip N
cargo run -p choz-plugin-vst3 --example gui_probe
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run -p choz-plugin-clap --example gui_probe
DISPLAY=:99 cargo run -p choz-plugin-lv2  --example ui_probe -- --mapped

# Tests de runtime con los instrumentos VST2 del usuario (los directorios
# estándar sólo tienen efectos).
CHOZ_VST2_DIR=/ruta/a/tus/vst cargo test -p choz-plugin-vst2
```
