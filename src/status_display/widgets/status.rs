use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Point, Size, Drawable, Primitive};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::text::{Baseline, Text, Alignment, TextStyleBuilder};
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use core::cmp::min;
use core::fmt::Debug;

#[derive(PartialEq)]
pub enum BoxLevel {
	Info,
	Warn,
	Error,
}

pub fn draw_16_16<D>(l1: &str, l2: &str, top_left: Point, level: BoxLevel, display: &mut D, blink: bool)
where
	D: DrawTarget<Color = BinaryColor>,
	D::Error: Debug,
{
	let (fill_color, text_color) = match level {
		BoxLevel::Info => (BinaryColor::Off, BinaryColor::On),
		BoxLevel::Warn => (BinaryColor::On, BinaryColor::Off),
		BoxLevel::Error => {
			if blink {
				// Blink state: Filled background, inverted text
				(BinaryColor::On, BinaryColor::Off)
			} else {
				// Normal state: Empty background, normal text
				(BinaryColor::Off, BinaryColor::On)
			}
		}
	};

	// Draw rectangle background
	Rectangle::new(top_left, Size::new(16, 16))
		.into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
		.draw(display)
		.unwrap();

	// Draw border
	Rectangle::new(top_left + Point::new(1,1), Size::new(14, 14))
		.into_styled(PrimitiveStyle::with_fill(fill_color))
		.draw(display)
		.unwrap();

	let center_point = top_left + Point::new(7, 2);

	let text_style = TextStyleBuilder::new()
		.alignment(Alignment::Center)
		.baseline(Baseline::Top)
		.build();

	Text::with_text_style(
		&heapless::format!(7; "{}\n{}", &l1[..min(l1.len(), 3)], &l2[..min(l2.len(), 3)]).unwrap(),
		center_point,
		MonoTextStyleBuilder::new().font(&FONT_4X6).text_color(text_color).build(),
		text_style,
	)
		.draw(display)
		.unwrap();
}
