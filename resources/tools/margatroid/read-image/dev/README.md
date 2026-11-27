# Read image previews

`read_image` loads a source PNG/JPEG/GIF/WebP without a fixed 5MiB input cutoff and returns a model-facing preview. The source file is never changed. It reads actual image content, not filename extensions; corrupted files fail explicitly. Animated inputs use their first decoded frame.

Default options:

```json
{"path":"image.png","max_dimension":1280,"quality":80,"target_bytes":307200}
```

The preview preserves aspect ratio to pixel rounding. A smaller source is not enlarged. Opaque images become JPEG: try quality80, then at most70 and60; if encoding does not fit the target, shrink dimensions proportionally and retry. Images with nonopaque alpha become lossless WebP so transparency survives; if necessary they also shrink to fit the target. A target too small even for a1×1 encoded image fails rather than returning an oversized image.

`max_dimension` and `target_bytes` accept positive integers; `quality` is1–100. These explicit options can raise detail and byte targets, without a separate hidden preview quota. JPEG quality does not affect lossless alpha WebP. Increasing these options increases context/request bytes. The old source-file5MiB cutoff has been removed; compression runs on the decoded original, and downstream protocol/media checks apply to the generated preview.

The shared pure-Rust `margatroid.data:image_prepare(bytes, options)` performs the conversion in both tool workers and embedded Lua environments. No Python/Pillow, shell command, ImageMagick, helper executable or temporary image file is required. This method performs no filesystem/network IO and is available only after execute capabilities are enabled.

Result text gives original dimensions/bytes and actual preview dimensions, format and byte size. The image is for the model to inspect and answer the user's request; loading it is not delivery to the user. For fine text or a detailed region, explicitly request a larger dimension/target or crop through an appropriate separate workflow. Existing full-size images already in conversation/history are not rewritten by this tool update.
