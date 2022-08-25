pub mod lfo;

use std::f64::consts::TAU;

use array_init::array_init;
use duplicate::duplicate_item;
use vst::buffer::AudioBuffer;

use crate::audio::parameters::{common::AudioParameter, OperatorAudioParameters};
use crate::audio::voices::log10_table::Log10Table;
use crate::audio::AudioState;
use crate::common::*;
use crate::parameters::operator_wave_type::WaveType;
use crate::parameters::{MasterParameter, ModTargetStorage, OperatorParameter, Parameter};
use crate::simd::*;

use lfo::*;

const MASTER_VOLUME_FACTOR: f64 = 0.2;
const LIMIT: f64 = 10.0;

const MAX_PD_WIDTH: usize = 4;

pub trait AudioGen {
    #[allow(clippy::missing_safety_doc)]
    unsafe fn process_f32(
        octasine: &mut AudioState,
        lefts: &mut [f32],
        rights: &mut [f32],
        position: usize,
    );
}

pub struct AudioGenData {
    lfo_target_values: LfoTargetValues,
    voices: [VoiceData; 128],
}

impl Default for AudioGenData {
    fn default() -> Self {
        Self {
            lfo_target_values: Default::default(),
            voices: array_init(|_| VoiceData::default()),
        }
    }
}

#[derive(Debug, Default)]
struct VoiceData {
    active: bool,
    key_velocity: [f64; MAX_PD_WIDTH],
    /// Master volume is calculated per-voice, since it can be an LFO target
    master_volume: [f64; MAX_PD_WIDTH],
    operators: [VoiceOperatorData; 4],
}

#[derive(Debug, Default)]
struct VoiceOperatorData {
    volume: [f64; MAX_PD_WIDTH],
    mix_out: [f64; MAX_PD_WIDTH],
    mod_out: [f64; MAX_PD_WIDTH],
    feedback: [f64; MAX_PD_WIDTH],
    panning: [f64; MAX_PD_WIDTH],
    constant_power_panning: [f64; MAX_PD_WIDTH],
    envelope_volume: [f64; MAX_PD_WIDTH],
    phase: [f64; MAX_PD_WIDTH],
    wave_type: WaveType,
    modulation_targets: ModTargetStorage,
}

#[inline]
pub fn process_f32_runtime_select(
    audio_state: &mut AudioState,
    audio_buffer: &mut AudioBuffer<f32>,
) {
    let num_samples = audio_buffer.samples();

    let mut outputs = audio_buffer.split().1;
    let lefts = outputs.get_mut(0);
    let rights = outputs.get_mut(1);

    let mut position = 0;

    loop {
        let num_remaining_samples = (num_samples - position) as u64;

        unsafe {
            match num_remaining_samples {
                #[cfg(all(feature = "simd", target_arch = "x86_64"))]
                (2..) if is_x86_feature_detected!("avx") => {
                    let new_position = position + 2;

                    Avx::process_f32(
                        audio_state,
                        &mut lefts[position..new_position],
                        &mut rights[position..new_position],
                        position,
                    );

                    position = new_position;
                }
                1.. => {
                    let new_position = position + 1;

                    cfg_if::cfg_if!(
                        if #[cfg(feature = "simd")] {
                            cfg_if::cfg_if!(
                                if #[cfg(target_arch = "x86_64")] {
                                    // SSE2 is always supported on x86_64
                                    Sse2::process_f32(
                                        audio_state,
                                        &mut lefts[position..new_position],
                                        &mut rights[position..new_position],
                                        position,
                                    );
                                } else {
                                    FallbackSleef::process_f32(
                                        audio_state,
                                        &mut lefts[position..new_position],
                                        &mut rights[position..new_position],
                                        position,
                                    );
                                }
                            )
                        } else {
                            FallbackStd::process_f32(
                                audio_state,
                                &mut lefts[position..new_position],
                                &mut rights[position..new_position],
                                position,
                            );
                        }
                    );

                    position = new_position;
                }
                0 => {
                    break;
                }
            }
        }
    }
}

