//! JPEG con `zune-jpeg` (ADR 0009): básicos y progresivos, con submuestreo de color.
//!
//! Se le pide la salida en RGBA directamente y con los topes de tamaño de [`MAX_PIXELS`]: la
//! cabecera de un JPEG chiquito puede declarar 65535 × 65535, y eso no se reserva a ciegas.

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::{Error, Image, MAX_PIXELS};

pub fn decode(data: &[u8]) -> Result<Image, Error> {
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(8192)
        .set_max_height(8192)
        .set_strict_mode(false);
    let mut dec = JpegDecoder::new_with_options(ZCursor::new(data), options);
    dec.decode_headers()
        .map_err(|_| Error::Invalid("JPEG: cabecera inválida"))?;
    let info = dec.info().ok_or(Error::Invalid("JPEG: sin dimensiones"))?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    if width == 0 || height == 0 {
        return Err(Error::Invalid("JPEG: imagen vacía"));
    }
    if width * height > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    let rgba = dec
        .decode()
        .map_err(|_| Error::Invalid("JPEG: datos dañados"))?;
    if rgba.len() != width * height * 4 {
        return Err(Error::Invalid("JPEG: salida de otro tamaño"));
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}
