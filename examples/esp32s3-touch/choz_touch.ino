// choz control surface for an ESP32-S3 with a touchscreen.
//
// Four channel faders + mutes and a one-octave keyboard, sent to choz as OSC
// over WiFi. See README.md for the boards this targets and how to flash it.
//
// The board is the surface, never the host: choz runs on the computer with the
// audio interface, and nothing here is in the audio path.
//
// LVGL draws and dispatches touch; this file only builds messages. No OSC
// library: a message is an address, a type tag and big-endian arguments, all
// padded to four bytes — thirty lines, against a dependency that would have to
// be pinned for every board variant.

#include <Arduino.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <lvgl.h>

// ── Edit these four ─────────────────────────────────────────────────────────
static const char *WIFI_SSID = "your-network";
static const char *WIFI_PASS = "your-password";
static const char *CHOZ_HOST = "192.168.1.20";  // the machine running choz
static const uint16_t CHOZ_PORT = 9000;         // Settings → AUDIO → OSC
// ────────────────────────────────────────────────────────────────────────────

static const int CHANNELS = 4;   // rack tabs 1..4
static const uint8_t BASE_NOTE = 60;  // middle C
static const int KEYS = 12;

static WiFiUDP udp;

// ── OSC ─────────────────────────────────────────────────────────────────────

struct OscMsg {
  uint8_t buf[128];
  size_t len = 0;

  void pad() {
    // Every OSC chunk is a multiple of four bytes, zero-filled.
    while (len % 4 != 0 && len < sizeof(buf)) buf[len++] = 0;
  }

  void str(const char *s) {
    while (*s && len < sizeof(buf)) buf[len++] = (uint8_t)*s++;
    if (len < sizeof(buf)) buf[len++] = 0;  // the terminator counts
    pad();
  }

  void be32(uint32_t v) {
    if (len + 4 > sizeof(buf)) return;
    buf[len++] = v >> 24; buf[len++] = v >> 16; buf[len++] = v >> 8; buf[len++] = v;
  }

  void begin(const char *addr, const char *tags) {
    len = 0;
    str(addr);
    str(tags);  // ",f", ",i", ",ii" — the leading comma is part of it
  }

  void f32(float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);  // OSC floats are IEEE-754, big-endian
    be32(bits);
  }

  void i32(int32_t v) { be32((uint32_t)v); }

  void send() {
    udp.beginPacket(CHOZ_HOST, CHOZ_PORT);
    udp.write(buf, len);
    udp.endPacket();
  }
};

// choz's addresses carry their target in the path, and the indices are 1-based
// exactly as the RACK draws them.
static void send_gain(int tab, float gain) {
  char addr[32];
  snprintf(addr, sizeof(addr), "/mix/%d/gain", tab);
  OscMsg m; m.begin(addr, ",f"); m.f32(gain); m.send();
}

static void send_mute(int tab, bool on) {
  char addr[32];
  snprintf(addr, sizeof(addr), "/mix/%d/mute", tab);
  OscMsg m; m.begin(addr, ",i"); m.i32(on ? 1 : 0); m.send();
}

static void send_note(uint8_t key, uint8_t velocity) {
  // Velocity 0 is a note-off, which is what every MIDI source does anyway.
  OscMsg m; m.begin("/note", ",ii"); m.i32(key); m.i32(velocity); m.send();
}

// ── UI ──────────────────────────────────────────────────────────────────────

static void fader_moved(lv_event_t *e) {
  lv_obj_t *slider = lv_event_get_target(e);
  int tab = (int)(intptr_t)lv_event_get_user_data(e);
  // The slider is 0..100; choz takes 0..2, where 1.0 is unity gain.
  float gain = lv_slider_get_value(slider) / 100.0f * 2.0f;
  send_gain(tab, gain);
}

static void mute_toggled(lv_event_t *e) {
  lv_obj_t *btn = lv_event_get_target(e);
  int tab = (int)(intptr_t)lv_event_get_user_data(e);
  send_mute(tab, lv_obj_has_state(btn, LV_STATE_CHECKED));
}

static void key_pressed(lv_event_t *e) {
  lv_obj_t *btn = lv_event_get_target(e);
  uint8_t note = BASE_NOTE + (uint8_t)(intptr_t)lv_event_get_user_data(e);
  // Press and release are separate events, so a held key stays held — the same
  // contract choz's own QWERTY piano has.
  send_note(note, lv_event_get_code(e) == LV_EVENT_PRESSED ? 100 : 0);
  (void)btn;
}

