use crate::constants::*;
use crate::common::*;
use crate::utils::*;


#[derive(Debug, Copy, Clone)]
pub struct OperatorDuration(pub f64);


#[derive(Debug, Copy, Clone)]
pub struct OperatorStepData {
    pub step_size: f64,
    pub steps_remaining: usize,
    pub last_time: NoteTime,
    pub num_steps: usize
}

impl Default for OperatorStepData {
    fn default() -> Self {
        Self {
            step_size: 0.0,
            steps_remaining: 0,
            last_time: NoteTime(0.0),
            num_steps: 32,
        }
    }
}

pub trait InterpolatableValue {
    fn get_value(&mut self, time: NoteTime) -> f64;
    fn set_value(&mut self, value: f64);
}


pub trait AutomatableValue {
    fn set_host_value_float(&mut self, value: f64);
    fn get_host_value_float(&self) -> f64;
    fn get_host_value_text(&self) -> String;
}


#[macro_export]
macro_rules! create_operator_value {
    ($struct_name:ident, $default_value:ident) => {

        #[derive(Debug, Copy, Clone)]
        pub struct $struct_name {
            current_value: f64,
            pub target_value: f64,
            step_data: OperatorStepData,
        }

        impl Default for $struct_name {
            fn default() -> Self {
                Self {
                    current_value: $default_value,
                    target_value: $default_value,
                    step_data: OperatorStepData::default(),
                }
            }
        }

        impl InterpolatableValue for $struct_name {
            fn get_value(&mut self, time: NoteTime) -> f64 {
                if self.step_data.num_steps == 0 {
                    return self.current_value;
                }

                if time != self.step_data.last_time && self.step_data.steps_remaining > 0 {
                    self.current_value += self.step_data.step_size;
                    self.step_data.steps_remaining -= 1;
                    self.step_data.last_time = time;
                }

                self.current_value
            }

            fn set_value(&mut self, value: f64){
                self.target_value = value;

                if self.step_data.num_steps == 0 {
                    self.current_value = value;

                    return;
                }

                if value == self.current_value {
                    self.step_data.steps_remaining = 0;
                }
                else {
                    // Restart stepping process
                    let diff = value - self.current_value;
                    self.step_data.step_size = diff / self.step_data.num_steps as f64;
                    self.step_data.steps_remaining = self.step_data.num_steps;
                }
            }
        }

        impl AutomatableValue for $struct_name {
            fn set_host_value_float(&mut self, value: f64){
                self.set_value(self.from_host_value(value));
            }
            fn get_host_value_float(&self) -> f64 {
                self.to_host_value(self.target_value)
            }
            fn get_host_value_text(&self) -> String {
                format!("{:.2}", self.target_value)
            }
        }
    };  
}


#[macro_export]
macro_rules! create_operator_envelope_value {
    ($struct_name:ident, $default_value:ident) => {

        #[derive(Debug, Copy, Clone)]
        pub struct $struct_name(pub f64);

        impl Default for $struct_name {
            fn default() -> Self {
                $struct_name($default_value)
            }
        }

        impl AutomatableValue for $struct_name {
            fn set_host_value_float(&mut self, value: f64){
                self.0 = self.from_host_value(value);
            }
            fn get_host_value_float(&self) -> f64 {
                self.to_host_value(self.0)
            }
            fn get_host_value_text(&self) -> String {
                format!("{:.2}", self.to_host_value(self.0))
            }
        }
    };  
}



create_operator_value!(OperatorVolume, OPERATOR_DEFAULT_VOLUME);

impl OperatorVolume {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value * 2.0
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value / 2.0
    }
}


create_operator_value!(OperatorSkipChainFactor, OPERATOR_DEFAULT_SKIP_CHAIN_FACTOR);

