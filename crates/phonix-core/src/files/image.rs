//! How large a picture is, read from its header.
//!
//! A byte count is not a size. A 400 KB PNG can be 30,000 pixels square, and
//! anything that later decodes it - a thumbnailer, a browser tab, a printer -
//! allocates four bytes per pixel to do so. That is 3.6 GB from a file that
//! passed a 2 MB limit without comment. The limit that matters for a picture is
//! its dimensions, and the byte count says nothing about them.
//!
//! # Why there is no image library here
//!
//! Because measuring is not decoding. Every format below writes its width and
//! height in the first few dozen bytes, in a fixed place, and reading them is
//! arithmetic on a header. Decoding is the expensive, attack-prone part - the
//! part a decompression bomb is aimed at - and doing it in order to find out
//! whether we should have done it is exactly backwards.
//!
//! So this reads headers and nothing else. It never allocates in proportion to
//! the file, never follows a length the file supplies, and cannot loop for
//! longer than the bytes it was given.
//!
//! # An unmeasurable picture is not a rejected one
//!
//! [`dimensions`] returns `None` for a format whose header this does not know
//! how to read - TIFF, AVIF - and for a file truncated before its header ends.
//! `None` means "no opinion", and the caller treats it as such: the size limit
//! and the type check have already had their say, and refusing a valid TIFF
//! over a measurement nobody needs would be a rule enforcing itself rather than
//! anything real.

/// A picture's pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

impl Dimensions {
    /// Total pixels, saturating rather than wrapping.
    ///
    /// Wrapping here would be the bug the whole module exists to prevent: two
    /// dimensions whose product overflows would come out small and pass.
    pub fn pixels(self) -> u64 {
        u64::from(self.width).saturating_mul(u64::from(self.height))
    }

    /// Whether this fits inside `width` x `height`.
    pub fn fits_within(self, width: u32, height: u32) -> bool {
        self.width <= width && self.height <= height
    }
}

/// Measure a picture from its header, or answer `None`.
///
/// `mime` selects the reader; it is the *detected* type, never a declared one,
/// so a file claiming to be a PNG is not parsed as one on the strength of the
/// claim.
pub fn dimensions(bytes: &[u8], mime: &str) -> Option<Dimensions> {
    match mime {
        "image/png" => png(bytes),
        "image/jpeg" => jpeg(bytes),
        "image/gif" => gif(bytes),
        "image/bmp" => bmp(bytes),
        "image/webp" => webp(bytes),
        // Everything else is measurable in principle and not worth the code
        // until something stores one. `None` is a real answer here.
        _ => None,
    }
}

/// PNG: the IHDR chunk is mandatory and must come first, so both numbers are at
/// a fixed offset - big-endian, as everything in PNG is.
fn png(bytes: &[u8]) -> Option<Dimensions> {
    // Guarding on the chunk type as well as the length: a file that merely
    // starts with the signature has not proven it has an IHDR.
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }

    Some(Dimensions {
        width: be_u32(bytes, 16)?,
        height: be_u32(bytes, 20)?,
    })
}

/// GIF: width and height sit in the logical screen descriptor, little-endian,
/// immediately after the six-byte signature.
fn gif(bytes: &[u8]) -> Option<Dimensions> {
    Some(Dimensions {
        width: u32::from(le_u16(bytes, 6)?),
        height: u32::from(le_u16(bytes, 8)?),
    })
}

/// BMP: the DIB header follows the 14-byte file header. Height is signed, and a
/// negative one means the rows are stored top-down - the magnitude is still the
/// height, so it is the absolute value that is wanted.
fn bmp(bytes: &[u8]) -> Option<Dimensions> {
    let width = le_u32(bytes, 18)? as i32;
    let height = le_u32(bytes, 22)? as i32;

    Some(Dimensions {
        width: width.unsigned_abs(),
        height: height.unsigned_abs(),
    })
}

