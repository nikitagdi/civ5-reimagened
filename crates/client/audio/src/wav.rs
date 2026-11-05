//! RIFF/WAVE reader: PCM 8 and 16 bit and IMA ADPCM, the formats of the game's sounds.

use platform::Pcm;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WavError {
    #[error("not a RIFF/WAVE file")]
    NotWav,
    #[error("the file ends inside a chunk")]
    Truncated,
    #[error("no fmt chunk before the data chunk")]
    NoFormat,
    #[error("no data chunk")]
    NoData,
    #[error("unsupported wave format {tag} with {bits} bits and {channels} channels")]
    Unsupported { tag: u16, bits: u16, channels: u16 },
    #[error("bad ADPCM block layout")]
    BadBlock,
}

const INDEX_TABLE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

struct Fmt {
    tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
    samples_per_block: u16,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, WavError> {
    b.get(i..i + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(WavError::Truncated)
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, WavError> {
    b.get(i..i + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(WavError::Truncated)
}

/// Decodes a whole file into 16-bit samples.
pub fn decode(bytes: &[u8]) -> Result<Pcm, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut fmt: Option<Fmt> = None;
    let mut data: Option<&[u8]> = None;
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32_at(bytes, i + 4)? as usize;
        let body_start = i + 8;
        // A data chunk whose size field overruns the file (streamed writers) takes the rest.
        let body_end = body_start.saturating_add(len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(WavError::Truncated);
                }
                let extra_size = if body.len() >= 18 {
                    u16_at(body, 16)?
                } else {
                    0
                };
                fmt = Some(Fmt {
                    tag: u16_at(body, 0)?,
                    channels: u16_at(body, 2)?,
                    rate: u32_at(body, 4)?,
                    block_align: u16_at(body, 12)?,
                    bits: u16_at(body, 14)?,
                    samples_per_block: if extra_size >= 2 && body.len() >= 20 {
                        u16_at(body, 18)?
                    } else {
                        0
                    },
                });
            }
            b"data" => {
                data = Some(body);
                break;
            }
            _ => {}
        }
        i = body_start.saturating_add(len).saturating_add(len & 1);
    }
    let f = fmt.ok_or(WavError::NoFormat)?;
    let data = data.ok_or(WavError::NoData)?;
    let unsupported = || WavError::Unsupported {
        tag: f.tag,
        bits: f.bits,
        channels: f.channels,
    };
    if f.channels == 0 || f.channels > 2 {
        return Err(unsupported());
    }
    let samples = match (f.tag, f.bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect(),
        (1, 8) => data.iter().map(|b| (i16::from(*b) - 128) << 8).collect(),
        (17, 4) => decode_ima(data, &f)?,
        _ => return Err(unsupported()),
    };
    Ok(Pcm {
        rate: f.rate,
        channels: f.channels,
        samples,
    })
}

struct Chan {
    predictor: i32,
    index: i32,
}

impl Chan {
    fn step(&mut self, nibble: u8) -> i16 {
        let step = STEP_TABLE[self.index as usize];
        let mut diff = step >> 3;
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        if nibble & 8 != 0 {
            diff = -diff;
        }
        self.predictor = (self.predictor + diff).clamp(-32768, 32767);
        self.index = (self.index + INDEX_TABLE[usize::from(nibble & 7)]).clamp(0, 88);
        self.predictor as i16
    }
}

