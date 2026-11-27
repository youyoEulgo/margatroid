//! Pure image-to-preview conversion, shared by both Lua execution environments.
use image::{DynamicImage, GenericImageView, ImageEncoder, ImageFormat, imageops::FilterType};
use mlua::{Lua, MultiValue, Table, Value};

fn invalid(message: &'static str) -> mlua::Error {
    super::error(message)
}

struct Options {
    max_dimension: u32,
    target_bytes: usize,
    quality: u8,
}
impl Options {
    fn parse(value: Value) -> mlua::Result<Self> {
        let mut options = Self {
            max_dimension: 1280,
            target_bytes: 300 * 1024,
            quality: 80,
        };
        let table = match value {
            Value::Nil => return Ok(options),
            Value::Table(table) => table,
            _ => return Err(invalid("image options must be an object")),
        };
        for pair in table.pairs::<Value, Value>() {
            let (key, value) = pair?;
            let Value::String(key) = key else {
                return Err(invalid("image options require named fields"));
            };
            let Value::Integer(value) = value else {
                return Err(invalid("image options must be positive integers"));
            };
            if value <= 0 {
                return Err(invalid("image options must be positive integers"));
            }
            match key.to_str()?.as_ref() {
                "max_dimension" => {
                    options.max_dimension = u32::try_from(value)
                        .map_err(|_| invalid("image dimension is out of range"))?
                }
                "target_bytes" => {
                    options.target_bytes = usize::try_from(value)
                        .map_err(|_| invalid("image target is out of range"))?
                }
                "quality" if value <= 100 => options.quality = value as u8,
                "quality" => return Err(invalid("JPEG quality must be 1 through 100")),
                _ => return Err(invalid("unknown image option")),
            }
        }
        Ok(options)
    }
}
struct Prepared {
    data: Vec<u8>,
    media_type: &'static str,
    width: u32,
    height: u32,
    original_width: u32,
    original_height: u32,
    original_bytes: usize,
}

fn dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_dimension {
        return (width, height);
    }
    let scaled = |side: u32| {
        ((u64::from(side) * u64::from(max_dimension) / u64::from(longest)).max(1)) as u32
    };
    (scaled(width), scaled(height))
}
fn encode(image: &DynamicImage, alpha: bool, quality: u8) -> mlua::Result<Vec<u8>> {
    let mut output = Vec::new();
    if alpha {
        let rgba = image.to_rgba8();
        image::codecs::webp::WebPEncoder::new_lossless(&mut output)
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|_| invalid("WebP preview encoding failed"))?;
    } else {
        let rgb = image.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality)
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|_| invalid("JPEG preview encoding failed"))?;
    }
    Ok(output)
}
fn prepare(bytes: &[u8], options: Options) -> mlua::Result<Prepared> {
    let format =
        image::guess_format(bytes).map_err(|_| invalid("image format is not recognized"))?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(invalid("only PNG, JPEG, GIF and WebP images are supported"));
    }
    let original = image::load_from_memory_with_format(bytes, format)
        .map_err(|_| invalid("image decoding failed"))?;
    let (original_width, original_height) = original.dimensions();
    let alpha = original.to_rgba8().pixels().any(|pixel| pixel.0[3] != 255);
    let (mut width, mut height) =
        dimensions(original_width, original_height, options.max_dimension);
    loop {
        let resized = if (width, height) == original.dimensions() {
            original.clone()
        } else {
            original.resize_exact(width, height, FilterType::Lanczos3)
        };
        let qualities = [
            options.quality,
            options.quality.min(70),
            options.quality.min(60),
        ];
        let mut best: Option<Vec<u8>> = None;
        for quality in qualities {
            let encoded = encode(&resized, alpha, quality)?;
            if encoded.len() <= options.target_bytes {
                return Ok(Prepared {
                    data: encoded,
                    media_type: if alpha { "image/webp" } else { "image/jpeg" },
                    width,
                    height,
                    original_width,
                    original_height,
                    original_bytes: bytes.len(),
                });
            }
            if best
                .as_ref()
                .is_none_or(|current| encoded.len() < current.len())
            {
                best = Some(encoded);
            }
            if alpha {
                break;
            }
        }
        if width == 1 && height == 1 {
            return Err(invalid("image target is too small for an encoded preview"));
        }
        let factor = ((options.target_bytes as f64 / best.as_ref().unwrap().len() as f64).sqrt()
            * 0.9)
            .clamp(0.1, 0.9);
        let longest = width.max(height);
        let next = ((longest as f64 * factor).floor() as u32)
            .max(1)
            .min(longest.saturating_sub(1).max(1));
        (width, height) = dimensions(original_width, original_height, next);
    }
}

pub(super) fn install(lua: &Lua, callbacks: &Table) -> mlua::Result<()> {
    callbacks.set(
        "data_image_prepare",
        lua.create_function(|lua, args: MultiValue| {
            if !(1..=2).contains(&args.len()) {
                return Err(invalid("image_prepare expects bytes and optional options"));
            }
            let mut args = args.into_iter();
            let Some(Value::String(bytes)) = args.next() else {
                return Err(invalid("image_prepare requires an image byte string"));
            };
            let options = Options::parse(args.next().unwrap_or(Value::Nil))?;
            let prepared = prepare(&bytes.as_bytes(), options)?;
            let result = lua.create_table()?;
            result.set("data", lua.create_string(prepared.data)?)?;
            result.set("media_type", prepared.media_type)?;
            result.set("width", prepared.width)?;
            result.set("height", prepared.height)?;
            result.set("original_width", prepared.original_width)?;
            result.set("original_height", prepared.original_height)?;
            result.set("original_bytes", prepared.original_bytes)?;
            Ok(result)
        })?,
    )
}
