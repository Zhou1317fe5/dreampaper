use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"DPOC";
pub const VERSION: u16 = 1;
pub const OP_ANALYZE: u16 = 1;
pub const OP_SHUTDOWN: u16 = 2;
pub const STATUS_READY: u16 = 0;
pub const STATUS_OK: u16 = 1;
pub const STATUS_ERROR: u16 = 2;
pub const MAX_DIMENSION: u32 = 8192;
pub const MAX_PIXELS: u64 = 5504 * 3072;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_HEADER_BYTES: usize = 32;
const RESPONSE_HEADER_BYTES: usize = 20;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("读取协议帧失败")]
    Read(#[source] io::Error),
    #[error("写入协议帧失败")]
    Write(#[source] io::Error),
    #[error("协议 magic 无效")]
    Magic,
    #[error("协议版本不支持：{0}")]
    Version(u16),
    #[error("操作码不支持：{0}")]
    Operation(u16),
    #[error("响应状态不支持：{0}")]
    Status(u16),
    #[error("图像尺寸无效：{0}×{1}")]
    Dimensions(u32, u32),
    #[error("RGB 数据长度无效：期望 {expected}，收到 {actual}")]
    Payload { expected: u64, actual: u64 },
    #[error("响应超过协议上限")]
    ResponseTooLarge,
    #[error("响应序列化失败")]
    Serialize(#[source] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Analyze {
        id: u64,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    },
    Shutdown {
        id: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub id: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Line {
    pub text: String,
    pub score: f32,
    pub box_score: f32,
    pub angle: u16,
    pub angle_score: f32,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageTimes {
    pub prepare_us: u64,
    pub detection_us: u64,
    pub crop_us: u64,
    pub classification_us: u64,
    pub recognition_us: u64,
    pub assemble_us: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AnalyzeResult {
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub stages: StageTimes,
    pub lines: Vec<Line>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Ready {
    pub protocol: u16,
    pub engine: String,
    pub init_ms: u64,
    pub runtime: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Failure<'a> {
    pub code: &'a str,
    pub message: String,
}

pub fn read_request<R: Read>(reader: &mut R) -> Result<Option<Request>, ProtocolError> {
    let mut header = [0_u8; REQUEST_HEADER_BYTES];
    let read = read_header(reader, &mut header)?;
    if !read {
        return Ok(None);
    }
    if header[0..4] != MAGIC {
        return Err(ProtocolError::Magic);
    }

    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != VERSION {
        return Err(ProtocolError::Version(version));
    }
    let operation = u16::from_le_bytes([header[6], header[7]]);
    let id = u64::from_le_bytes(header[8..16].try_into().expect("fixed header"));
    let width = u32::from_le_bytes(header[16..20].try_into().expect("fixed header"));
    let height = u32::from_le_bytes(header[20..24].try_into().expect("fixed header"));
    let payload_len = u64::from_le_bytes(header[24..32].try_into().expect("fixed header"));

    match operation {
        OP_ANALYZE => {
            let expected = validate_image(width, height)?;
            if payload_len != expected {
                return Err(ProtocolError::Payload {
                    expected,
                    actual: payload_len,
                });
            }
            let mut rgb = vec![0; expected as usize];
            reader.read_exact(&mut rgb).map_err(ProtocolError::Read)?;
            Ok(Some(Request::Analyze {
                id,
                width,
                height,
                rgb,
            }))
        }
        OP_SHUTDOWN => {
            if width != 0 || height != 0 || payload_len != 0 {
                return Err(ProtocolError::Payload {
                    expected: 0,
                    actual: payload_len,
                });
            }
            Ok(Some(Request::Shutdown { id }))
        }
        other => Err(ProtocolError::Operation(other)),
    }
}

pub fn read_response<R: Read>(reader: &mut R) -> Result<Option<Response>, ProtocolError> {
    let mut header = [0_u8; RESPONSE_HEADER_BYTES];
    let read = read_header(reader, &mut header)?;
    if !read {
        return Ok(None);
    }
    if header[0..4] != MAGIC {
        return Err(ProtocolError::Magic);
    }

    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != VERSION {
        return Err(ProtocolError::Version(version));
    }
    let status = u16::from_le_bytes([header[6], header[7]]);
    if ![STATUS_READY, STATUS_OK, STATUS_ERROR].contains(&status) {
        return Err(ProtocolError::Status(status));
    }
    let id = u64::from_le_bytes(header[8..16].try_into().expect("fixed header"));
    let payload_len = u32::from_le_bytes(header[16..20].try_into().expect("fixed header")) as usize;
    if payload_len > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::ResponseTooLarge);
    }
    let mut payload = vec![0; payload_len];
    reader
        .read_exact(&mut payload)
        .map_err(ProtocolError::Read)?;
    Ok(Some(Response {
        status,
        id,
        payload,
    }))
}

pub fn write_analyze<W: Write>(
    writer: &mut W,
    id: u64,
    width: u32,
    height: u32,
    rgb: &[u8],
) -> Result<(), ProtocolError> {
    let expected = validate_image(width, height)?;
    if rgb.len() as u64 != expected {
        return Err(ProtocolError::Payload {
            expected,
            actual: rgb.len() as u64,
        });
    }
    write_request_header(writer, OP_ANALYZE, id, width, height, expected)?;
    writer.write_all(rgb).map_err(ProtocolError::Write)?;
    writer.flush().map_err(ProtocolError::Write)
}

pub fn write_shutdown<W: Write>(writer: &mut W, id: u64) -> Result<(), ProtocolError> {
    write_request_header(writer, OP_SHUTDOWN, id, 0, 0, 0)?;
    writer.flush().map_err(ProtocolError::Write)
}

pub fn write_response<W: Write, T: Serialize>(
    writer: &mut W,
    status: u16,
    id: u64,
    payload: &T,
) -> Result<(), ProtocolError> {
    let bytes = serde_json::to_vec(payload).map_err(ProtocolError::Serialize)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::ResponseTooLarge);
    }

    let mut header = [0_u8; RESPONSE_HEADER_BYTES];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&status.to_le_bytes());
    header[8..16].copy_from_slice(&id.to_le_bytes());
    header[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    writer.write_all(&header).map_err(ProtocolError::Write)?;
    writer.write_all(&bytes).map_err(ProtocolError::Write)?;
    writer.flush().map_err(ProtocolError::Write)
}

fn write_request_header<W: Write>(
    writer: &mut W,
    operation: u16,
    id: u64,
    width: u32,
    height: u32,
    payload_len: u64,
) -> Result<(), ProtocolError> {
    let mut header = [0_u8; REQUEST_HEADER_BYTES];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&operation.to_le_bytes());
    header[8..16].copy_from_slice(&id.to_le_bytes());
    header[16..20].copy_from_slice(&width.to_le_bytes());
    header[20..24].copy_from_slice(&height.to_le_bytes());
    header[24..32].copy_from_slice(&payload_len.to_le_bytes());
    writer.write_all(&header).map_err(ProtocolError::Write)
}

pub(crate) fn validate_image(width: u32, height: u32) -> Result<u64, ProtocolError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(ProtocolError::Dimensions(width, height));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ProtocolError::Dimensions(width, height))?;
    if pixels > MAX_PIXELS {
        return Err(ProtocolError::Dimensions(width, height));
    }
    pixels
        .checked_mul(3)
        .ok_or(ProtocolError::Dimensions(width, height))
}

