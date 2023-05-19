pub mod gen;
mod interpolation;
pub mod parameters;
pub mod voices;

use std::mem::MaybeUninit;

use fastrand::Rng;
use ringbuf::{LocalRb, Rb};

use crate::{
    common::*,
    parameters::{portamento_mode::PortamentoMode, voice_mode::VoiceMode, Parameter},
};

use parameters::*;
use voices::*;

use self::{
    gen::AudioGenData, parameters::common::AudioParameter, voices::log10_table::Log10Table,
};

#[cfg(feature = "clap")]
pub struct ClapNoteEnded {
    pub key: u8,
    pub clap_note_id: i32,
    pub sample_index: u32,
}

#[cfg(feature = "clap")]
pub type ClapEndedNotesRb =
    ringbuf::LocalRb<ClapNoteEnded, Vec<::std::mem::MaybeUninit<ClapNoteEnded>>>;

pub struct AudioState {
    sample_rate: SampleRate,
    time_per_sample: TimePerSample,
    bpm: BeatsPerMinute,
    bpm_lfo_multiplier: BpmLfoMultiplier,
    pub global_pitch_bend: GlobalPitchBend,
    sustain_pedal_on: bool,
    parameters: AudioParameters,
    rng: Rng,
    log10table: Log10Table,
    pub voices: IndexMap<u8, Voice>,
    pressed_keys: IndexSet<u8>,
    pending_note_events: LocalRb<NoteEvent, Vec<MaybeUninit<NoteEvent>>>,
    audio_gen_data_w2: Box<AudioGenData<2>>,
    #[cfg(target_arch = "x86_64")]
    audio_gen_data_w4: Box<AudioGenData<4>>,
    #[cfg(feature = "clap")]
    pub clap_ended_notes: ClapEndedNotesRb,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::default(),
            time_per_sample: SampleRate::default().into(),
            bpm: Default::default(),
            bpm_lfo_multiplier: BeatsPerMinute::default().into(),
            global_pitch_bend: Default::default(),
            sustain_pedal_on: false,
            parameters: AudioParameters::default(),
            rng: Rng::new(),
            log10table: Default::default(),
            voices: Default::default(),
            pressed_keys: Default::default(),
            pending_note_events: LocalRb::new(1024),
            audio_gen_data_w2: Default::default(),
            #[cfg(target_arch = "x86_64")]
            audio_gen_data_w4: Default::default(),
            #[cfg(feature = "clap")]
            clap_ended_notes: ringbuf::LocalRb::new(256),
        }
    }
}

impl AudioState {
    pub fn set_parameter_from_patch(&mut self, parameter: Parameter, value: f32) {
        self.parameters.set_parameter_from_patch(parameter, value);
    }

    pub fn set_sample_rate(&mut self, sample_rate: SampleRate) {
        self.sample_rate = sample_rate;
        self.time_per_sample = sample_rate.into();
    }

    pub fn set_bpm(&mut self, bpm: BeatsPerMinute) {
        self.bpm = bpm;
        self.bpm_lfo_multiplier = bpm.into();
    }

    pub fn enqueue_note_events<I: Iterator<Item = NoteEvent>>(&mut self, mut events: I) {
        self.pending_note_events.push_iter(&mut events);

        if events.next().is_some() {
            ::log::error!("Audio note event buffer full");
        }
    }

    pub fn enqueue_note_event(&mut self, event: NoteEvent) {
        if self.pending_note_events.push(event).is_err() {
            ::log::error!("Audio note event buffer full");
        }
    }

    #[cfg(feature = "vst2")]
    pub fn sort_note_events(&mut self) {
        let (a, b) = self.pending_note_events.as_mut_slices();

        a.sort_unstable_by_key(|e| e.delta_frames);
        b.sort_unstable_by_key(|e| e.delta_frames);
    }

    fn process_events_for_sample(&mut self, buffer_offset: usize) {
        loop {
            match self
                .pending_note_events
                .iter()
                .next()
                .map(|e| e.delta_frames as usize)
            {
                Some(event_delta_frames) if event_delta_frames == buffer_offset => {
                    let event = self.pending_note_events.pop().unwrap();

                    self.process_note_event(event.event);
                }
                _ => break,
            }
        }
    }