#[duplicate_item(
    [
        S [ FallbackStd ]
        target_feature_enable [ cfg(not(feature = "fake-feature")) ]
        feature_gate [ cfg(not(feature = "fake-feature")) ]
    ]
    [
        S [ FallbackSleef ]
        target_feature_enable [ cfg(not(feature = "fake-feature")) ]
        feature_gate [ cfg(all(feature = "simd")) ]
    ]
    [
        S [ Sse2 ]
        target_feature_enable [ target_feature(enable = "sse2") ]
        feature_gate [ cfg(all(feature = "simd", target_arch = "x86_64")) ]
    ]
    [
        S [ Avx ]
        target_feature_enable [ target_feature(enable = "avx") ]
        feature_gate [ cfg(all(feature = "simd", target_arch = "x86_64")) ]
    ]
)]
mod gen {
    #[feature_gate]
    use super::*;

    #[feature_gate]
    impl AudioGen for S {
        #[target_feature_enable]
        unsafe fn process_f32(
            audio_state: &mut AudioState,
            lefts: &mut [f32],
            rights: &mut [f32],
            position: usize,
        ) {
            assert_eq!(lefts.len(), S::SAMPLES);
            assert_eq!(rights.len(), S::SAMPLES);

            if audio_state.pending_midi_events.is_empty()
                && !audio_state.voices.iter().any(|v| v.active)
            {
                for (l, r) in lefts.iter_mut().zip(rights.iter_mut()) {
                    *l = 0.0;
                    *r = 0.0;
                }

                return;
            }

            extract_voice_data(audio_state, position);
            gen_audio(
                &mut audio_state.rng,
                &audio_state.audio_gen_data,
                lefts,
                rights,
            );
        }
    }

    #[feature_gate]
    #[target_feature_enable]
    unsafe fn extract_voice_data(audio_state: &mut AudioState, position: usize) {
        for voice_data in audio_state.audio_gen_data.voices.iter_mut() {
            voice_data.active = false;
        }

        for sample_index in 0..S::SAMPLES {
            let time_per_sample = audio_state.time_per_sample;

            audio_state
                .parameters
                .advance_one_sample(audio_state.sample_rate);
            audio_state.process_events_for_sample(position + sample_index);

            let operators = &mut audio_state.parameters.operators;
            let lfo_values = &mut audio_state.audio_gen_data.lfo_target_values;

            for (voice, voice_data) in audio_state
                .voices
                .iter_mut()
                .zip(audio_state.audio_gen_data.voices.iter_mut())
                .filter(|(voice, _)| voice.active)
            {
                voice.deactivate_if_envelopes_ended();

                if voice.active {
                    voice_data.active = true;
                } else {
                    // If voice was deactivated this sample in avx mode, ensure that audio isn't
                    // generated for next sample due to lingering data from previous passes. If
                    // voice gets activated though midi events next sample, new data gets written.
                    //
                    // Since we deactivate envelopes the sample after they ended, we know
                    // at this point that valid data was written for the previous sample, meaning
                    // that we don't need to worry about setting it to zero.
                    if (S::SAMPLES == 2) & (sample_index == 0) {
                        for operator in voice_data.operators.iter_mut() {
                            set_value_for_both_channels(&mut operator.envelope_volume, 1, 0.0);
                        }
                    }
                }

                voice.advance_velocity_interpolator_one_sample(audio_state.sample_rate);

                for (operator_index, operator) in operators.iter_mut().enumerate() {
                    voice.operators[operator_index]
                        .volume_envelope
                        .advance_one_sample(
                            &operator.volume_envelope,
                            voice.key_pressed,
                            time_per_sample,
                        );
                }

                update_lfo_target_values(
                    lfo_values,
                    &mut audio_state.parameters.lfos,
                    &mut voice.lfos,
                    audio_state.sample_rate,
                    time_per_sample,
                    audio_state.bpm_lfo_multiplier,
                );

                set_value_for_both_channels(
                    &mut voice_data.key_velocity,
                    sample_index,
                    voice.get_key_velocity().0 as f64,
                );

                const MASTER_VOLUME_INDEX: u8 =
                    Parameter::Master(MasterParameter::Volume).to_index();

                let master_volume = audio_state
                    .parameters
                    .master_volume
                    .get_value_with_lfo_addition(lfo_values.get(MASTER_VOLUME_INDEX));

                set_value_for_both_channels(
                    &mut voice_data.master_volume,
                    sample_index,
                    master_volume as f64,
                );

                const MASTER_FREQUENCY_INDEX: u8 =
                    Parameter::Master(MasterParameter::Frequency).to_index();

                let master_frequency = audio_state
                    .parameters
                    .master_frequency
                    .get_value_with_lfo_addition(lfo_values.get(MASTER_FREQUENCY_INDEX));

                let voice_base_frequency = voice.midi_pitch.get_frequency(master_frequency);

                for (operator_index, operator) in operators.iter_mut().enumerate() {
                    extract_voice_operator_data(
                        &audio_state.log10table,
                        sample_index,
                        operator_index,
                        operator,
                        &mut voice.operators[operator_index],
                        &mut voice_data.operators[operator_index],
                        &lfo_values,
                        time_per_sample,
                        voice_base_frequency,
                    )
                }
            }
        }
    }

