//! Capped stream decoding for everything the engine reads (page content, soft masks, ToUnicode, font files).
//! lopdf's own decoders are never called on a stream we have not validated: they have no size limit and overflow on crafted
//! predictor parameters. A stream we cannot decode is an error, never "the raw bytes": the caller locks the page or fails closed.
use lopdf::{Dictionary, Object, Stream};
use std::io::Read;

/// A stream may not inflate beyond this (crafted "zip bombs").
pub const MAX_STREAM: usize = 256 << 20;

#[derive(Debug, PartialEq)]
pub enum DecErr {
    OverCap,
    Unsupported,
}

pub fn plain(s: &Stream) -> Result<Vec<u8>, DecErr> {
    plain_cap(s, MAX_STREAM)
}

fn name_of(o: &Object) -> Option<String> {
    o.as_name().ok().map(|n| String::from_utf8_lossy(n).to_string())
}

/// Decoded bytes of a stream, at most `cap`. Supported: no filter, and any chain of FlateDecode (with a PNG or TIFF predictor),
/// ASCIIHexDecode, ASCII85Decode and RunLengthDecode. Anything else (LZW, DCT, CCITT, JBIG2, JPX, Crypt, external files) is Unsupported.
pub fn plain_cap(s: &Stream, cap: usize) -> Result<Vec<u8>, DecErr> {
    if s.dict.has(b"F") {
        return Err(DecErr::Unsupported); // data in an external file
    }
    let filters: Vec<String> = match s.dict.get(b"Filter") {
        Err(_) => vec![],
        Ok(Object::Name(n)) => vec![String::from_utf8_lossy(n).to_string()],
        Ok(Object::Array(a)) => a.iter().map(name_of).collect::<Option<Vec<_>>>().ok_or(DecErr::Unsupported)?,
        Ok(_) => return Err(DecErr::Unsupported),
    };
    let parms: Vec<Option<&Dictionary>> = match s.dict.get(b"DecodeParms") {
        Err(_) | Ok(Object::Null) => vec![None; filters.len()],
        Ok(Object::Dictionary(d)) => vec![Some(d)],
        Ok(Object::Array(a)) => a
            .iter()
            .map(|o| match o {
                Object::Dictionary(d) => Ok(Some(d)),
                Object::Null => Ok(None),
                _ => Err(DecErr::Unsupported), // indirect parameters: not followed
            })
            .collect::<Result<Vec<_>, _>>()?,
        Ok(_) => return Err(DecErr::Unsupported),
    };
    if s.content.len() > cap {
        return Err(DecErr::OverCap);
    }
    let mut data = s.content.clone();
    for (i, f) in filters.iter().enumerate() {
        let p = parms.get(i).copied().flatten();
        data = match f.as_str() {
            "FlateDecode" | "Fl" => predictor(inflate(&data, cap)?, p, cap)?,
            "ASCIIHexDecode" | "AHx" => ascii_hex(&data),
            "ASCII85Decode" | "A85" => ascii85(&data, cap)?,
            "RunLengthDecode" | "RL" => run_length(&data, cap)?,
            _ => return Err(DecErr::Unsupported),
        };
        if data.len() > cap {
            return Err(DecErr::OverCap);
        }
    }
    Ok(data)
}

/// Inflate with a hard cap. A damaged stream keeps what was read before the damage (as lopdf does).
fn inflate(input: &[u8], cap: usize) -> Result<Vec<u8>, DecErr> {
    let mut out = Vec::new();
    if input.is_empty() {
        return Ok(out);
    }
    let _ = flate2::read::ZlibDecoder::new(input).take(cap as u64 + 1).read_to_end(&mut out);
    if out.len() > cap {
        Err(DecErr::OverCap)
    } else {
        Ok(out)
    }
}

