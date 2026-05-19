use crate::StateId;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 4] = b"VASF";
const FORMAT_VERSION: u16 = 2;
const ZSTD_LEVEL: i32 = 3;

const TOKENS_PER_RAW_FRAME: u64 = 16_000;

const TOKENS_PER_VASF_FRAME: u64 = 900;

pub struct VasfHeader {
    pub version: u16,
    pub frame_count: u32,
    pub created_at: i64,
    pub total_input: u32,
}

pub struct VasfFrame {
    pub state_id: StateId,
    pub timestamp: i64,
    pub vasp_data: Vec<u8>,
    pub delta_data: Option<Vec<u8>>,
}

pub struct VasfFile {
    pub header: VasfHeader,
    pub frames: Vec<VasfFrame>,
}

impl VasfFile {
    pub fn new(frames: Vec<VasfFrame>, total_input: u32) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            header: VasfHeader {
                version: FORMAT_VERSION,
                frame_count: frames.len() as u32,
                created_at: now,
                total_input,
            },
            frames,
        }
    }

    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut w = BufWriter::new(file);
        write_header(&mut w, &self.header)?;
        for frame in &self.frames {
            write_frame(&mut w, frame)?;
        }
        w.flush()
    }

    pub fn read_from(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut r = BufReader::new(file);
        let header = read_header(&mut r)?;
        let frames = (0..header.frame_count)
            .map(|_| read_frame(&mut r))
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self { header, frames })
    }

    pub fn total_frames(&self) -> u32 {
        self.header.total_input
    }

    pub fn unique_states(&self) -> u32 {
        self.frames.len() as u32
    }

    pub fn duplicate_count(&self) -> u32 {
        self.header
            .total_input
            .saturating_sub(self.frames.len() as u32)
    }

    pub fn dedup_percentage(&self) -> f32 {
        let t = self.header.total_input;
        if t == 0 {
            return 0.0;
        }
        self.duplicate_count() as f32 / t as f32 * 100.0
    }

    pub fn tokens_raw(&self) -> u64 {
        self.header.total_input as u64 * TOKENS_PER_RAW_FRAME
    }

    pub fn tokens_farscry(&self) -> u64 {
        self.frames.len() as u64 * TOKENS_PER_VASF_FRAME
    }

    pub fn reduction_x(&self) -> f32 {
        let f = self.tokens_farscry();
        if f == 0 {
            return 0.0;
        }
        self.tokens_raw() as f32 / f as f32
    }

    pub fn duration_ms(&self) -> Option<i64> {
        if self.frames.len() < 2 {
            return None;
        }
        let first = self.frames.first()?.timestamp;
        let last = self.frames.last()?.timestamp;
        if first == 0 || last == 0 {
            return None;
        }
        Some(last.saturating_sub(first))
    }

    pub fn screen_type_breakdown(&self) -> HashMap<String, u32> {
        let mut map: HashMap<String, u32> = HashMap::new();
        for frame in &self.frames {
            let text = std::str::from_utf8(&frame.vasp_data).unwrap_or("");
            let st = vasp_field(text, "screen_type: ").to_string();
            *map.entry(st).or_insert(0) += 1;
        }
        map
    }
}

fn vasp_field<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::trim)
        .unwrap_or("unknown")
}

fn compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    zstd::encode_all(data, ZSTD_LEVEL)
}

fn decompress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    zstd::decode_all(data)
}

fn write_header<W: Write>(w: &mut W, h: &VasfHeader) -> std::io::Result<()> {
    w.write_all(MAGIC)?;
    w.write_all(&h.version.to_le_bytes())?;
    w.write_all(&h.frame_count.to_le_bytes())?;
    w.write_all(&h.created_at.to_le_bytes())?;
    w.write_all(&h.total_input.to_le_bytes())?;
    Ok(())
}