    #[feature_gate]
    #[target_feature_enable]
    unsafe fn extract_voice_operator_data(
        log10table: &Log10Table,
        sample_index: usize,
        operator_index: usize,
        operator_parameters: &mut OperatorAudioParameters,
        voice_operator: &mut crate::audio::voices::VoiceOperator,
        operator_data: &mut VoiceOperatorData,
        lfo_values: &LfoTargetValues,
        time_per_sample: TimePerSample,
        voice_base_frequency: f64,
    ) {
        const VOLUME_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::Volume.index_array();
        const MIX_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::MixOut.index_array();
        /// Note: MOD_INDICES index 0 is invalid (0) and must never be used
        const MOD_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::ModOut.index_array();
        const FEEDBACK_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::Feedback.index_array();
        const PANNING_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::Panning.index_array();
        const RATIO_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::FrequencyRatio.index_array();
        const FREE_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::FrequencyFree.index_array();
        const FINE_INDICES: [u8; NUM_OPERATORS] = OperatorParameter::FrequencyFine.index_array();

        assert!(operator_index < NUM_OPERATORS);

        operator_data.wave_type = operator_parameters.wave_type.get_value();

        if let Some(p) = &mut operator_parameters.mod_targets {
            operator_data.modulation_targets = p.get_value();
        }

        let envelope_volume = voice_operator
            .volume_envelope
            .get_volume(log10table, &operator_parameters.volume_envelope);

        set_value_for_both_channels(
            &mut operator_data.envelope_volume,
            sample_index,
            envelope_volume as f64,
        );

        let volume = operator_parameters
            .volume
            .get_value_with_lfo_addition(lfo_values.get(VOLUME_INDICES[operator_index]));

        let volume_active = operator_parameters.active.get_value();

        set_value_for_both_channels(
            &mut operator_data.volume,
            sample_index,
            (volume * volume_active) as f64,
        );

        let mix_out = operator_parameters
            .mix_out
            .get_value_with_lfo_addition(lfo_values.get(MIX_INDICES[operator_index]));

        set_value_for_both_channels(&mut operator_data.mix_out, sample_index, mix_out as f64);

        let mod_out = operator_parameters.mod_out.as_mut().map_or(0.0, |p| {
            p.get_value_with_lfo_addition(lfo_values.get(MOD_INDICES[operator_index]))
        });

        set_value_for_both_channels(&mut operator_data.mod_out, sample_index, mod_out as f64);

        let feedback = operator_parameters
            .feedback
            .get_value_with_lfo_addition(lfo_values.get(FEEDBACK_INDICES[operator_index]));

        set_value_for_both_channels(&mut operator_data.feedback, sample_index, feedback as f64);

        let panning = operator_parameters
            .panning
            .get_value_with_lfo_addition(lfo_values.get(PANNING_INDICES[operator_index]));

        set_value_for_both_channels(&mut operator_data.panning, sample_index, panning as f64);

        {
            let [l, r] = operator_parameters.panning.left_and_right;

            let sample_index_offset = sample_index * 2;

            operator_data.constant_power_panning[sample_index_offset] = l as f64;
            operator_data.constant_power_panning[sample_index_offset + 1] = r as f64;
        }

        let frequency_ratio = operator_parameters
            .frequency_ratio
            .get_value_with_lfo_addition(lfo_values.get(RATIO_INDICES[operator_index]));
        let frequency_free = operator_parameters
            .frequency_free
            .get_value_with_lfo_addition(lfo_values.get(FREE_INDICES[operator_index]));
        let frequency_fine = operator_parameters
            .frequency_fine
            .get_value_with_lfo_addition(lfo_values.get(FINE_INDICES[operator_index]));

        let frequency =
            voice_base_frequency * frequency_ratio.value * frequency_free * frequency_fine;
        let new_phase = voice_operator.last_phase.0 + frequency * time_per_sample.0;

        set_value_for_both_channels(&mut operator_data.phase, sample_index, new_phase);

        // Save phase
        voice_operator.last_phase.0 = new_phase;
    }

