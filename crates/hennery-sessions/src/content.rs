//! Prompt content (ACP core §7, plan 6a): what the collector accepts in a
//! prompt, and what it stores of it.
//!
//! A prompt is ACP ContentBlocks, text and images, in order. `check` reads
//! it before anything is written: it refuses another block type, an image
//! of another type or whose bytes are not what it claims, and the limits of
//! ACP core §11. What it accepts it splits into the blocks to store, where
//! an image is the attachment it becomes (`StoredBlock`), the blocks the
//! host is sent, which keep only what was checked, and the images' bytes,
//! for the attachment files.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hennery_proto::rest::StoredBlock;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The image types a prompt may carry (ACP core §7).
pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// The most one image may take, decoded (ACP core §11).
pub const MAX_IMAGE_BYTES: usize = 5 << 20;
/// The most images one prompt may carry (ACP core §11).
pub const MAX_IMAGES: usize = 20;
/// The most a prompt's images may take together, decoded (ACP core §11).
pub const MAX_IMAGES_TOTAL: usize = 16 << 20;
/// The most a prompt request's body may take: 16 MiB of images is 21.4 MiB
/// of base64, and the rest is for the text and the JSON around it. Below
/// the host connection's frame limit (`crate::ws::MAX_FRAME`), so a prompt
/// accepted here always fits in one frame to the host.
pub const PROMPT_BODY_LIMIT: usize = 24 << 20;

const _: () = assert!(
    PROMPT_BODY_LIMIT >= MAX_IMAGES_TOTAL.div_ceil(3) * 4 + (1 << 20),
    "a prompt with the most images allowed must fit in a request body"
);

/// An image of an accepted prompt, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// 64 lowercase hex digits.
    pub sha256: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// An accepted prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// What `turns.content` and the `user_turn` event hold.
    pub stored: Vec<StoredBlock>,
    /// What the host is sent (the review's A1): each block's type and its
    /// text, or its `mimeType` and `data`, and nothing else. Never longer
    /// than the request it came from, so it fits in one frame.
    pub sent: Vec<Value>,
    /// Its images, in order, a repeated one each time it appears.
    pub images: Vec<Image>,
}

impl Checked {
    /// The stored blocks as JSON, for the turn row.
    pub fn stored_json(&self) -> Vec<Value> {
        self.stored
            .iter()
            .map(|block| serde_json::to_value(block).expect("a stored block serializes"))
            .collect()
    }
}

/// Why a prompt is refused (ACP core §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No image, and no text but whitespace: 400 `empty_prompt`.
    Empty,
    /// A block the collector does not accept: 400 `invalid_content`.
    Invalid(String),
    /// Over one of the limits: 413 `content_too_large`.
    TooLarge(String),
}

/// Check a prompt's content, and split what it accepts into the blocks to
/// store, the blocks to send and the images' bytes. Takes the content, so
/// an image's base64 moves into what is sent rather than being copied.
pub fn check(content: Vec<Value>) -> Result<Checked, Refusal> {
    let mut stored = Vec::with_capacity(content.len());
    let mut sent = Vec::with_capacity(content.len());
    let mut images = Vec::new();
    let mut total = 0usize;
    let mut said_something = false;
    for (n, block) in content.into_iter().enumerate() {
        let Value::Object(mut block) = block else {
            return Err(Refusal::Invalid(format!("block {n}: a content block needs a type")));
        };
        let kind = block.get("type").and_then(Value::as_str).map(str::to_owned);
        match kind.as_deref() {
            Some("text") => {
                let Some(Value::String(text)) = block.remove("text") else {
                    return Err(Refusal::Invalid(format!("block {n}: a text block needs its text")));
                };
                said_something |= !text.trim().is_empty();
                stored.push(StoredBlock::Text { text: text.clone() });
                sent.push(json!({ "type": "text", "text": text }));
            }
            Some("image") => {
                if images.len() == MAX_IMAGES {
                    return Err(Refusal::TooLarge(format!("a prompt takes at most {MAX_IMAGES} images")));
                }
                let (image, data) = image(n, &mut block, MAX_IMAGES_TOTAL - total)?;
                total += image.bytes.len();
                stored.push(StoredBlock::Image {
                    mime_type: image.mime.clone(),
                    sha256: image.sha256.clone(),
                    size: image.bytes.len() as u64,
                });
                sent.push(json!({ "type": "image", "mimeType": image.mime, "data": data }));
                images.push(image);
            }
            Some(other) => {
                return Err(Refusal::Invalid(format!(
                    "block {n}: {} blocks are not accepted; a prompt is text and images",
                    shown(other)
                )));
            }
            None => return Err(Refusal::Invalid(format!("block {n}: a content block needs a type"))),
        }
    }
    if !said_something && images.is_empty() {
        return Err(Refusal::Empty);
    }
    Ok(Checked { stored, sent, images })
}

