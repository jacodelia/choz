use super::FxProcessor;

const MAX_LOOP_SECS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LooperState {
    Idle,
    Recording,
    Playing,
    Overdub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Cmd { Record, StopRecord, Play, Stop, Overdub, Clear }

pub struct Looper {
    buf: Vec<f32>,
    cap_frames: usize,
    loop_frames: usize,
    write_pos: usize,
    read_pos: usize,
    state: LooperState,
    pending_cmd: Option<Cmd>,
    wet: f32,
    overdub_mix: f32,
}

#[allow(dead_code)]
impl Looper {
    pub fn new(sample_rate: u32) -> Self {
        let cap = MAX_LOOP_SECS * sample_rate as usize;
        Self {
            buf: vec![0.0f32; cap * 2],
            cap_frames: cap,
            loop_frames: 0,
            write_pos: 0,
            read_pos: 0,
            state: LooperState::Idle,
            pending_cmd: None,
            wet: 1.0,
            overdub_mix: 0.85,
        }
    }

    pub fn state(&self) -> LooperState { self.state }

    pub fn record(&mut self) { self.pending_cmd = Some(Cmd::Record); }
    pub fn stop_record(&mut self) { self.pending_cmd = Some(Cmd::StopRecord); }
    pub fn toggle_record(&mut self) {
        match self.state {
            LooperState::Recording => self.stop_record(),
            _ => self.record(),
        }
    }
    pub fn play(&mut self) { self.pending_cmd = Some(Cmd::Play); }
    pub fn stop(&mut self) { self.pending_cmd = Some(Cmd::Stop); }
    pub fn toggle_play(&mut self) {
        match self.state {
            LooperState::Playing | LooperState::Overdub => self.stop(),
            _ => self.play(),
        }
    }
    pub fn overdub(&mut self) { self.pending_cmd = Some(Cmd::Overdub); }
    pub fn clear(&mut self) { self.pending_cmd = Some(Cmd::Clear); }
    pub fn set_overdub_mix(&mut self, v: f32) { self.overdub_mix = v.clamp(0.0, 1.0); }
}

impl FxProcessor for Looper {
    fn process_block(&mut self, buf: &mut [f32], sample_rate: u32) {
        if let Some(cmd) = self.pending_cmd.take() {
            match cmd {
                Cmd::Record => {
                    let new_cap = MAX_LOOP_SECS * sample_rate as usize;
                    if new_cap != self.cap_frames {
                        self.buf = vec![0.0f32; new_cap * 2];
                        self.cap_frames = new_cap;
                    }
                    self.write_pos = 0;
                    self.loop_frames = 0;
                    self.state = LooperState::Recording;
                }
                Cmd::StopRecord => {
                    if self.state == LooperState::Recording {
                        self.loop_frames = self.write_pos.min(self.cap_frames);
                        self.read_pos = 0;
                        self.state = LooperState::Playing;
                    }
                }
                Cmd::Play => {
                    self.read_pos = 0;
                    self.state = LooperState::Playing;
                }
                Cmd::Stop => { self.state = LooperState::Idle; }
                Cmd::Overdub => {
                    if self.loop_frames > 0 {
                        self.read_pos = 0;
                        self.state = LooperState::Overdub;
                    }
                }
                Cmd::Clear => {
                    self.buf.fill(0.0);
                    self.loop_frames = 0;
                    self.write_pos = 0;
                    self.read_pos = 0;
                    self.state = LooperState::Idle;
                }
            }
        }

        let frames = buf.len() / 2;
        match self.state {
            LooperState::Idle => {}
            LooperState::Recording => {
                for i in 0..frames {
                    if self.write_pos < self.cap_frames {
                        self.buf[self.write_pos * 2]     = buf[i * 2];
                        self.buf[self.write_pos * 2 + 1] = buf[i * 2 + 1];
                        self.write_pos += 1;
                    } else {
                        self.loop_frames = self.cap_frames;
                        self.read_pos    = 0;
                        self.state       = LooperState::Playing;
                        break;
                    }
                }
            }
            LooperState::Playing => {
                if self.loop_frames == 0 { return; }
                for i in 0..frames {
                    let loop_l = self.buf[self.read_pos * 2];
                    let loop_r = self.buf[self.read_pos * 2 + 1];
                    buf[i * 2]     = buf[i * 2]     + self.wet * (loop_l - buf[i * 2]);
                    buf[i * 2 + 1] = buf[i * 2 + 1] + self.wet * (loop_r - buf[i * 2 + 1]);
                    self.read_pos = (self.read_pos + 1) % self.loop_frames;
                }
            }
            LooperState::Overdub => {
                if self.loop_frames == 0 { return; }
                for i in 0..frames {
                    self.buf[self.read_pos * 2]     =
                        self.buf[self.read_pos * 2]     * self.overdub_mix + buf[i * 2];
                    self.buf[self.read_pos * 2 + 1] =
                        self.buf[self.read_pos * 2 + 1] * self.overdub_mix + buf[i * 2 + 1];
                    let loop_l = self.buf[self.read_pos * 2];
                    let loop_r = self.buf[self.read_pos * 2 + 1];
                    buf[i * 2]     = buf[i * 2]     + self.wet * (loop_l - buf[i * 2]);
                    buf[i * 2 + 1] = buf[i * 2 + 1] + self.wet * (loop_r - buf[i * 2 + 1]);
                    self.read_pos = (self.read_pos + 1) % self.loop_frames;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.loop_frames = 0;
        self.write_pos   = 0;
        self.read_pos    = 0;
        self.state       = LooperState::Idle;
        self.pending_cmd = None;
    }

    fn set_mix(&mut self, wet: f32) { self.wet = wet.clamp(0.0, 1.0); }
}