fn write_frame<W: Write>(w: &mut W, frame: &VasfFrame) -> std::io::Result<()> {
    w.write_all(&frame.state_id.to_bits().to_le_bytes())?;
    w.write_all(&frame.timestamp.to_le_bytes())?;
    let vasp_c = compress(&frame.vasp_data)?;
    w.write_all(&(vasp_c.len() as u32).to_le_bytes())?;
    w.write_all(&vasp_c)?;
    match &frame.delta_data {
        Some(delta) => {
            let dc = compress(delta)?;
            w.write_all(&(dc.len() as u32).to_le_bytes())?;
            w.write_all(&dc)?;
        }
        None => w.write_all(&0u32.to_le_bytes())?,
    }
    Ok(())
}

fn read_u16_le<R: Read>(r: &mut R) -> std::io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32_le<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_i64_le<R: Read>(r: &mut R) -> std::io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_le_bytes(b))
}

fn read_bytes_n<R: Read>(r: &mut R, n: u32) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_header<R: Read>(r: &mut R) -> std::io::Result<VasfHeader> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid VASF magic bytes",
        ));
    }
    let version = read_u16_le(r)?;
    let frame_count = read_u32_le(r)?;
    let created_at = read_i64_le(r)?;
    let total_input = if version >= 2 {
        read_u32_le(r)?
    } else {
        frame_count
    };
    Ok(VasfHeader {
        version,
        frame_count,
        created_at,
        total_input,
    })
}

fn read_frame<R: Read>(r: &mut R) -> std::io::Result<VasfFrame> {
    let mut bits_b = [0u8; 8];
    r.read_exact(&mut bits_b)?;
    let state_id = StateId::from_bits(u64::from_le_bytes(bits_b));
    let timestamp = read_i64_le(r)?;
    let vasp_len = read_u32_le(r)?;
    let vasp_data = decompress(&read_bytes_n(r, vasp_len)?)?;
    let delta_len = read_u32_le(r)?;
    let delta_data = if delta_len > 0 {
        Some(decompress(&read_bytes_n(r, delta_len)?)?)
    } else {
        None
    };
    Ok(VasfFrame {
        state_id,
        timestamp,
        vasp_data,
        delta_data,
    })
}

pub struct VasfWriter {
    writer: BufWriter<std::fs::File>,
    pub frame_count: u32,
    pub total_input: u32,
}

