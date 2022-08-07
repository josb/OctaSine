use super::utils::*;
use super::ParameterValue;

const LFO_FREQUENCY_FREE_STEPS: [f32; 7] = [1.0 / 16.0, 0.5, 0.9, 1.0, 1.1, 2.0, 16.0];

#[derive(Debug, Clone, Copy)]
pub struct LfoFrequencyFreeValue(pub f64);

impl Default for LfoFrequencyFreeValue {
    fn default() -> Self {
        Self(1.0)
    }
}

impl ParameterValue for LfoFrequencyFreeValue {
    type Value = f64;

    fn new_from_audio(value: Self::Value) -> Self {
        Self(value)
    }
    fn new_from_text(text: String) -> Option<Self> {
        const MIN: f32 = LFO_FREQUENCY_FREE_STEPS[0];
        const MAX: f32 = LFO_FREQUENCY_FREE_STEPS[LFO_FREQUENCY_FREE_STEPS.len() - 1];

        parse_valid_f32(text, MIN, MAX).map(|v| Self(v.into()))
    }
    fn get(self) -> Self::Value {
        self.0
    }
    fn new_from_patch(value: f32) -> Self {
        Self(map_patch_to_audio_value_with_steps(&LFO_FREQUENCY_FREE_STEPS, value) as f64)
    }
    fn to_patch(self) -> f32 {
        map_audio_to_patch_value_with_steps(&LFO_FREQUENCY_FREE_STEPS, self.0 as f32)
    }
    fn get_formatted(self) -> String {
        format!("{:.04}", self.0)
    }
}
