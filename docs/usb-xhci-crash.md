# USB muerto: `xhci_hcd HC died`

Incidente 2026-08-01. Todos los dispositivos USB desaparecen de golpe durante
una sesión: interfaz de audio, teclado MIDI, teclado, mouse y ethernet. No es
un bug de choz ni de PipeWire — el host controller USB del kernel se cuelga.

## Síntoma

```
19:50:02 kernel: xhci_hcd 0000:04:00.3: xHCI host not responding to stop endpoint command
19:50:02 kernel: xhci_hcd 0000:04:00.3: xHCI host controller not responding, assume dead
19:50:02 kernel: xhci_hcd 0000:04:00.3: HC died; cleaning up
19:50:02 kernel: usb 1-1: USB disconnect, device number 2
19:50:02 kernel: usb 1-1.4.3: USB disconnect, device number 13
...
```

PipeWire aparece en el log inmediatamente después, pero como víctima:

```
19:50:02 pipewire: spa.alsa: hw:5,0p: snd_pcm_drop: No existe el dispositivo
19:50:02 pipewire: spa.alsa: hw:5,0p: close failed: No existe el dispositivo
```

El `spa.alsa: impossible htimestamp diff:255` justo antes es un xrun causado
por el bus ya muriéndose, no la causa.

## Causa raíz

Precursores horas antes del cuelgue, todos en el Keystation Pro 88
(`0a4d:00b5`, full-speed):

```
16:47:19 kernel: usb 1-1.4.3: urb status -32          # EPIPE, stall de endpoint
16:47:19 kernel: usb 1-1.4.3: USB disconnect, device number 12
16:47:20 kernel: usb 1-1.4.3: device descriptor read/64, error -71
19:33:03 kernel: usb 1-1.4.3: urb status -32
19:50:02 → HC died
```

El dispositivo stallea endpoints de forma repetida. El xHCI de AMD (Renoir,
`0000:04:00.3`) se cuelga esperando la respuesta al *stop endpoint command* y
declara el controlador muerto, arrastrando el bus completo.

Agravante topológico — el teclado cuelga de dos hubs encadenados:

```
usb1
 └─ 1-1     Microchip 0424:2807
     └─ 1-1.4   Terminus 1a40:0101
         └─ 1-1.4.3   Keystation Pro 88
```

Kernel afectado: 7.0.0-28-generic. Hardware: HP ENVY x360 15-ee1xxx (AMD).

## Recuperación sin reiniciar

Rebind del controlador PCI, re-enumera todo el bus:

```bash
sudo sh -c 'echo 0000:04:00.3 > /sys/bus/pci/drivers/xhci_hcd/unbind; sleep 2; echo 0000:04:00.3 > /sys/bus/pci/drivers/xhci_hcd/bind'
```

Requiere teclado interno o SSH: en ese momento el USB está muerto y no
responde ningún teclado externo. Confirmar el ID del controlador con
`lspci | grep USB` si el hardware cambia.

## Prevención

1. **Conectar Keystation y UMC1820 directo al laptop.** El hub Terminus
   encadenado multiplica los stalls.

2. **Autosuspend off en los hubs.** Por defecto `1-1` y `1-1.4` quedan en
   `auto`:

   ```bash
   sudo sh -c 'echo on > /sys/bus/usb/devices/1-1/power/control; echo on > /sys/bus/usb/devices/1-1.4/power/control'
   ```

   Permanente: agregar `usbcore.autosuspend=-1` a `GRUB_CMDLINE_LINUX_DEFAULT`
   en `/etc/default/grub` y correr `sudo update-grub`.

3. **Vigilar precursores en sesiones largas.** Un `urb status -32` es aviso;
   replugear el teclado antes de que escale.

   ```bash
   journalctl -kf | grep -E 'urb status|error -71|xhci'
   ```

## Descartado

- **PipeWire**: sus errores son posteriores a `HC died`; solo reporta que el
  dispositivo ALSA desapareció.
- **choz**: userspace no puede matar un host controller. Los cuatro clientes
  `choz-in` en ALSA seq (uno por puerto) son normales.