    fn process_note_event(&mut self, event: NoteEventInner) {
        match event {
            NoteEventInner::Midi { mut data } => {
                // Discard channel bits of status byte
                data[0] >>= 4;

                match data {
                    [0b_1000, key, _] => self.key_off(key),
                    [0b_1001, key, 0] => self.key_off(key),
                    [0b_1001, key, velocity] => {
                        self.key_on(key, KeyVelocity::from_midi_velocity(velocity), None)
                    }
                    [0b_1010, key, pressure] => {
                        self.aftertouch(key, KeyVelocity::from_midi_velocity(pressure));
                    }
                    [0b_1011, 64, v] => {
                        self.sustain_pedal_on = v >= 64;
                    }
                    [0b_1110, lsb, msb] => {
                        self.global_pitch_bend.update_from_midi(lsb, msb);
                    }
                    _ => (),
                }
            }
            NoteEventInner::ClapNoteOn {
                key,
                velocity,
                clap_note_id,
            } => {
                self.key_on(key, KeyVelocity(velocity as f32), Some(clap_note_id));
            }
            NoteEventInner::ClapNotePressure { key, pressure } => {
                self.aftertouch(key, KeyVelocity(pressure as f32));
            }
            NoteEventInner::ClapNoteOff { key } => {
                self.key_off(key);
            }
            NoteEventInner::ClapBpm { bpm } => {
                self.set_bpm(bpm);
            }
        }
    }

    fn key_on(&mut self, key: u8, velocity: KeyVelocity, opt_clap_note_id: Option<i32>) {
        self.pressed_keys.shift_remove(&key);
        self.pressed_keys.insert(key);

        let voice_mode = self.parameters.voice_mode.get_value();
        let portamento_mode = self.parameters.portamento_mode.get_value();

        match voice_mode {
            VoiceMode::Polyphonic => {
                // FIXME: an option would be to use first pressed instead?
                let opt_glide_from_key = match portamento_mode {
                    PortamentoMode::Off => None,
                    PortamentoMode::Auto => self
                        .voices
                        .iter()
                        .rev()
                        .filter(|(k, v)| **k != key && v.key_pressed)
                        .map(|(key, _)| *key)
                        .next(),
                    // FIXME: should maybe prefer pressed keys?
                    PortamentoMode::Always => self
                        .voices
                        .iter()
                        .rev()
                        .filter(|(k, _)| **k != key)
                        .map(|(key, _)| *key)
                        .next(),
                };

                let voice = if let Some(voice) = self.voices.shift_remove(&key) {
                    // Shift voice to last position (most recently pressed)
                    self.voices.entry(key).or_insert(voice)
                } else {
                    self.voices
                        .entry(key)
                        .or_insert(Voice::new(MidiPitch::new(key)))
                };

                if let Some(glide_from_key) = opt_glide_from_key {
                    voice.press_key(
                        &self.parameters,
                        velocity,
                        Some(glide_from_key),
                        Some(key),
                        opt_clap_note_id,
                    );
                } else {
                    voice.press_key(
                        &self.parameters,
                        velocity,
                        Some(key),
                        None,
                        opt_clap_note_id,
                    );
                }
            }
            VoiceMode::Monophonic => {
                let mut pressed_key_iter = self
                    .pressed_keys
                    .iter()
                    .rev()
                    .filter(|k| **k != key)
                    .copied();

                let (opt_replace_key, opt_glide_from_key) = match portamento_mode {
                    PortamentoMode::Off => {
                        let opt_replace_key = pressed_key_iter.next();

                        (opt_replace_key, None)
                    }
                    PortamentoMode::Auto => {
                        let opt_replace_key = pressed_key_iter.next();

                        (opt_replace_key, opt_replace_key)
                    }
                    PortamentoMode::Always => {
                        let opt_replace_key = pressed_key_iter
                            .chain(
                                self.voices
                                    .iter()
                                    .rev()
                                    .filter(|(k, _)| **k != key)
                                    .map(|(key, _)| *key),
                            )
                            .next();

                        (opt_replace_key, opt_replace_key)
                    }
                };

                let voice = if let Some(voice) = self.voices.shift_remove(&key) {
                    self.voices.entry(key).or_insert(voice)
                } else if let Some(voice) =
                    opt_replace_key.and_then(|k| self.voices.shift_remove(&k))
                {
                    self.voices.entry(key).or_insert(voice)
                } else {
                    self.voices
                        .entry(key)
                        .or_insert(Voice::new(MidiPitch::new(key)))
                };

                if let Some(glide_from_key) = opt_glide_from_key {
                    voice.press_key(
                        &self.parameters,
                        velocity,
                        Some(glide_from_key),
                        Some(key),
                        opt_clap_note_id,
                    );
                } else {
                    voice.press_key(
                        &self.parameters,
                        velocity,
                        Some(key),
                        None,
                        opt_clap_note_id,
                    );
                }

                for (k, voice) in self.voices.iter_mut() {
                    if voice.key_pressed && *k != key {
                        voice.release_key(); // FIXME: should be fast release
                    }
                }
            }
        }
    }

