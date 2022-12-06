use iced_baseview::widget::canvas::{Frame, Path, Stroke};
use iced_baseview::{Color, Point};
use palette::gradient::Gradient;
use palette::Srgba;

use crate::gui::interface::SnapPoint;

use super::StyleSheet;

pub struct MixOutLine {
    path: Path,
    line_color: Color,
    calculated_color: Color,
}

impl MixOutLine {
    pub fn new(from: Point, to_y: f32, additive: f32, style_sheet: Box<dyn StyleSheet>) -> Self {
        let mut to = from;

        to.y = to_y;

        let path = Path::line(from.snap(), to.snap());

        let mut line = Self {
            path,
            line_color: style_sheet.appearance().mix_out_line_color,
            calculated_color: Color::TRANSPARENT,
        };

        line.update(additive, style_sheet);

        line
    }

    pub fn update(&mut self, additive: f32, style_sheet: Box<dyn StyleSheet>) {
        self.calculated_color = self.calculate_color(additive, style_sheet);
    }

    fn calculate_color(&self, additive: f32, style_sheet: Box<dyn StyleSheet>) -> Color {
        let bg = style_sheet.appearance().background_color;
        let c = style_sheet.appearance().line_max_color;

        let gradient = Gradient::new(vec![
            Srgba::new(bg.r, bg.g, bg.b, 1.0).into_linear(),
            // Srgba::new(0.23, 0.69, 0.06, 1.0).into_linear(),
            Srgba::new(
                self.line_color.r,
                self.line_color.g,
                self.line_color.b,
                self.line_color.a,
            )
            .into_linear(),
            Srgba::new(c.r, c.g, c.b, 1.0).into_linear(),
        ]);

        let color = gradient.get(additive as f32);
        let color = Srgba::from_linear(color);

        Color::new(color.red, color.green, color.blue, color.alpha)
    }

    pub fn draw(&self, frame: &mut Frame) {
        let stroke = Stroke::default()
            .with_width(3.0)
            .with_color(self.calculated_color);

        frame.stroke(&self.path, stroke);
    }
}