fn ascii_hex(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut hi: Option<u8> = None;
    for &c in b {
        if c == b'>' {
            break;
        }
        let Some(v) = (c as char).to_digit(16) else { continue };
        match hi.take() {
            None => hi = Some(v as u8),
            Some(h) => out.push(h << 4 | v as u8),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

fn ascii85(b: &[u8], cap: usize) -> Result<Vec<u8>, DecErr> {
    let b = b.strip_suffix(b"~>").unwrap_or(b);
    let b = b.strip_prefix(b"<~").unwrap_or(b);
    let (mut out, mut acc, mut n) = (Vec::new(), 0u32, 0usize);
    for &c in b {
        match c {
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                acc = acc.wrapping_mul(85).wrapping_add((c - b'!') as u32);
                n += 1;
                if n == 5 {
                    out.extend_from_slice(&acc.to_be_bytes());
                    (acc, n) = (0, 0);
                }
            }
            c if c.is_ascii_whitespace() => {}
            _ => return Err(DecErr::Unsupported),
        }
        if out.len() > cap {
            return Err(DecErr::OverCap);
        }
    }
    if n > 1 {
        for _ in n..5 {
            acc = acc.wrapping_mul(85).wrapping_add(84);
        }
        out.extend_from_slice(&acc.to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

fn run_length(b: &[u8], cap: usize) -> Result<Vec<u8>, DecErr> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        let l = b[i] as usize;
        i += 1;
        if l == 128 {
            break;
        } else if l < 128 {
            let end = (i + l + 1).min(b.len());
            out.extend_from_slice(&b[i..end]);
            i = end;
        } else if i < b.len() {
            out.extend(std::iter::repeat(b[i]).take(257 - l));
            i += 1;
        }
        if out.len() > cap {
            return Err(DecErr::OverCap);
        }
    }
    Ok(out)
}

fn int(p: &Dictionary, k: &[u8], default: i64) -> Result<i64, DecErr> {
    match p.get(k) {
        Err(_) => Ok(default),
        Ok(Object::Integer(i)) => Ok(*i),
        Ok(_) => Err(DecErr::Unsupported),
    }
}

/// PNG (10-15) and TIFF (2, 8 bits) predictors, with every parameter validated first.
fn predictor(data: Vec<u8>, p: Option<&Dictionary>, cap: usize) -> Result<Vec<u8>, DecErr> {
    let Some(p) = p else { return Ok(data) };
    let pred = int(p, b"Predictor", 1)?;
    if pred == 1 {
        return Ok(data);
    }
    let (colors, bpc, cols) = (int(p, b"Colors", 1)?, int(p, b"BitsPerComponent", 8)?, int(p, b"Columns", 1)?);
    if !(1..=4).contains(&colors) || ![1, 2, 4, 8, 16].contains(&bpc) || !(1..=100_000).contains(&cols) {
        return Err(DecErr::Unsupported);
    }
    let (colors, bpc, cols) = (colors as usize, bpc as usize, cols as usize);
    let row = (cols * colors * bpc).div_ceil(8);
    let bpp = (colors * bpc).div_ceil(8).max(1);
    match pred {
        2 if bpc == 8 => {
            let mut out = data;
            for r in out.chunks_mut(row) {
                for i in colors..r.len() {
                    r[i] = r[i].wrapping_add(r[i - colors]);
                }
            }
            Ok(out)
        }
        10..=15 => {
            let mut out: Vec<u8> = Vec::with_capacity(data.len());
            let mut prev = vec![0u8; row];
            for chunk in data.chunks(row + 1) {
                if chunk.len() < 2 {
                    continue;
                }
                let (tag, src) = (chunk[0], &chunk[1..]);
                let mut cur = vec![0u8; row];
                for i in 0..src.len() {
                    let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
                    let b = prev[i] as i32;
                    let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
                    let add = match tag {
                        0 => 0,
                        1 => a,
                        2 => b,
                        3 => (a + b) / 2,
                        4 => {
                            let pa = (b - c).abs();
                            let pb = (a - c).abs();
                            let pc = (a + b - 2 * c).abs();
                            if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                        }
                        _ => return Err(DecErr::Unsupported),
                    };
                    cur[i] = (src[i] as i32 + add) as u8;
                }
                out.extend_from_slice(&cur[..src.len()]);
                if out.len() > cap {
                    return Err(DecErr::OverCap);
                }
                prev = cur;
            }
            Ok(out)
        }
        _ => Err(DecErr::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn st(filter: Object, data: &[u8]) -> Stream {
        Stream::new(dictionary! { "Filter" => filter }, data.to_vec())
    }

    #[test]
    fn simple_filters() {
        assert_eq!(plain(&st("ASCIIHexDecode".into(), b"48 65 6C6c 6F>")).unwrap(), b"Hello");
        assert_eq!(plain(&st("RunLengthDecode".into(), &[4, b'H', b'e', b'l', b'l', b'o', 254, b'!', 128])).unwrap(), b"Hello!!!");
        assert_eq!(plain(&st("ASCII85Decode".into(), b"87cURD]i,\"Ebo80~>")).unwrap(), b"Hello World!");
        assert_eq!(plain(&st("LZWDecode".into(), b"x")), Err(DecErr::Unsupported));
        assert_eq!(plain(&st("DCTDecode".into(), b"x")), Err(DecErr::Unsupported));
    }

    #[test]
    fn predictor_parameters_are_validated() {
        let mut d = dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 15, "Colors" => i64::MAX } };
        let s = Stream::new(d.clone(), vec![0x78, 0x9c, 3, 0]);
        assert_eq!(plain(&s), Err(DecErr::Unsupported));
        d.set("DecodeParms", dictionary! { "Predictor" => 15, "Columns" => 1_000_000 });
        assert_eq!(plain(&Stream::new(d, vec![0x78, 0x9c, 3, 0])), Err(DecErr::Unsupported));
    }
}