    fn key_off(&mut self, key: u8) {
        self.pressed_keys.shift_remove(&key);

        let voice_mode = self.parameters.voice_mode.get_value();
        let portamento_mode = self.parameters.portamento_mode.get_value();

        match voice_mode {
            VoiceMode::Polyphonic => {
                if let Some(voice) = self.voices.get_mut(&key) {
                    voice.release_key();
                }
            }
            VoiceMode::Monophonic => {
                if let Some(go_to_key) = self.pressed_keys.iter().rev().next() {
                    if let Some(voice) = self.voices.shift_remove(&key) {
                        let glide = !matches!(portamento_mode, PortamentoMode::Off);

                        // FIXME: likely causes issues if voice already exists here
                        self.voices
                            .entry(*go_to_key)
                            .or_insert(voice)
                            .change_pitch(*go_to_key, glide);
                    }
                } else if let Some(voice) = self.voices.get_mut(&key) {
                    voice.release_key();
                }
            }
        }
    }

    #[allow(unused_variables)]
    fn aftertouch(&mut self, key: u8, velocity: KeyVelocity) {
        // Disabled for now
        // if let Some(voice) = self.voices.get_mut(&key) {
        //     voice.aftertouch(velocity);
        // }
    }

    #[cfg(test)]
    pub fn advance_one_sample(&mut self, sample_rate: SampleRate) {
        self.parameters.advance_one_sample(sample_rate);
    }

    #[cfg(test)]
    pub fn compare_parameter_patch_value(&mut self, parameter: Parameter, value: f32) -> bool {
        self.parameters
            .compare_patch_value(parameter, value)
            .unwrap()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlobalPitchBend {
    factor: f32,
}

impl Default for GlobalPitchBend {
    fn default() -> Self {
        Self { factor: 0.0 }
    }
}

impl GlobalPitchBend {
    pub fn update_from_midi(&mut self, lsb: u8, msb: u8) {
        let amount = ((msb as u16) << 7) | (lsb as u16);

        let mut x = (amount as f32) - 8_192.0;

        // Do we really want to do this? Another option is to clamp negative
        // values at -8191 (e.g. treat -8192 as equivalent to -8191)
        if x > 0.0 {
            x *= 1.0 / 8_191.0;
        }
        if x < 0.0 {
            x *= 1.0 / 8_192.0;
        }

        self.factor = x;
    }
    pub fn as_frequency_multiplier(&self, range_up: f32, range_down: f32) -> f64 {
        let semitone_range = if self.factor >= 0.0 {
            range_up
        } else {
            -range_down
        };

        crate::math::exp2_fast(self.factor * semitone_range * (1.0 / 12.0)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::GlobalPitchBend;

    #[test]
    fn test_global_pitch_bend_from_midi() {
        let mut pitch_bend = GlobalPitchBend::default();

        pitch_bend.update_from_midi(0, 64);
        assert_eq!(pitch_bend.factor, 0.0);

        pitch_bend.update_from_midi(0, 0);
        assert_eq!(pitch_bend.factor, -1.0);

        pitch_bend.update_from_midi(127, 127);
        assert_eq!(pitch_bend.factor, 1.0);
    }
}
