# choz control surface on an ESP32-S3 with a touchscreen

A touch panel that drives choz over WiFi: four channel faders with mute buttons
and a one-octave keyboard, sending OSC to the port choz already listens on.

**choz does not run on this board, and cannot.** The ESP32-S3 is a Xtensa LX7
microcontroller with no MMU, a few hundred kilobytes of SRAM and no `dlopen`;
there is no Linux for it — the S3 boards with screens (ESP32-S3-BOX-3, LilyGO
T-Display-S3 Touch, Waveshare ESP32-S3-Touch-LCD) run ESP-IDF/FreeRTOS with
LVGL. Hosting a plugin *is* loading native code at runtime, so the board's job
here is the one it is good at: being the surface, while choz runs on the
computer with the audio interface.

Nothing in choz needs changing for this. The OSC server is the one that has been
there since 2026-07-26; this firmware just speaks to it.

## Boards

Any ESP32-S3 with a touchscreen LVGL already supports. Tested layout targets a
320×240 panel; the sketch scales to whatever `TFT_WIDTH`/`TFT_HEIGHT` report.

| Board | Panel | Notes |
|---|---|---|
| Waveshare ESP32-S3-Touch-LCD-2.8 | 320×240 capacitive | the layout below was drawn for this |
| LilyGO T-Display-S3 Touch | 320×170 | two rows of faders instead of one |
| ESP32-S3-BOX-3 | 320×240 capacitive | has a speaker/mic that this does not use |

## What it sends

choz's OSC addresses (`crates/choz-engine/src/osc.rs`), unchanged:

| Control | Message | Range |
|---|---|---|
| Channel fader | `/mix/<tab>/gain <float>` | `0..2`, 1.0 = unity |
| Pan | `/mix/<tab>/pan <float>` | `-1..1` |
| Mute | `/mix/<tab>/mute <int>` | 0 / 1 |
| FX knob | `/fx/<tab>/<fx>/<param> <float>` | `0..1`, both indices 1-based |
| Key down | `/note <int key> <int velocity>` | velocity 0 = note off |
| Key up | `/note/off <int key>` | |

Tabs and FX are **1-based, as the RACK draws them**.

## Flashing

Arduino IDE or `arduino-cli`, board `ESP32S3 Dev Module`, with:

- **LVGL** 8.3+ and the display/touch driver your board needs (`TFT_eSPI`,
  `Arduino_GFX`, or the vendor's — the sketch only calls LVGL).
- PSRAM enabled if your board has it (LVGL's buffers are happier there).

Then edit the four lines at the top of `choz_touch.ino`:

```c
static const char *WIFI_SSID = "your-network";
static const char *WIFI_PASS = "your-password";
static const char *CHOZ_HOST = "192.168.1.20";   // the machine running choz
static const uint16_t CHOZ_PORT = 9000;          // Settings → AUDIO → OSC
```

`CHOZ_HOST` is the computer's address on the LAN, not the board's. The port is
whatever Settings → AUDIO → OSC shows; 9000 is the default, and choz prints the
one it actually bound at startup.

## Why OSC and not USB MIDI

The S3 can be a USB MIDI device, and choz would see it as another MIDI input —
that works too, and needs no WiFi. OSC is what this example uses because it
reaches the mixer and the FX parameters, not just notes: a MIDI controller can
only send CCs that then have to be learned one by one in the RACK.

## Latency

One UDP packet per touch event on a LAN is well under a millisecond of network
time; what you feel is the panel's own touch scan (LVGL's default 30 ms tick)
plus choz's audio buffer. Nothing here is in the audio path — the board never
carries audio.
