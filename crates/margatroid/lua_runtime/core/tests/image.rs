use image::{DynamicImage, GenericImageView, ImageFormat, RgbImage, RgbaImage};
use lua_runtime_core::host::{self, HostDefinition, HostKind, Margatroid};
use mlua::{Lua, Table};
use serde_json::json;
use std::io::Cursor;

fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let image = if alpha {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                100,
                if x % 2 == 0 { 0 } else { 127 },
            ])
        }))
    } else {
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([
                ((x * 41 + y * 13) % 256) as u8,
                ((x * 7 + y * 59) % 256) as u8,
                ((x * 31 + y * 23) % 256) as u8,
            ])
        }))
    };
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn fixture(kind: HostKind) -> (Lua, mlua::AnyUserData) {
    let lua = Lua::new();
    let object = Margatroid::install(
        &lua,
        HostDefinition {
            kind,
            context: json!({}),
            arguments: json!({}),
        },
    )
    .unwrap();
    let callbacks = lua.create_table().unwrap();
    host::data::install(&lua, &callbacks, std::env::temp_dir()).unwrap();
    Margatroid::enable(&object, callbacks).unwrap();
    (lua, object)
}

#[test]
fn previews_preserve_aspect_and_never_enlarge_small_images_in_either_host() {
    for kind in [HostKind::Driver, HostKind::Tool] {
        let (lua, _object) = fixture(kind);
        let original = png(2000, 1000, false);
        lua.globals()
            .set("bytes", lua.create_string(&original).unwrap())
            .unwrap();
        let preview: Table = lua
            .load("return margatroid.data:image_prepare(bytes)")
            .eval()
            .unwrap();
        let width = preview.get::<u32>("width").unwrap();
        let height = preview.get::<u32>("height").unwrap();
        assert!(width <= 1280 && height <= 640 && width.abs_diff(height * 2) <= 1);
        assert_eq!(preview.get::<u32>("original_width").unwrap(), 2000);
        assert_eq!(preview.get::<String>("media_type").unwrap(), "image/jpeg");
        let data = preview.get::<mlua::String>("data").unwrap();
        assert!(data.as_bytes().len() <= 307200);
        assert_eq!(
            image::load_from_memory(&data.as_bytes())
                .unwrap()
                .dimensions(),
            (width, height)
        );
        let small = png(20, 10, false);
        lua.globals()
            .set("bytes", lua.create_string(small).unwrap())
            .unwrap();
        lua.load("local value=margatroid.data:image_prepare(bytes);assert(value.width==20 and value.height==10);assert(not pcall(function()margatroid.data.image_prepare=function()end end))").exec().unwrap();
    }
}

#[test]
fn previews_shrink_further_for_small_targets_and_preserve_alpha() {
    let (lua, _object) = fixture(HostKind::Tool);
    lua.globals()
        .set("bytes", lua.create_string(png(1000, 500, false)).unwrap())
        .unwrap();
    let preview:Table=lua.load("return margatroid.data:image_prepare(bytes,{max_dimension=800,target_bytes=2000,quality=90})").eval().unwrap();
    let data = preview.get::<mlua::String>("data").unwrap();
    assert!(data.as_bytes().len() <= 2000);
    assert!(preview.get::<u32>("width").unwrap() < 800);
    let width = preview.get::<u32>("width").unwrap();
    let height = preview.get::<u32>("height").unwrap();
    assert!(width.abs_diff(height * 2) <= 1);
    lua.globals()
        .set("bytes", lua.create_string(png(40, 20, true)).unwrap())
        .unwrap();
    let alpha: Table = lua
        .load("return margatroid.data:image_prepare(bytes)")
        .eval()
        .unwrap();
    assert_eq!(alpha.get::<String>("media_type").unwrap(), "image/webp");
    let data = alpha.get::<mlua::String>("data").unwrap();
    let image = image::load_from_memory(&data.as_bytes())
        .unwrap()
        .to_rgba8();
    assert!(image.pixels().any(|pixel| pixel.0[3] != 255));
}

#[test]
fn image_options_and_decoder_fail_explicitly_and_configure_remains_closed() {
    let (lua, _object) = fixture(HostKind::Tool);
    lua.globals()
        .set("bytes", lua.create_string(png(10, 10, false)).unwrap())
        .unwrap();
    for options in [
        "{quality=0}",
        "{quality=101}",
        "{target_bytes=0}",
        "{max_dimension=0}",
        "{other=1}",
        "{quality=1.5}",
        "{target_bytes=1}",
    ] {
        assert!(
            lua.load(format!(
                "return margatroid.data:image_prepare(bytes,{options})"
            ))
            .eval::<Table>()
            .is_err(),
            "{options}"
        );
    }
    assert!(
        lua.load("return margatroid.data:image_prepare('not an image')")
            .eval::<Table>()
            .is_err()
    );
    let lua = Lua::new();
    let _object = Margatroid::install(
        &lua,
        HostDefinition {
            kind: HostKind::Driver,
            context: json!({}),
            arguments: json!({}),
        },
    )
    .unwrap();
    let err = lua
        .load("return margatroid.data:image_prepare('x')")
        .eval::<Table>()
        .unwrap_err();
    assert!(err.to_string().contains("configure"));
}
