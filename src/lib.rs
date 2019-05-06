#[macro_use]
extern crate log;

use std::sync::Arc;

use parking_lot::Mutex;
use rand::{FromEntropy, Rng};
use rand::rngs::SmallRng;
use smallvec::{SmallVec, smallvec};

use vst::api::{Supported, Events};
use vst::buffer::AudioBuffer;
use vst::event::Event;
use vst::host::Host;
use vst::plugin::{Category, Plugin, Info, CanDo, HostCallback, PluginParameters};
use vst::plugin_main;

pub mod common;
pub mod constants;
pub mod notes;
pub mod parameters;
pub mod utils;
pub mod operators;

use crate::common::*;
use crate::constants::*;
use crate::notes::*;
use crate::parameters::*;
use crate::operators::*;


type Notes = SmallVec<[Note; 128]>;
type FadeoutNotes = SmallVec<[Note; 1024]>;
type Operators = SmallVec<[Operator; NUM_OPERATORS]>;


/// State that can be changed with parameters. Only accessed through mutex
pub struct AutomatableState {
    pub master_frequency: MasterFrequency,
    pub operators: Operators,
}


/// State used for processing
pub struct ProcessingState {
    pub global_time: TimeCounter,
    pub sample_rate: SampleRate,
    pub bpm: BeatsPerMinute,
    pub rng: SmallRng,
    pub notes: Notes,

    /// When notes are pressed again while they're still active, they get
    /// copied here so they can fade out in peace
    pub fadeout_notes: FadeoutNotes,

    /// Reference to automatable state
    pub automatable: Arc<Mutex<AutomatableState>>,
}


/// Thread-safe state used for parameter and preset calls
pub struct SyncOnlyState {
    pub parameters: Parameters,
    pub host: HostCallback,
    pub automatable: Arc<Mutex<AutomatableState>>,
}


pub struct OutputChannel {
    pub additive: f64,
    pub chain: f64,
    pub skip_chain: f64,
}

impl Default for OutputChannel {
    fn default() -> Self {
        Self {
            additive: 0.0,
            chain: 0.0,
            skip_chain: 0.0,
        }
    }
}


/// Main structure
pub struct FmSynth {
    processing: ProcessingState,
    sync_only: Arc<SyncOnlyState>,
}

impl Default for FmSynth {
    fn default() -> Self {
        Self::new(HostCallback::default())
    }
}


impl FmSynth {
    fn time_per_sample(&self) -> f64 {
        1.0 / self.processing.sample_rate.0
    }

    fn limit(value: f64) -> f64 {
        value.min(1.0).max(-1.0)
    }