/// JPEG: the only format here that has to be walked.
///
/// Dimensions live in a start-of-frame segment, and how far in that is depends
/// on how much metadata the camera wrote first - a phone photo can carry a
/// 60 KB thumbnail ahead of it. So the segment chain is followed, each segment
/// declaring its own length.
///
/// Two things keep that from being a place to hang the server: the walk is
/// bounded by the bytes handed in, and a segment whose declared length would
/// not advance the cursor ends it. A malformed file therefore stops the walk
/// rather than looping in it.
fn jpeg(bytes: &[u8]) -> Option<Dimensions> {
    // Past the SOI marker.
    let mut cursor = 2usize;

    loop {
        if bytes.get(cursor)? != &0xFF {
            return None;
        }

        // Padding: a run of FF bytes before a marker is legal.
        let mut marker_at = cursor.checked_add(1)?;
        while bytes.get(marker_at) == Some(&0xFF) {
            marker_at = marker_at.checked_add(1)?;
        }

        let marker = *bytes.get(marker_at)?;
        let payload_at = marker_at.checked_add(1)?;

        match marker {
            // Start of frame, in every one of its flavours. C4, C8 and CC share
            // the range but are Huffman tables and arithmetic coding
            // definitions, not frames.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                // Inside the segment: two length bytes, one precision byte,
                // then height and width - in that order, which is the way
                // round people get wrong.
                let height = be_u16(bytes, payload_at.checked_add(3)?)?;
                let width = be_u16(bytes, payload_at.checked_add(5)?)?;

                return Some(Dimensions {
                    width: u32::from(width),
                    height: u32::from(height),
                });
            }

            // Start of scan: the compressed data begins, and its length is not
            // declared. There is no frame header after this point.
            0xDA => return None,

            // Standalone markers carrying no payload.
            0xD0..=0xD9 | 0x01 => {
                cursor = payload_at;
            }

            _ => {
                let length = usize::from(be_u16(bytes, payload_at)?);
                // A length below 2 does not include its own two length bytes,
                // so the cursor would not advance and this would spin.
                if length < 2 {
                    return None;
                }
                cursor = payload_at.checked_add(length)?;
            }
        }
    }
}

/// WebP: three encodings under one RIFF container, each storing its size
/// differently and none of them in bytes anybody would call aligned.
fn webp(bytes: &[u8]) -> Option<Dimensions> {
    match bytes.get(12..16)? {
        // Lossy. A three-byte start code, then two 14-bit values, each in the
        // low bits of a little-endian u16 - the top two bits are the scale.
        b"VP8 " => {
            if bytes.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some(Dimensions {
                width: u32::from(le_u16(bytes, 26)? & 0x3FFF),
                height: u32::from(le_u16(bytes, 28)? & 0x3FFF),
            })
        }

        // Lossless. One signature byte, then 28 bits packed across four bytes:
        // width - 1 in the low 14, height - 1 in the next 14.
        b"VP8L" => {
            if bytes.get(20)? != &0x2F {
                return None;
            }
            let packed = le_u32(bytes, 21)?;
            Some(Dimensions {
                width: (packed & 0x3FFF).saturating_add(1),
                height: ((packed >> 14) & 0x3FFF).saturating_add(1),
            })
        }

        // Extended: animation, transparency, metadata. The canvas size is two
        // 24-bit values, again stored one less than they are.
        b"VP8X" => Some(Dimensions {
            width: le_u24(bytes, 24)?.saturating_add(1),
            height: le_u24(bytes, 27)?.saturating_add(1),
        }),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Fixed-width reads
//
// All of them `get` a slice and convert it, so an offset past the end of a
// truncated file is `None` rather than a panic. This crate compiles to
// WebAssembly, where an out-of-bounds index is not an error a page recovers
// from - it is the end of the page.
// ---------------------------------------------------------------------------

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let window: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(window))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let window: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(window))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let window: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_be_bytes(window))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let window: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(window))
}

