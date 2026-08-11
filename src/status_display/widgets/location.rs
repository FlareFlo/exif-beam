use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Point, Drawable};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyleBuilder};
use embedded_graphics::text::{Baseline, Text};
use core::fmt::Debug;

pub fn draw_location<D>(
	display: &mut D,
	top_left: Point,
	lat: f64,
	lon: f64,
	font: &MonoFont,
) -> Result<(), D::Error>
where
	D: DrawTarget<Color = BinaryColor>,
	D::Error: Debug,
{
	let txt_style = MonoTextStyleBuilder::new().font(font).text_color(BinaryColor::On).build();

	// Lat
	Text::with_baseline(
		&heapless::format!(10; "N{:>8.5}", lat).unwrap(),
		top_left,
		txt_style,
		Baseline::Top,
	).draw(display)?;

	// Lon
	let yoffs = font.character_size.height as i32;
	Text::with_baseline(
		&heapless::format!(10; "E{:>8.5}", lon).unwrap(),
		top_left + Point::new(0, yoffs),
		txt_style,
		Baseline::Top,
	).draw(display)?;

	Ok(())
}