    #[feature_gate]
    #[target_feature_enable]
    unsafe fn gen_audio(
        rng: &mut fastrand::Rng,
        audio_gen_data: &AudioGenData,
        audio_buffer_lefts: &mut [f32],
        audio_buffer_rights: &mut [f32],
    ) {
        // S::SAMPLES * 2 because of two channels. Even index = left channel
        let mut mix_out_sum = S::pd_setzero();

        for voice_data in audio_gen_data
            .voices
            .iter()
            .filter(|voice_data| voice_data.active)
        {
            let operator_generate_audio = run_operator_dependency_analysis(voice_data);

            // Voice modulation input storage, indexed by operator
            let mut voice_modulation_inputs = [S::pd_setzero(); 4];

            let key_velocity = S::pd_loadu(voice_data.key_velocity.as_ptr());
            let master_volume = S::pd_loadu(voice_data.master_volume.as_ptr());

            // Go through operators downwards, starting with operator 4
            for operator_index in (0..4).map(|i| 3 - i) {
                // Possibly skip generation based on previous dependency analysis
                if !operator_generate_audio[operator_index] {
                    continue;
                }

                let operator_voice_data = &voice_data.operators[operator_index];

                let (mix_out, mod_out) = gen_voice_operator_audio(
                    rng,
                    operator_voice_data,
                    voice_modulation_inputs[operator_index],
                    key_velocity,
                );

                // Apply master volume
                let mix_out = S::pd_mul(mix_out, master_volume);

                mix_out_sum = S::pd_add(mix_out_sum, mix_out);

                // Add modulation output to target operators' modulation inputs
                for target in operator_voice_data.modulation_targets.active_indices() {
                    voice_modulation_inputs[target] =
                        S::pd_add(voice_modulation_inputs[target], mod_out);
                }
            }
        }

        // Apply master volume factor and hard limit

        mix_out_sum = S::pd_mul(mix_out_sum, S::pd_set1(MASTER_VOLUME_FACTOR));
        mix_out_sum = S::pd_min(mix_out_sum, S::pd_set1(LIMIT));
        mix_out_sum = S::pd_max(mix_out_sum, S::pd_set1(-LIMIT));

        // Write additive outputs to audio buffer

        let mut out = [0.0f64; S::PD_WIDTH];

        S::pd_storeu(out.as_mut_ptr(), mix_out_sum);

        for sample_index in 0..S::SAMPLES {
            let sample_index_offset = sample_index * 2;

            audio_buffer_lefts[sample_index] = out[sample_index_offset] as f32;
            audio_buffer_rights[sample_index] = out[sample_index_offset + 1] as f32;
        }
    }