/// Client text echoed in a refusal (the review's O4): at most 40
/// characters of it, quoted.
fn shown(text: &str) -> String {
    let mut cut: String = text.chars().take(40).collect();
    if cut.len() < text.len() {
        cut.push('…');
    }
    format!("{cut:?}")
}

/// Decode and check block `n`, an image, with `room` bytes left of the
/// prompt's total: the image, and its base64 as it came. The size is
/// checked from the base64's length before it is decoded, so an oversized
/// image costs no decoding.
fn image(n: usize, block: &mut Map<String, Value>, room: usize) -> Result<(Image, String), Refusal> {
    let Some(Value::String(mime)) = block.remove("mimeType") else {
        return Err(Refusal::Invalid(format!("block {n}: an image needs its mimeType")));
    };
    if !IMAGE_TYPES.contains(&mime.as_str()) {
        return Err(Refusal::Invalid(format!(
            "block {n}: images must be PNG, JPEG, GIF or WebP, not {}",
            shown(&mime)
        )));
    }
    let Some(Value::String(data)) = block.remove("data") else {
        return Err(Refusal::Invalid(format!("block {n}: an image needs its data, base64")));
    };
    // Base64 is 4 characters for every 3 bytes; padding only shortens it.
    let at_least = (data.len() / 4 * 3).saturating_sub(2);
    if at_least > MAX_IMAGE_BYTES {
        return Err(too_large_image(n));
    }
    if at_least > room {
        return Err(too_large_total());
    }
    let bytes = STANDARD
        .decode(&data)
        .map_err(|_| Refusal::Invalid(format!("block {n}: the image's data is not base64")))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(too_large_image(n));
    }
    if bytes.len() > room {
        return Err(too_large_total());
    }
    if sniff(&bytes) != Some(mime.as_str()) {
        return Err(Refusal::Invalid(format!(
            "block {n}: the image is not the {mime} it says it is"
        )));
    }
    let image = Image {
        sha256: hex::encode(Sha256::digest(&bytes)),
        mime,
        bytes,
    };
    Ok((image, data))
}

fn too_large_image(n: usize) -> Refusal {
    Refusal::TooLarge(format!("block {n}: an image takes at most 5 MiB"))
}

fn too_large_total() -> Refusal {
    Refusal::TooLarge("a prompt's images take at most 16 MiB together".to_string())
}

