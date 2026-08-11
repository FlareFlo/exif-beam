use chrono::{DateTime, FixedOffset, Timelike};
use embedded_graphics::image::{Image, ImageRaw};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Point, Drawable};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyleBuilder};
use embedded_graphics::text::{Baseline, Text};
use core::fmt::Debug;

pub fn draw_time<D>(
	display: &mut D,
	top_left: Point,
	time: &DateTime<FixedOffset>,
	is_utc: bool,
	font: &MonoFont,
) -> Result<(), D::Error>
where
	D: DrawTarget<Color = BinaryColor>,
	D::Error: Debug,
{
	let txt_style = MonoTextStyleBuilder::new().font(font).text_color(BinaryColor::On).build();

	Text::with_baseline(
		&heapless::format!(30; " {:02}:{:02}:{:02}", time.hour(), time.minute(), time.second()).unwrap(), 
		top_left, 
		txt_style, 
		Baseline::Top
	).draw(display)?;

	// The icon is drawn at a slight offset based on the font height
	Image::new(&if is_utc { UTC_90DEG } else { LOC_90DEG }, top_left + Point::new(0, 1)).draw(display)?;

	Ok(())
}

const UTC_90DEG: ImageRaw<BinaryColor> = ImageRaw::new(&[
	0x88, // #...#... (C right)
	0x88, // #...#... (C middle)
	0xF8, // #####... (C left/back)
	0x00, // ........ (Spacing)
	0x80, // #....... (T right)
	0xF8, // #####... (T stem)
	0x80, // #....... (T left)
	0x00, // ........ (Spacing)
	0xF8, // #####... (U right)
	0x08, // ....#... (U bottom)
	0xF8, // #####... (U left)
], 6);

const LOC_90DEG: ImageRaw<BinaryColor> = ImageRaw::new(&[
	0x88, // #...#... (C right)
	0x88, // #...#... (C middle)
	0xF8, // #####... (C left/back)
	0x00, // ........ (Spacing)
	0xF8, // #####... (O right)
	0x88, // #...#... (O middle)
	0xF8, // #####... (O left)
	0x00, // ........ (Spacing)
	0x08, // ....#... (L right)
	0x08, // ....#... (L middle)
	0xF8, // #####... (L vertical stem)
], 5);