impl OperatorSkipChainFactor {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_value!(OperatorFrequencyRatio, OPERATOR_DEFAULT_FREQUENCY_RATIO);

impl OperatorFrequencyRatio {
    pub fn from_host_value(&self, value: f64) -> f64 {
        map_host_param_value_to_step(&OPERATOR_RATIO_STEPS[..], value)
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        map_step_to_host_param_value(&OPERATOR_RATIO_STEPS[..], value)
    }
}


create_operator_value!(OperatorFrequencyFree, OPERATOR_DEFAULT_FREQUENCY_FREE);

impl OperatorFrequencyFree {
    pub fn from_host_value(&self, value: f64) -> f64 {
        (value + 0.5).powf(3.0)
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value.powf(1.0/3.0) - 0.5
    }
}


create_operator_value!(OperatorFrequencyFine, OPERATOR_DEFAULT_FREQUENCY_FINE);

impl OperatorFrequencyFine {
    pub fn from_host_value(&self, value: f64) -> f64 {
        (value + 0.5).sqrt()
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value.powf(2.0) - 0.5
    }
}


create_operator_value!(OperatorFeedback, OPERATOR_DEFAULT_FEEDBACK);

impl OperatorFeedback {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_value!(OperatorModulationIndex, OPERATOR_DEFAULT_MODULATION_INDEX);

impl OperatorModulationIndex {
    pub fn from_host_value(&self, value: f64) -> f64 {
        map_host_param_value_to_step_smooth(&OPERATOR_BETA_STEPS[..], value)
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        map_step_to_host_param_value(&OPERATOR_BETA_STEPS[..], value) // TODO: add util for smooth reverse step finding
    }
}


create_operator_envelope_value!(VolumeEnvelopeAttackDuration, OPERATOR_DEFAULT_VOLUME_ENVELOPE_ATTACK_DURATION);

impl VolumeEnvelopeAttackDuration {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_envelope_value!(VolumeEnvelopeAttackValue, OPERATOR_DEFAULT_VOLUME_ENVELOPE_ATTACK_VALUE);

impl VolumeEnvelopeAttackValue {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_envelope_value!(VolumeEnvelopeDecayDuration, OPERATOR_DEFAULT_VOLUME_ENVELOPE_DECAY_DURATION);

impl VolumeEnvelopeDecayDuration {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_envelope_value!(VolumeEnvelopeDecayValue, OPERATOR_DEFAULT_VOLUME_ENVELOPE_DECAY_VALUE);

impl VolumeEnvelopeDecayValue {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


create_operator_envelope_value!(VolumeEnvelopeReleaseDuration, OPERATOR_DEFAULT_VOLUME_ENVELOPE_RELEASE_DURATION);

impl VolumeEnvelopeReleaseDuration {
    pub fn from_host_value(&self, value: f64) -> f64 {
        value
    }
    pub fn to_host_value(&self, value: f64) -> f64 {
        value
    }
}


#[derive(Debug, Copy, Clone)]
pub struct OperatorVolumeEnvelope {
    pub attack_duration: VolumeEnvelopeAttackDuration,
    pub attack_end_value: VolumeEnvelopeAttackValue,
    pub decay_duration: VolumeEnvelopeDecayDuration,
    pub decay_end_value: VolumeEnvelopeDecayValue,
    pub release_duration: VolumeEnvelopeReleaseDuration,
}

impl Default for OperatorVolumeEnvelope {
    fn default() -> Self {
        Self {
            attack_duration: VolumeEnvelopeAttackDuration::default(),
            attack_end_value: VolumeEnvelopeAttackValue::default(),
            decay_duration: VolumeEnvelopeDecayDuration::default(),
            decay_end_value: VolumeEnvelopeDecayValue::default(),
            release_duration: VolumeEnvelopeReleaseDuration::default(),
        }
    }
}


#[derive(Debug, Copy, Clone)]
pub struct Operator {
    pub duration: OperatorDuration,
    pub volume: OperatorVolume,
    pub skip_chain_factor: OperatorSkipChainFactor,
    pub frequency_ratio: OperatorFrequencyRatio,
    pub frequency_free: OperatorFrequencyFree,
    pub frequency_fine: OperatorFrequencyFine,
    pub feedback: OperatorFeedback,
    pub modulation_index: OperatorModulationIndex,
    pub volume_envelope: OperatorVolumeEnvelope,
    pub last_phase: f64,
}

impl Default for Operator {
    fn default() -> Self {
        Self {
            duration: OperatorDuration(0.0),
            skip_chain_factor: OperatorSkipChainFactor::default(),
            volume: OperatorVolume::default(),
            frequency_ratio: OperatorFrequencyRatio::default(),
            frequency_free: OperatorFrequencyFree::default(),
            frequency_fine: OperatorFrequencyFine::default(),
            feedback: OperatorFeedback::default(),
            modulation_index: OperatorModulationIndex::default(),
            volume_envelope: OperatorVolumeEnvelope::default(),
            last_phase: 0.0,
        }
    }
}