static void build_ui() {
  lv_obj_t *scr = lv_scr_act();
  lv_obj_set_style_bg_color(scr, lv_color_hex(0x0d1117), 0);

  const lv_coord_t w = lv_disp_get_hor_res(NULL);
  const lv_coord_t h = lv_disp_get_ver_res(NULL);
  const lv_coord_t strip = w / CHANNELS;
  const lv_coord_t keys_h = h / 4;

  for (int i = 0; i < CHANNELS; i++) {
    int tab = i + 1;  // 1-based, like the RACK

    lv_obj_t *slider = lv_slider_create(scr);
    lv_obj_set_size(slider, strip / 3, h - keys_h - 40);
    lv_obj_set_pos(slider, i * strip + strip / 3, 8);
    lv_slider_set_range(slider, 0, 100);
    lv_slider_set_value(slider, 50, LV_ANIM_OFF);  // 50 % of 0..2 = unity
    lv_obj_add_event_cb(slider, fader_moved, LV_EVENT_VALUE_CHANGED, (void *)(intptr_t)tab);

    lv_obj_t *mute = lv_btn_create(scr);
    lv_obj_add_flag(mute, LV_OBJ_FLAG_CHECKABLE);
    lv_obj_set_size(mute, strip - 12, 28);
    lv_obj_set_pos(mute, i * strip + 6, h - keys_h - 32);
    lv_obj_add_event_cb(mute, mute_toggled, LV_EVENT_VALUE_CHANGED, (void *)(intptr_t)tab);
    lv_obj_t *label = lv_label_create(mute);
    lv_label_set_text_fmt(label, "M%d", tab);
    lv_obj_center(label);
  }

  // One octave, white keys wide and black keys on top of them.
  static const bool black[KEYS] = {false, true, false, true, false, false,
                                   true, false, true, false, true, false};
  lv_coord_t white_w = w / 7;
  int white_index = 0;
  for (int k = 0; k < KEYS; k++) {
    lv_obj_t *key = lv_btn_create(scr);
    lv_obj_add_event_cb(key, key_pressed, LV_EVENT_PRESSED, (void *)(intptr_t)k);
    lv_obj_add_event_cb(key, key_pressed, LV_EVENT_RELEASED, (void *)(intptr_t)k);
    if (black[k]) {
      lv_obj_set_size(key, white_w * 2 / 3, keys_h * 3 / 5);
      lv_obj_set_pos(key, white_index * white_w - white_w / 3, h - keys_h);
      lv_obj_set_style_bg_color(key, lv_color_hex(0x161b22), 0);
    } else {
      lv_obj_set_size(key, white_w - 2, keys_h - 2);
      lv_obj_set_pos(key, white_index * white_w, h - keys_h);
      lv_obj_set_style_bg_color(key, lv_color_hex(0xdce2f0), 0);
      white_index++;
    }
  }
}

// ── Board bring-up ──────────────────────────────────────────────────────────
//
// `lv_init`, the display flush callback and the touch read callback belong to
// your board's driver (TFT_eSPI, Arduino_GFX, or the vendor's example). Call
// them here, then `build_ui`.

extern void board_display_init();  // provided by your board's LVGL example

void setup() {
  Serial.begin(115200);

  WiFi.mode(WIFI_STA);
  WiFi.begin(WIFI_SSID, WIFI_PASS);
  // A control surface with no network is a panel that does nothing, so say so
  // rather than failing quietly.
  for (int i = 0; i < 40 && WiFi.status() != WL_CONNECTED; i++) {
    delay(250);
    Serial.print('.');
  }
  if (WiFi.status() == WL_CONNECTED) {
    Serial.printf("\nchoz-touch: %s -> %s:%u\n", WiFi.localIP().toString().c_str(),
                  CHOZ_HOST, CHOZ_PORT);
  } else {
    Serial.println("\nchoz-touch: no WiFi; the panel will draw but send nothing");
  }
  udp.begin(WiFi.localIP(), 0);  // any local port; choz never answers

  lv_init();
  board_display_init();
  build_ui();
}

void loop() {
  lv_timer_handler();
  delay(5);
}
