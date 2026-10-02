//! Hat logos (kernel spec §5.1; plan 4d-B2). A logo is uploaded as a PNG,
//! at most 64 KiB, and stored and served only as this module re-encodes it:
//! decoded to plain 8-bit pixels, then written afresh as `IHDR`, `IDAT` and
//! `IEND`, so no text, metadata, profile, animation or trailing bytes of the
//! upload survive (the security review's ruling B: no SVG, no WebP). The
//! browser turns any other format into a PNG before it uploads.
//!
//! Memory is bounded before anything is allocated by the image's size: the
//! header alone is read first, and an image over `MAX_SIDE` pixels either way
//! is refused there (the review's A2).

use crate::hats::HatChange;
use crate::hosts::Hosts;
use anyhow::Result;
use base64::Engine;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::io::Cursor;

/// The largest upload, decoded (kernel spec §5.1).
pub const MAX_UPLOAD: usize = 64 * 1024;

/// The widest and tallest logo, in pixels.
pub const MAX_SIDE: u32 = 1024;

/// The largest logo stored, once re-encoded: a small upload can decode to
/// pixels that compress worse than it did.
pub const MAX_STORED: usize = 256 * 1024;

/// The one kind a logo is stored and served as.
pub const MIME_PNG: &str = "image/png";

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// What the decoder may allocate besides the frame: its row buffers. The
/// frame itself is bounded by `MAX_SIDE`, checked from the header first.
const DECODER_LIMIT: usize = 4 << 20;

/// A logo as it is stored: always a PNG of ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    pub bytes: Vec<u8>,
    /// What `GET /api/hats/{id}/logo` answers as its `ETag` (without the
    /// quotes): the first 128 bits of the bytes' SHA-256, in hex.
    pub etag: String,
}

/// A stored logo, as the logo route serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLogo {
    /// Always `MIME_PNG` (the schema's check); the route still answers
    /// only a kind it knows.
    pub mime: String,
    pub bytes: Vec<u8>,
    pub etag: String,
}

/// Why an upload is not a logo. Each has its own status and fixed message
/// at the route; none carries the decoder's words or the upload's bytes
/// (the review's A8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not strict standard base64 (padded, no whitespace, no `data:`).
    NotBase64,
    /// Over `MAX_UPLOAD` decoded, or over the stored cap once re-encoded.
    TooLarge,
    /// Not a PNG by its signature: SVG, WebP, JPEG, GIF, anything else.
    Unsupported,
    /// Over `MAX_SIDE` either way (a side of 0 is refused by the decoder
    /// first, as `Damaged`).
    Dimensions,
    /// The decoder refused it.
    Damaged,
}

/// The upload `data` as a logo: decoded from base64, then re-encoded
/// (`reencode`) under `MAX_STORED`.
pub fn from_upload(data: &str) -> Result<Logo, Refusal> {
    // Checked from the length before anything is decoded (the review's O2).
    if data.len() > MAX_UPLOAD.div_ceil(3) * 4 {
        return Err(Refusal::TooLarge);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| Refusal::NotBase64)?;
    if bytes.len() > MAX_UPLOAD {
        return Err(Refusal::TooLarge);
    }
    reencode(&bytes, MAX_STORED)
}

/// `input`, a PNG, decoded and written afresh: 8-bit, the colour type its
/// pixels have once palettes and transparency are expanded, only `IHDR`,
/// `IDAT` and `IEND`. An animated PNG keeps its first frame. Refused when
/// it is not a PNG, a side is 0 or over `MAX_SIDE`, the decoder fails, or the
/// result is over `max_stored` bytes (a parameter, so a test can reach it:
/// the review's A4).
pub fn reencode(input: &[u8], max_stored: usize) -> Result<Logo, Refusal> {
    if !input.starts_with(SIGNATURE) {
        return Err(Refusal::Unsupported);
    }
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(input), png::Limits { bytes: DECODER_LIMIT });
    // The header alone first, so nothing is sized by it before it is
    // checked (the review's A2).
    let header = decoder.read_header_info().map_err(|_| Refusal::Damaged)?;
    if !(1..=MAX_SIDE).contains(&header.width) || !(1..=MAX_SIDE).contains(&header.height) {
        return Err(Refusal::Dimensions);
    }
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let mut reader = decoder.read_info().map_err(|_| Refusal::Damaged)?;
    let size = reader.output_buffer_size().ok_or(Refusal::Damaged)?;
    let mut pixels = vec![0; size];
    // The first frame; the encoder refuses pixels of any other length
    // than the frame it is given (the review's A3).
    let frame = reader.next_frame(&mut pixels).map_err(|_| Refusal::Damaged)?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, frame.width, frame.height);
    encoder.set_color(frame.color_type);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|_| Refusal::Damaged)?;
    writer.write_image_data(&pixels).map_err(|_| Refusal::Damaged)?;
    writer.finish().map_err(|_| Refusal::Damaged)?;
    if bytes.len() > max_stored {
        return Err(Refusal::TooLarge);
    }
    let etag = hex::encode(&Sha256::digest(&bytes)[..16]);
    Ok(Logo { bytes, etag })
}