fn decode_ima(data: &[u8], f: &Fmt) -> Result<Vec<i16>, WavError> {
    let ch = usize::from(f.channels);
    let align = usize::from(f.block_align);
    if align < 4 * ch {
        return Err(WavError::BadBlock);
    }
    let per_block = (align - 4 * ch) * 2 / ch + 1;
    if f.samples_per_block != 0 && usize::from(f.samples_per_block) != per_block {
        return Err(WavError::BadBlock);
    }
    let mut out: Vec<i16> = Vec::with_capacity(data.len() * 2 + 16);
    for block in data.chunks(align) {
        if block.len() < 4 * ch {
            break;
        }
        let mut chans: Vec<Chan> = (0..ch)
            .map(|c| {
                let h = &block[c * 4..c * 4 + 4];
                Chan {
                    predictor: i32::from(i16::from_le_bytes([h[0], h[1]])),
                    index: i32::from(h[2]).clamp(0, 88),
                }
            })
            .collect();
        // Per channel samples of this block, header sample first.
        let mut per: Vec<Vec<i16>> = chans.iter().map(|c| vec![c.predictor as i16]).collect();
        // Groups of 4 bytes per channel (8 samples), channels interleaved by group.
        let body = &block[4 * ch..];
        for group in body.chunks(4 * ch) {
            for (c, chan) in chans.iter_mut().enumerate() {
                let Some(bytes) = group.get(c * 4..(c * 4 + 4).min(group.len())) else {
                    continue;
                };
                for b in bytes {
                    per[c].push(chan.step(b & 0x0f));
                    per[c].push(chan.step(b >> 4));
                }
            }
        }
        let n = per.iter().map(Vec::len).min().unwrap_or(0);
        for k in 0..n {
            for p in &per {
                out.push(p[k]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn wav(
        tag: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        align: u16,
        extra: &[u8],
        data: &[u8],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend(tag.to_le_bytes());
        fmt.extend(channels.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend((rate * u32::from(align)).to_le_bytes());
        fmt.extend(align.to_le_bytes());
        fmt.extend(bits.to_le_bytes());
        if !extra.is_empty() {
            fmt.extend((extra.len() as u16).to_le_bytes());
            fmt.extend(extra);
        }
        let mut out = b"RIFF".to_vec();
        out.extend(((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
        out.extend(b"WAVE");
        out.extend(b"fmt ");
        out.extend((fmt.len() as u32).to_le_bytes());
        out.extend(&fmt);
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    /// 16-bit mono PCM file of the given samples.
    pub fn pcm16(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        wav(1, 1, rate, 16, 2, &[], &data)
    }

    #[test]
    fn pcm16_round_trip() {
        let p = decode(&pcm16(22_050, &[0, 1, -1, 32767, -32768])).unwrap();
        assert_eq!((p.rate, p.channels), (22_050, 1));
        assert_eq!(p.samples, [0, 1, -1, 32767, -32768]);
    }

    #[test]
    fn pcm8_is_centred_and_scaled() {
        let p = decode(&wav(1, 1, 8000, 8, 1, &[], &[128, 255, 0])).unwrap();
        assert_eq!(p.samples, [0, 127 << 8, -128 << 8]);
    }

    #[test]
    fn ima_adpcm_hand_computed_block() {
        // Header: predictor 0, index 0. Nibbles 7 then 0.
        // Step 7: diff = 0 + 7 + 3 + 1 = 11 -> 11, index 0 + 8 = 8.
        // Step 16: diff = 2 -> 13, index 8 - 1 = 7.
        let block = [0, 0, 0, 0, 0x07];
        let extra = 3u16.to_le_bytes();
        let p = decode(&wav(17, 1, 22_050, 4, 5, &extra, &block)).unwrap();
        assert_eq!(p.samples, [0, 11, 13]);
    }

    #[test]
    fn ima_adpcm_negative_and_clamped() {
        // Nibble 0xF from 0: diff = -(0 + 7 + 3 + 1) = -11; index 0 + 8.
        let p = decode(&wav(
            17,
            1,
            8000,
            4,
            5,
            &3u16.to_le_bytes(),
            &[0, 0, 0, 0, 0x0f],
        ))
        .unwrap();
        assert_eq!(p.samples[1], -11);
        // Predictor near the top stays at 32767.
        let top = 32760i16.to_le_bytes();
        let p = decode(&wav(
            17,
            1,
            8000,
            4,
            5,
            &3u16.to_le_bytes(),
            &[top[0], top[1], 40, 0, 0x77],
        ))
        .unwrap();
        assert_eq!(p.samples[1], 32767);
    }

    #[test]
    fn ima_adpcm_stereo_interleaves_channels() {
        // align = 8 header + 8 data = 16: per channel 4 data bytes = 8 samples + header = 9.
        let mut block = vec![100, 0, 0, 0, 200, 0, 0, 0];
        block.extend([0x00; 4]); // left: all nibble 0
        block.extend([0x00; 4]); // right
        let p = decode(&wav(17, 2, 22_050, 4, 16, &9u16.to_le_bytes(), &block)).unwrap();
        assert_eq!(p.channels, 2);
        assert_eq!(p.samples.len(), 18);
        assert_eq!((p.samples[0], p.samples[1]), (100, 200));
        // Nibble 0 adds step>>3 = 0 at index 0, then the index falls back to 0.
        assert_eq!((p.samples[2], p.samples[3]), (100, 200));
    }

    #[test]
    fn a_partial_last_block_gives_what_it_has_and_garbage_is_refused() {
        let mut data = vec![0, 0, 0, 0, 0x07];
        data.extend([0, 0, 0, 0]); // header only
        let p = decode(&wav(17, 1, 8000, 4, 5, &3u16.to_le_bytes(), &data)).unwrap();
        assert_eq!(p.samples.len(), 4);
        assert_eq!(decode(b"nope"), Err(WavError::NotWav));
        assert_eq!(
            decode(&wav(3, 1, 8000, 32, 4, &[], &[0; 4])).unwrap_err(),
            WavError::Unsupported {
                tag: 3,
                bits: 32,
                channels: 1
            }
        );
        let mut cut = pcm16(8000, &[1, 2, 3]);
        cut.truncate(20);
        assert!(decode(&cut).is_err());
    }
}
