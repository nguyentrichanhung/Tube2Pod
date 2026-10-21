use crate::error::{AppError, AppResult};
use ab_glyph::{FontRef, PxScale};
use image::{imageops, ImageFormat, ImageReader, Limits, Rgb, RgbImage};
use imageproc::drawing::{draw_text_mut, text_size};
use std::io::Cursor;

const SIZE: u32 = 3000;
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const FONT_BYTES: &[u8] = include_bytes!("../../assets/BeVietnamPro-Bold.ttf");

pub fn render_cover(title: &str) -> AppResult<Vec<u8>> {
    let mut image = RgbImage::from_pixel(SIZE, SIZE, Rgb([247, 239, 226]));
    let font = FontRef::try_from_slice(FONT_BYTES)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("cover font: {e}")))?;
    let initials = initials(title);
    let scale = PxScale::from(980.0);
    let (text_width, text_height) = text_size(scale, &font, &initials);
    draw_text_mut(
        &mut image,
        Rgb([122, 59, 32]),
        (SIZE.saturating_sub(text_width) / 2) as i32,
        (SIZE.saturating_sub(text_height) / 2) as i32,
        scale,
        &font,
        &initials,
    );
    encode_jpeg(image)
}

pub fn normalize_upload(bytes: &[u8]) -> AppResult<Vec<u8>> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::BadRequest("Ảnh bìa tối đa 10 MB.".into()));
    }
    let (width, height) = image_reader(bytes, Limits::default())?
        .into_dimensions()
        .map_err(|_| invalid_cover())?;
    if width != height || !(1400..=3000).contains(&width) {
        return Err(AppError::BadRequest(
            "Ảnh bìa phải vuông, từ 1400 đến 3000 px.".into(),
        ));
    }
    let image = image_reader(bytes, decode_limits())?
        .decode()
        .map_err(|_| invalid_cover())?;
    let resized = if (width, height) == (SIZE, SIZE) {
        image.to_rgb8()
    } else {
        imageops::resize(&image.to_rgb8(), SIZE, SIZE, imageops::FilterType::Lanczos3)
    };
    encode_jpeg(resized)
}

fn image_reader(bytes: &[u8], limits: Limits) -> AppResult<ImageReader<Cursor<&[u8]>>> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid_cover())?;
    reader.limits(limits);
    Ok(reader)
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(SIZE);
    limits.max_image_height = Some(SIZE);
    limits
}

fn invalid_cover() -> AppError {
    AppError::BadRequest("Ảnh bìa không hợp lệ.".into())
}

fn encode_jpeg(image: RgbImage) -> AppResult<Vec<u8>> {
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(out)
}

fn initials(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    let value = words
        .iter()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect::<String>();
    if value.is_empty() {
        "T2P".to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn renders_a_square_jpeg_with_vietnamese_diacritics() {
        let bytes = render_cover("Công nghệ mỗi ngày").unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!(image.dimensions(), (3000, 3000));
    }

    #[test]
    fn rejects_non_square_and_oversized_uploads() {
        let image = image::RgbImage::from_pixel(1400, 1200, image::Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        assert!(normalize_upload(&bytes).is_err());

        assert!(normalize_upload(&vec![0u8; MAX_UPLOAD_BYTES + 1]).is_err());
    }

    #[test]
    fn rejects_huge_dimension_header_before_decoding() {
        let bytes = png_header(50_000, 50_000);
        let error = normalize_upload(&bytes).unwrap_err();
        assert!(
            error.to_string().contains("phải vuông"),
            "unexpected error: {error}"
        );
    }

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        for chunk in [b"IDAT".as_slice(), b"IEND".as_slice()] {
            png.extend_from_slice(&0u32.to_be_bytes());
            png.extend_from_slice(chunk);
            png.extend_from_slice(&crc32(chunk).to_be_bytes());
        }
        png
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn normalizes_square_upload_to_3000() {
        let image = image::RgbImage::from_pixel(1400, 1400, image::Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let out = normalize_upload(&bytes).unwrap();
        let image = image::load_from_memory(&out).unwrap();
        assert_eq!(image.dimensions(), (3000, 3000));
    }
}