impl Hosts {
    /// `hat_id`'s logo, if the owner has that hat and it has one.
    pub fn hat_logo(&self, hat_id: &str) -> Result<Option<StoredLogo>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT logo_mime, logo_bytes, logo_etag FROM hats
                 WHERE id = ?1 AND owner_id = ?2 AND logo_bytes IS NOT NULL",
                [hat_id, self.owner_id()],
                |r| {
                    Ok(StoredLogo {
                        mime: r.get(0)?,
                        bytes: r.get(1)?,
                        etag: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Give `hat_id` `logo`, replacing any it had: `Done` with the hat as it
    /// is now; `NotFound`; or `Purging` for a hat frozen for its purge,
    /// which keeps the logo it had. The freeze is checked in the statement
    /// that would set it (plan 9c decision 10c).
    pub fn set_hat_logo(&self, hat_id: &str, logo: &Logo) -> Result<HatChange> {
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let set = tx.execute(
            "UPDATE hats SET logo_mime = ?3, logo_bytes = ?4, logo_etag = ?5
             WHERE id = ?1 AND owner_id = ?2
                 AND NOT EXISTS (SELECT 1 FROM purged_hats p WHERE p.hat_id = hats.id AND p.owner_id = ?2)",
            params![hat_id, owner, MIME_PNG, logo.bytes, logo.etag],
        )?;
        let hat = crate::hats::hat_in(&tx, owner, hat_id)?;
        tx.commit()?;
        Ok(match hat {
            Some(hat) if set == 1 => HatChange::Done(hat),
            Some(_) => HatChange::Purging,
            None => HatChange::NotFound,
        })
    }

    /// Take `hat_id`'s logo away, if it has one: `Done` with the hat as it
    /// is now, or `NotFound`. A frozen hat's logo goes too: that is the
    /// purge's own direction (the review's A6).
    pub fn clear_hat_logo(&self, hat_id: &str) -> Result<HatChange> {
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE hats SET logo_mime = NULL, logo_bytes = NULL, logo_etag = NULL
             WHERE id = ?1 AND owner_id = ?2",
            [hat_id, owner],
        )?;
        let hat = crate::hats::hat_in(&tx, owner, hat_id)?;
        tx.commit()?;
        Ok(hat.map_or(HatChange::NotFound, HatChange::Done))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use png::{BitDepth, ColorType};

    /// PNG's CRC-32 (ISO 3309), bit by bit: test fixtures only.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = kind.to_vec();
        crc.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc).to_be_bytes());
        out
    }