    #[feature_gate]
    #[target_feature_enable]
    unsafe fn gen_voice_operator_audio(
        rng: &mut fastrand::Rng,
        operator_data: &VoiceOperatorData,
        modulation_inputs: <S as Simd>::PackedDouble,
        key_velocity: <S as Simd>::PackedDouble,
    ) -> (<S as Simd>::PackedDouble, <S as Simd>::PackedDouble) {
        let sample = if operator_data.wave_type == WaveType::WhiteNoise {
            let mut random_numbers = [0.0f64; S::PD_WIDTH];

            for sample_index in 0..S::SAMPLES {
                let random = rng.f64();

                let sample_index_offset = sample_index * 2;

                random_numbers[sample_index_offset] = random;
                random_numbers[sample_index_offset + 1] = random;
            }

            let random_numbers = S::pd_loadu(random_numbers.as_ptr());

            // Convert random numbers to range -1.0 to 1.0
            S::pd_mul(S::pd_set1(2.0), S::pd_sub(random_numbers, S::pd_set1(0.5)))
        } else {
            let phase = S::pd_mul(S::pd_loadu(operator_data.phase.as_ptr()), S::pd_set1(TAU));

            let feedback = S::pd_mul(
                key_velocity,
                S::pd_mul(
                    S::pd_loadu(operator_data.feedback.as_ptr()),
                    S::pd_fast_sin(phase),
                ),
            );

            S::pd_fast_sin(S::pd_add(phase, S::pd_add(feedback, modulation_inputs)))
        };

        let sample = S::pd_mul(sample, key_velocity);
        let sample = S::pd_mul(sample, S::pd_loadu(operator_data.volume.as_ptr()));
        let sample = S::pd_mul(sample, S::pd_loadu(operator_data.envelope_volume.as_ptr()));

        // Mix channels depending on panning of current operator. If panned to
        // the middle, just pass through the stereo signals. If panned to any
        // side, mix out the original stereo signals and mix in mono.
        let sample = {
            let pan = S::pd_loadu(operator_data.panning.as_ptr());

            // Get panning as value between -1 and 1
            let pan = S::pd_mul(S::pd_set1(2.0), S::pd_sub(pan, S::pd_set1(0.5)));

            let pan_tendency = S::pd_max(
                S::pd_mul(pan, S::pd_distribute_left_right(-1.0, 1.0)),
                S::pd_setzero(),
            );
            let one_minus_pan_tendency = S::pd_sub(S::pd_set1(1.0), pan_tendency);

            let mono = S::pd_mul(S::pd_pairwise_horizontal_sum(sample), S::pd_set1(0.5));

            S::pd_add(
                S::pd_mul(pan_tendency, mono),
                S::pd_mul(one_minus_pan_tendency, sample),
            )
        };

        let mix_out = {
            let pan_factor = S::pd_loadu(operator_data.constant_power_panning.as_ptr());

            S::pd_mul(
                S::pd_mul(sample, pan_factor),
                S::pd_loadu(operator_data.mix_out.as_ptr()),
            )
        };
        let mod_out = {
            let pan_factor = {
                let factor = S::pd_loadu(operator_data.panning.as_ptr());
                let factor = S::pd_interleave(S::pd_sub(S::pd_set1(1.0), factor), factor);
                let factor = S::pd_mul(factor, S::pd_set1(2.0));

                S::pd_min(factor, S::pd_set1(1.0))
            };

            S::pd_mul(
                S::pd_mul(sample, pan_factor),
                S::pd_loadu(operator_data.mod_out.as_ptr()),
            )
        };

        (mix_out, mod_out)
    }

    /// Operator dependency analysis to allow skipping audio generation when possible
    #[feature_gate]
    #[target_feature_enable]
    unsafe fn run_operator_dependency_analysis(voice_data: &VoiceData) -> [bool; 4] {
        let mut operator_generate_audio = [true; 4];
        let mut operator_mix_out_active = [false; 4];

        for operator_index in 0..4 {
            let volume = S::pd_loadu(voice_data.operators[operator_index].volume.as_ptr());
            let mix_out = S::pd_loadu(voice_data.operators[operator_index].mix_out.as_ptr());
            let mod_out = S::pd_loadu(voice_data.operators[operator_index].mod_out.as_ptr());

            let volume_active = S::pd_any_over_zero(volume);
            let mix_out_active = S::pd_any_over_zero(mix_out);
            let mod_out_active = S::pd_any_over_zero(mod_out);

            operator_generate_audio[operator_index] =
                volume_active & (mod_out_active | mix_out_active);
            operator_mix_out_active[operator_index] = mix_out_active;
        }

        for operator_index in 1..4 {
            let all_targets_inactive = voice_data.operators[operator_index]
                .modulation_targets
                .active_indices()
                .all(|mod_target| !operator_generate_audio[mod_target]);

            if all_targets_inactive & !operator_mix_out_active[operator_index] {
                operator_generate_audio[operator_index] = false;
            }
        }

        operator_generate_audio
    }

    #[feature_gate]
    #[target_feature_enable]
    unsafe fn set_value_for_both_channels(
        target: &mut [f64; MAX_PD_WIDTH],
        sample_index: usize,
        value: f64,
    ) {
        let offset = sample_index * 2;

        target[offset] = value;
        target[offset + 1] = value;
    }
}
