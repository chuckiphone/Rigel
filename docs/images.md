# Images in a turn

A user turn can carry text and images together. The text stays the turn's prompt; image parts ride
on the same user message. A turn with no images is unchanged on the wire.

## Submitting one

| Surface | How |
|---|---|
| CLI | `rigel agent run --prompt "read the sign" --image ./sign.png` (repeat `--image`) |
| HTTP API | `POST /v1/sessions/{id}/turns` with `"images": ["..."]` beside `"prompt"` |
| Desktop composer | The add-file picker copies files into `.rigel/attachments`; image paths in that list are also sent as image parts |

An image reference can be:

* an `https://` URL, sent to the provider as-is;
* a `data:image/...;base64,...` URL;
* a path inside the project root, absolute or project-relative. The desktop attachments live at
  `.rigel/attachments/`. The file is read once, checked by its leading bytes (PNG, JPEG, GIF,
  WebP, AVIF or BMP), capped at 8 MB, and inlined as a `data:` URL.

A path outside the project, an `http://` or `file://` URL, and a non-image file (even one named
`.png`) are refused before the model call. The project-file rule means an image the user did not
name cannot be discovered and sent by the agent.

## Wire shape

OpenAI-compatible adapters (OpenRouter, NVIDIA NIM, Gemini AI Studio, Vertex, and the vendor
rows) send the standard content array:

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "read the traffic sign in this image" },
    { "type": "image_url", "image_url": { "url": "https://example.com/sign.png" } }
  ]
}
```

A text-only message serializes exactly as it did before: `"content": "..."` and no empty image
field. The adapter-level dispatcher is covered by `agent::openrouter::tests`, including a stub
transport that parses the actual request body.

## Provider support

| Adapter | Images |
|---|---|
| OpenAI-compatible (`OpenRouterModel`) | yes — `image_url` content parts |
| Anthropic Messages | **no** — returns `ModelError::ImagesNotSupported` before sending |
| Remote `rigel` relay | **no** — returns `ImagesNotSupported` before sending |

A model on an OpenAI-compatible door may still reject an image with its own 400; that is a model
capability, not a dropped input. The picker's `vision` capability is the catalogue's published
signal, and the request is the final authority.

## Storage and remaining gaps

* The first user message keeps its image parts in memory and in a paused-turn snapshot, so a
  pause/resume keeps them. The `turns` row itself stores only the text prompt; a later turn's
  rebuilt history does not re-send earlier images, and retrying a completed image turn from the
  transcript starts with text only.
* There is no downscale. An image below the 8 MB cap is sent at its original dimensions; a
  provider's own image-token limits may still reject it.
* The Anthropic image block and Gemini's native `inline_data` encoder are not built. Those
  adapters fail explicitly rather than silently omitting the image.
* The desktop composer marks every attached image as an image part by extension, but the model
  still receives the attachment paths in the text preamble. The tool can read text attachments;
  it cannot read image bytes through `read_file`.

Tests: `agent::image::tests` (local MIME, path jail, URL rules, inline data), the OpenRouter body
and stub-transport tests, `agent::anthropic::tests::an_image_is_refused_by_this_adapter_before_the_wire`,
`tests/native.rs` (image turn reaches the first user message; a text-only model refuses before a
turn row), and `store::paused::tests` (image parts survive a pause).