    /// `data` as a zlib stream of stored (uncompressed) blocks.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(65535).collect()
        };
        for (n, block) in blocks.iter().enumerate() {
            out.push(u8::from(n + 1 == blocks.len()));
            let len = block.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        out.extend_from_slice(&((b << 16) | a).to_be_bytes());
        out
    }

    fn ihdr(width: u32, height: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
        let mut data = width.to_be_bytes().to_vec();
        data.extend_from_slice(&height.to_be_bytes());
        data.extend_from_slice(&[depth, colour, 0, 0, interlace]);
        chunk(b"IHDR", &data)
    }

    fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        for chunk in chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    /// A PNG of `pixels` as the `png` crate writes it.
    fn encode(width: u32, height: u32, colour: ColorType, depth: BitDepth, pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(colour);
        encoder.set_depth(depth);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
        out
    }

    /// The chunks of `png`, in order, by type; and the bytes after `IEND`.
    fn chunks_of(png: &[u8]) -> (Vec<String>, usize) {
        assert!(png.starts_with(SIGNATURE));
        let mut at = SIGNATURE.len();
        let mut kinds = Vec::new();
        while at + 8 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned();
            at += 12 + len;
            let end = kind == "IEND";
            kinds.push(kind);
            if end {
                break;
            }
        }
        (kinds, png.len() - at)
    }

    /// The output holds `IHDR`, then `IDAT`s, then `IEND`, and nothing
    /// after it.
    fn assert_clean(logo: &Logo) {
        let (kinds, trailing) = chunks_of(&logo.bytes);
        assert_eq!(kinds.first().map(String::as_str), Some("IHDR"), "{kinds:?}");
        assert_eq!(kinds.last().map(String::as_str), Some("IEND"), "{kinds:?}");
        assert!(kinds[1..kinds.len() - 1].iter().all(|k| k == "IDAT"), "{kinds:?}");
        assert!(kinds.len() >= 3, "{kinds:?}");
        assert_eq!(trailing, 0);
        assert_eq!(logo.etag, hex::encode(&Sha256::digest(&logo.bytes)[..16]));
    }

    /// `png` decoded as the browser would see it: 8-bit, palettes expanded.
    fn pixels(png: &[u8]) -> (u32, u32, ColorType, Vec<u8>) {
        let mut decoder = png::Decoder::new(Cursor::new(png));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buf).unwrap();
        buf.truncate(frame.buffer_size());
        (frame.width, frame.height, frame.color_type, buf)
    }

    fn reencoded(input: &[u8]) -> Logo {
        let logo = reencode(input, MAX_STORED).unwrap();
        assert_clean(&logo);
        logo
    }

    /// The chunks of a clean `png`, with `before` inserted before its first
    /// `IDAT`, `after` before its `IEND`, and `trailing` after `IEND`.
    fn with_extras(png: &[u8], before: &[Vec<u8>], after: &[Vec<u8>], trailing: &[u8]) -> Vec<u8> {
        let mut at = SIGNATURE.len();
        let mut out = SIGNATURE.to_vec();
        let mut seen_idat = false;
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            if kind == b"IDAT" && !seen_idat {
                seen_idat = true;
                before.iter().for_each(|c| out.extend_from_slice(c));
            }
            if kind == b"IEND" {
                after.iter().for_each(|c| out.extend_from_slice(c));
            }
            out.extend_from_slice(&png[at..at + 12 + len]);
            at += 12 + len;
        }
        out.extend_from_slice(trailing);
        out
    }

    /// Every 8-bit colour type comes back as its pixels, in its own colour
    /// type, written afresh.
    #[test]
    fn each_colour_type_comes_back_as_its_pixels() {
        let cases = [
            (ColorType::Grayscale, 1),
            (ColorType::GrayscaleAlpha, 2),
            (ColorType::Rgb, 3),
            (ColorType::Rgba, 4),
        ];
        for (colour, channels) in cases {
            let data: Vec<u8> = (0..3 * 2 * channels).map(|n| (n * 37 % 251) as u8).collect();
            let input = encode(3, 2, colour, BitDepth::Eight, &data);
            let logo = reencoded(&input);
            assert_eq!(pixels(&logo.bytes), (3, 2, colour, data), "{colour:?}");
        }
    }

    /// A palette with transparency becomes RGBA, and 16-bit samples are cut
    /// to 8: the pixels the browser would have shown.
    #[test]
    fn palettes_and_16_bit_samples_become_plain_8_bit_pixels() {
        let palette = file(&[
            ihdr(2, 1, 8, 3, 0),
            chunk(b"PLTE", &[255, 0, 0, 0, 0, 255]),
            chunk(b"tRNS", &[128]),
            chunk(b"IDAT", &zlib_stored(&[0, 0, 1])),
            chunk(b"IEND", &[]),
        ]);
        let logo = reencoded(&palette);
        assert_eq!(
            pixels(&logo.bytes),
            (2, 1, ColorType::Rgba, vec![255, 0, 0, 128, 0, 0, 255, 255])
        );
        let wide = encode(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Sixteen,
            &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
        );
        assert_eq!(chunks_of(&wide).0[0], "IHDR");
        let logo = reencoded(&wide);
        assert_eq!(pixels(&logo.bytes), (1, 1, ColorType::Rgb, vec![0x12, 0x56, 0x9a]));
        assert_eq!(logo.bytes[24], 8, "the bit depth in IHDR");
    }

    /// An interlaced PNG (Adam7) comes back as the same pixels, not
    /// interlaced.
    #[test]
    fn an_interlaced_png_comes_back_progressive_no_more() {
        // 3 × 3 grey, pixel (x, y) = 10 * y + x, in Adam7's seven passes.
        let (w, h) = (3usize, 3usize);
        let passes = [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ];
        let mut data = Vec::new();
        for (x0, y0, dx, dy) in passes {
            for y in (y0..h).step_by(dy) {
                let row: Vec<u8> = (x0..w).step_by(dx).map(|x| (10 * y + x) as u8).collect();
                if !row.is_empty() {
                    data.push(0);
                    data.extend(row);
                }
            }
        }
        let input = file(&[
            ihdr(3, 3, 8, 0, 1),
            chunk(b"IDAT", &zlib_stored(&data)),
            chunk(b"IEND", &[]),
        ]);
        let logo = reencoded(&input);
        assert_eq!(logo.bytes[28], 0, "not interlaced");
        let expected: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (10 * y + x) as u8)).collect();
        assert_eq!(pixels(&logo.bytes), (3, 3, ColorType::Grayscale, expected));
    }

    /// Nothing of the upload but its pixels survives: text, compressed and
    /// international text, EXIF, an ICC profile, physical size, gamma, a
    /// private chunk, before and after the image data, and a document
    /// hidden after `IEND` (a polyglot).
    #[test]
    fn metadata_private_chunks_and_trailing_documents_do_not_survive() {
        let clean = encode(2, 2, ColorType::Rgb, BitDepth::Eight, &[9; 12]);
        let mut itxt = b"Comment\0\0\0en\0Kommentar\0".to_vec();
        itxt.extend_from_slice(b"<script>alert(1)</script>");
        let before = [
            chunk(b"tEXt", b"Comment\0<svg onload=alert(1)>"),
            chunk(
                b"zTXt",
                &[b"Comment\0\0".as_slice(), &zlib_stored(b"<script>x</script>")].concat(),
            ),
            chunk(b"iTXt", &itxt),
            chunk(b"eXIf", b"MM\0*\0\0\0\x08\0\0secret-camera"),
            chunk(
                b"iCCP",
                &[b"profile\0\0".as_slice(), &zlib_stored(b"not a profile")].concat(),
            ),
            chunk(b"pHYs", &[0, 0, 0x0b, 0x13, 0, 0, 0x0b, 0x13, 1]),
            chunk(b"gAMA", &45455u32.to_be_bytes()),
            chunk(b"prVt", b"private <iframe src=x>"),
        ];
        let after = [
            chunk(b"tEXt", b"Late\0<html><script>alert(2)</script>"),
            chunk(b"prVt", b"after the pixels"),
        ];
        let trailing = b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(3)\"/><script>alert(4)</script>";
        let input = with_extras(&clean, &before, &after, trailing);
        assert_eq!(chunks_of(&input).1, trailing.len(), "the fixture hides a document");
        let logo = reencoded(&input);
        assert_eq!(pixels(&logo.bytes), pixels(&clean));
        for needle in [
            "script", "svg", "secret", "profile", "private", "iframe", "html", "Comment", "alert",
        ] {
            assert!(
                !logo.bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} survived"
            );
        }
    }

    /// An animated PNG keeps its first frame alone: no animation chunks.
    #[test]
    fn an_animated_png_keeps_its_first_frame() {
        let mut input = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut input, 2, 1);
            encoder.set_color(ColorType::Grayscale);
            encoder.set_depth(BitDepth::Eight);
            encoder.set_animated(2, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2]).unwrap();
            writer.write_image_data(&[200, 201]).unwrap();
            writer.finish().unwrap();
        }
        let kinds = chunks_of(&input).0;
        assert!(
            kinds.contains(&"acTL".to_string()) && kinds.contains(&"fdAT".to_string()),
            "{kinds:?}"
        );
        let logo = reencoded(&input);
        assert_eq!(pixels(&logo.bytes), (2, 1, ColorType::Grayscale, vec![1, 2]));
    }

    /// Only a PNG, by its signature: an SVG, XML, WebP, JPEG, GIF, an
    /// empty upload and a near miss of the signature are each refused
    /// before any decoder reads them (the review's A1).
    #[test]
    fn anything_but_a_png_is_unsupported() {
        let cases: [(&str, &[u8]); 9] = [
            (
                "svg",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
            ),
            (
                "xml",
                b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            ),
            ("bom svg", b"\xef\xbb\xbf<svg/>"),
            (
                "webp",
                b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0\x2f\0\0\0\x10\x07\x10\x11\x11\x88\x88\xfe\x07\0",
            ),
            (
                "jpeg",
                b"\xff\xd8\xff\xe0\0\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xff\xd9",
            ),
            ("gif", b"GIF89a\x01\0\x01\0\0\0\0;"),
            ("empty", b""),
            ("short", &SIGNATURE[..7]),
            ("near miss", b"\x89PNG\r\n\x1a\x0b"),
        ];
        for (name, input) in cases {
            assert_eq!(reencode(input, MAX_STORED), Err(Refusal::Unsupported), "{name}");
        }
    }

    /// The size is checked from the header alone, before anything else is
    /// read (the review's A2): a header claiming 2³¹ pixels, with nothing
    /// after it, is `Dimensions`, not `Damaged`. 1024 either way is taken,
    /// grey or RGBA as a canvas exports it.
    #[test]
    fn a_side_over_1024_is_refused_from_the_header_alone() {
        for (w, h) in [(1025, 1), (1, 1025), (0x7fff_ffff, 0x7fff_ffff), (1 << 20, 16)] {
            let header_only = file(&[ihdr(w, h, 8, 6, 0)]);
            assert_eq!(reencode(&header_only, MAX_STORED), Err(Refusal::Dimensions), "{w}×{h}");
        }
        let largest = encode(1024, 1024, ColorType::Grayscale, BitDepth::Eight, &vec![7; 1024 * 1024]);
        assert!(largest.len() < MAX_UPLOAD, "{}", largest.len());
        let logo = reencoded(&largest);
        let (w, h, colour, data) = pixels(&logo.bytes);
        assert_eq!((w, h, colour), (1024, 1024, ColorType::Grayscale));
        assert!(data.iter().all(|&p| p == 7));

        let rgba = encode(
            1024,
            1024,
            ColorType::Rgba,
            BitDepth::Eight,
            &[1, 2, 3, 4].repeat(1024 * 1024),
        );
        assert!(rgba.len() < MAX_UPLOAD, "{}", rgba.len());
        let logo = reencoded(&rgba);
        let (w, h, colour, data) = pixels(&logo.bytes);
        assert_eq!((w, h, colour), (1024, 1024, ColorType::Rgba));
        assert!(data.chunks(4).all(|p| p == [1, 2, 3, 4]));
    }

    /// What the decoder refuses is `Damaged`: a zero side, a truncated
    /// image, a broken checksum, an Apple `CgBI` file, no image data.
    #[test]
    fn a_damaged_png_is_refused() {
        let good = encode(4, 4, ColorType::Rgb, BitDepth::Eight, &[3; 48]);
        let mut bad_crc = good.clone();
        bad_crc[29] ^= 0xff;
        let cases: [(&str, Vec<u8>); 5] = [
            ("zero width", file(&[ihdr(0, 1, 8, 0, 0)])),
            ("truncated", good[..good.len() - 20].to_vec()),
            ("header crc", bad_crc),
            (
                "CgBI",
                file(&[
                    chunk(b"CgBI", &[0x50, 0, 0x20, 0x06]),
                    ihdr(1, 1, 8, 6, 0),
                    chunk(b"IDAT", &[0x63, 0, 0, 0, 0, 0]),
                    chunk(b"IEND", &[]),
                ]),
            ),
            ("no image data", file(&[ihdr(1, 1, 8, 0, 0), chunk(b"IEND", &[])])),
        ];
        for (name, input) in cases {
            assert_eq!(reencode(&input, MAX_STORED), Err(Refusal::Damaged), "{name}");
        }
    }

    /// Image data that inflates past its rows gives the rows and no more.
    #[test]
    fn image_data_past_its_rows_is_not_kept() {
        let mut rows = vec![0, 5, 6];
        rows.extend(std::iter::repeat_n(0xaa, 60_000));
        let input = file(&[
            ihdr(2, 1, 8, 0, 0),
            chunk(b"IDAT", &zlib_stored(&rows)),
            chunk(b"IEND", &[]),
        ]);
        let logo = reencoded(&input);
        assert_eq!(pixels(&logo.bytes), (2, 1, ColorType::Grayscale, vec![5, 6]));
        assert!(logo.bytes.len() < 100, "{}", logo.bytes.len());
    }

    /// The stored cap is checked after the re-encoding (the review's A4):
    /// at the size it is taken, a byte under it is refused.
    #[test]
    fn a_logo_over_the_stored_cap_is_too_large() {
        let input = encode(8, 8, ColorType::Rgba, BitDepth::Eight, &(0..=255).collect::<Vec<u8>>());
        let size = reencoded(&input).bytes.len();
        assert!(reencode(&input, size).is_ok());
        assert_eq!(reencode(&input, size - 1), Err(Refusal::TooLarge));
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A PNG of exactly `len` bytes: a small image padded with a private
    /// chunk.
    fn png_of_len(len: usize) -> Vec<u8> {
        let small = encode(1, 1, ColorType::Grayscale, BitDepth::Eight, &[1]);
        let pad = len - small.len() - 12;
        let out = with_extras(&small, &[chunk(b"prVt", &vec![b'p'; pad])], &[], b"");
        assert_eq!(out.len(), len);
        out
    }

    /// 64 KiB decoded is taken, a byte more is not; and an upload whose
    /// length alone is over is refused before it is decoded (the review's
    /// O2): `TooLarge` even when it is not base64 at all.
    #[test]
    fn the_upload_is_at_most_64_kib() {
        let logo = from_upload(&b64(&png_of_len(MAX_UPLOAD))).unwrap();
        assert_clean(&logo);
        assert_eq!(from_upload(&b64(&png_of_len(MAX_UPLOAD + 1))), Err(Refusal::TooLarge));
        let longest = MAX_UPLOAD.div_ceil(3) * 4;
        assert_eq!(b64(&png_of_len(MAX_UPLOAD)).len(), longest);
        assert_eq!(from_upload(&"!".repeat(longest)), Err(Refusal::NotBase64));
        assert_eq!(from_upload(&"!".repeat(longest + 1)), Err(Refusal::TooLarge));
    }

    /// Strict standard base64 only: no whitespace, no line breaks, no
    /// `data:` URL, no URL-safe alphabet, no missing padding.
    #[test]
    fn the_upload_is_strict_standard_base64() {
        // Bytes chosen so the encoding holds `+` and `/`.
        let mut input = encode(1, 1, ColorType::Rgb, BitDepth::Eight, &[0xfb, 0xff, 0xbf]);
        input = with_extras(&input, &[chunk(b"prVt", &[0xfb, 0xef, 0xbe, 0xff, 0xfb])], &[], b"");
        let good = b64(&input);
        assert!(
            good.contains('+') && good.contains('/') && good.ends_with('='),
            "{good}"
        );
        assert!(from_upload(&good).is_ok());
        let cases = [
            ("space", format!("{} {}", &good[..8], &good[8..])),
            ("newline", format!("{}\n{}", &good[..76], &good[76..])),
            ("trailing newline", format!("{good}\n")),
            ("data url", format!("data:image/png;base64,{good}")),
            ("url-safe", good.replace('+', "-").replace('/', "_")),
            ("unpadded", good.trim_end_matches('=').to_string()),
        ];
        for (name, data) in cases {
            assert_eq!(from_upload(&data), Err(Refusal::NotBase64), "{name}");
        }
    }

    /// The same image gives the same logo and `ETag`; another, another.
    #[test]
    fn the_etag_follows_the_bytes() {
        let a = reencoded(&encode(1, 1, ColorType::Grayscale, BitDepth::Eight, &[1]));
        let again = reencoded(&encode(1, 1, ColorType::Grayscale, BitDepth::Eight, &[1]));
        let b = reencoded(&encode(1, 1, ColorType::Grayscale, BitDepth::Eight, &[2]));
        assert_eq!(a, again);
        assert_ne!(a.etag, b.etag);
        assert_eq!(a.etag.len(), 32);
    }
}