fn read_header<R: Read>(reader: &mut R, header: &mut [u8]) -> Result<bool, ProtocolError> {
    let mut offset = 0;
    while offset < header.len() {
        match reader.read(&mut header[offset..]) {
            Ok(0) if offset == 0 => return Ok(false),
            Ok(0) => {
                return Err(ProtocolError::Read(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "协议头不完整",
                )))
            }
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ProtocolError::Read(error)),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn analyze_round_trip() {
        let request = Request::Analyze {
            id: 9,
            width: 2,
            height: 2,
            rgb: vec![7; 12],
        };
        let mut bytes = Vec::new();
        write_analyze(&mut bytes, 9, 2, 2, &[7; 12]).unwrap();
        assert_eq!(
            read_request(&mut Cursor::new(bytes)).unwrap(),
            Some(request)
        );
    }

    #[test]
    fn rejects_length_before_allocating_payload() {
        let mut bytes = Vec::new();
        write_analyze(&mut bytes, 1, 2, 2, &[0; 12]).unwrap();
        bytes[24..32].copy_from_slice(&11_u64.to_le_bytes());
        assert!(matches!(
            read_request(&mut Cursor::new(bytes)),
            Err(ProtocolError::Payload {
                expected: 12,
                actual: 11
            })
        ));
    }

    #[test]
    fn rejects_oversized_dimensions_before_payload_read() {
        assert!(matches!(
            write_analyze(&mut Vec::new(), 1, MAX_DIMENSION, MAX_DIMENSION, &[]),
            Err(ProtocolError::Dimensions(_, _))
        ));
    }

    #[test]
    fn rejects_invalid_write_length() {
        assert!(matches!(
            write_analyze(&mut Vec::new(), 1, 2, 2, &[0; 11]),
            Err(ProtocolError::Payload {
                expected: 12,
                actual: 11
            })
        ));
    }

    #[test]
    fn shutdown_round_trip() {
        let mut bytes = Vec::new();
        write_shutdown(&mut bytes, 4).unwrap();
        assert_eq!(
            read_request(&mut Cursor::new(bytes)).unwrap(),
            Some(Request::Shutdown { id: 4 })
        );
    }

    #[test]
    fn accepts_clean_eof_and_rejects_partial_header() {
        assert_eq!(
            read_request(&mut Cursor::new(Vec::<u8>::new())).unwrap(),
            None
        );
        assert!(matches!(
            read_request(&mut Cursor::new(MAGIC)),
            Err(ProtocolError::Read(error)) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn response_is_bounded_and_framed() {
        let mut output = Vec::new();
        write_response(
            &mut output,
            STATUS_READY,
            0,
            &Ready {
                protocol: VERSION,
                engine: "native".into(),
                init_ms: 12,
                runtime: "test".into(),
            },
        )
        .unwrap();
        assert_eq!(&output[0..4], &MAGIC);
        assert_eq!(
            u16::from_le_bytes(output[6..8].try_into().unwrap()),
            STATUS_READY
        );
        assert_eq!(u64::from_le_bytes(output[8..16].try_into().unwrap()), 0);
        let len = u32::from_le_bytes(output[16..20].try_into().unwrap()) as usize;
        assert_eq!(output.len(), RESPONSE_HEADER_BYTES + len);
        let response = read_response(&mut Cursor::new(output)).unwrap().unwrap();
        assert_eq!(response.status, STATUS_READY);
        assert_eq!(response.id, 0);
        assert_eq!(
            serde_json::from_slice::<Ready>(&response.payload)
                .unwrap()
                .init_ms,
            12
        );
    }

    #[test]
    fn rejects_oversized_response_before_payload_read() {
        let mut header = [0_u8; RESPONSE_HEADER_BYTES];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..6].copy_from_slice(&VERSION.to_le_bytes());
        header[6..8].copy_from_slice(&STATUS_OK.to_le_bytes());
        header[16..20].copy_from_slice(&((MAX_RESPONSE_BYTES + 1) as u32).to_le_bytes());
        assert!(matches!(
            read_response(&mut Cursor::new(header)),
            Err(ProtocolError::ResponseTooLarge)
        ));
    }
}