    /// Generate a sample for a note
    /// 
    /// Doesn't take self parameter due to conflicting borrowing of Notes
    /// in calling function `process`
    fn generate_note_sample(
        rng: &mut impl Rng,
        time: TimeCounter,
        sample_rate: SampleRate,
        master_frequency: MasterFrequency,
        operators: &mut Operators,
        note: &mut Note,
    ) -> (f64, f64) {
        let base_frequency = note.midi_pitch.get_frequency(master_frequency);

        let mut output_channels: SmallVec<[OutputChannel; 2]> =
            smallvec![OutputChannel::default(), OutputChannel::default()];

        for (operator_index, operator) in (operators.iter_mut().enumerate()).rev() {
            // Fetch all operator values here to make sure all interpolatable
            // ones are advanced even if calculations are skipped below.

            let operator_volume = operator.volume.get_value(time);
            // let operator_output_operator = operator.output_operator.get_value(time);
            let operator_additive = operator.additive_factor.get_value(time);
            let operator_feedback = operator.feedback.get_value(time);
            let operator_modulation_index = operator.modulation_index.get_value(time);
            let operator_panning = operator.panning.get_value(time);

            let operator_frequency = base_frequency *
                operator.frequency_ratio.0 *
                operator.frequency_free.0 *
                operator.frequency_fine.0;

            // Always calculate envelope to make sure it advances
            let envelope_volume = {
                let note_envelope = &mut note.operators[operator_index].volume_envelope;

                note_envelope.calculate_volume(
                    &operator.volume_envelope,
                    note.pressed,
                    note.duration
                )
            };

            // Only do sound generation if volume is on

            let volume_on = operator_volume > ZERO_VALUE_LIMIT &&
                envelope_volume > ZERO_VALUE_LIMIT;

            // Only calculate panning if volume is on (is irrelevant otherwise)
            let (pan_left, pan_right) = {
                if volume_on {
                    OperatorPanning::get_left_and_right(operator_panning)
                } else {
                    (0.0, 0.0)
                }
            };

            {
                // Mix modulator into current operator depending on panning of
                // current operator. If panned to the middle, just pass through
                // the stereo signals: if panned to any side, mix out the
                // original stereo signals and mix in mono.

                let left_chain = output_channels[0].chain;
                let right_chain = output_channels[1].chain;

                let pan_transformed = 2.0 * (operator_panning - 0.5);

                let right_tendency = pan_transformed.max(0.0);
                let left_tendency = (-pan_transformed).max(0.0);

                let mono = left_chain + right_chain;

                output_channels[0].chain = left_chain * (1.0 - left_tendency) + left_tendency * mono;
                output_channels[1].chain = right_chain * (1.0 - right_tendency) + right_tendency * mono;
            }

            for i in 0..2 {
                let new_signal = if volume_on {
                    envelope_volume * match operator.wave_type.0 {
                        WaveType::Sine => {
                            let phase_increment = (operator_frequency / sample_rate.0) * TAU;
                            let new_phase = note.operators[operator_index].last_phase.0 + phase_increment;

                            // Only do feedback calculation if feedback is on
                            let new_feedback = {
                                if operator_feedback > ZERO_VALUE_LIMIT {
                                    operator_feedback * new_phase.sin()
                                }
                                else {
                                    0.0
                                }
                            };

                            let chain_input = if operator_index == 0 {
                                output_channels[i].chain + output_channels[i].skip_chain
                            } else {
                                output_channels[i].chain
                            };

                            let signal = (
                                new_phase +
                                operator_modulation_index *
                                (chain_input + new_feedback)
                            ).sin();

                            note.operators[operator_index].last_phase.0 = new_phase;

                            signal
                        },
                        WaveType::WhiteNoise => {
                            (rng.gen::<f64>() - 0.5) * 2.0
                        }
                    }
                }
                else {
                    0.0
                };

                let pan_volume = if i == 0 {
                    pan_left
                } else {
                    pan_right
                };

                output_channels[i].additive += operator_additive *
                    operator_volume * pan_volume * new_signal;
                
                // output_channels[i].skip_chain += operator_skip_chain *
                //     operator_volume * pan_volume * new_signal * (1.0 - operator_additive);

                output_channels[i].chain =
                    output_channels[i].chain * operator_additive +
                    operator_volume * pan_volume * new_signal * (1.0 - operator_additive);
            }
        }

        let signal_left = output_channels[0].chain + output_channels[0].additive;
        let signal_right = output_channels[1].chain + output_channels[1].additive;

        let volume_factor = 0.1 * note.velocity.0;

        (signal_left * volume_factor, signal_right * volume_factor)
    }

    /// MIDI keyboard support

    pub fn process_midi_event(&mut self, data: [u8; 3]) {
        match data[0] {
            128 => self.note_off(data[1]),
            144 => self.note_on(data[1], data[2]),
            _   => ()
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: u8) {
        let mut note_clone = self.processing.notes[pitch as usize].clone();

        if note_clone.active {
            note_clone.release();

            self.processing.fadeout_notes.push(note_clone);
        }

        self.processing.notes[pitch as usize].press(velocity);
    }

    fn note_off(&mut self, pitch: u8) {
        self.processing.notes[pitch as usize].release();
    }

    fn fetch_bpm(&mut self){
        // Use TEMPO_VALID constant content as mask directly because
        // of problems with using TimeInfoFlags
        if let Some(time_info) = self.sync_only.host.get_time_info(1 << 10) {
            self.processing.bpm = BeatsPerMinute(time_info.tempo);
        }
    }
}


impl Plugin for FmSynth {

    fn process(&mut self, audio_buffer: &mut AudioBuffer<f32>){
        let time_per_sample = self.time_per_sample();

        let outputs = audio_buffer.split().1;

        for (output_sample_left, output_sample_right) in outputs.get_mut(0)
            .iter_mut()
            .zip(outputs.get_mut(1).iter_mut()) {

            *output_sample_left = 0.0;
            *output_sample_right = 0.0;

            let mut automatable = self.processing.automatable.lock();

            for note in self.processing.notes.iter_mut()
                .chain(self.processing.fadeout_notes.iter_mut()){

                if note.active {
                    let (out_left, out_right) = Self::generate_note_sample(
                        &mut self.processing.rng,
                        self.processing.global_time,
                        self.processing.sample_rate,
                        automatable.master_frequency,
                        &mut automatable.operators,
                        note,
                    );

                    *output_sample_left += Self::limit(out_left) as f32;
                    *output_sample_right += Self::limit(out_right) as f32;

                    note.deactivate_if_finished();

                    note.duration.0 += time_per_sample;
                }
            }

            self.processing.global_time.0 += time_per_sample;
        }

        self.processing.fadeout_notes.retain(|note| note.active);
    }