fn le_u24(bytes: &[u8], at: usize) -> Option<u32> {
    let window = bytes.get(at..at.checked_add(3)?)?;
    let (a, b, c) = (window.first()?, window.get(1)?, window.get(2)?);
    Some(u32::from(*a) | (u32::from(*b) << 8) | (u32::from(*c) << 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        // IHDR length, then the chunk type.
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn jpeg_bytes(width: u16, height: u16, leading_metadata: usize) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];

        // An APP1 segment standing in for the EXIF block a camera writes.
        if leading_metadata > 0 {
            let length = u16::try_from(leading_metadata + 2).unwrap();
            bytes.extend_from_slice(&[0xFF, 0xE1]);
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend(std::iter::repeat_n(0u8, leading_metadata));
        }

        // SOF0: length, precision, height, width, components.
        bytes.extend_from_slice(&[0xFF, 0xC0]);
        bytes.extend_from_slice(&17u16.to_be_bytes());
        bytes.push(8);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.push(3);
        bytes
    }

    #[test]
    fn png_is_measured_from_its_ihdr() {
        assert_eq!(
            dimensions(&png_bytes(1920, 1080), "image/png"),
            Some(Dimensions {
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn a_decompression_bomb_is_visible_before_anything_decodes_it() {
        // 30,000 square is a few hundred kilobytes of PNG and 3.6 GB decoded.
        // The byte count says nothing; this is the only thing that catches it.
        let bomb = png_bytes(30_000, 30_000);

        let measured = dimensions(&bomb, "image/png").unwrap();
        assert_eq!(measured.pixels(), 900_000_000);
        assert!(!measured.fits_within(1024, 1024));
    }

    #[test]
    fn pixel_counts_saturate_rather_than_wrap() {
        let absurd = Dimensions {
            width: u32::MAX,
            height: u32::MAX,
        };
        // The bug this guards: a wrapping multiply makes the largest possible
        // picture measure as one pixel.
        assert!(absurd.pixels() > u64::from(u32::MAX));
    }

    #[test]
    fn jpeg_is_found_past_whatever_metadata_precedes_it() {
        assert_eq!(
            dimensions(&jpeg_bytes(4032, 3024, 0), "image/jpeg"),
            Some(Dimensions {
                width: 4032,
                height: 3024
            })
        );

        // A phone photo with a thumbnail in front of the frame header.
        assert_eq!(
            dimensions(&jpeg_bytes(4032, 3024, 60_000), "image/jpeg"),
            Some(Dimensions {
                width: 4032,
                height: 3024
            })
        );
    }

    #[test]
    fn a_malformed_jpeg_ends_the_walk_instead_of_spinning_in_it() {
        // A segment declaring a length of zero would leave the cursor where it
        // was, which is an infinite loop rather than a parse failure.
        let stuck = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(dimensions(&stuck, "image/jpeg"), None);

        // A segment whose length runs off the end of the file.
        let overrun = vec![0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF, 0x00];
        assert_eq!(dimensions(&overrun, "image/jpeg"), None);

        // Compressed data begins; there is no frame header to find.
        let scan_only = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x0C];
        assert_eq!(dimensions(&scan_only, "image/jpeg"), None);
    }

    #[test]
    fn gif_and_bmp_are_read_little_endian() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&800u16.to_le_bytes());
        gif.extend_from_slice(&600u16.to_le_bytes());
        assert_eq!(
            dimensions(&gif, "image/gif"),
            Some(Dimensions {
                width: 800,
                height: 600
            })
        );

        let mut bmp = b"BM".to_vec();
        bmp.extend(std::iter::repeat_n(0u8, 16));
        bmp.extend_from_slice(&640i32.to_le_bytes());
        // Negative: rows stored top-down. Still 480 pixels tall.
        bmp.extend_from_slice(&(-480i32).to_le_bytes());
        assert_eq!(
            dimensions(&bmp, "image/bmp"),
            Some(Dimensions {
                width: 640,
                height: 480
            })
        );
    }

    #[test]
    fn the_three_webp_encodings_are_all_understood() {
        let mut lossy = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        lossy.extend(std::iter::repeat_n(0u8, 7));
        lossy.extend_from_slice(&[0x9D, 0x01, 0x2A]);
        lossy.extend_from_slice(&512u16.to_le_bytes());
        lossy.extend_from_slice(&384u16.to_le_bytes());
        assert_eq!(
            dimensions(&lossy, "image/webp"),
            Some(Dimensions {
                width: 512,
                height: 384
            })
        );

        let mut extended = b"RIFF\x00\x00\x00\x00WEBPVP8X".to_vec();
        extended.extend(std::iter::repeat_n(0u8, 8));
        // Stored one less than the real value, three bytes each.
        extended.extend_from_slice(&[0xFF, 0x03, 0x00]);
        extended.extend_from_slice(&[0xFF, 0x01, 0x00]);
        assert_eq!(
            dimensions(&extended, "image/webp"),
            Some(Dimensions {
                width: 1024,
                height: 512
            })
        );
    }

    #[test]
    fn a_truncated_header_is_no_opinion_rather_than_a_panic() {
        for length in 0..24 {
            let truncated = png_bytes(100, 100);
            let head = truncated.get(..length.min(truncated.len())).unwrap();
            // The point is that none of these panics; in wasm a panic here
            // would take the whole page with it.
            let _ = dimensions(head, "image/png");
        }

        assert_eq!(dimensions(b"", "image/png"), None);
        assert_eq!(dimensions(b"\x89PNG\r\n\x1a\n", "image/png"), None);
    }

    #[test]
    fn a_format_with_no_reader_is_no_opinion_not_a_zero() {
        // Zero would look like a picture that fits inside every limit.
        assert_eq!(dimensions(b"II\x2a\x00whatever", "image/tiff"), None);
        assert_eq!(dimensions(b"%PDF-1.7", "application/pdf"), None);
    }
}