impl VasfWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let file = std::fs::File::create(path)?;
        let mut w = BufWriter::new(file);
        w.write_all(MAGIC)?;
        w.write_all(&FORMAT_VERSION.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
        w.write_all(&created_at.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
        w.flush()?;
        Ok(Self {
            writer: w,
            frame_count: 0,
            total_input: 0,
        })
    }

    pub fn append_frame(&mut self, frame: &VasfFrame) -> std::io::Result<()> {
        write_frame(&mut self.writer, frame)?;
        self.writer.flush()?;
        self.frame_count += 1;
        Ok(())
    }

    pub fn append_state(&mut self, state_id: StateId, vasp_text: &str) -> std::io::Result<()> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        write_frame(
            &mut self.writer,
            &VasfFrame {
                state_id,
                timestamp: ts,
                vasp_data: vasp_text.as_bytes().to_vec(),
                delta_data: None,
            },
        )?;
        self.frame_count = self.frame_count.saturating_add(1);
        self.update_header_in_place()
    }

    pub fn append_timeline(
        &mut self,
        _timestamp_ms: i64,
        _state_id: StateId,
    ) -> std::io::Result<()> {
        self.total_input = self.total_input.saturating_add(1);
        self.update_header_in_place()
    }

    fn update_header_in_place(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        let file = self.writer.get_mut();
        file.seek(SeekFrom::Start(6))?;
        file.write_all(&self.frame_count.to_le_bytes())?;
        file.seek(SeekFrom::Start(18))?;
        file.write_all(&self.total_input.to_le_bytes())?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn finalize(&mut self) -> std::io::Result<()> {
        self.update_header_in_place()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let frame = VasfFrame {
            state_id: StateId::from_bits(0x0102030405060708),
            timestamp: 1_700_000_000_000,
            vasp_data: b"screen_type: config\nagent_context: \"test\"\n".to_vec(),
            delta_data: Some(b"=== farscry diff ===\n".to_vec()),
        };
        let vasf = VasfFile::new(vec![frame], 10);
        let path = std::env::temp_dir().join("_test_vasf_v2.vasf");
        vasf.write_to(&path).unwrap();
        let loaded = VasfFile::read_from(&path).unwrap();
        assert_eq!(loaded.frames.len(), 1);
        assert_eq!(loaded.header.total_input, 10);
        assert_eq!(loaded.header.frame_count, 1);
        assert_eq!(loaded.total_frames(), 10);
        assert_eq!(loaded.unique_states(), 1);
        assert_eq!(loaded.duplicate_count(), 9);
        assert!((loaded.dedup_percentage() - 90.0).abs() < 0.01);
        assert_eq!(loaded.tokens_raw(), 10 * TOKENS_PER_RAW_FRAME);
        assert_eq!(loaded.tokens_farscry(), TOKENS_PER_VASF_FRAME);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_screen_type_breakdown() {
        let make_frame = |st: &str| VasfFrame {
            state_id: StateId::from_bits(0),
            timestamp: 0,
            vasp_data: format!("screen_type: {st}\n").into_bytes(),
            delta_data: None,
        };
        let vasf = VasfFile::new(
            vec![
                make_frame("config"),
                make_frame("error"),
                make_frame("config"),
            ],
            3,
        );
        let bd = vasf.screen_type_breakdown();
        assert_eq!(*bd.get("config").unwrap_or(&0), 2);
        assert_eq!(*bd.get("error").unwrap_or(&0), 1);
    }

    #[test]
    fn test_duration_none_when_timestamps_zero() {
        let make_frame = |ts: i64| VasfFrame {
            state_id: StateId::from_bits(0),
            timestamp: ts,
            vasp_data: vec![],
            delta_data: None,
        };
        let vasf = VasfFile::new(vec![make_frame(0), make_frame(0)], 2);
        assert!(vasf.duration_ms().is_none());
    }

    #[test]
    fn test_duration_computed() {
        let make_frame = |ts: i64| VasfFrame {
            state_id: StateId::from_bits(0),
            timestamp: ts,
            vasp_data: vec![],
            delta_data: None,
        };
        let vasf = VasfFile::new(vec![make_frame(1000), make_frame(61000)], 2);
        assert_eq!(vasf.duration_ms(), Some(60000));
    }

    #[test]
    fn test_vasf_writer_dedup_stats() {
        let path = std::env::temp_dir().join("_test_vasf_writer_dedup.vasf");
        let mut w = VasfWriter::create(&path).unwrap();
        let state_id = StateId::from_bits(0xABCDEF01_23456789);
        w.append_state(state_id, "screen_type: terminal\nagent_context: \"test\"\n")
            .unwrap();
        w.append_timeline(1000, state_id).unwrap();
        w.append_timeline(2000, state_id).unwrap();
        w.append_timeline(3000, state_id).unwrap();
        w.append_timeline(4000, state_id).unwrap();
        w.finalize().unwrap();

        let vasf = VasfFile::read_from(&path).unwrap();
        assert_eq!(vasf.header.frame_count, 1);
        assert_eq!(vasf.header.total_input, 4);
        assert_eq!(vasf.frames.len(), 1);
        assert_eq!(vasf.unique_states(), 1);
        assert_eq!(vasf.total_frames(), 4);
        assert!((vasf.dedup_percentage() - 75.0).abs() < 0.01);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_vasf_writer_crash_safe() {
        let path = std::env::temp_dir().join("_test_vasf_writer_crash.vasf");
        let mut w = VasfWriter::create(&path).unwrap();
        let state_id = StateId::from_bits(0x1122334455667788);
        w.append_state(state_id, "screen_type: terminal\n").unwrap();
        w.append_timeline(1000, state_id).unwrap();
        w.append_timeline(2000, state_id).unwrap();

        let vasf = VasfFile::read_from(&path).unwrap();
        assert_eq!(
            vasf.header.frame_count, 1,
            "frame_count must be live without finalize"
        );
        assert_eq!(
            vasf.header.total_input, 2,
            "total_input must be live without finalize"
        );
        assert_eq!(vasf.frames.len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