    fn new(host: HostCallback) -> Self {
        let mut operators = smallvec![];

        for operator_index in 0..NUM_OPERATORS {
            operators.push(Operator::new(operator_index));
        }

        let mut notes = SmallVec::new();

        for i in 0..128 {
            notes.push(Note::new(MidiPitch(i)));
        }

        let parameters = Parameters::new();

        let automatable = Arc::new(Mutex::new(AutomatableState {
            master_frequency: MasterFrequency(440.0),
            operators: operators,
        }));

        let sync_only = Arc::new(SyncOnlyState {
            host: host,
            parameters: parameters,
            automatable: automatable.clone(),
        });

        let processing = ProcessingState {
            global_time: TimeCounter(0.0),
            sample_rate: SampleRate(44100.0),
            bpm: BeatsPerMinute(120.0),
            rng: SmallRng::from_entropy(),
            notes: notes,
            fadeout_notes: SmallVec::new(),
            automatable: automatable.clone(),
        };

        Self {
            processing,
            sync_only
        }
    }

    fn get_info(&self) -> Info {
        Info {
            name: "FM".to_string(),
            vendor: "Joakim Frostegård".to_string(),
            unique_id: 43789,
            category: Category::Synth,
            inputs: 0,
            outputs: 2,
            parameters: self.sync_only.parameters.len() as i32,
            initial_delay: 0,
            ..Info::default()
        }
    }

	fn init(&mut self) {
        let log_folder = dirs::home_dir().unwrap().join("tmp");

        let _ = ::std::fs::create_dir(log_folder.clone());

		let log_file = ::std::fs::File::create(
            log_folder.join("rust-vst.log")
        ).unwrap();

		let _ = simplelog::CombinedLogger::init(vec![
            simplelog::WriteLogger::new(
                simplelog::LogLevelFilter::Info,
                simplelog::Config::default(),
                log_file
            )
        ]);

        log_panics::init();

		info!("init");

        self.fetch_bpm();
	}

    fn process_events(&mut self, events: &Events) {
        for event in events.events() {
            if let Event::Midi(ev) = event {
                self.process_midi_event(ev.data);
            } 
        }
    }

    fn set_sample_rate(&mut self, rate: f32) {
        self.processing.sample_rate = SampleRate(f64::from(rate));
    }

    fn can_do(&self, can_do: CanDo) -> Supported {
        match can_do {
            CanDo::ReceiveMidiEvent | CanDo::ReceiveTimeInfo
            | CanDo::SendEvents | CanDo::ReceiveEvents => Supported::Yes,
            _ => Supported::Maybe,
        }
    }

    fn get_parameter_object(&mut self) -> Arc<PluginParameters> {
        Arc::clone(&self.sync_only) as Arc<PluginParameters>
    }
}


impl PluginParameters for SyncOnlyState {

    /// Get parameter label for parameter at `index` (e.g. "db", "sec", "ms", "%").
    fn get_parameter_label(&self, index: i32) -> String {
        self.parameters.get(index as usize)
            .map_or("".to_string(), |p| p.get_unit_of_measurement(&self.automatable.lock()))
    }

    /// Get the parameter value for parameter at `index` (e.g. "1.0", "150", "Plate", "Off").
    fn get_parameter_text(&self, index: i32) -> String {
        self.parameters.get(index as usize)
            .map_or("".to_string(), |p| p.get_value_text(&self.automatable.lock()))
    }

    /// Get the name of parameter at `index`.
    fn get_parameter_name(&self, index: i32) -> String {
        self.parameters.get(index as usize)
            .map_or("".to_string(), |p| p.get_name(&self.automatable.lock()))
    }

    /// Get the value of paramater at `index`. Should be value between 0.0 and 1.0.
    fn get_parameter(&self, index: i32) -> f32 {
        self.parameters.get(index as usize)
            .map_or(0.0, |p| p.get_value_float(&self.automatable.lock())) as f32
    }

    /// Set the value of parameter at `index`. `value` is between 0.0 and 1.0.
    fn set_parameter(&self, index: i32, value: f32) {
        if let Some(p) = self.parameters.get(index as usize) {
            p.set_value_float(&mut self.automatable.lock(), f64::from(value).min(1.0).max(0.0))
        }
    }

    /// Use String as input for parameter value. Used by host to provide an editable field to
    /// adjust a parameter value. E.g. "100" may be interpreted as 100hz for parameter. Returns if
    /// the input string was used.
    fn string_to_parameter(&self, index: i32, text: String) -> bool {
        if let Some(p) = self.parameters.get(index as usize){
            p.set_value_text(&mut self.automatable.lock(), text)
        }
        else {
            false
        }
    }

    /// Return whether parameter at `index` can be automated.
    fn can_be_automated(&self, index: i32) -> bool {
        self.parameters.get(index as usize).is_some()
    }
}

plugin_main!(FmSynth);