/// The image type `bytes` begin like, by their signature.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `len` bytes that begin like a `mime` image.
    fn bytes(mime: &str, len: usize) -> Vec<u8> {
        let head: &[u8] = match mime {
            "image/png" => b"\x89PNG\r\n\x1a\n",
            "image/jpeg" => &[0xff, 0xd8, 0xff, 0xe0],
            "image/gif" => b"GIF89a",
            "image/webp" => b"RIFF\0\0\0\0WEBP",
            _ => b"",
        };
        let mut out = head.to_vec();
        out.resize(len.max(head.len()), 7);
        out
    }

    fn image_block(mime: &str, data: &[u8]) -> Value {
        json!({ "type": "image", "mimeType": mime, "data": STANDARD.encode(data) })
    }

    fn refusal(content: Value) -> Refusal {
        check(content.as_array().unwrap().clone()).unwrap_err()
    }

    #[test]
    fn text_and_images_are_stored_in_order_with_the_images_as_attachments() {
        let png = bytes("image/png", 100);
        let checked = check(vec![
            json!({ "type": "text", "text": "look at" }),
            image_block("image/png", &png),
            json!({ "type": "text", "text": "and" }),
            image_block("image/png", &png),
        ])
        .unwrap();
        let sha = hex::encode(Sha256::digest(&png));
        let image = StoredBlock::Image {
            mime_type: "image/png".into(),
            sha256: sha.clone(),
            size: 100,
        };
        assert_eq!(
            checked.stored,
            vec![
                StoredBlock::Text { text: "look at".into() },
                image.clone(),
                StoredBlock::Text { text: "and".into() },
                image,
            ]
        );
        assert_eq!(checked.images.len(), 2);
        assert_eq!(
            (checked.images[0].sha256.as_str(), checked.images[0].bytes.as_slice()),
            (sha.as_str(), &png[..])
        );
        // What is stored never carries the bytes.
        let stored = serde_json::to_string(&checked.stored_json()).unwrap();
        assert!(
            !stored.contains("data") && !stored.contains(&STANDARD.encode(&png)),
            "{stored}"
        );
    }

    /// The review's A1: the host is sent what was checked, and nothing
    /// else. An image's `uri`, and any block's `annotations` or `_meta`,
    /// never reach the agent.
    #[test]
    fn the_host_is_sent_only_what_was_checked() {
        let png = bytes("image/png", 64);
        let data = STANDARD.encode(&png);
        let checked = check(vec![
            json!({ "type": "text", "text": "see", "annotations": { "priority": 1 }, "_meta": { "x": 1 } }),
            json!({ "type": "image", "mimeType": "image/png", "data": data, "uri": "file:///etc/passwd", "_meta": {} }),
        ])
        .unwrap();
        assert_eq!(
            checked.sent,
            vec![
                json!({ "type": "text", "text": "see" }),
                json!({ "type": "image", "mimeType": "image/png", "data": data }),
            ]
        );
    }

    /// The review's O4: client text echoed in a refusal is cut short.
    #[test]
    fn a_refusal_echoes_little_of_what_it_refuses() {
        let long = "x".repeat(10_000);
        for content in [
            json!([{ "type": long.clone() }]),
            json!([{ "type": "image", "mimeType": long.clone(), "data": "AAAA" }]),
        ] {
            let Refusal::Invalid(why) = refusal(content) else {
                panic!("expected invalid");
            };
            assert!(why.len() < 200, "{why}");
        }
    }

    #[test]
    fn every_accepted_type_is_recognised_by_its_signature() {
        for mime in IMAGE_TYPES {
            let checked = check(vec![image_block(mime, &bytes(mime, 64))]).unwrap();
            assert_eq!(checked.images[0].mime, mime);
            assert_eq!(sniff(&bytes(mime, 64)), Some(mime));
        }
        assert_eq!(sniff(b"<svg xmlns"), None);
    }

    #[test]
    fn an_image_that_is_not_what_it_says_is_refused() {
        // A JPEG sent as a PNG, an SVG sent as a PNG, and bytes of no type.
        for (mime, data) in [
            ("image/png", bytes("image/jpeg", 64)),
            ("image/png", b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec()),
            ("image/gif", vec![0u8; 64]),
        ] {
            let why = refusal(json!([image_block(mime, &data)]));
            assert!(matches!(&why, Refusal::Invalid(m) if m.contains("not the")), "{why:?}");
        }
    }

    #[test]
    fn another_image_type_is_refused() {
        for mime in ["image/svg+xml", "image/bmp", "text/html", "IMAGE/PNG"] {
            let why = refusal(json!([image_block(mime, &bytes("image/png", 64))]));
            assert!(
                matches!(&why, Refusal::Invalid(m) if m.contains("PNG, JPEG, GIF or WebP")),
                "{why:?}"
            );
        }
    }

    #[test]
    fn blocks_other_than_text_and_images_are_refused() {
        for block in [
            json!({ "type": "audio", "mimeType": "audio/wav", "data": "AAAA" }),
            json!({ "type": "resource_link", "uri": "file:///etc/passwd", "name": "passwd" }),
            json!({ "type": "resource", "resource": { "uri": "file:///x", "text": "x" } }),
            json!({ "text": "no type" }),
            json!("text"),
        ] {
            let why = refusal(json!([{ "type": "text", "text": "hi" }, block]));
            assert!(
                matches!(&why, Refusal::Invalid(m) if m.starts_with("block 1:")),
                "{why:?}"
            );
        }
    }

    #[test]
    fn malformed_blocks_are_refused() {
        for block in [
            json!({ "type": "text" }),
            json!({ "type": "text", "text": 3 }),
            json!({ "type": "image", "data": "AAAA" }),
            json!({ "type": "image", "mimeType": "image/png" }),
            json!({ "type": "image", "mimeType": "image/png", "data": "not base64!" }),
            // Base64 with line breaks, as some encoders write it.
            json!({ "type": "image", "mimeType": "image/png", "data": "iVBORw0K\nGgo=" }),
            // Without its padding: the engine is strict.
            json!({ "type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo" }),
        ] {
            assert!(matches!(refusal(json!([block])), Refusal::Invalid(_)));
        }
    }

    #[test]
    fn a_prompt_with_nothing_to_say_is_empty() {
        assert_eq!(check(vec![]).unwrap_err(), Refusal::Empty);
        assert_eq!(refusal(json!([{ "type": "text", "text": "" }])), Refusal::Empty);
        assert_eq!(refusal(json!([{ "type": "text", "text": " \n\t" }])), Refusal::Empty);
        // An image alone says something.
        assert!(check(vec![image_block("image/gif", &bytes("image/gif", 16))]).is_ok());
    }

    #[test]
    fn the_limits_hold_exactly() {
        // One image of exactly 5 MiB is accepted; one byte more is not.
        assert!(check(vec![image_block("image/png", &bytes("image/png", MAX_IMAGE_BYTES))]).is_ok());
        let why = refusal(json!([image_block(
            "image/png",
            &bytes("image/png", MAX_IMAGE_BYTES + 1)
        )]));
        assert!(matches!(&why, Refusal::TooLarge(m) if m.contains("5 MiB")), "{why:?}");

        // Twenty images are accepted; twenty-one are not.
        let small = image_block("image/png", &bytes("image/png", 16));
        assert!(check(vec![small.clone(); MAX_IMAGES]).is_ok());
        let why = refusal(Value::Array(vec![small; MAX_IMAGES + 1]));
        assert!(
            matches!(&why, Refusal::TooLarge(m) if m.contains("20 images")),
            "{why:?}"
        );

        // Exactly 16 MiB together is accepted; one byte more is not.
        let five = image_block("image/png", &bytes("image/png", MAX_IMAGE_BYTES));
        let rest = MAX_IMAGES_TOTAL - 3 * MAX_IMAGE_BYTES;
        let mut content = vec![five.clone(), five.clone(), five];
        content.push(image_block("image/png", &bytes("image/png", rest)));
        assert_eq!(check(content.clone()).unwrap().images.len(), 4);
        content.pop();
        content.push(image_block("image/png", &bytes("image/png", rest + 1)));
        let why = check(content).unwrap_err();
        assert!(matches!(&why, Refusal::TooLarge(m) if m.contains("16 MiB")), "{why:?}");
    }

    #[test]
    fn an_oversized_image_is_refused_before_it_is_decoded() {
        // Not base64 at all, and long enough to be over the limit: refused
        // for its size, so it was never decoded.
        let huge = "!".repeat(MAX_IMAGE_BYTES / 3 * 4 + 16);
        let why = refusal(json!([{ "type": "image", "mimeType": "image/png", "data": huge }]));
        assert!(matches!(&why, Refusal::TooLarge(m) if m.contains("5 MiB")), "{why:?}");
    }
}
